// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Module Uploader (envoi HTTP parallèle)
// ═══════════════════════════════════════════════════════════════════════
//
// Ce module est l'« expéditeur » de l'Agent Terrain.
// Il reçoit les fichiers détectés par le watcher via un canal tokio,
// et les uploade vers l'API Attimo en parallèle (1 à 3 workers).
//
// Flux concret :
//   1. Le watcher détecte IMG_1234.JPG et l'envoie dans le canal
//   2. Un worker libre prend le fichier du canal
//   3. Il lit le fichier depuis le disque
//   4. (Optionnel) Il compresse le JPEG en mémoire si l'option est activée
//   5. Il envoie le fichier en POST multipart vers /api/sport/upload
//   6. Si succès → met à jour SQLite + notifie le frontend
//   7. Si échec → retry avec backoff exponentiel (5s → 15s → 45s)
//   8. Après 3 échecs → marque le fichier comme "failed" dans SQLite
//
// Chaque worker est une tâche tokio indépendante qui tourne en boucle.
// La PAUSE arrête les workers (ils finissent l'upload en cours puis attendent).
// L'ARRÊT termine les workers proprement.
//

use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Emitter;
use tokio::sync::mpsc;
use log::{info, warn, error};

use crate::database;
use crate::watcher::FileJob;

// ─── Constantes ─────────────────────────────────────────────

/// URL de base de l'API Attimo (même que dans auth.rs)
/// TODO: passer en "https://attimo-gallery.com" pour la production
const API_BASE_URL: &str = "https://dev-saas.attimo-gallery.com";

/// Endpoint d'upload des photos sport
const UPLOAD_ENDPOINT: &str = "/api/sport/upload";

/// Timeout par upload : 60 secondes.
/// Un JPEG de 4 Mo sur une 4G à 500 Ko/s prend ~8 secondes.
/// 60 secondes laissent une large marge pour les connexions lentes.
const UPLOAD_TIMEOUT_SECS: u64 = 60;

/// Nombre maximum de tentatives par fichier (1 essai + 2 retries = 3 au total)
const MAX_ATTEMPTS: u32 = 3;

/// Délais de retry en secondes (backoff exponentiel)
/// Après le 1er échec : attendre 5s, puis 15s, puis 45s
const RETRY_DELAYS: &[u64] = &[5, 15, 45];

/// Intervalle de ping réseau (détection hors ligne)
const PING_INTERVAL_SECS: u64 = 10;

// ─── Structures ─────────────────────────────────────────────

/// Réponse de l'API Attimo après un upload réussi
#[derive(Debug, serde::Deserialize)]
struct UploadApiResponse {
    success: bool,
    data: Option<UploadedPhotoData>,
    message: Option<String>,
}

#[derive(Debug, serde::Deserialize)]
struct UploadedPhotoData {
    photo_id: Option<i64>,
}

/// Configuration d'une session d'upload
#[derive(Debug, Clone)]
pub struct UploadConfig {
    /// Token Sanctum pour l'authentification API
    pub token: String,
    /// ID de l'événement sport sur le serveur
    pub event_id: i64,
    /// ID du checkpoint (optionnel — None = pas de checkpoint)
    pub checkpoint_id: Option<i64>,
    /// Nombre de workers parallèles (1 à 3)
    pub parallel: u32,
}

// ─── Client HTTP ────────────────────────────────────────────

/// Crée un client HTTP réutilisable avec les bons timeouts
fn create_http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(UPLOAD_TIMEOUT_SECS))
        .http1_only()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .map_err(|e| format!("Erreur création client HTTP: {}", e))
}

// ─── Upload d'un fichier ────────────────────────────────────

