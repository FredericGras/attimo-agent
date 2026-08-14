// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Commandes IPC (commands.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// Ce fichier est le "tableau de commandes" de l'application.
// Chaque fonction #[tauri::command] correspond à un bouton que le
// frontend JavaScript peut presser via `await invoke('nom', { params })`.
//
// SAAS 240 — Phase 6 (Vague 2C) :
//   - REMPLACÉ : la commande `login(email, password)` → `validate_app_password(token)`
//   - Inchangées : logout, get_stored_auth, fetch_events, fetch_checkpoints,
//                  start_session, pause_session, resume_session, stop_session,
//                  get_session_stats, retry_failed, retry_file
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

/// Réponse de la validation d'un app password.
/// Conserve le même format que l'ancienne LoginResponse pour préserver
/// la compatibilité du frontend (token + user).
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
// COMMANDES AUTH (SAAS 240 — Phase 6)
// ═══════════════════════════════════════════════════════════════════════

// ─── Commande: validate_app_password ───
//
// Remplace l'ancienne commande `login(email, password, remember)`.
// Appelée par le JS quand le photographe colle son mot de passe d'application
// dans le formulaire de connexion :
//
//   await invoke('validate_app_password', { token: 'attimo_pat_xxx', remember: true })
//
// Le serveur Attimo valide le token et renvoie les infos utilisateur.
// Si remember=true, le token est sauvegardé localement pour les prochains
// lancements de l'agent (auto-login).

