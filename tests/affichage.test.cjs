// Tests des libellés du tableau de bord (0.3.3, bloc HD revu en 0.3.4).
// Lancement : node --test tests   (fait par test.bat)
//
// .cjs : package.json déclare le projet en modules ES. affichage.js est un
// script classique de l'interface ; il est chargé ici tel quel, comme le
// fait la page.

const test = require('node:test');
const assert = require('node:assert');
const fs = require('node:fs');
const path = require('node:path');
const vm = require('node:vm');

const source = fs.readFileSync(path.join(__dirname, '..', 'src', 'affichage.js'), 'utf8');
const contexte = { module: { exports: {} } };
vm.runInNewContext(source, contexte);
const A = contexte.module.exports;

// Traduction factice : la clé et ses paramètres, lisibles dans les tests.
const t = (cle, p) => (p ? `${cle}${JSON.stringify(p)}` : cle);
const tPlural = (cle, n) => `${cle}_${n === 1 ? 'one' : 'other'}{${n}}`;

function stats(champs) {
    return Object.assign({
        hd_pending: 0, hd_sending: 0, hd_sent: 0, hd_failed: 0, hd_missing: 0, hd_bytes_pending: 0
    }, champs);
}

test('envoi photo : les trois libellés', () => {
    assert.match(A.messageEnvoiPhoto('reseau_serveur', 3100, 2800, 300, t), /^upload\.success_timing/);
    assert.strictEqual(A.messageEnvoiPhoto('serveur', 2600, 40, 2100, t),
        'upload.success_server{"duration":"2.6","server":"2.1"}');
    assert.strictEqual(A.messageEnvoiPhoto('total', 1200, null, null, t),
        'upload.success_total{"duration":"1.2"}');
});

test('bloc HD après une purge : introuvables, et non « tous envoyés »', () => {
    const b = A.blocHd(stats({ hd_sent: 24, hd_missing: 5 }), false, null, t);

    assert.strictEqual(b.statut, 'hd.sent_missing{"sent":24,"missing":5}');
    // 0.3.4 : barre et compteur sur les clips envoyables, 24 / 24.
    assert.strictEqual(b.progression, 'hd.progress{"sent":24,"total":24}');
    assert.strictEqual(b.jauge, 100);
});

test('bloc HD : les introuvables ne bloquent plus la barre (cas B6 de Fabien)', () => {
    // 13 clips : 5 envoyables, 8 introuvables. Pendant l'envoi…
    const pendant = A.blocHd(stats({ hd_pending: 3, hd_sent: 2, hd_missing: 8, hd_bytes_pending: 90000000 }), true, null, t);

    assert.strictEqual(pendant.progression,
        'hd.progress{"sent":2,"total":5} · hd.missing_apart{"count":8}');
    assert.strictEqual(pendant.jauge, 40);

    // …et une fois tout envoyé : 5 / 5, et non 5 / 13 (38 %).
    const fin = A.blocHd(stats({ hd_sent: 5, hd_missing: 8 }), true, null, t);

    assert.strictEqual(fin.statut, 'hd.sent_missing{"sent":5,"missing":8}');
    assert.strictEqual(fin.progression, 'hd.progress{"sent":5,"total":5}');
    assert.strictEqual(fin.jauge, 100);
});

test('bloc HD : seul le bilan de la session en cours compte', () => {
    // L'épreuve a 5 HD envoyés et 8 introuvables d'une session précédente ;
    // la nouvelle session n'a encore rien fait : pas de bilan affiché.
    const b = A.blocHd(stats({ hd_sent: 5, hd_missing: 8, hd_sent_session: 0, hd_missing_session: 0 }), false, null, t);

    assert.strictEqual(b.statut, 'hd.none');
    assert.strictEqual(b.progression, '');
    assert.strictEqual(b.jauge, 0);

    // Un clip d'une session précédente reste à envoyer : il compte.
    const reste = A.blocHd(stats({ hd_pending: 1, hd_sent: 5, hd_sent_session: 0, hd_missing_session: 0,
        hd_bytes_pending: 50000000 }), true, 20, t);

    assert.match(reste.progression, /^hd\.progress\{"sent":0,"total":1\} · hd\.rate/);
});