/// Uploade un seul fichier vers l'API Attimo.
///
/// Lit le fichier depuis le disque, le met dans un formulaire multipart,
/// et l'envoie en POST. Le header `Accept: application/json` est essentiel
/// (sans lui, Laravel retourne du HTML au lieu de JSON).
///
/// # Retour
/// - `Ok(Some(photo_id))` — succès, avec l'ID de la photo sur le serveur
/// - `Ok(None)` — succès mais pas d'ID retourné (ne devrait pas arriver)
/// - `Err(message)` — échec avec description de l'erreur
async fn upload_single_file(
    client: &reqwest::Client,
    config: &UploadConfig,
    file_path: &Path,
    filename: &str,
) -> Result<Option<i64>, String> {
    // Lire le fichier depuis le disque
    let file_bytes = tokio::fs::read(file_path).await.map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("Fichier introuvable: {}", file_path.display())
        } else if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("Accès refusé: {}", file_path.display())
        } else {
            format!("Erreur lecture fichier: {}", e)
        }
    })?;

    // Construire le formulaire multipart
    // C'est exactement comme un <form enctype="multipart/form-data"> en HTML
    let file_part = reqwest::multipart::Part::bytes(file_bytes)
        .file_name(filename.to_string())
        .mime_str("image/jpeg")
        .map_err(|e| format!("Erreur MIME: {}", e))?;

    let mut form = reqwest::multipart::Form::new()
        .part("photo", file_part)
        .text("event_id", config.event_id.to_string());

    // Ajouter le checkpoint si défini
    if let Some(cp_id) = config.checkpoint_id {
        form = form.text("checkpoint_id", cp_id.to_string());
    }

    // Envoyer la requête POST
    let url = format!("{}{}", API_BASE_URL, UPLOAD_ENDPOINT);
    let response = client
        .post(&url)
        .header("Accept", "application/json")
        .header("X-API-TOKEN", &config.token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "Timeout (60s dépassées)".to_string()
            } else if e.is_connect() {
                "Impossible de contacter le serveur".to_string()
            } else if e.is_request() {
                "Erreur d'envoi de la requête".to_string()
            } else {
                format!("Erreur réseau: {}", e)
            }
        })?;

    let status = response.status();

    // Token expiré → erreur spéciale que le frontend interceptera
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("SESSION_EXPIRED".to_string());
    }

    // Erreur 413 : fichier trop gros pour le serveur
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        return Err("Fichier trop volumineux pour le serveur".to_string());
    }

    // Erreur 429 : trop de requêtes (rate limiting)
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS {
        return Err("Trop de requêtes, ralentissement".to_string());
    }

    // Erreur serveur 5xx
    if status.is_server_error() {
        return Err(format!("Erreur serveur ({})", status.as_u16()));
    }

    // Autres erreurs HTTP
    if !status.is_success() {
        return Err(format!("Erreur HTTP ({})", status.as_u16()));
    }

    // Parser la réponse JSON
    let api_response: UploadApiResponse = response
        .json()
        .await
        .map_err(|e| format!("Erreur lecture réponse: {}", e))?;

    if !api_response.success {
        return Err(
            api_response
                .message
                .unwrap_or_else(|| "Erreur inconnue du serveur".to_string()),
        );
    }

    // Extraire l'ID de la photo créée sur le serveur
    let photo_id = api_response.data.and_then(|d| d.photo_id);
    Ok(photo_id)
}

// ─── Worker d'upload ────────────────────────────────────────