#[tauri::command]
pub async fn validate_app_password(
    token: String,
    remember: bool,
) -> Result<LoginResponse, String> {
    let auth_data = auth::validate_app_password(&token).await?;

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
// COMMANDES SESSION (Phase 4 — inchangées)
// ═══════════════════════════════════════════════════════════════════════

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
    let folder_path = PathBuf::from(&folder);
    if !folder_path.exists() || !folder_path.is_dir() {
        return Err(format!("Le dossier n'existe pas : {}", folder));
    }

    {
        let mut session_lock = state.active_session.lock().await;
        if let Some(mut prev) = session_lock.take() {
            info!("Arrêt de la session précédente (id={})", prev.session_id);
            prev.running.store(false, Ordering::Relaxed);
            prev.watcher_handle.take();
            let _ = database::stop_session(prev.session_id);
        }
    }

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

    let running = Arc::new(AtomicBool::new(true));
    let paused = Arc::new(AtomicBool::new(false));

    let (tx, rx) = mpsc::unbounded_channel::<watcher::FileJob>();
    let tx_for_state = tx.clone();

    let watcher_handle = watcher::start_watching(
        folder_path,
        false,
        session_id,
        include_existing,
        tx,
        running.clone(),
        app_handle.clone(),
    )?;

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

#[tauri::command]
pub async fn pause_session(state: tauri::State<'_, AppState>) -> Result<(), String> {
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

#[tauri::command]
pub async fn resume_session(state: tauri::State<'_, AppState>) -> Result<(), String> {
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

#[tauri::command]
pub async fn stop_session(state: tauri::State<'_, AppState>) -> Result<(), String> {
    let mut session_lock = state.active_session.lock().await;
    match session_lock.take() {
        Some(mut session) => {
            info!("Arrêt de la session #{}", session.session_id);
            session.running.store(false, Ordering::Relaxed);
            session.watcher_handle.take();
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
    let failed_files = database::get_failed_files(session_id)
        .map_err(|e| format!("Erreur base de données: {}", e))?;

    let count = database::reset_failed_files(session_id)
        .map_err(|e| format!("Erreur base de données: {}", e))?;

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

// ═══════════════════════════════════════════════════════════════════════
// COMMANDES CAPTURE VIDÉO (SAAS 430)
// ═══════════════════════════════════════════════════════════════════════

/// Vérifie que le FFmpeg embarqué répond.
#[tauri::command]
pub async fn check_ffmpeg(app: tauri::AppHandle) -> Result<String, String> {
    crate::capture::check_ffmpeg(&app).await
}

/// Liste les caméras et micros vus par Windows.
#[tauri::command]
pub async fn list_video_devices(
    app: tauri::AppHandle,
) -> Result<Vec<crate::capture::VideoDevice>, String> {
    crate::capture::list_devices(&app).await
}

/// Interroge un périphérique sur les résolutions qu'il accepte.
#[tauri::command]
pub async fn probe_video_device(
    app: tauri::AppHandle,
    device_name: String,
) -> Result<crate::capture::DeviceCapabilities, String> {
    crate::capture::probe_device(&app, &device_name).await
}

// ═══════════════════════════════════════════════════════════════════════
// ENREGISTREMENT VIDÉO (SAAS 430)
// ═══════════════════════════════════════════════════════════════════════

/// Calcule le découpage sans rien enregistrer.
///
/// Permet à l'interface de montrer au photographe ce que ses réglages
/// impliquent — durée des morceaux, nombre par clip — avant qu'il ne lance
/// une captation de plusieurs heures.
#[tauri::command]
pub async fn plan_recording(
    config: crate::recorder::RecordingConfig,
) -> Result<crate::recorder::SegmentPlan, String> {
    config.plan()
}

/// Démarre la captation.
#[tauri::command]
pub async fn start_recording(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    config: crate::recorder::RecordingConfig,
    session_id: String,
    attimo_tenant_id: i64,
    attimo_event_id: i64,
    attimo_checkpoint_id: Option<i64>,
    interval_secs: f32,
) -> Result<crate::recorder::SegmentPlan, String> {
    // Une seule captation à la fois : deux enregistrements simultanés sur le
    // même périphérique échoueraient, et sur deux périphériques ils se
    // disputeraient le disque.
    {
        let mut verrou = state.active_recording.lock().await;

        if let Some(mut precedente) = verrou.take() {
            info!("Arrêt de la captation précédente");
            precedente.stop();
        }
    }

    let plan = config.plan()?;
    let dossier = std::path::PathBuf::from(&config.output_dir);
    let config_manifeste = config.clone();

    // Espace disque (SAAS 430).
    //
    // Le débit de la version légère n'est pas dans les réglages de captation
    // — c'est l'interface qui le passe à `generate_proxy` au moment voulu. On
    // retient donc la valeur par défaut, qui est celle réellement employée.
    let flux = crate::disk::FluxParams::depuis_config(&config_manifeste, &plan, None, Some(interval_secs));

    let estimation = crate::disk::estimer(&dossier, &flux)?;

    // Refus de démarrer plutôt qu'arrêt au bout de trois minutes : mieux vaut
    // que le photographe l'apprenne maintenant, quand il peut encore libérer
    // de la place ou changer de disque.
    if estimation.autonomy_secs < 600 {
        return Err(format!(
            "Espace disque insuffisant : {} Go utilisables pour {} Go/h. \
             Libérez de la place ou choisissez un autre disque.",
            estimation.usable_bytes / 1_073_741_824,
            estimation.total_per_hour / 1_073_741_824
        ));
    }

    info!(
        "Captation {} : {} Go/h estimés, {} h d'autonomie",
        session_id,
        estimation.total_per_hour / 1_073_741_824,
        estimation.autonomy_secs / 3600
    );

    let poignee = crate::recorder::start_recording(&app, config, session_id.clone()).await?;
    let debut = poignee.started_at;

    // Le drapeau est celui de la captation : la surveillance ne peut donc pas
    // lui survivre.
    let running = poignee.running.clone();

    // Le manifeste naît avec la session, pas à la fin : il doit être
    // exploitable pendant la course, car l'envoi peut démarrer alors que la
    // captation continue.
    let writer = crate::manifest::ManifestWriter::new(
        &dossier,
        session_id,
        &config_manifeste,
        &plan,
        crate::manifest::AttimoInfo {
            tenant_id: attimo_tenant_id,
            event_id: attimo_event_id,
            checkpoint_id: attimo_checkpoint_id,
        },
        debut,
        interval_secs,
    )?;

    {
        let mut verrou = state.active_recording.lock().await;
        *verrou = Some(poignee);
    }

    {
        let mut verrou = state.manifest_writer.lock().await;
        *verrou = Some(writer);
    }

    // Un nouveau suivi de clips pour cette session.
    {
        let mut verrou = state.clip_tracker.lock().await;
        *verrou = None;
    }

    // Surveillance de l'espace disque (SAAS 430).
    //
    // Lancée en dernier, une fois l'état complet : elle peut arrêter la
    // captation, ce qui suppose de trouver le manifeste en place.
    crate::disk::surveiller(app.clone(), dossier, flux, running);

    Ok(plan)
}

/// Assemble un clip si les morceaux nécessaires sont prêts.
///
/// Appelée par le frontend à chaque morceau terminé. Ce découplage est
/// volontaire : l'assemblage ne doit jamais ralentir la captation, car
/// perdre des images est irrattrapable alors qu'assembler avec quelques
/// secondes de retard ne se voit pas.
#[tauri::command]
pub async fn on_segment_ready(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    segment_index: u32,
) -> Result<Option<crate::assembler::AssembledClip>, String> {
    let verrou = state.active_recording.lock().await;

    let Some(enregistrement) = verrou.as_ref() else {
        return Ok(None);
    };

    let plan = enregistrement.plan.clone();
    let dossier = enregistrement.work_dir.clone();
    let session = enregistrement.session_id.clone();

    // Le verrou est relâché avant l'assemblage : celui-ci peut durer
    // plusieurs secondes, et le garder bloquerait l'arrêt de la captation.
    drop(verrou);

    let mut suivi = state.clip_tracker.lock().await;

    if suivi.is_none() {
        *suivi = Some(crate::assembler::ClipTracker::new(plan, &dossier)?);
    }

    let tracker = suivi.as_mut().unwrap();

    let Some(morceaux) = tracker.segment_ready(segment_index) else {
        return Ok(None);
    };

    let clip = crate::assembler::assembler_clip(&app, tracker, &morceaux, &session).await?;

    tracker.clip_termine();

    drop(suivi);

    // Le manifeste est réécrit à chaque clip : il doit refléter l'état réel
    // à tout instant, pas seulement en fin de session.
    {
        let mut verrou = state.manifest_writer.lock().await;

        if let Some(writer) = verrou.as_mut() {
            writer.ajouter_clip(&clip, true)?;
        }
    }

    Ok(Some(clip))
}

/// Extrait les images d'analyse d'un morceau.
///
/// Séparée de l'assemblage des clips : les deux opérations portent sur le
/// même morceau mais n'ont rien à voir. L'une prépare ce qui se vend,
/// l'autre ce qui permet de le retrouver. Les mener indépendamment évite
/// qu'un échec de l'une n'empêche l'autre.
#[tauri::command]
pub async fn extract_frames(
    state: tauri::State<'_, AppState>,
    app: tauri::AppHandle,
    segment_index: u32,
    interval_secs: f32,
) -> Result<crate::frames::ExtractionResult, String> {
    let verrou = state.active_recording.lock().await;

    let Some(enregistrement) = verrou.as_ref() else {
        return Err("Aucune captation en cours.".to_string());
    };

    let dossier = enregistrement.work_dir.clone();
    let plan = enregistrement.plan;
    let debut_session = enregistrement.started_at;

    drop(verrou);

    // Instant absolu du début de ce morceau : début de la captation, plus le
    // numéro du morceau multiplié par sa durée.
    //
    // Ce calcul suppose une captation continue, sans coupure. C'est le cas
    // nominal ; une reprise après perte de flux devra recaler ce point.
    let decalage_ms = (segment_index as i64) * (plan.segment_secs as i64) * 1000;
    let debut_morceau = debut_session.plus_millis(decalage_ms);

    let config = crate::frames::FrameConfig { interval_secs };

    let resultat =
        crate::frames::extraire_images(&app, &dossier, segment_index, debut_morceau, &config)
            .await?;

    // Index en ajout seul : jamais relu, jamais réécrit.
    {
        let mut verrou = state.manifest_writer.lock().await;

        if let Some(writer) = verrou.as_mut() {
            writer.ajouter_images(&resultat.frames)?;
        }
    }

    Ok(resultat)
}

// ═══════════════════════════════════════════════════════════════════════
// ENVOI VERS LE SERVEUR (SAAS 430)
// ═══════════════════════════════════════════════════════════════════════

/// Envoie un clip au serveur.
///
/// Chaque variante part indépendamment. L'appariement se fait côté serveur
/// sur le couple (session, numéro de clip) : le premier fichier arrivé crée
/// la fiche, le second la complète. L'ordre est donc indifférent — ce qui
/// compte, car un envoi peut échouer et repartir plus tard.
#[tauri::command]
pub async fn upload_clip(
    token: String,
    event_id: i64,
    checkpoint_id: Option<i64>,
    session_id: String,
    clip_path: String,
    variant: String,
    clip_index: u32,
    started_at: String,
    ended_at: String,
) -> Result<crate::video_uploader::ClipInfo, String> {
    let config = crate::video_uploader::VideoUploadConfig {
        token,
        event_id,
        checkpoint_id,
        session_id,
    };

    let variante = match variant.as_str() {
        "proxy" => crate::video_uploader::ClipVariant::Proxy,
        "hd" => crate::video_uploader::ClipVariant::Hd,
        autre => return Err(format!("Variante inconnue : {}", autre)),
    };

    crate::video_uploader::envoyer_clip(
        &config,
        std::path::Path::new(&clip_path),
        variante,
        clip_index,
        &started_at,
        &ended_at,
    )
    .await
}

/// Envoie un lot d'images d'analyse.
///
/// Ce flux est le plus léger des quatre — 1,3 Mb/s contre 33 pour les clips
/// pleine qualité — et pourtant le plus important : sans lui aucun dossard
/// n'est lu, donc personne ne retrouve ses vidéos.
#[tauri::command]
pub async fn upload_frames(
    token: String,
    event_id: i64,
    checkpoint_id: Option<i64>,
    session_id: String,
    frames: Vec<serde_json::Value>,
) -> Result<crate::video_uploader::FrameTotals, String> {
    let config = crate::video_uploader::VideoUploadConfig {
        token,
        event_id,
        checkpoint_id,
        session_id,
    };

    let images: Vec<crate::video_uploader::FrameToUpload> = frames
        .iter()
        .filter_map(|v| {
            Some(crate::video_uploader::FrameToUpload {
                path: v.get("path")?.as_str()?.to_string(),
                instant_at: v.get("instant_at")?.as_str()?.to_string(),
            })
        })
        .collect();

    crate::video_uploader::envoyer_images(&config, &images).await
}

/// Demande au serveur ce qu'il a déjà reçu pour cette session.
///
/// À interroger avant de réémettre après une coupure : sans cela, une
/// reprise reposterait des gigaoctets déjà reçus.
#[tauri::command]
pub async fn clips_status(
    token: String,
    event_id: i64,
    session_id: String,
) -> Result<crate::video_uploader::SessionStatus, String> {
    let config = crate::video_uploader::VideoUploadConfig {
        token,
        event_id,
        checkpoint_id: None,
        session_id,
    };

    crate::video_uploader::etat_session(&config).await
}

// ═══════════════════════════════════════════════════════════════════════
// FILE D'ATTENTE VIDÉO (SAAS 430)
// ═══════════════════════════════════════════════════════════════════════

/// Met un fichier en attente d'envoi.
///
/// Rien ne part directement : tout passe par la file. C'est ce qui permet au
/// photographe de couper l'envoi vidéo sans rien perdre, et de le reprendre
/// plus tard — y compris après avoir fermé l'agent.
#[tauri::command]
pub async fn queue_video_file(
    session_id: String,
    event_id: i64,
    kind: String,
    file_path: String,
    clip_index: Option<u32>,
    started_at: Option<String>,
    ended_at: Option<String>,
    instant_at: Option<String>,
) -> Result<i64, String> {
    let nature = match kind.as_str() {
        "frames" => crate::video_queue::QueueKind::Frames,
        "clip_proxy" => crate::video_queue::QueueKind::ClipProxy,
        "clip_hd" => crate::video_queue::QueueKind::ClipHd,
        autre => return Err(format!("Nature inconnue : {}", autre)),
    };

    let conn = database::connexion().map_err(|e| format!("Base indisponible : {}", e))?;

    crate::video_queue::enfiler(
        &conn,
        &session_id,
        event_id,
        nature,
        &file_path,
        clip_index,
        started_at.as_deref(),
        ended_at.as_deref(),
        instant_at.as_deref(),
    )
}

/// Ce qui attend d'être envoyé.
///
/// Le photographe doit voir ce chiffre avant de décider d'activer l'envoi
/// vidéo : sans lui, il bascule à l'aveugle sur un réseau qu'il ne connaît
/// pas.
#[tauri::command]
pub async fn video_queue_stats(event_id: i64) -> Result<crate::video_queue::QueueStats, String> {
    let conn = database::connexion().map_err(|e| format!("Base indisponible : {}", e))?;

    crate::video_queue::statistiques(&conn, event_id)
}

/// Autorise ou interdit l'envoi des clips pleine qualité.
#[tauri::command]
pub async fn set_hd_upload(
    state: tauri::State<'_, AppState>,
    allow: bool,
) -> Result<(), String> {
    let mut verrou = state.allow_hd_upload.lock().await;
    *verrou = allow;

    info!(
        "Envoi des clips pleine qualité : {}",
        if allow { "autorisé" } else { "suspendu" }
    );

    Ok(())
}

/// Envoie le prochain élément de la file.
///
/// Un seul à la fois, appelé en boucle par le frontend. Ce découpage permet
/// d'afficher l'avancement, et surtout d'interrompre proprement : un
/// photographe qui coupe l'envoi vidéo voit l'effet au fichier suivant, pas
/// au bout de trois clips.
#[tauri::command]
pub async fn process_video_queue(
    state: tauri::State<'_, AppState>,
    token: String,
    event_id: i64,
    checkpoint_id: Option<i64>,
) -> Result<Option<serde_json::Value>, String> {
    let autoriser_hd = *state.allow_hd_upload.lock().await;

    let conn = database::connexion().map_err(|e| format!("Base indisponible : {}", e))?;

    let elements = crate::video_queue::prochains(&conn, event_id, autoriser_hd, 1)?;

    let Some(element) = elements.into_iter().next() else {
        return Ok(None);
    };

    // Reprise après coupure (SAAS 430).
    //
    // Avant le premier envoi d'une session, on demande au serveur ce qu'il
    // détient déjà. La file sait ce qu'elle a tenté d'envoyer, pas ce qui est
    // arrivé : un envoi coupé au dernier octet lui est indiscernable d'un
    // envoi jamais commencé.
    //
    // Le verrou est tenu pendant l'appel réseau, volontairement : deux
    // passages simultanés interrogeraient le serveur deux fois pour rien.
    {
        let mut recoupees = state.reconciled_sessions.lock().await;

        if !recoupees.contains(&element.session_id) {
            let sonde = crate::video_uploader::VideoUploadConfig {
                token: token.clone(),
                event_id,
                checkpoint_id,
                session_id: element.session_id.clone(),
            };

            // En cas d'échec, la session n'est pas marquée : on réessaiera au
            // passage suivant. Mieux vaut redemander que de repartir aveugle.
            let etat = crate::video_uploader::etat_session(&sonde).await?;

            let deja: Vec<(u32, bool, bool)> = etat
                .clips
                .iter()
                .map(|c| (c.index, c.has_proxy, c.has_hd))
                .collect();

            let bilan = crate::video_queue::reconcilier(&conn, &element.session_id, &deja)?;

            recoupees.insert(element.session_id.clone());

            if bilan.skipped > 0 {
                info!(
                    "Reprise {} : {} fichier(s) déjà reçus, {} Mo épargnés",
                    element.session_id,
                    bilan.skipped,
                    bilan.bytes_saved / 1_048_576
                );

                // L'élément retenu vient peut-être d'être écarté. On rend la
                // main plutôt que de l'envoyer pour rien : le passage suivant,
                // dans une seconde, repartira d'une file assainie.
                return Ok(Some(serde_json::json!({
                    "reconciled": bilan.skipped,
                    "bytes_saved": bilan.bytes_saved,
                })));
            }
        }
    }

    // Réservation atomique. Si un autre passage l'a déjà prise, on ne fait
    // rien : c'est ce qui empêche d'envoyer plusieurs fois le même fichier.
    if !crate::video_queue::reserver(&conn, element.id)? {
        return Ok(None);
    }

    // Une ligne héritée d'une base antérieure n'a pas d'événement. On lui
    // attribue celui qui est ouvert : c'est le seul rattachement possible,
    // et il vaut mieux que de la laisser flotter d'une course à l'autre.
    let _ = conn.execute(
        "UPDATE video_queue SET event_id = ?2 WHERE id = ?1 AND event_id IS NULL",
        rusqlite::params![element.id, event_id],
    );

    let config = crate::video_uploader::VideoUploadConfig {
        token,
        event_id,
        checkpoint_id,
        session_id: element.session_id.clone(),
    };

    let resultat = match element.kind {
        crate::video_queue::QueueKind::Frames => {
            let images = vec![crate::video_uploader::FrameToUpload {
                path: element.file_path.clone(),
                instant_at: element.instant_at.clone().unwrap_or_default(),
            }];

            crate::video_uploader::envoyer_images(&config, &images)
                .await
                .map(|t| serde_json::json!({ "kind": "frames", "totals": t }))
        }
        crate::video_queue::QueueKind::ClipProxy | crate::video_queue::QueueKind::ClipHd => {
            let variante = if element.kind == crate::video_queue::QueueKind::ClipProxy {
                crate::video_uploader::ClipVariant::Proxy
            } else {
                crate::video_uploader::ClipVariant::Hd
            };

            crate::video_uploader::envoyer_clip(
                &config,
                std::path::Path::new(&element.file_path),
                variante,
                element.clip_index.unwrap_or(0),
                element.started_at.as_deref().unwrap_or(""),
                element.ended_at.as_deref().unwrap_or(""),
            )
            .await
            .map(|c| serde_json::json!({ "kind": "clip", "clip": c }))
        }
    };

    match resultat {
        Ok(detail) => {
            crate::video_queue::marquer_envoye(&conn, element.id)?;

            Ok(Some(serde_json::json!({
                "id": element.id,
                "file": element.file_path,
                "detail": detail,
            })))
        }
        Err(e) => {
            // Un token expiré n'est pas un échec du fichier : inutile de
            // consommer une tentative, il faut se reconnecter.
            if e == "SESSION_EXPIRED" {
                // Ce n'est pas le fichier qui est en cause : on le remet en
                // file sans consommer de tentative.
                crate::video_queue::liberer(&conn, element.id)?;
                return Err(e);
            }

            let abandonne = crate::video_queue::marquer_echec(&conn, element.id, &e)?;

            Ok(Some(serde_json::json!({
                "id": element.id,
                "file": element.file_path,
                "error": e,
                "abandoned": abandonne,
            })))
        }
    }
}

/// Remet en file les éléments abandonnés.
#[tauri::command]
pub async fn retry_video_queue() -> Result<usize, String> {
    let conn = database::connexion().map_err(|e| format!("Base indisponible : {}", e))?;

    crate::video_queue::relancer_echecs(&conn)
}

/// Produit la version légère d'un clip.
///
/// Séparée de l'assemblage : c'est la seule opération qui réencode, donc la
/// plus lente. La lancer à part permet au clip pleine qualité d'exister —
/// et d'être envoyé — sans attendre.
#[tauri::command]
pub async fn generate_proxy(
    app: tauri::AppHandle,
    clip_path: String,
    height: u32,
    bitrate_mbps: u32,
) -> Result<String, String> {
    let chemin = std::path::PathBuf::from(&clip_path);

    let proxy = crate::assembler::generer_proxy(&app, &chemin, height, bitrate_mbps).await?;

    Ok(proxy.to_string_lossy().to_string())
}

/// Arrête la captation en cours.
#[tauri::command]
pub async fn stop_recording(state: tauri::State<'_, AppState>) -> Result<(), String> {
    arreter(&state).await
}

/// Arrête la captation depuis l'intérieur du programme.
///
/// La surveillance disque doit pouvoir couper une captation sans passer par
/// l'interface — un poste saturé ne doit pas dépendre d'un clic pour cesser
/// d'écrire. Elle ne dispose que d'un `AppHandle`, d'où cette porte d'entrée
/// parallèle à la commande.
pub(crate) async fn arreter_captation(app: &tauri::AppHandle) -> Result<(), String> {
    use tauri::Manager;

    arreter(&app.state::<AppState>()).await
}

/// Corps commun aux deux points d'entrée.
async fn arreter(state: &AppState) -> Result<(), String> {
    let mut verrou = state.active_recording.lock().await;

    match verrou.take() {
        Some(mut poignee) => {
            info!("Arrêt de la captation {}", poignee.session_id);
            poignee.stop();

            drop(verrou);

            // Clôture du manifeste : renseigne l'heure de fin, ce qui permet
            // au serveur de distinguer une session terminée d'une session
            // interrompue.
            {
                let mut verrou_manifeste = state.manifest_writer.lock().await;

                if let Some(writer) = verrou_manifeste.as_mut() {
                    let fin = crate::frames::chrono_simple::Instant::maintenant();
                    writer.cloturer(fin)?;
                }
            }

            Ok(())
        }
        None => Err("Aucune captation en cours.".to_string()),
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ESPACE DISQUE (SAAS 430)
// ═══════════════════════════════════════════════════════════════════════

/// Estime l'occupation disque d'une captation, sans rien enregistrer.
///
/// Appelée dès que le photographe touche un réglage : c'est ce qui lui
/// permet de voir qu'un débit plus élevé ou un chevauchement plus large lui
/// coûtera deux heures d'autonomie, avant qu'il ne parte sur le terrain.
#[tauri::command]
pub async fn disk_estimate(
    config: crate::recorder::RecordingConfig,
    proxy_bitrate_mbps: Option<u32>,
    interval_secs: Option<f32>,
) -> Result<crate::disk::DiskEstimate, String> {
    let plan = config.plan()?;
    let dossier = PathBuf::from(&config.output_dir);

    let flux = crate::disk::FluxParams::depuis_config(
        &config,
        &plan,
        proxy_bitrate_mbps,
        interval_secs,
    );

    crate::disk::estimer(&dossier, &flux)
}
