// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Contrôle des traductions (traductions.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// 0.3.2 — Contrôle automatique, lancé avec la suite de tests (test.bat) :
//
//   - les 27 fichiers de src/lang existent et sont du JSON valide ;
//   - chaque langue a toutes les clés du français, et aucune de plus ;
//   - chaque texte porte exactement les mêmes variables {…} que le
//     français : une variable oubliée s'afficherait telle quelle, une
//     variable de trop ne serait jamais remplie ;
//   - chaque clé appelée par l'interface — t('…'), data-i18n="…" — existe
//     en français.
//
// Un oubli ne se découvre donc plus à l'écran, dans une langue que personne
// ne lit à l'équipe : il fait échouer les tests.

#![cfg(test)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const LANGUES: [&str; 27] = [
    "fr", "en", "de", "es", "it", "pt", "nl", "da", "sv", "no", "fi", "pl", "cs", "sk", "hu", "ro",
    "bg", "hr", "sl", "et", "lv", "lt", "el", "mt", "ga", "tr", "us",
];

fn dossier_interface() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("..").join("src")
}

/// Toutes les clés d'un fichier, à plat (« section.cle » → texte).
fn a_plat(valeur: &serde_json::Value, prefixe: &str, sortie: &mut BTreeMap<String, String>) {
    match valeur {
        serde_json::Value::Object(champs) => {
            for (nom, v) in champs {
                let cle = if prefixe.is_empty() { nom.clone() } else { format!("{}.{}", prefixe, nom) };
                a_plat(v, &cle, sortie);
            }
        }
        serde_json::Value::String(texte) => {
            sortie.insert(prefixe.to_string(), texte.clone());
        }
        autre => {
            sortie.insert(prefixe.to_string(), autre.to_string());
        }
    }
}

fn charger(langue: &str) -> BTreeMap<String, String> {
    let chemin = dossier_interface().join("lang").join(format!("{}.json", langue));

    let texte = std::fs::read_to_string(&chemin)
        .unwrap_or_else(|e| panic!("{} illisible : {}", chemin.display(), e));

    let valeur: serde_json::Value = serde_json::from_str(&texte)
        .unwrap_or_else(|e| panic!("{}.json n'est pas du JSON valide : {}", langue, e));

    let mut cles = BTreeMap::new();
    a_plat(&valeur, "", &mut cles);
    cles
}

/// Les variables {…} d'un texte.
fn variables(texte: &str) -> BTreeSet<String> {
    let mut trouvees = BTreeSet::new();
    let mut reste = texte;

    while let Some(debut) = reste.find('{') {
        let apres = &reste[debut + 1..];

        match apres.find('}') {
            Some(fin) => {
                let nom = &apres[..fin];

                if !nom.is_empty() && nom.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                    trouvees.insert(nom.to_string());
                }

                reste = &apres[fin + 1..];
            }
            None => break,
        }
    }

    trouvees
}

#[test]
fn chaque_langue_a_toutes_les_cles_du_francais() {
    let reference = charger("fr");
    let mut erreurs = Vec::new();

    for langue in LANGUES {
        let cles = charger(langue);

        for (cle, texte) in &reference {
            match cles.get(cle) {
                None => erreurs.push(format!("{} : clé manquante {}", langue, cle)),
                Some(traduit) => {
                    if variables(traduit) != variables(texte) {
                        erreurs.push(format!(
                            "{} : {} — variables {:?} au lieu de {:?}",
                            langue,
                            cle,
                            variables(traduit),
                            variables(texte)
                        ));
                    }

                    if traduit.trim().is_empty() {
                        erreurs.push(format!("{} : {} est vide", langue, cle));
                    }
                }
            }
        }

        for cle in cles.keys() {
            if !reference.contains_key(cle) {
                erreurs.push(format!("{} : clé inconnue du français {}", langue, cle));
            }
        }
    }

    assert!(erreurs.is_empty(), "Traductions incomplètes :\n{}", erreurs.join("\n"));
}

/// Clés appelées dans un texte source : t('a.b'), tPlural('a.b', …),
/// data-i18n="a.b", data-i18n-title="a.b", data-i18n-placeholder="a.b".
fn cles_appelees(source: &str) -> BTreeSet<String> {
    let mut cles = BTreeSet::new();

    let motifs = ["t('", "tPlural('", "data-i18n=\"", "data-i18n-title=\"", "data-i18n-placeholder=\""];

    for motif in motifs {
        let mut reste = source;

        while let Some(position) = reste.find(motif) {
            // `t('` ne doit pas être la fin d'un autre nom (« alert(' »…).
            let precedent = reste[..position].chars().last();

            let apres = &reste[position + motif.len()..];
            let fin = apres.find(|c| c == '\'' || c == '"').unwrap_or(0);
            let cle = &apres[..fin];

            let identifiant_colle = motif.starts_with('t')
                && precedent.map_or(false, |c| c.is_ascii_alphanumeric() || c == '_');

            if !identifiant_colle
                && cle.contains('.')
                && cle.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.')
            {
                cles.insert(cle.to_string());
            }

            reste = &apres[fin..];
        }
    }

    cles
}

#[test]
fn chaque_cle_appelee_par_linterface_existe() {
    let reference = charger("fr");

    let mut manquantes = Vec::new();

    for fichier in ["main.js", "affichage.js", "index.html"] {
        let source = std::fs::read_to_string(dossier_interface().join(fichier)).unwrap();

        for cle in cles_appelees(&source) {
            // tPlural('x.y', n) lit x.y_one ou x.y_other.
            let plurielle = reference.contains_key(&format!("{}_one", cle))
                && reference.contains_key(&format!("{}_other", cle));

            if !reference.contains_key(&cle) && !plurielle {
                manquantes.push(format!("{} : {}", fichier, cle));
            }
        }
    }

    assert!(manquantes.is_empty(), "Clés absentes de fr.json :\n{}", manquantes.join("\n"));
}

#[test]
fn reconnait_les_variables_et_les_appels() {
    assert_eq!(
        variables("OK en {duration} s — {count} {x-y} {}"),
        ["count", "duration"].iter().map(|s| s.to_string()).collect()
    );

    let appels = cles_appelees("t('a.b'); alert('x.y'); tPlural('c.d', 2); <b data-i18n=\"e.f\">");
    assert!(appels.contains("a.b") && appels.contains("c.d") && appels.contains("e.f"));
    assert!(!appels.contains("x.y"));
}