/// Un worker d'upload.
///
/// Tourne en boucle infinie, prend les fichiers du canal un par un,
/// et les uploade avec retry. Plusieurs workers tournent en parallèle
/// (le photographe choisit 1, 2 ou 3 dans l'écran de configuration).
///
/// Pendant une PAUSE :
/// - Le worker vérifie `paused` avant chaque upload
/// - S'il est en pause, il attend 500ms et revérifie (boucle d'attente)
/// - L'upload en cours n'est PAS interrompu (il finit proprement)
///
/// Pendant un ARRÊT :
/// - `running` passe à false
/// - Le worker sort de sa boucle à la prochaine itération
async fn upload_worker(
    worker_id: u32,
    mut rx: mpsc::UnboundedReceiver<FileJob>,
    config: UploadConfig,
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    app_handle: tauri::AppHandle,
) {
    let client = match create_http_client() {
        Ok(c) => c,
        Err(e) => {
            error!("Worker {} : impossible de créer le client HTTP: {}", worker_id, e);
            return;
        }
    };

    info!("Worker {} démarré", worker_id);

    // Boucle principale : prendre un fichier du canal et l'uploader
    while let Some(job) = rx.recv().await {
        // Vérifier si l'arrêt a été demandé
        if !running.load(Ordering::Relaxed) {
            info!("Worker {} : arrêt demandé, terminaison", worker_id);
            break;
        }

        // Attendre si en pause (vérifier toutes les 500ms)
        while paused.load(Ordering::Relaxed) && running.load(Ordering::Relaxed) {
            tokio::time::sleep(Duration::from_millis(500)).await;
        }
        // Revérifier running après la pause
        if !running.load(Ordering::Relaxed) {
            break;
        }

        // ── Tentatives d'upload avec retry ──
        let mut success = false;
        let start_time = Instant::now();

        for attempt in 0..MAX_ATTEMPTS {
            // Mettre à jour le statut en base : "uploading"
            if attempt == 0 {
                let _ = database::update_file_status(
                    job.db_file_id,
                    "uploading",
                    None,
                    None,
                );
            }

            // Notifier le frontend : upload démarré
            let _ = app_handle.emit(
                "upload_started",
                serde_json::json!({
                    "filename": &job.filename,
                    "size": job.file_size,
                    "attempt": attempt + 1,
                }),
            );

            // Tenter l'upload
            match upload_single_file(&client, &config, &job.file_path, &job.filename).await {
                Ok(photo_id) => {
                    // ── SUCCÈS ──
                    let duration_ms = start_time.elapsed().as_millis() as u64;
                    let speed_kbps = if duration_ms > 0 {
                        (job.file_size as u64 * 1000) / (duration_ms * 1024)
                    } else {
                        0
                    };

                    // Mettre à jour SQLite
                    let _ = database::update_file_status(
                        job.db_file_id,
                        "success",
                        None,
                        photo_id,
                    );

                    // Notifier le frontend
                    let _ = app_handle.emit(
                        "upload_success",
                        serde_json::json!({
                            "filename": &job.filename,
                            "duration_ms": duration_ms,
                            "speed_kbps": speed_kbps,
                            "server_photo_id": photo_id,
                        }),
                    );

                    info!(
                        "✓ {} uploadé en {}ms ({} Ko/s)",
                        job.filename, duration_ms, speed_kbps
                    );

                    success = true;
                    break;
                }

                Err(error_msg) => {
                    // ── ÉCHEC ──
                    warn!(
                        "✗ {} — tentative {}/{} : {}",
                        job.filename,
                        attempt + 1,
                        MAX_ATTEMPTS,
                        error_msg
                    );

                    // Si token expiré, pas la peine de réessayer
                    if error_msg == "SESSION_EXPIRED" {
                        let _ = database::update_file_status(
                            job.db_file_id,
                            "failed",
                            Some(&error_msg),
                            None,
                        );
                        let _ = app_handle.emit(
                            "upload_failed",
                            serde_json::json!({
                                "filename": &job.filename,
                                "error": &error_msg,
                                "attempt": attempt + 1,
                                "max_attempts": MAX_ATTEMPTS,
                                "fatal": true,
                            }),
                        );
                        // Signaler au frontend de réafficher l'écran login
                        let _ = app_handle.emit(
                            "session_expired",
                            serde_json::json!({}),
                        );
                        return; // Arrêter ce worker
                    }

                    // Si encore des tentatives restantes → retry avec backoff
                    if attempt < MAX_ATTEMPTS - 1 {
                        let delay = RETRY_DELAYS
                            .get(attempt as usize)
                            .copied()
                            .unwrap_or(45);

                        let _ = app_handle.emit(
                            "upload_retry",
                            serde_json::json!({
                                "filename": &job.filename,
                                "attempt": attempt + 1,
                                "delay_seconds": delay,
                            }),
                        );

                        // Attendre avant de réessayer
                        tokio::time::sleep(Duration::from_secs(delay)).await;

                        // Vérifier l'arrêt pendant le retry
                        if !running.load(Ordering::Relaxed) {
                            let _ = database::update_file_status(
                                job.db_file_id,
                                "pending",
                                Some("Arrêt pendant le retry"),
                                None,
                            );
                            break;
                        }
                    } else {
                        // Dernière tentative échouée → marquer comme failed
                        let _ = database::update_file_status(
                            job.db_file_id,
                            "failed",
                            Some(&error_msg),
                            None,
                        );

                        let _ = app_handle.emit(
                            "upload_failed",
                            serde_json::json!({
                                "filename": &job.filename,
                                "error": &error_msg,
                                "attempt": attempt + 1,
                                "max_attempts": MAX_ATTEMPTS,
                                "fatal": false,
                            }),
                        );
                    }
                }
            }
        }

        if success {
            // Émettre les stats mises à jour après chaque succès
            emit_stats(job.session_id, &app_handle);
        } else {
            emit_stats(job.session_id, &app_handle);
        }

        // Vérifier si tous les fichiers sont traités
        check_all_complete(job.session_id, &app_handle);
    }

    info!("Worker {} terminé", worker_id);
}

// ─── Émission des statistiques ──────────────────────────────

/// Récupère les stats depuis SQLite et les envoie au frontend.
/// Appelé après chaque upload (succès ou échec) pour que le dashboard
/// affiche des compteurs à jour en temps réel.
fn emit_stats(session_id: i64, app_handle: &tauri::AppHandle) {
    if let Ok(stats) = database::get_session_stats(session_id) {
        let _ = app_handle.emit(
            "stats_updated",
            serde_json::json!({
                "sent": stats.sent,
                "pending": stats.pending + stats.uploading,
                "failed": stats.failed,
                "speed_kbps": 0,
            }),
        );
    }
}

