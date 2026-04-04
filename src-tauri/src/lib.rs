// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Point d'entrée (lib.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// C'est le "chef d'usine" de l'application.
// Il initialise tout au démarrage :
//   1. La base de données SQLite locale
//   2. Les plugins Tauri (dialogue natif, notifications, stockage)
//   3. L'état partagé (ActiveSession) accessible par toutes les commandes
//   4. La liste des commandes IPC que le frontend JS peut appeler
//

// ─── Déclaration des modules ───
// Chaque `mod` dit à Rust "va chercher le fichier du même nom"
mod auth;        // auth.rs     → login API, gestion token
mod commands;    // commands.rs → commandes IPC (JS → Rust)
mod database;    // database.rs → SQLite locale
mod watcher;     // watcher.rs  → surveillance dossier (Phase 4)
mod uploader;    // uploader.rs → upload HTTP parallèle (Phase 4)

// ─── Imports pour l'état partagé ───
use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// ÉTAT PARTAGÉ — Session active
// ═══════════════════════════════════════════════════════════════════════
//
// Quand le photographe démarre une session de surveillance,
// on crée un ActiveSession qui contient tous les handles nécessaires
// pour contrôler le watcher et l'uploader depuis n'importe quelle
// commande IPC (pause, reprise, arrêt).
//
// C'est un Mutex<Option<...>> :
//   - None       → pas de session en cours
//   - Some(...)  → session active avec tous ses handles
//
// Le Mutex (verrou) empêche deux commandes d'accéder au même moment.
// C'est comme un cadenas sur la salle de contrôle : un seul opérateur
// peut tourner les boutons à la fois.

/// Contient les handles d'une session de surveillance active
pub struct ActiveSession {
    /// ID de la session dans la base SQLite (pour les stats, les fichiers, etc.)
    pub session_id: i64,

    /// Flag partagé : `true` = la session tourne, `false` = tout s'arrête.
    /// Utilisé par le watcher ET l'uploader pour savoir quand stopper.
    /// AtomicBool = peut être lu/écrit depuis plusieurs threads sans verrou.
    pub running: Arc<AtomicBool>,

    /// Flag de pause : `true` = les uploads sont en pause.
    /// Le watcher continue de détecter les fichiers (ils s'empilent),
    /// mais les workers d'upload attendent avant de prendre un nouveau fichier.
    pub paused: Arc<AtomicBool>,

    /// Le watcher notify. On le garde ici pour qu'il reste en vie.
    /// Si on le drop (= libère), la surveillance du dossier s'arrête.
    /// C'est un `Option` pour pouvoir le `take()` lors de l'arrêt.
    pub watcher_handle: Option<notify::RecommendedWatcher>,

    /// Canal d'envoi vers les workers d'upload.
    /// Gardé ici pour pouvoir re-envoyer des fichiers (retry).
    pub tx: tokio::sync::mpsc::UnboundedSender<crate::watcher::FileJob>,
}

/// État global de l'application, géré par Tauri.
/// Accessible dans chaque commande IPC via `tauri::State<AppState>`.
pub struct AppState {
    pub active_session: Mutex<Option<ActiveSession>>,
}

// ═══════════════════════════════════════════════════════════════════════
// DÉMARRAGE DE L'APPLICATION
// ═══════════════════════════════════════════════════════════════════════

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Initialiser la base de données SQLite locale
    // (crée les tables upload_sessions et upload_files si elles n'existent pas)
    if let Err(e) = database::init_db() {
        eprintln!("ERREUR: Impossible d'initialiser la base de données: {}", e);
    }

    tauri::Builder::default()
        // --- Plugins Tauri ---
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())

        // --- État partagé ---
        // .manage() rend l'AppState accessible dans toutes les commandes
        // via le paramètre `state: tauri::State<AppState>`
        .manage(AppState {
            active_session: Mutex::new(None),
        })

        // --- Commandes IPC (JS → Rust) ---
        // Chaque entrée ici correspond à un `await invoke('nom_commande', {...})`
        // dans le JavaScript du frontend
        .invoke_handler(tauri::generate_handler![
            // Auth (Phase 2)
            commands::login,
            commands::logout,
            commands::get_stored_auth,
            // Événements (Phase 3)
            commands::fetch_events,
            commands::fetch_checkpoints,
            // Session de surveillance (Phase 4 — NOUVEAU)
            commands::start_session,
            commands::pause_session,
            commands::resume_session,
            commands::stop_session,
            // Stats et retry (existants, utilisés par le dashboard)
            commands::get_session_stats,
            commands::retry_failed,
            commands::retry_file,
        ])
        .run(tauri::generate_context!())
        .expect("Erreur lors du lancement de l'application");
}
