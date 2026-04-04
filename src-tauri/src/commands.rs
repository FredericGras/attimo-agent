// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Commandes IPC (commands.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// Ce fichier est le "tableau de commandes" de l'application.
// Chaque fonction #[tauri::command] correspond à un bouton que le
// frontend JavaScript peut presser via `await invoke('nom', { params })`.
//
// Les commandes existantes (Phase 2-3) :
//   - login, logout, get_stored_auth
//   - fetch_events, fetch_checkpoints
//   - get_session_stats, retry_failed, retry_file
//
// Les nouvelles commandes (Phase 4) :
//   - start_session  → Démarrer la surveillance + les uploads
//   - pause_session   → Mettre en pause les uploads (le watcher continue)
//   - resume_session  → Reprendre les uploads
//   - stop_session    → Tout arrêter proprement
//

use crate::auth;
use crate::database;
use crate::watcher;
use crate::uploader;
use crate::{AppState, ActiveSession};
use serde::Serialize;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tokio::sync::mpsc;
use log::info;

// ─── Structures de réponse standardisées ───

#[derive(Debug, Serialize, Clone)]
pub struct LoginResponse {
    pub token: String,
    pub user: auth::UserInfo,
}

#[derive(Debug, Serialize, Clone)]
pub struct StartSessionResponse {
    pub session_id: i64,
    pub message: String,
}

// ═══════════════════════════════════════════════════════════════════════
// COMMANDES AUTH (Phase 2 — inchangées)
// ═══════════════════════════════════════════════════════════════════════

// ─── Commande: login ───
// Appelée par le JS : await invoke('login', { email, password, remember })

#[tauri::command]
pub async fn login(email: String, password: String, remember: bool) -> Result<LoginResponse, String> {
    let auth_data = auth::login(&email, &password).await?;

    if remember {
        auth::save_auth(&auth_data)?;
    }

    Ok(LoginResponse {
        token: auth_data.token,
        user: auth_data.user,
    })
}

// ─── Commande: logout ───

#[tauri::command]
pub async fn logout() -> Result<(), String> {
    auth::clear_auth();
    Ok(())
}

// ─── Commande: get_stored_auth ───