test('bloc HD : rien d\'envoyable, tout introuvable', () => {
    const b = A.blocHd(stats({ hd_missing: 3 }), false, null, t);

    assert.strictEqual(b.statut, 'hd.sent_missing{"sent":0,"missing":3}');
    assert.strictEqual(b.progression, '');
    assert.strictEqual(b.jauge, 0);
});

test('bloc HD : tout envoyé, sans fichier manquant', () => {
    const b = A.blocHd(stats({ hd_sent: 24 }), false, null, t);

    assert.strictEqual(b.statut, 'hd.all_sent');
    assert.strictEqual(b.jauge, 100);
});

test('bloc HD : rien à envoyer', () => {
    assert.strictEqual(A.blocHd(stats({}), false, null, t).statut, 'hd.none');
    assert.strictEqual(A.blocHd(null, false, null, t), null);
});

test('bloc HD en cours d\'envoi : débit et temps restant', () => {
    const b = A.blocHd(stats({ hd_pending: 3, hd_sending: 1, hd_sent: 2, hd_bytes_pending: 120000000 }), true, 32, t);

    assert.match(b.statut, /^hd\.remaining.* · hd\.sending$/);
    assert.match(b.progression, /hd\.rate\{"rate":"32\.0"\}/);
    // 120 Mo à 32 Mb/s : 30 s.
    assert.match(b.progression, /hd\.time_left\{"time":"hd\.duration_s\{\\"n\\":30\}"\}/);
});

test('envoi manuel : débit et temps restant dès qu\'un débit est mesuré', () => {
    // « Envoyer les HD maintenant » : l'envoi est actif, le débit vient de
    // l'agent (mesuré pendant l'envoi réel).
    const b = A.blocHd(stats({ hd_pending: 4, hd_sending: 1, hd_bytes_pending: 250000000 }), true, 20, t);

    assert.match(b.progression, /hd\.rate\{"rate":"20\.0"\}/);
    // 250 Mo à 20 Mb/s : 100 s, soit 2 min.
    assert.match(b.progression, /hd\.time_left\{"time":"hd\.duration_min\{\\"n\\":2\}"\}/);
});

test('bloc HD suspendu : ni débit ni temps restant', () => {
    const b = A.blocHd(stats({ hd_pending: 3, hd_bytes_pending: 120000000 }), false, 32, t);

    assert.doesNotMatch(b.progression, /hd\.rate/);
});

test('durées courtes', () => {
    assert.strictEqual(A.dureeCourte(45, t), 'hd.duration_s{"n":45}');
    assert.strictEqual(A.dureeCourte(230, t), 'hd.duration_min{"n":4}');
    assert.strictEqual(A.dureeCourte(4800, t), 'hd.duration_h{"h":1,"m":20}');
});

test('écran des épreuves : champs présents ou absents, sans erreur', () => {
    assert.strictEqual(A.modeIdentification('mixed', t), 'events.mode{"mode":"events.mode_mixed"}');
    assert.strictEqual(A.modeIdentification('none', t), 'events.mode{"mode":"events.mode_none"}');
    assert.strictEqual(A.modeIdentification(undefined, t), null);
    assert.strictEqual(A.modeIdentification('inconnu', t), null);

    assert.strictEqual(A.videosEnLigne(12, tPlural), 'events.videos_other{12}');
    assert.strictEqual(A.videosEnLigne(1, tPlural), 'events.videos_one{1}');
    assert.strictEqual(A.videosEnLigne(undefined, tPlural), null);
    assert.strictEqual(A.videosEnLigne(null, tPlural), null);
});
