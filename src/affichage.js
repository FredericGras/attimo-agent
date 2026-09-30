// ═══════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Libellés du tableau de bord (0.3.3, bloc HD revu en 0.3.4)
// ═══════════════════════════════════════════════════════
//
// Fonctions pures : elles reçoivent des chiffres et la fonction de
// traduction, et rendent du texte. Aucun accès à l'écran, ce qui permet de
// les tester hors de l'agent (tests/affichage.test.js, lancé par test.bat).

const Affichage = {
    /**
     * Libellé d'un envoi de photo réussi.
     *
     * `format` vient de l'agent (uploader.rs, format_mesure) : la
     * répartition « réseau + serveur » n'est affichée que si le temps
     * d'envoi mesuré pèse au moins 30 % du total.
     */
    messageEnvoiPhoto(format, totalMs, envoiMs, serveurMs, t) {
        const secondes = (ms) => (ms / 1000).toFixed(1);

        if (format === 'reseau_serveur') {
            return t('upload.success_timing', {
                duration: secondes(totalMs),
                network: secondes(envoiMs),
                server: secondes(serveurMs)
            });
        }

        if (format === 'serveur') {
            return t('upload.success_server', {
                duration: secondes(totalMs),
                server: secondes(serveurMs)
            });
        }

        return t('upload.success_total', { duration: secondes(totalMs) });
    },

    /**
     * Bilan des photos, une fois la file vide (0.3.5).
     *
     * Pour l'agent, `sent` compte les photos reçues par le serveur, qu'elles
     * viennent de partir ou qu'il les ait déjà (`already`). Ici, « envoyées »
     * ne désigne plus que les premières : « 25 envoyées (dont 25 déjà en
     * ligne) » disait qu'aucune n'était partie, mais pas clairement.
     */
    bilanPhotos(sent, failed, already, t) {
        const envoyees = Math.max(0, sent - (already || 0));

        if (!already) {
            return failed > 0
                ? t('complete.with_errors', { sent: envoyees, failed })
                : t('complete.success', { count: envoyees });
        }

        if (failed > 0) {
            return t('complete.sent_online_failed', { sent: envoyees, already, failed });
        }

        return envoyees === 0
            ? t('complete.all_online', { count: already })
            : t('complete.sent_and_online', { sent: envoyees, already });
    },

    /**
     * Photos réellement envoyées, et celles déjà en ligne, non renvoyées
     * (0.3.5) : la tuile « Envoyées » et le bandeau ne les mélangent plus.
     */
    comptePhotos(sent, already) {
        const deja = already || 0;

        return { envoyees: Math.max(0, sent - deja), deja };
    },

    /**
     * Libellé de la tuile « images d'analyse » (0.3.6).
     *
     * `ses` : décompte de la file pour la session (video_queue_stats).
     * Avant, seules les images confirmées par le serveur étaient comptées :
     * un lot de dix n'est confirmé qu'une fois les dix analysées, et la
     * tuile restait à « 0 envoyées » pendant que le serveur produisait déjà
     * ses lectures. Les images en cours d'envoi et celles abandonnées après
     * trois essais sont désormais dites.
     */
    libelleImagesAnalyse(ses, desactivee, t) {
        if (desactivee) {
            return t('dashboard.video_frames_disabled');
        }

        const envoyees = ses ? ses.frames_sent || 0 : 0;
        const enCours = ses ? ses.frames_sending || 0 : 0;
        const echecs = ses ? ses.frames_failed || 0 : 0;

        let libelle = t('dashboard.video_frames_detail', { sent: envoyees });

        if (enCours > 0) {
            libelle += ', ' + t('dashboard.video_frames_sending', { count: enCours });
        }

        if (echecs > 0) {
            libelle += ', ' + t('dashboard.video_frames_failed', { count: echecs });
        }

        return libelle;
    },

    /**
     * Temps restant, en secondes, pour envoyer `octets` à `mbps`.
     */
    resteSecondes(octets, mbps) {
        if (!mbps || mbps <= 0 || !octets || octets <= 0) {
            return null;
        }

        return Math.ceil((octets * 8) / (mbps * 1000000));
    },

    /**
     * Durée courte et arrondie : « 45 s », « 4 min », « 1 h 20 min ».
     */
    dureeCourte(secondes, t) {
        if (secondes < 60) {
            return t('hd.duration_s', { n: Math.max(1, Math.round(secondes)) });
        }

        if (secondes < 3600) {
            return t('hd.duration_min', { n: Math.ceil(secondes / 60) });
        }

        return t('hd.duration_h', {
            h: Math.floor(secondes / 3600),
            m: Math.round((secondes % 3600) / 60)
        });
    },

    /**
     * Bloc « Vidéo HD ».
     *
     * `evt` : décompte de la file pour l'épreuve (video_queue_stats).
     * `envoiActif` : l'envoi des HD est autorisé.
     * `mbps` : débit HD mesuré pendant l'envoi réel, ou null.
     *
     * Les clips HD sortis de la file faute de fichier (supprimés du disque)
     * ne sont plus annoncés « tous envoyés » : « 24 envoyés — 5 introuvables
     * sur le disque (restent en version légère) ».
     *
     * 0.3.4 — La barre et le compteur portent sur les clips envoyables :
     * « 5 / 5 », et non « 5 / 13 » avec 8 introuvables ; ceux-ci sont dits à
     * part. Envoyés et introuvables sont ceux de la session en cours
     * (`hd_sent_session`, `hd_missing_session`) : le bilan d'une session
     * précédente ne s'affiche plus. Ce qui reste à envoyer, ou en échec,
     * compte pour toute l'épreuve : c'est encore à faire.
     */
    blocHd(evt, envoiActif, mbps, t) {
        if (!evt) {
            return null;
        }

        const restant = evt.hd_pending + evt.hd_sending;
        const envoyes = evt.hd_sent_session !== undefined ? evt.hd_sent_session : evt.hd_sent;
        const manquants = (evt.hd_missing_session !== undefined ? evt.hd_missing_session : evt.hd_missing) || 0;
        const envoyables = restant + envoyes + evt.hd_failed;

        if (envoyables === 0 && manquants === 0) {
            return { statut: t('hd.none'), progression: '', jauge: 0, restant: 0, echecs: 0 };
        }

        let statut;
        let introuvablesDits = false;

        if (restant > 0) {
            statut = t('hd.remaining', {
                count: restant,
                size: (evt.hd_bytes_pending / 1073741824).toFixed(1)
            }) + (envoiActif && evt.hd_sending > 0 ? ' · ' + t('hd.sending') : '');
        } else if (evt.hd_failed > 0) {
            statut = t('hd.failed', { count: evt.hd_failed });
        } else if (manquants > 0) {
            statut = t('hd.sent_missing', { sent: envoyes, missing: manquants });
            introuvablesDits = true;
        } else {
            statut = t('hd.all_sent');
        }

        let progression = envoyables > 0
            ? t('hd.progress', { sent: envoyes, total: envoyables })
            : '';

        // Les introuvables, à part : ils ne pèsent ni sur la barre ni sur le
        // compteur.
        if (manquants > 0 && !introuvablesDits) {
            progression += (progression ? ' · ' : '') + t('hd.missing_apart', { count: manquants });
        }

        if (envoiActif && restant > 0 && mbps) {
            progression += ' · ' + t('hd.rate', { rate: mbps.toFixed(1) });

            const reste = Affichage.resteSecondes(evt.hd_bytes_pending, mbps);

            if (reste) {
                progression += ' · ' + t('hd.time_left', { time: Affichage.dureeCourte(reste, t) });
            }
        }

        return {
            statut,
            progression,
            jauge: envoyables > 0 ? Math.round((envoyes / envoyables) * 100) : 0,
            restant,
            echecs: evt.hd_failed
        };
    },

    /**
     * Mode d'identification d'une épreuve, s'il est connu. Le serveur ne
     * l'envoie pas encore : rien alors, et rien non plus pour une valeur
     * inconnue.
     */
    modeIdentification(mode, t) {
        const connus = {
            bib: 'events.mode_bib',
            face: 'events.mode_face',
            mixed: 'events.mode_mixed',
            roster: 'events.mode_roster',
            vehicle: 'events.mode_vehicle',
            none: 'events.mode_none'
        };

        const cle = typeof mode === 'string' ? connus[mode.toLowerCase()] : undefined;

        return cle ? t('events.mode', { mode: t(cle) }) : null;
    },

    /**
     * « N vidéos en ligne », si le serveur donne le nombre.
     */
    videosEnLigne(nombre, tPlural) {
        return Number.isInteger(nombre) && nombre >= 0 ? tPlural('events.videos', nombre) : null;
    }
};

if (typeof module !== 'undefined' && module.exports) {
    module.exports = Affichage;
}