#[tauri::command]
pub async fn get_stored_auth() -> Result<Option<LoginResponse>, String> {
    match auth::load_auth() {
        Some(auth_data) => Ok(Some(LoginResponse {
            token: auth_data.token,
            user: auth_data.user,
        })),
        None => Ok(None),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// COMMANDES ÉVÉNEMENTS (Phase 3 — inchangées)
// ═══════════════════════════════════════════════════════════════════════

#[tauri::command]
pub async fn fetch_events(token: String) -> Result<Vec<auth::SportEvent>, String> {
    auth::fetch_events(&token).await
}

#[tauri::command]
pub async fn fetch_checkpoints(token: String, event_id: i64) -> Result<Vec<auth::Checkpoint>, String> {
    auth::fetch_checkpoints(&token, event_id).await
}

// ═══════════════════════════════════════════════════════════════════════
// COMMANDES SESSION (Phase 4 — NOUVELLES)
// ═══════════════════════════════════════════════════════════════════════

// ─── Commande: start_session ───
//
// Appelée quand le photographe clique "Démarrer la surveillance".
// C'est le chef d'orchestre qui lance toute la mécanique :
//
//   1. Crée une session dans la base SQLite
//   2. Crée le canal de communication watcher → uploader
//   3. Démarre le watcher (surveillance du dossier)
//   4. Démarre les workers d'upload (envoi vers l'API)
//   5. Stocke tous les handles dans l'état partagé (pour pause/stop)
//
// Le frontend appelle :
//   await invoke('start_session', {
//       token, eventId, eventName, checkpointId, checkpointName,
//       folder, includeExisting, parallel
//   })

#[tauri::command]
pub async fn start_session(
    state: tauri::State<'_, AppState>,
    app_handle: tauri::AppHandle,
    token: String,
    event_id: i64,
    event_name: String,
    checkpoint_id: Option<i64>,
    checkpoint_name: Option<String>,
    folder: String,
    include_existing: bool,
    parallel: i32,
) -> Result<StartSessionResponse, String> {
    // ── Vérification : le dossier existe-t-il ? ──
    let folder_path = PathBuf::from(&folder);
    if !folder_path.exists() || !folder_path.is_dir() {
        return Err(format!("Le dossier n'existe pas : {}", folder));
    }

    // ── Arrêter une session précédente si elle existe ──
    {
        let mut session_lock = state.active_session.lock().await;
        if let Some(mut prev) = session_lock.take() {
            info!("Arrêt de la session précédente (id={})", prev.session_id);
            prev.running.store(false, Ordering::Relaxed);
            // Dropper le watcher pour arrêter la surveillance
            prev.watcher_handle.take();
            let _ = database::stop_session(prev.session_id);
        }
    }

    // ── Créer la session dans SQLite ──
    let session_id = database::create_session(
        event_id,
        &event_name,
        checkpoint_id,
        checkpoint_name.as_deref(),
        &folder,
        include_existing,
        parallel,
    )
    .map_err(|e| format!("Erreur création session: {}", e))?;

    info!(
        "Session #{} créée : événement='{}', dossier='{}', parallel={}",
        session_id, event_name, folder, parallel
    );

    // ── Créer les flags partagés ──
    // running : true tant que la session tourne (false = tout s'arrête)
    // paused  : true quand l'utilisateur met en pause
    let running = Arc::new(AtomicBool::new(true));
    let paused = Arc::new(AtomicBool::new(false));

    // ── Créer le canal watcher → uploader ──
    // C'est un tuyau sans limite (unbounded) :
    // le watcher envoie d'un côté, les workers d'upload reçoivent de l'autre.
    // Unbounded = pas de limite de taille (les fichiers s'empilent si l'upload
    // est plus lent que la détection, ce qui est normal sur le terrain).
    let (tx, rx) = mpsc::unbounded_channel::<watcher::FileJob>();
    let tx_for_state = tx.clone();

    // ── Démarrer le watcher ──
    // Il surveille le dossier et envoie les JPEG détectés dans le canal.
    // Le booléen `recursive` est fixé à false pour l'instant (V1 simplifiée).
    // On pourra ajouter un toggle dans l'interface plus tard.
    let watcher_handle = watcher::start_watching(
        folder_path,
        false,          // récursif : désactivé en V1
        session_id,
        include_existing,
        tx,
        running.clone(),
        app_handle.clone(),
    )?;

    // ── Démarrer les workers d'upload ──
    let upload_config = uploader::UploadConfig {
        token,
        event_id,
        checkpoint_id,
        parallel: parallel as u32,
    };

    uploader::start_upload_workers(
        rx,
        upload_config,
        running.clone(),
        paused.clone(),
        app_handle.clone(),
    );

    // ── Stocker la session active dans l'état partagé ──
    // C'est ce qui permet aux commandes pause/resume/stop d'accéder
    // aux flags et au watcher.
    {
        let mut session_lock = state.active_session.lock().await;
        *session_lock = Some(ActiveSession {
            session_id,
            running,
            paused,
            watcher_handle: Some(watcher_handle),
            tx: tx_for_state,
        });
    }

    Ok(StartSessionResponse {
        session_id,
        message: format!("Surveillance démarrée pour '{}'", event_name),
    })
}

// ─── Commande: pause_session ───
//
// Met en pause les uploads. Le watcher CONTINUE de détecter les fichiers
// (ils s'empilent dans le canal). Quand on reprend, les fichiers accumulés
// sont uploadés d'un coup.

#[tauri::command]
pub async fn pause_session(
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let session_lock = state.active_session.lock().await;
    match &*session_lock {
        Some(session) => {
            session.paused.store(true, Ordering::Relaxed);
            info!("Session #{} mise en pause", session.session_id);
            Ok(())
        }
        None => Err("Aucune session active.".to_string()),
    }
}

// ─── Commande: resume_session ───
//
// Reprend les uploads après une pause. Les fichiers empilés pendant
// la pause commencent à s'uploader.

#[tauri::command]
pub async fn resume_session(
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let session_lock = state.active_session.lock().await;
    match &*session_lock {
        Some(session) => {
            session.paused.store(false, Ordering::Relaxed);
            info!("Session #{} reprise", session.session_id);
            Ok(())
        }
        None => Err("Aucune session active.".to_string()),
    }
}

// ─── Commande: stop_session ───
//
// Arrête tout proprement :
//   1. Met running à false (les workers finissent l'upload en cours puis s'arrêtent)
//   2. Droppe le watcher (arrête la surveillance du dossier)
//   3. Marque la session comme arrêtée dans SQLite
//   4. Libère l'état partagé
//
// L'upload en cours n'est PAS coupé brutalement — il finit proprement.
// C'est important car couper un upload en cours pourrait créer un
// fichier corrompu côté serveur.

#[tauri::command]
pub async fn stop_session(
    state: tauri::State<'_, AppState>,
) -> Result<(), String> {
    let mut session_lock = state.active_session.lock().await;
    match session_lock.take() {
        Some(mut session) => {
            info!("Arrêt de la session #{}", session.session_id);

            // 1. Signaler l'arrêt à tous les threads/tâches
            session.running.store(false, Ordering::Relaxed);

            // 2. Dropper le watcher (arrête la surveillance filesystem)
            session.watcher_handle.take();

            // 3. Marquer la session comme arrêtée dans SQLite
            let _ = database::stop_session(session.session_id);

            Ok(())
        }
        None => Err("Aucune session active.".to_string()),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// COMMANDES STATS ET RETRY (existantes — inchangées)
// ═══════════════════════════════════════════════════════════════════════

#[tauri::command]
pub async fn get_session_stats(session_id: i64) -> Result<database::SessionStats, String> {
    database::get_session_stats(session_id)
        .map_err(|e| format!("Erreur base de données: {}", e))
}

#[tauri::command]
pub async fn retry_failed(
    state: tauri::State<'_, AppState>,
    session_id: i64,
) -> Result<u64, String> {
    // 1. Récupérer les fichiers échoués AVANT de les remettre en pending
    let failed_files = database::get_failed_files(session_id)
        .map_err(|e| format!("Erreur base de données: {}", e))?;

    // 2. Remettre leur statut en pending dans SQLite
    let count = database::reset_failed_files(session_id)
        .map_err(|e| format!("Erreur base de données: {}", e))?;

    // 3. Les renvoyer dans le canal vers les workers
    let session_lock = state.active_session.lock().await;
    if let Some(session) = &*session_lock {
        for file in &failed_files {
            let job = watcher::FileJob {
                db_file_id: file.id,
                session_id: file.session_id,
                file_path: std::path::PathBuf::from(&file.file_path),
                filename: file.filename.clone(),
                file_size: file.file_size as u64,
            };
            let _ = session.tx.send(job);
        }
    }

    Ok(count)
}

#[tauri::command]
pub async fn retry_file(file_id: i64) -> Result<(), String> {
    database::reset_file(file_id)
        .map_err(|e| format!("Erreur base de données: {}", e))
}
