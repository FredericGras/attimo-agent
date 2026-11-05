// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Codes d'erreur pour l'écran (erreurs.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// 0.3.7 — Une erreur qui remonte à l'écran n'est plus une phrase française :
// c'est un code, que l'interface traduit dans la langue du photographe
// (Affichage.messageErreur, section « err » de src/lang/*.json).
//
//   - « err.network » : le code seul ;
//   - « err.folder_missing|{"folder":"D:\\Photos"} » : le code et ses
//     variables, en JSON.
//
// Le détail technique (réponse de FFmpeg, erreur système) ne part pas à
// l'écran : il reste au journal de l'agent. Tout code écrit ici ou ailleurs
// doit exister dans fr.json, avec les mêmes variables : traductions.rs le
// vérifie à chaque passage des tests.
//
// ⚠️ Jamais `journalisee` sous le verrou du fichier journal (journal.rs) : le
// log y écrit lui-même, il attendrait le verrou pour toujours.

/// Un code et ses variables.
pub fn avec(cle: &str, variables: &[(&str, String)]) -> String {
    let mut champs = serde_json::Map::new();

    for (nom, valeur) in variables {
        champs.insert((*nom).to_string(), serde_json::Value::String(valeur.clone()));
    }

    format!("{}|{}", cle, serde_json::Value::Object(champs))
}

/// Le code pour l'écran, le détail pour le journal.
pub fn journalisee(cle: &str, detail: impl std::fmt::Display) -> String {
    log::warn!("{} : {}", cle, detail);
    cle.to_string()
}

/// Une erreur réseau : le photographe doit savoir s'il attend ou s'il vérifie
/// sa connexion, le détail n'intéresse que le diagnostic.
pub fn reseau(e: &reqwest::Error) -> String {
    let cle = if e.is_timeout() {
        "err.timeout"
    } else if e.is_connect() {
        "err.server_unreachable"
    } else {
        "err.network"
    };

    journalisee(cle, e)
}

/// Le serveur a répondu par une erreur HTTP.
pub fn statut_serveur(code: u16) -> String {
    avec("err.server_status", &[("code", code.to_string())])
}

/// Ce que dit le serveur, encadré d'un texte traduit ; `repli` s'il ne dit rien.
pub fn du_serveur(message: Option<String>, repli: &str) -> String {
    match message {
        Some(detail) if !detail.trim().is_empty() => {
            avec("err.server_message", &[("detail", detail)])
        }
        _ => repli.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn un_code_porte_ses_variables_en_json() {
        let code = avec("err.folder_missing", &[("folder", "D:\\Photos \"été\"".to_string())]);

        let (cle, json) = code.split_once('|').unwrap();
        let valeur: serde_json::Value = serde_json::from_str(json).unwrap();

        assert_eq!(cle, "err.folder_missing");
        assert_eq!(valeur["folder"], "D:\\Photos \"été\"");
    }

    #[test]
    fn le_detail_reste_au_journal() {
        assert_eq!(journalisee("err.database", "disk I/O error"), "err.database");
    }

    #[test]
    fn le_message_du_serveur_est_encadre() {
        assert_eq!(
            du_serveur(Some("Event not found.".to_string()), "err.server_refused"),
            avec("err.server_message", &[("detail", "Event not found.".to_string())])
        );

        assert_eq!(du_serveur(None, "err.server_refused"), "err.server_refused");
        assert_eq!(du_serveur(Some("  ".to_string()), "err.server_refused"), "err.server_refused");
    }
}