/// Vérifie si tous les fichiers de la session ont été traités.
/// Si pending == 0 et uploading == 0, émet l'événement `all_complete`.
fn check_all_complete(session_id: i64, app_handle: &tauri::AppHandle) {
    if let Ok(stats) = database::get_session_stats(session_id) {
        if stats.pending == 0 && stats.uploading == 0 {
            let _ = app_handle.emit(
                "all_complete",
                serde_json::json!({
                    "total_sent": stats.sent,
                    "total_failed": stats.failed,
                }),
            );
        }
    }
}

// ─── Ping réseau ────────────────────────────────────────────

/// Tâche de ping périodique pour détecter les coupures réseau.
///
/// Fait un GET léger vers l'API toutes les 10 secondes quand aucun
/// upload n'est actif. Émet `connection_status` pour que le dashboard
/// affiche la pastille verte/rouge.
pub async fn start_network_monitor(
    running: Arc<AtomicBool>,
    app_handle: tauri::AppHandle,
) {
    let client = match create_http_client() {
        Ok(c) => c,
        Err(_) => return,
    };

    // URL de ping : on utilise le endpoint events sans token
    // → retourne 401 (token manquant) mais ça prouve que le serveur répond
    let ping_url = format!("{}/api/sport/events", API_BASE_URL);
    let mut was_online = true;

    while running.load(Ordering::Relaxed) {
        tokio::time::sleep(Duration::from_secs(PING_INTERVAL_SECS)).await;

        if !running.load(Ordering::Relaxed) {
            break;
        }

        let is_online = client
            .head(&ping_url)
            .header("Accept", "application/json")
            .timeout(Duration::from_secs(5))
            .send()
            .await
            .is_ok();

        // Ne notifier que quand l'état change (éviter le spam d'événements)
        if is_online != was_online {
            let _ = app_handle.emit(
                "connection_status",
                serde_json::json!({ "online": is_online }),
            );
            if is_online {
                info!("Connexion rétablie");
            } else {
                warn!("Connexion perdue");
            }
            was_online = is_online;
        }
    }
}

// ─── Démarrage du système d'upload ──────────────────────────

