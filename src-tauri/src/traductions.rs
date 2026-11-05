// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Contrôle des traductions (traductions.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// 0.3.2 — Contrôle automatique, lancé avec la suite de tests (test.bat) :
//
//   - les 28 fichiers de src/lang existent et sont du JSON valide ;
//   - chaque langue a toutes les clés du français, et aucune de plus ;
//   - chaque texte porte exactement les mêmes variables {…} que le
//     français : une variable oubliée s'afficherait telle quelle, une
//     variable de trop ne serait jamais remplie ;
//   - chaque clé appelée par l'interface — t('…'), data-i18n="…" — existe
//     en français ;
//   - 0.3.7 : chaque code d'erreur renvoyé par le Rust (« err.… ») existe en
//     français, avec les variables que le code lui passe ; aucun n'est resté
//     en français dans une autre langue ; le portugais du Brésil garde son
//     vocabulaire.
//
// Un oubli ne se découvre donc plus à l'écran, dans une langue que personne
// ne lit à l'équipe : il fait échouer les tests.

#![cfg(test)]

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;

const LANGUES: [&str; 28] = [
    "fr", "en", "de", "es", "it", "pt", "pt_BR", "nl", "da", "sv", "no", "fi", "pl", "cs", "sk", "hu",
    "ro", "bg", "hr", "sl", "et", "lv", "lt", "el", "mt", "ga", "tr", "us",
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

/// Sources Rust de l'agent, hors ce fichier (ses exemples ne sont pas des
/// codes réels).
fn sources_rust() -> Vec<(String, String)> {
    let dossier = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("src");

    std::fs::read_dir(&dossier)
        .unwrap()
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.extension().map_or(false, |x| x == "rs")
                && p.file_name().map_or(true, |n| n != "traductions.rs")
        })
        .map(|p| {
            let nom = p.file_name().unwrap().to_string_lossy().to_string();
            (nom, std::fs::read_to_string(&p).unwrap())
        })
        .collect()
}

/// Codes d'erreur écrits dans un source : "err.xxx".
fn codes_erreur(source: &str) -> BTreeSet<String> {
    let mut codes = BTreeSet::new();
    let mut reste = source;

    while let Some(position) = reste.find("\"err.") {
        let apres = &reste[position + 1..];
        let fin = apres.find('"').unwrap_or(0);
        let code = &apres[..fin];

        if code.chars().all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '.') {
            codes.insert(code.to_string());
        }

        reste = &apres[fin..];
    }

    codes
}

/// Appels `avec("err.xxx", &[("nom", …), …])` : le code et ses variables.
fn appels_avec(source: &str) -> Vec<(String, BTreeSet<String>)> {
    let mut appels = Vec::new();
    let mut reste = source;

    while let Some(position) = reste.find("avec(") {
        let apres = reste[position + 5..].trim_start();

        reste = &reste[position + 5..];

        let Some(sans_guillemet) = apres.strip_prefix("\"err.") else {
            continue;
        };

        let fin_code = sans_guillemet.find('"').unwrap_or(0);
        let code = format!("err.{}", &sans_guillemet[..fin_code]);
        let corps = &sans_guillemet[fin_code..];
        let corps = &corps[..corps.find(']').unwrap_or(corps.len())];

        let mut noms = BTreeSet::new();
        let mut morceau = corps;

        while let Some(debut) = morceau.find("(\"") {
            let suite = &morceau[debut + 2..];
            let fin = suite.find('"').unwrap_or(0);
            let nom = &suite[..fin];

            if !nom.is_empty() && nom.chars().all(|c| c.is_ascii_alphanumeric() || c == '_') {
                noms.insert(nom.to_string());
            }

            morceau = &suite[fin..];
        }

        appels.push((code, noms));
    }

    appels
}

#[test]
fn chaque_code_derreur_du_rust_existe_en_francais() {
    let reference = charger("fr");
    let mut manquants = Vec::new();

    for (fichier, source) in sources_rust() {
        for code in codes_erreur(&source) {
            if !reference.contains_key(&code) {
                manquants.push(format!("{} : {}", fichier, code));
            }
        }
    }

    assert!(manquants.is_empty(), "Codes absents de fr.json :\n{}", manquants.join("\n"));
    assert!(reference.contains_key("err.unknown"), "le message générique doit exister");
}

