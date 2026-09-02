fn main() {
    // Force Cargo à recompiler dès que la cible API change (preprod <-> prod).
    // Sans cette ligne, un binaire en cache peut être réutilisé alors que
    // ATTIMO_API_URL a changé : on livrerait un exécutable pointant sur le
    // mauvais environnement sans aucun signal d'erreur.
    println!("cargo:rerun-if-env-changed=ATTIMO_API_URL");

    tauri_build::build()
}