/// Démarre les workers d'upload.
///
/// Crée N tâches tokio (N = `config.parallel`, entre 1 et 3).
/// Chaque worker écoute le même canal `rx` — tokio distribue les
/// messages automatiquement (le premier worker libre prend le fichier).
///
/// Pour utiliser un seul `rx` avec plusieurs workers, on utilise
/// `Arc<Mutex<mpsc::UnboundedReceiver>>` — chaque worker verrouille
/// brièvement le canal pour prendre un message.
///
/// # Arguments
/// * `rx` — Récepteur du canal depuis le watcher
/// * `config` — Configuration (token, event_id, checkpoint_id, parallel)
/// * `running` — Flag d'arrêt partagé avec le watcher
/// * `paused` — Flag de pause (l'uploader s'arrête, le watcher continue)
/// * `app_handle` — Handle Tauri pour les événements frontend
pub fn start_upload_workers(
    rx: mpsc::UnboundedReceiver<FileJob>,
    config: UploadConfig,
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    app_handle: tauri::AppHandle,
) {
    let num_workers = config.parallel.clamp(1, 3);

    // On wrappe le receiver dans un Arc<Mutex> pour le partager entre workers.
    // Chaque worker va le verrouiller brièvement pour prendre un fichier.
    let shared_rx = Arc::new(tokio::sync::Mutex::new(rx));

    for i in 0..num_workers {
        let config_clone = config.clone();
        let running_clone = running.clone();
        let paused_clone = paused.clone();
        let app_clone = app_handle.clone();
        let rx_clone = shared_rx.clone();

        tokio::spawn(async move {
            // Créer un faux receiver qui prend du shared_rx
            // On ne peut pas cloner un UnboundedReceiver, mais on peut
            // le verrouiller via Mutex pour recv() un message à la fois
            let client = match create_http_client() {
                Ok(c) => c,
                Err(e) => {
                    error!("Worker {} : impossible de créer le client HTTP: {}", i, e);
                    return;
                }
            };

            info!("Worker {} démarré", i);

            loop {
                // Vérifier l'arrêt
                if !running_clone.load(Ordering::Relaxed) {
                    break;
                }

                // Attendre si en pause
                while paused_clone.load(Ordering::Relaxed)
                    && running_clone.load(Ordering::Relaxed)
                {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                }
                if !running_clone.load(Ordering::Relaxed) {
                    break;
                }

                // Prendre un fichier du canal (verrou bref)
                let job = {
                    let mut rx_guard = rx_clone.lock().await;
                    // try_recv pour ne pas bloquer indéfiniment avec le Mutex
                    match rx_guard.try_recv() {
                        Ok(job) => Some(job),
                        Err(mpsc::error::TryRecvError::Empty) => None,
                        Err(mpsc::error::TryRecvError::Disconnected) => {
                            info!("Worker {} : canal fermé", i);
                            return;
                        }
                    }
                };

                let job = match job {
                    Some(j) => j,
                    None => {
                        // Rien dans le canal, attendre un peu avant de revérifier
                        tokio::time::sleep(Duration::from_millis(200)).await;
                        continue;
                    }
                };

                // ── Tentatives d'upload avec retry ──
                let start_time = Instant::now();

                // Marquer comme "uploading" en base
                let _ = database::update_file_status(
                    job.db_file_id,
                    "uploading",
                    None,
                    None,
                );

                let mut final_success = false;

                for attempt in 0..MAX_ATTEMPTS {
                    // Notifier le frontend
                    let _ = app_clone.emit(
                        "upload_started",
                        serde_json::json!({
                            "filename": &job.filename,
                            "size": job.file_size,
                            "attempt": attempt + 1,
                        }),
                    );

                    match upload_single_file(
                        &client,
                        &config_clone,
                        &job.file_path,
                        &job.filename,
                    )
                    .await
                    {
                        Ok(photo_id) => {
                            // ── SUCCÈS ──
                            let duration_ms = start_time.elapsed().as_millis() as u64;
                            let speed_kbps = if duration_ms > 0 {
                                (job.file_size as u64 * 1000) / (duration_ms * 1024)
                            } else {
                                0
                            };

                            let _ = database::update_file_status(
                                job.db_file_id,
                                "success",
                                None,
                                photo_id,
                            );

                            let _ = app_clone.emit(
                                "upload_success",
                                serde_json::json!({
                                    "filename": &job.filename,
                                    "duration_ms": duration_ms,
                                    "speed_kbps": speed_kbps,
                                    "server_photo_id": photo_id,
                                }),
                            );

                            info!(
                                "Worker {} ✓ {} uploadé en {}ms ({} Ko/s)",
                                i, job.filename, duration_ms, speed_kbps
                            );

                            final_success = true;
                            break;
                        }

                        Err(error_msg) => {
                            warn!(
                                "Worker {} ✗ {} — tentative {}/{} : {}",
                                i,
                                job.filename,
                                attempt + 1,
                                MAX_ATTEMPTS,
                                error_msg
                            );

                            // Token expiré → arrêter tout
                            if error_msg == "SESSION_EXPIRED" {
                                let _ = database::update_file_status(
                                    job.db_file_id,
                                    "failed",
                                    Some(&error_msg),
                                    None,
                                );
                                let _ = app_clone.emit(
                                    "session_expired",
                                    serde_json::json!({}),
                                );
                                return;
                            }

                            if attempt < MAX_ATTEMPTS - 1 {
                                let delay = RETRY_DELAYS
                                    .get(attempt as usize)
                                    .copied()
                                    .unwrap_or(45);

                                let _ = app_clone.emit(
                                    "upload_retry",
                                    serde_json::json!({
                                        "filename": &job.filename,
                                        "attempt": attempt + 1,
                                        "delay_seconds": delay,
                                    }),
                                );

                                tokio::time::sleep(Duration::from_secs(delay)).await;

                                if !running_clone.load(Ordering::Relaxed) {
                                    let _ = database::update_file_status(
                                        job.db_file_id,
                                        "pending",
                                        Some("Arrêt pendant le retry"),
                                        None,
                                    );
                                    return;
                                }
                            } else {
                                let _ = database::update_file_status(
                                    job.db_file_id,
                                    "failed",
                                    Some(&error_msg),
                                    None,
                                );

                                let _ = app_clone.emit(
                                    "upload_failed",
                                    serde_json::json!({
                                        "filename": &job.filename,
                                        "error": &error_msg,
                                        "attempt": attempt + 1,
                                        "max_attempts": MAX_ATTEMPTS,
                                        "fatal": false,
                                    }),
                                );
                            }
                        }
                    }
                }

                // Émettre les stats après chaque fichier traité
                emit_stats(job.session_id, &app_clone);
                if final_success || !final_success {
                    check_all_complete(job.session_id, &app_clone);
                }
            }

            info!("Worker {} terminé", i);
        });
    }

    // Lancer le moniteur réseau en parallèle
    let running_net = running.clone();
    let app_net = app_handle.clone();
    tokio::spawn(async move {
        start_network_monitor(running_net, app_net).await;
    });
}