#[test]
fn chaque_code_recoit_les_variables_de_son_texte() {
    let reference = charger("fr");
    let mut erreurs = Vec::new();

    for (fichier, source) in sources_rust() {
        for (code, noms) in appels_avec(&source) {
            let attendues = reference.get(&code).map(|t| variables(t)).unwrap_or_default();

            if noms != attendues {
                erreurs.push(format!("{} : {} — {:?} au lieu de {:?}", fichier, code, noms, attendues));
            }
        }
    }

    assert!(erreurs.is_empty(), "Variables des codes d'erreur :\n{}", erreurs.join("\n"));
}

#[test]
fn aucun_message_derreur_nest_reste_en_francais() {
    let reference = charger("fr");
    let mut erreurs = Vec::new();

    for langue in LANGUES.iter().filter(|l| **l != "fr") {
        let cles = charger(langue);

        for (cle, texte) in reference.iter().filter(|(c, _)| c.starts_with("err.")) {
            if cles.get(cle) == Some(texte) {
                erreurs.push(format!("{} : {}", langue, cle));
            }
        }
    }

    assert!(erreurs.is_empty(), "Restés en français :\n{}", erreurs.join("\n"));
}

#[test]
fn chaque_langue_est_proposee_par_linterface() {
    let source = std::fs::read_to_string(dossier_interface().join("i18n.js")).unwrap();

    for langue in LANGUES {
        assert!(source.contains(&format!("'{}'", langue)), "{} absente de i18n.js", langue);
    }
}

#[test]
fn le_bresilien_garde_son_vocabulaire() {
    // Mots du Portugal, en minuscules. Contrôle identique dans son esprit à
    // tools/i18n/check-pt-br.php du dépôt Attimo.
    const INTERDITS: [&str; 16] = [
        "ficheiro", "ecrã", "telemóvel", "utilizador", "palavra-passe", "transferir", "equipa",
        "desportiv", "desporto", "contacto", "contactar", "encomenda", "definições", "aceder",
        "registar", "detetad",
    ];

    let bresil = charger("pt_BR");
    let portugal = charger("pt");
    let francais = charger("fr");
    let mut erreurs = Vec::new();

    for (cle, texte) in &bresil {
        let minuscule = texte.to_lowercase();

        for mot in INTERDITS {
            if minuscule.contains(mot) {
                erreurs.push(format!("{} : « {} » dans {}", cle, mot, texte));
            }
        }

        // Le gérondif du Portugal : « a enviar » au lieu de « enviando ».
        for gerondif in [" a enviar", " a carregar", " a gravar", " a procurar"] {
            if minuscule.contains(gerondif) {
                erreurs.push(format!("{} : gérondif du Portugal dans {}", cle, texte));
            }
        }
    }

    assert!(erreurs.is_empty(), "Formes du Portugal :\n{}", erreurs.join("\n"));
    assert_ne!(bresil, portugal, "le Brésil n'est pas une copie du Portugal");
    assert!(
        bresil.iter().filter(|(cle, texte)| francais.get(*cle) == Some(*texte)).count() < 15,
        "trop de textes restés en français"
    );
}

#[test]
fn reconnait_les_codes_derreur_et_leurs_variables() {
    let source = r#"
        return Err("err.simple".to_string());
        avec("err.deux", &[("folder", folder.clone()), ("size", format!("{:.1}", x))]);
        avec(
            "err.multi",
            &[("index", i.to_string())],
        );
        log::info!("err. pas un code");
    "#;

    let codes = codes_erreur(source);
    assert!(codes.contains("err.simple") && codes.contains("err.deux") && codes.contains("err.multi"));
    assert!(!codes.iter().any(|c| c.contains(' ')));

    let appels = appels_avec(source);
    assert_eq!(appels.len(), 2);
    assert_eq!(appels[0].0, "err.deux");
    assert_eq!(appels[0].1, ["folder", "size"].iter().map(|s| s.to_string()).collect());
    assert_eq!(appels[1].1, ["index"].iter().map(|s| s.to_string()).collect());
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
