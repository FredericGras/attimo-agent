fn main() {
    // Force Cargo à recompiler dès que la cible API change (preprod <-> prod).
    // Sans cette ligne, un binaire en cache peut être réutilisé alors que
    // ATTIMO_API_URL a changé : on livrerait un exécutable pointant sur le
    // mauvais environnement sans aucun signal d'erreur.
    println!("cargo:rerun-if-env-changed=ATTIMO_API_URL");

    // Même raison pour l'édition (0.3.1) : un binaire de DEV en cache ne doit
    // jamais ressortir dans un build de production, avec la mémoire locale
    // de l'agent de DEV.
    println!("cargo:rerun-if-env-changed=ATTIMO_EDITION");

    // Dossier de la mémoire locale dans %APPDATA%, calculé ici pour figurer
    // en toutes lettres dans le binaire : les scripts de build le vérifient.
    // Production : le dossier historique, pour qu'une mise à jour retrouve
    // ses sessions. DEV : un dossier à part, jamais celui de la production.
    let edition = std::env::var("ATTIMO_EDITION")
        .unwrap_or_default()
        .trim()
        .to_lowercase();

    let dossier = if edition.is_empty() {
        "com.attimo-gallery.agent".to_string()
    } else {
        format!("com.attimo-gallery.agent.{}", edition)
    };

    println!("cargo:rustc-env=ATTIMO_DATA_DIR={}", dossier);

    tauri_build::build()
}
