// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Point d'entrée (lib.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// SAAS 240 — Phase 6 (Vague 2C) : remplacement de la commande `login`
// (email/password) par `validate_app_password` (token attimo_pat_*).

mod assembler;
mod auth;
mod capture;
mod commands;
mod database;
mod disk;
mod frames;
mod manifest;
mod recorder;
mod watcher;
mod uploader;
mod video_queue;
mod video_uploader;

use std::sync::atomic::AtomicBool;
use std::sync::Arc;
use tokio::sync::Mutex;

// ═══════════════════════════════════════════════════════════════════════
// ÉTAT PARTAGÉ — Session active (inchangé)
// ═══════════════════════════════════════════════════════════════════════

pub struct ActiveSession {
    pub session_id: i64,
    pub running: Arc<AtomicBool>,
    pub paused: Arc<AtomicBool>,
    pub watcher_handle: Option<notify::RecommendedWatcher>,
    pub tx: tokio::sync::mpsc::UnboundedSender<crate::watcher::FileJob>,
}

pub struct AppState {
    pub active_session: Mutex<Option<ActiveSession>>,
    /// Captation vidéo en cours, s'il y en a une (SAAS 430).
    pub active_recording: Mutex<Option<crate::recorder::RecordingHandle>>,
    /// Suivi des morceaux et des clips assemblés (SAAS 430).
    pub clip_tracker: Mutex<Option<crate::assembler::ClipTracker>>,
    /// Manifeste de la session en cours (SAAS 430).
    pub manifest_writer: Mutex<Option<crate::manifest::ManifestWriter>>,
    /// Sessions déjà recoupées avec le serveur depuis ce lancement (SAAS 430).
    ///
    /// Le recoupement coûte un appel réseau et ne change pas en cours de
    /// route : une fois par session et par ouverture de l'agent suffit.
    pub reconciled_sessions: Mutex<std::collections::HashSet<String>>,
    /// Autorisation d'envoyer les clips pleine qualité (SAAS 430).
    ///
    /// Décidé par le photographe sur le terrain, lui seul sait ce que donne
    /// son réseau. Éteint, les clips lourds s'accumulent sans saturer la 4G ;
    /// allumé, tout ce qui attendait part.
    pub allow_hd_upload: Mutex<bool>,
}

// ═══════════════════════════════════════════════════════════════════════
// DÉMARRAGE DE L'APPLICATION
// ═══════════════════════════════════════════════════════════════════════

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    // Journal. Déclaré dans Cargo.toml de longue date mais jamais initialisé :
    // sans cet appel, tous les `info!` et `log::error!` de l'agent partent dans
    // le vide — y compris ceux qui expliqueraient un échec sur le terrain.
    //
    // `info` par défaut, surchargeable par la variable RUST_LOG pour un
    // diagnostic plus bavard sans recompiler.
    env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .init();

    if let Err(e) = database::init_db() {
        eprintln!("ERREUR: Impossible d'initialiser la base de données: {}", e);
    }

    // Table de la file vidéo (SAAS 430). Séparée de l'initialisation des
    // photos : les deux évoluent indépendamment.
    if let Err(e) = database::connexion().map_err(|e| e.to_string()).and_then(|c| {
        video_queue::init_schema(&c)?;

        // Un envoi interrompu par la fermeture de l'agent laisse des
        // éléments figés. On les remet en file : au pire un fichier part
        // deux fois, ce qui est sans conséquence côté serveur.
        let liberes = video_queue::liberer_orphelins(&c)?;

        if liberes > 0 {
            eprintln!("File vidéo : {} élément(s) remis en attente", liberes);
        }

        Ok(())
    }) {
        eprintln!("ERREUR: Impossible d'initialiser la file vidéo: {}", e);
    }

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_store::Builder::default().build())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_notification::init())
        // Permet de lancer FFmpeg, embarqué comme binaire compagnon.
        // Restreint aux seuls exécutables déclarés dans externalBin :
        // aucun programme arbitraire du poste ne peut être appelé.
        .plugin(tauri_plugin_shell::init())
        .manage(AppState {
            active_session: Mutex::new(None),
            active_recording: Mutex::new(None),
            clip_tracker: Mutex::new(None),
            manifest_writer: Mutex::new(None),
            reconciled_sessions: Mutex::new(std::collections::HashSet::new()),
            // Par défaut éteint : mieux vaut que le photographe active
            // sciemment 33 Mb/s que de les subir sans l'avoir voulu.
            allow_hd_upload: Mutex::new(false),
        })
        .invoke_handler(tauri::generate_handler![
            // Auth (SAAS 240 — Phase 6) : validate_app_password remplace login
            commands::validate_app_password,
            commands::logout,
            commands::get_stored_auth,
            // Événements (Phase 3)
            commands::fetch_events,
            commands::fetch_checkpoints,
            // Session de surveillance (Phase 4)
            commands::start_session,
            commands::pause_session,
            commands::resume_session,
            commands::stop_session,
            // Stats et retry
            commands::get_session_stats,
            commands::retry_failed,
            commands::retry_file,
            // Espace disque (SAAS 430)
            commands::disk_estimate,
            // Capture vidéo (SAAS 430)
            commands::check_ffmpeg,
            commands::list_video_devices,
            commands::probe_video_device,
            // Enregistrement (SAAS 430)
            commands::plan_recording,
            commands::start_recording,
            commands::stop_recording,
            commands::on_segment_ready,
            commands::extract_frames,
            commands::generate_proxy,
            // Envoi vers le serveur (SAAS 430)
            commands::upload_clip,
            commands::upload_frames,
            commands::clips_status,
            // File d'attente vidéo (SAAS 430)
            commands::queue_video_file,
            commands::video_queue_stats,
            commands::set_hd_upload,
            commands::process_video_queue,
            commands::retry_video_queue,
        ])
        .run(tauri::generate_context!())
        .expect("Erreur lors du lancement de l'application");
}
