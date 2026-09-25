// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Module Uploader (envoi HTTP parallèle)
// ═══════════════════════════════════════════════════════════════════════
//
// Ce module est l'« expéditeur » de l'Agent Terrain.
// Il reçoit les fichiers détectés par le watcher via un canal tokio,
// et les uploade vers l'API Attimo en parallèle (1 à 6 workers).
//
// Flux concret :
//   1. Le watcher détecte IMG_1234.JPG et l'envoie dans le canal
//   2. Un worker libre prend le fichier du canal
//   3. Il lit le fichier depuis le disque
//   4. Il envoie le fichier en POST multipart vers /api/sport/upload
//   5. Si succès → met à jour SQLite + notifie le frontend
//   6. Si échec → retry avec backoff exponentiel (5s → 15s → 45s)
//   7. Après 3 échecs → marque le fichier comme "failed" dans SQLite
//
// 429 et 503 (serveur saturé) sont traités à part (0.3.1) : on attend le
// délai que le serveur indique (Retry-After), on réduit temporairement le
// nombre d'envois simultanés, et un 429 ne compte jamais comme un échec.
//
// Chaque worker est une tâche tokio indépendante qui tourne en boucle.
// La PAUSE arrête les workers (ils finissent l'upload en cours puis attendent).
// L'ARRÊT termine les workers proprement.
//

use std::path::Path;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tauri::Emitter;
use tokio::sync::mpsc;
use log::{info, warn, error};

use crate::database;
use crate::watcher::FileJob;

// ─── Constantes ─────────────────────────────────────────────

/// URL de base de l'API Attimo (même que dans auth.rs)
/// URL injectée à la COMPILATION (voir auth.rs). Build via build-prod.bat / build-preprod.bat.
const API_BASE_URL: &str = env!("ATTIMO_API_URL");

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

/// Envois simultanés au plus (0.3.1 : 6 au lieu de 3).
///
/// Le serveur répond désormais dès la réception : une requête n'occupe plus
/// un processus PHP pendant deux secondes de traitement d'image, et 3 envois
/// suffisent déjà à remplir une 4G. 6 laisse de la marge aux bonnes liaisons.
pub const MAX_PARALLEL: u32 = 6;

/// Attente par défaut quand le serveur sature sans dire combien de temps.
const ATTENTE_429_DEFAUT_SECS: u64 = 5;
const ATTENTE_503_DEFAUT_SECS: u64 = 10;

/// Plafond de toute attente imposée par le serveur. Un Retry-After aberrant
/// ne doit pas figer l'agent une heure sur le terrain.
const ATTENTE_MAX_SECS: u64 = 300;

/// Après une saturation, le parallélisme remonte d'un cran toutes les
/// trente secondes sans nouvel incident, jusqu'au réglage du photographe.
const REMONTEE_SECS: u64 = 30;

// ─── Photos en cours (priorité sur la vidéo) ────────────────

/// Photos inscrites et pas encore traitées (en attente ou en cours d'envoi).
///
/// Lu par l'envoi vidéo : tant que ce compteur n'est pas à zéro, les clips
/// et les images d'analyse cèdent la liaison aux photos, qui se vendent tout
/// de suite. Un simple compteur plutôt qu'une requête SQLite : il est lu
/// avant chaque morceau de clip.
static PHOTOS_EN_COURS: AtomicUsize = AtomicUsize::new(0);

/// Une photo vient d'entrer dans le canal d'envoi.
pub fn photo_ajoutee() {
    PHOTOS_EN_COURS.fetch_add(1, Ordering::Relaxed);
}

/// Une photo a quitté le circuit (envoyée, en échec, ou rendue à l'arrêt).
fn photo_terminee() {
    // Jamais sous zéro, même si un arrêt a déjà remis le compteur à zéro.
    let _ = PHOTOS_EN_COURS.fetch_update(Ordering::Relaxed, Ordering::Relaxed, |n| {
        Some(n.saturating_sub(1))
    });
}

/// Nombre de photos qui attendent la liaison.
pub fn photos_en_cours() -> usize {
    PHOTOS_EN_COURS.load(Ordering::Relaxed)
}

/// Remise à zéro au démarrage et à l'arrêt d'une session : les fichiers
/// restés dans le canal d'une session close ne partiront plus.
pub fn reinitialiser_photos_en_cours() {
    PHOTOS_EN_COURS.store(0, Ordering::Relaxed);
    PHOTOS_EN_PAUSE.store(false, Ordering::Relaxed);
}

/// Envoi des photos mis en pause par le photographe.
///
/// Les photos en attente ne partent pas : la vidéo n'a alors aucune raison
/// de leur céder la liaison.
static PHOTOS_EN_PAUSE: AtomicBool = AtomicBool::new(false);

pub fn photos_en_pause(pause: bool) {
    PHOTOS_EN_PAUSE.store(pause, Ordering::Relaxed);
}

/// Des photos attendent la liaison et vont partir : la vidéo passe après.
pub fn photos_prioritaires() -> bool {
    photos_en_cours() > 0 && !PHOTOS_EN_PAUSE.load(Ordering::Relaxed)
}

// ─── Régulation en cas de saturation (429 / 503) ────────────

/// Nombre d'envois simultanés réellement autorisés.
///
/// Le photographe choisit un parallélisme ; quand le serveur répond 429 ou
/// 503, on le divise par deux et on suspend tout nouvel envoi pendant le
/// délai demandé. Il remonte ensuite d'un cran toutes les 30 s sans incident.
/// Mieux vaut ralentir de soi-même que brûler les trois tentatives d'une
/// photo contre un serveur qui demande d'attendre.
pub struct Regulateur {
    configure: AtomicU32,
    actif: AtomicU32,
    etat: std::sync::Mutex<EtatRegulateur>,
}

#[derive(Default)]
struct EtatRegulateur {
    /// Aucun envoi ne part avant cet instant.
    reprise: Option<Instant>,
    /// Dernière saturation ou dernière remontée.
    dernier_changement: Option<Instant>,
}

impl Regulateur {
    pub fn new(configure: u32) -> Self {
        let configure = configure.clamp(1, MAX_PARALLEL);

        Self {
            configure: AtomicU32::new(configure),
            actif: AtomicU32::new(configure),
            etat: std::sync::Mutex::new(EtatRegulateur::default()),
        }
    }

    /// Envois simultanés autorisés en ce moment.
    pub fn actif(&self) -> u32 {
        self.actif.load(Ordering::Relaxed)
    }

    /// Le worker numéro `worker` (à partir de 0) peut-il prendre un fichier ?
    pub fn autorise(&self, worker: u32) -> bool {
        worker < self.actif()
    }

    /// Le serveur sature : parallélisme divisé par deux, envois suspendus.
    /// Renvoie le nouveau parallélisme.
    pub fn saturation(&self, attente: Duration, maintenant: Instant) -> u32 {
        let nouveau = (self.actif() / 2).max(1);
        self.actif.store(nouveau, Ordering::Relaxed);

        let mut etat = self.etat.lock().unwrap_or_else(|e| e.into_inner());
        let fin = maintenant + attente;

        // Plusieurs workers peuvent recevoir un 429 en même temps : on garde
        // l'échéance la plus lointaine, jamais une plus courte.
        etat.reprise = Some(match etat.reprise {
            Some(existante) if existante > fin => existante,
            _ => fin,
        });
        etat.dernier_changement = Some(maintenant);

        nouveau
    }

    /// Un envoi vient de réussir : on remonte d'un cran si le calme dure.
    pub fn succes(&self, maintenant: Instant) {
        let configure = self.configure.load(Ordering::Relaxed);

        if self.actif() >= configure {
            return;
        }

        let mut etat = self.etat.lock().unwrap_or_else(|e| e.into_inner());

        let calme = etat
            .dernier_changement
            .map_or(true, |t| maintenant.duration_since(t) >= Duration::from_secs(REMONTEE_SECS));

        if calme {
            let nouveau = (self.actif() + 1).min(configure);
            self.actif.store(nouveau, Ordering::Relaxed);
            etat.dernier_changement = Some(maintenant);
        }
    }

    /// Le photographe change le réglage en cours de session (0.3.1).
    ///
    /// Appliqué aussitôt, sans relancer la session : relancer créerait une
    /// nouvelle session locale et réenverrait les photos déjà parties.
    pub fn regler(&self, parallele: u32) {
        let parallele = parallele.clamp(1, MAX_PARALLEL);

        self.configure.store(parallele, Ordering::Relaxed);
        self.actif.store(parallele, Ordering::Relaxed);
    }

    /// Temps à attendre avant d'envoyer, s'il y en a.
    pub fn attente_restante(&self, maintenant: Instant) -> Option<Duration> {
        let etat = self.etat.lock().unwrap_or_else(|e| e.into_inner());

        match etat.reprise {
            Some(fin) if fin > maintenant => Some(fin - maintenant),
            _ => None,
        }
    }
}

/// Lit l'en-tête Retry-After, en secondes.
///
/// La forme « date HTTP » est rare pour un 429 d'API : elle n'est pas lue,
/// et l'attente par défaut s'applique alors.
pub fn lire_retry_after(valeur: Option<&str>) -> Option<u64> {
    valeur?.trim().parse::<u64>().ok()
}

/// Attente à respecter après une saturation.
///
/// Celle du serveur si elle est donnée ; sinon une valeur par défaut qui
/// double à chaque saturation consécutive du même fichier.
pub fn attente_saturation(code: u16, retry_after: Option<u64>, consecutives: u32) -> u64 {
    let defaut = if code == 503 {
        ATTENTE_503_DEFAUT_SECS
    } else {
        ATTENTE_429_DEFAUT_SECS
    };

    let attente = retry_after.unwrap_or_else(|| defaut.saturating_mul(1 << consecutives.min(6)));

    attente.clamp(1, ATTENTE_MAX_SECS)
}

/// Lit le temps de traitement serveur dans l'en-tête Server-Timing.
///
/// Le serveur envoie `app;dur=123.4` (millisecondes). On prend la mesure
/// `app` si elle existe, sinon la première qui porte une durée.
pub fn lire_server_timing(valeur: &str) -> Option<f64> {
    let mut premiere: Option<f64> = None;

    for mesure in valeur.split(',') {
        let mut morceaux = mesure.split(';').map(|m| m.trim());
        let nom = morceaux.next().unwrap_or("");

        let duree = morceaux
            .filter_map(|p| p.strip_prefix("dur="))
            .filter_map(|d| d.trim_matches('"').parse::<f64>().ok())
            .next();

        if let Some(d) = duree {
            if nom == "app" {
                return Some(d);
            }

            premiere.get_or_insert(d);
        }
    }

    premiere
}

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
    /// Nombre de workers parallèles (1 à 6)
    pub parallel: u32,
}

/// Envoi réussi, avec ses temps mesurés.
struct EnvoiReussi {
    photo_id: Option<i64>,
    /// Durée de la tentative réussie : de l'envoi de la requête à la
    /// réception complète de la réponse.
    total_ms: u64,
    /// Temps passé dans le serveur (Server-Timing), s'il l'indique.
    serveur_ms: Option<u64>,
}

/// Pourquoi un envoi n'a pas abouti.
enum Echec {
    /// 401 : il faut se reconnecter, inutile d'insister.
    SessionExpiree,
    /// 429 ou 503 : le serveur demande de ralentir.
    Saturation { code: u16, retry_after: Option<u64> },
    /// Toute autre erreur : compte comme une tentative.
    Erreur(String),
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
/// - `Ok(EnvoiReussi)` — succès, avec l'ID de la photo et les temps mesurés
/// - `Err(Echec)` — échec, classé selon ce qu'il faut en faire
async fn upload_single_file(
    client: &reqwest::Client,
    config: &UploadConfig,
    file_path: &Path,
    filename: &str,
) -> Result<EnvoiReussi, Echec> {
    // Lire le fichier depuis le disque
    let file_bytes = tokio::fs::read(file_path).await.map_err(|e| {
        Echec::Erreur(if e.kind() == std::io::ErrorKind::NotFound {
            format!("Fichier introuvable: {}", file_path.display())
        } else if e.kind() == std::io::ErrorKind::PermissionDenied {
            format!("Accès refusé: {}", file_path.display())
        } else {
            format!("Erreur lecture fichier: {}", e)
        })
    })?;

    // Construire le formulaire multipart
    // C'est exactement comme un <form enctype="multipart/form-data"> en HTML
    let file_part = reqwest::multipart::Part::bytes(file_bytes)
        .file_name(filename.to_string())
        .mime_str("image/jpeg")
        .map_err(|e| Echec::Erreur(format!("Erreur MIME: {}", e)))?;

    let mut form = reqwest::multipart::Form::new()
        .part("photo", file_part)
        .text("event_id", config.event_id.to_string());

    // Ajouter le checkpoint si défini
    if let Some(cp_id) = config.checkpoint_id {
        form = form.text("checkpoint_id", cp_id.to_string());
    }

    // Le chrono part ici, fichier déjà lu : il mesure l'envoi et la réponse
    // de CETTE tentative, pas la lecture disque ni les attentes entre essais.
    let debut = Instant::now();

    // Envoyer la requête POST
    let url = format!("{}{}", API_BASE_URL, UPLOAD_ENDPOINT);
    let response = client
        .post(&url)
        .header("Accept", "application/json")
        // SAAS 240 — Phase 6 : Bearer token (le middleware app-password
        // ne lit plus X-API-TOKEN)
        .bearer_auth(&config.token)
        .multipart(form)
        .send()
        .await
        .map_err(|e| {
            Echec::Erreur(if e.is_timeout() {
                "Timeout (60s dépassées)".to_string()
            } else if e.is_connect() {
                "Impossible de contacter le serveur".to_string()
            } else if e.is_request() {
                "Erreur d'envoi de la requête".to_string()
            } else {
                format!("Erreur réseau: {}", e)
            })
        })?;

    let status = response.status();

    // Token expiré → erreur spéciale que le frontend interceptera
    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err(Echec::SessionExpiree);
    }

    // 429 (trop de requêtes) et 503 (serveur indisponible) : le serveur
    // demande de ralentir, éventuellement avec un délai (Retry-After).
    if status == reqwest::StatusCode::TOO_MANY_REQUESTS
        || status == reqwest::StatusCode::SERVICE_UNAVAILABLE
    {
        let retry_after = lire_retry_after(
            response
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
        );

        return Err(Echec::Saturation {
            code: status.as_u16(),
            retry_after,
        });
    }

    // Erreur 413 : fichier trop gros pour le serveur
    if status == reqwest::StatusCode::PAYLOAD_TOO_LARGE {
        return Err(Echec::Erreur("Fichier trop volumineux pour le serveur".to_string()));
    }

    // Erreur serveur 5xx
    if status.is_server_error() {
        return Err(Echec::Erreur(format!("Erreur serveur ({})", status.as_u16())));
    }

    // Autres erreurs HTTP
    if !status.is_success() {
        return Err(Echec::Erreur(format!("Erreur HTTP ({})", status.as_u16())));
    }

    // Temps de traitement annoncé par le serveur (0.3.1). Absent sur un
    // serveur plus ancien : on affiche alors le temps total seul.
    let serveur_ms = response
        .headers()
        .get("server-timing")
        .and_then(|v| v.to_str().ok())
        .and_then(lire_server_timing)
        .map(|ms| ms.round().max(0.0) as u64);

    // Parser la réponse JSON
    let api_response: UploadApiResponse = response
        .json()
        .await
        .map_err(|e| Echec::Erreur(format!("Erreur lecture réponse: {}", e)))?;

    let total_ms = debut.elapsed().as_millis() as u64;

    if !api_response.success {
        return Err(Echec::Erreur(
            api_response
                .message
                .unwrap_or_else(|| "Erreur inconnue du serveur".to_string()),
        ));
    }

    // Extraire l'ID de la photo créée sur le serveur
    let photo_id = api_response.data.and_then(|d| d.photo_id);

    Ok(EnvoiReussi {
        photo_id,
        total_ms,
        serveur_ms,
    })
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
/// Crée 6 tâches tokio, dont seules les N premières travaillent
/// (N = réglage du photographe, entre 1 et 6, tenu par le `Regulateur`).
/// Les autres attendent : le réglage peut ainsi monter ou descendre en
/// cours de session, et une saturation du serveur le réduire un temps.
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
/// * `regulateur` — Nombre d'envois simultanés autorisés, partagé par tous
///   les workers : une saturation vue par l'un ralentit tout le monde
/// * `app_handle` — Handle Tauri pour les événements frontend
pub fn start_upload_workers(
    rx: mpsc::UnboundedReceiver<FileJob>,
    config: UploadConfig,
    running: Arc<AtomicBool>,
    paused: Arc<AtomicBool>,
    regulateur: Arc<Regulateur>,
    app_handle: tauri::AppHandle,
) {
    // On wrappe le receiver dans un Arc<Mutex> pour le partager entre workers.
    // Chaque worker va le verrouiller brièvement pour prendre un fichier.
    let shared_rx = Arc::new(tokio::sync::Mutex::new(rx));

    for i in 0..MAX_PARALLEL {
        let config_clone = config.clone();
        let running_clone = running.clone();
        let paused_clone = paused.clone();
        let app_clone = app_handle.clone();
        let rx_clone = shared_rx.clone();
        let regulateur = regulateur.clone();

        tokio::spawn(async move {
            // Un client par worker, créé une fois : le pool garde la
            // connexion ouverte d'une photo à l'autre (keep-alive).
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

                // Attendre si en pause, ou si le serveur a demandé de réduire
                // le nombre d'envois simultanés et que ce worker est en trop.
                if paused_clone.load(Ordering::Relaxed) || !regulateur.autorise(i) {
                    tokio::time::sleep(Duration::from_millis(500)).await;
                    continue;
                }

                // Prendre un fichier du canal. L'attente est bornée à 500 ms
                // pour revoir régulièrement l'arrêt, la pause et la
                // régulation ; un fichier arrivé entre-temps part aussitôt,
                // sans la latence d'une interrogation périodique.
                let job = {
                    let mut rx_guard = rx_clone.lock().await;

                    match tokio::time::timeout(Duration::from_millis(500), rx_guard.recv()).await {
                        Ok(Some(job)) => Some(job),
                        Ok(None) => {
                            info!("Worker {} : canal fermé", i);
                            return;
                        }
                        Err(_) => None,
                    }
                };

                let Some(job) = job else {
                    continue;
                };

                // Marquer comme "uploading" en base
                let _ = database::update_file_status(
                    job.db_file_id,
                    "uploading",
                    None,
                    None,
                );

                // Tentatives comptées : un 429 n'en consomme pas.
                let mut tentatives: u32 = 0;
                let mut saturations: u32 = 0;

                loop {
                    // Délai imposé par le serveur après un 429 ou un 503.
                    while let Some(reste) = regulateur.attente_restante(Instant::now()) {
                        if !running_clone.load(Ordering::Relaxed) {
                            break;
                        }

                        tokio::time::sleep(reste.min(Duration::from_millis(500))).await;
                    }

                    if !running_clone.load(Ordering::Relaxed) {
                        let _ = database::update_file_status(
                            job.db_file_id,
                            "pending",
                            Some("Arrêt pendant l'attente"),
                            None,
                        );
                        photo_terminee();
                        return;
                    }

                    // Notifier le frontend
                    let _ = app_clone.emit(
                        "upload_started",
                        serde_json::json!({
                            "filename": &job.filename,
                            "size": job.file_size,
                            "attempt": tentatives + 1,
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
                        Ok(envoi) => {
                            // ── SUCCÈS ──
                            regulateur.succes(Instant::now());

                            let _ = database::update_file_status(
                                job.db_file_id,
                                "success",
                                None,
                                envoi.photo_id,
                            );

                            // Réseau = tout ce qui n'est pas le serveur :
                            // transfert, latence, attente de la réponse.
                            let reseau_ms = envoi
                                .serveur_ms
                                .map(|s| envoi.total_ms.saturating_sub(s));

                            let _ = app_clone.emit(
                                "upload_success",
                                serde_json::json!({
                                    "filename": &job.filename,
                                    "size": job.file_size,
                                    "duration_ms": envoi.total_ms,
                                    "server_ms": envoi.serveur_ms,
                                    "network_ms": reseau_ms,
                                    "server_photo_id": envoi.photo_id,
                                }),
                            );

                            match (reseau_ms, envoi.serveur_ms) {
                                (Some(r), Some(s)) => info!(
                                    "Worker {} ✓ {} en {} ms (réseau {} ms + serveur {} ms)",
                                    i, job.filename, envoi.total_ms, r, s
                                ),
                                _ => info!(
                                    "Worker {} ✓ {} en {} ms",
                                    i, job.filename, envoi.total_ms
                                ),
                            }

                            break;
                        }

                        Err(Echec::SessionExpiree) => {
                            // Token expiré → arrêter tout
                            warn!("Worker {} ✗ {} — session expirée", i, job.filename);

                            let _ = database::update_file_status(
                                job.db_file_id,
                                "failed",
                                Some("SESSION_EXPIRED"),
                                None,
                            );
                            let _ = app_clone.emit(
                                "session_expired",
                                serde_json::json!({}),
                            );
                            photo_terminee();
                            return;
                        }

                        Err(Echec::Saturation { code, retry_after }) => {
                            let attente = attente_saturation(code, retry_after, saturations);
                            saturations += 1;

                            let parallele = regulateur
                                .saturation(Duration::from_secs(attente), Instant::now());

                            warn!(
                                "Worker {} ⏸ {} — serveur saturé ({}), attente {} s, {} envoi(s) simultané(s)",
                                i, job.filename, code, attente, parallele
                            );

                            let _ = app_clone.emit(
                                "upload_throttled",
                                serde_json::json!({
                                    "filename": &job.filename,
                                    "code": code,
                                    "delay_seconds": attente,
                                    "parallel": parallele,
                                }),
                            );

                            // Un 429 n'est jamais un échec : le serveur a
                            // bien reçu la demande et dit simplement
                            // « plus tard ». Un 503 répété, si : le serveur
                            // peut être réellement en panne.
                            if code == 503 {
                                tentatives += 1;

                                if tentatives >= MAX_ATTEMPTS {
                                    let message = format!("Serveur indisponible ({})", code);

                                    let _ = database::update_file_status(
                                        job.db_file_id,
                                        "failed",
                                        Some(&message),
                                        None,
                                    );

                                    let _ = app_clone.emit(
                                        "upload_failed",
                                        serde_json::json!({
                                            "filename": &job.filename,
                                            "error": &message,
                                            "attempt": tentatives,
                                            "max_attempts": MAX_ATTEMPTS,
                                            "fatal": false,
                                        }),
                                    );

                                    break;
                                }
                            }
                        }

                        Err(Echec::Erreur(error_msg)) => {
                            tentatives += 1;

                            warn!(
                                "Worker {} ✗ {} — tentative {}/{} : {}",
                                i,
                                job.filename,
                                tentatives,
                                MAX_ATTEMPTS,
                                error_msg
                            );

                            if tentatives < MAX_ATTEMPTS {
                                let delay = RETRY_DELAYS
                                    .get((tentatives - 1) as usize)
                                    .copied()
                                    .unwrap_or(45);

                                let _ = app_clone.emit(
                                    "upload_retry",
                                    serde_json::json!({
                                        "filename": &job.filename,
                                        "attempt": tentatives,
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
                                    photo_terminee();
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
                                        "attempt": tentatives,
                                        "max_attempts": MAX_ATTEMPTS,
                                        "fatal": false,
                                    }),
                                );

                                break;
                            }
                        }
                    }
                }

                photo_terminee();

                // Émettre les stats après chaque fichier traité
                emit_stats(job.session_id, &app_clone);
                check_all_complete(job.session_id, &app_clone);
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

// ─── Tests ──────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn le_parallelisme_est_borne_de_1_a_6() {
        assert_eq!(Regulateur::new(0).actif(), 1);
        assert_eq!(Regulateur::new(3).actif(), 3);
        assert_eq!(Regulateur::new(6).actif(), 6);
        assert_eq!(Regulateur::new(12).actif(), 6);
    }

    #[test]
    fn une_saturation_divise_le_parallelisme_par_deux() {
        let r = Regulateur::new(6);
        let t0 = Instant::now();

        assert_eq!(r.saturation(Duration::from_secs(5), t0), 3);
        assert!(r.autorise(2));
        assert!(!r.autorise(3));

        // Jamais sous 1 : il faut bien que quelque chose parte.
        r.saturation(Duration::from_secs(5), t0);
        assert_eq!(r.saturation(Duration::from_secs(5), t0), 1);
        assert!(r.autorise(0));
    }

    #[test]
    fn les_envois_attendent_le_delai_demande() {
        let r = Regulateur::new(3);
        let t0 = Instant::now();

        r.saturation(Duration::from_secs(10), t0);

        assert!(r.attente_restante(t0 + Duration::from_secs(4)).is_some());
        assert!(r.attente_restante(t0 + Duration::from_secs(10)).is_none());
    }

    #[test]
    fn le_delai_le_plus_long_lemporte() {
        let r = Regulateur::new(3);
        let t0 = Instant::now();

        r.saturation(Duration::from_secs(30), t0);
        r.saturation(Duration::from_secs(5), t0);

        assert!(r.attente_restante(t0 + Duration::from_secs(20)).is_some());
    }

    #[test]
    fn le_parallelisme_remonte_apres_une_periode_calme() {
        let r = Regulateur::new(4);
        let t0 = Instant::now();

        r.saturation(Duration::from_secs(5), t0);
        assert_eq!(r.actif(), 2);

        // Trop tôt : on reste prudent.
        r.succes(t0 + Duration::from_secs(10));
        assert_eq!(r.actif(), 2);

        // Un cran toutes les 30 s sans incident.
        r.succes(t0 + Duration::from_secs(31));
        assert_eq!(r.actif(), 3);
        r.succes(t0 + Duration::from_secs(40));
        assert_eq!(r.actif(), 3);
        r.succes(t0 + Duration::from_secs(62));
        assert_eq!(r.actif(), 4);

        // Jamais au-delà du réglage du photographe.
        r.succes(t0 + Duration::from_secs(200));
        assert_eq!(r.actif(), 4);
    }

    #[test]
    fn le_reglage_change_en_cours_de_session() {
        let r = Regulateur::new(3);

        r.regler(6);
        assert_eq!(r.actif(), 6);
        assert!(r.autorise(5));

        r.regler(1);
        assert_eq!(r.actif(), 1);
        assert!(!r.autorise(1));

        // Borné comme au démarrage.
        r.regler(20);
        assert_eq!(r.actif(), 6);
    }

    #[test]
    fn lit_retry_after_en_secondes() {
        assert_eq!(lire_retry_after(Some("30")), Some(30));
        assert_eq!(lire_retry_after(Some(" 5 ")), Some(5));
        assert_eq!(lire_retry_after(Some("Wed, 21 Oct 2026 07:28:00 GMT")), None);
        assert_eq!(lire_retry_after(None), None);
    }

    #[test]
    fn lattente_suit_le_serveur_sinon_double() {
        assert_eq!(attente_saturation(429, Some(12), 0), 12);
        assert_eq!(attente_saturation(429, None, 0), 5);
        assert_eq!(attente_saturation(429, None, 1), 10);
        assert_eq!(attente_saturation(503, None, 0), 10);

        // Plafonnée : un en-tête aberrant ne fige pas l'agent.
        assert_eq!(attente_saturation(429, Some(86_400), 0), ATTENTE_MAX_SECS);
        assert_eq!(attente_saturation(429, None, 30), ATTENTE_MAX_SECS);
        assert_eq!(attente_saturation(429, Some(0), 0), 1);
    }

    #[test]
    fn lit_le_temps_serveur() {
        assert_eq!(lire_server_timing("app;dur=123.4"), Some(123.4));
        assert_eq!(lire_server_timing("db;dur=3, app;dur=250"), Some(250.0));
        assert_eq!(lire_server_timing("cache;desc=\"hit\", total;dur=\"42\""), Some(42.0));
        assert_eq!(lire_server_timing("miss"), None);
        assert_eq!(lire_server_timing(""), None);
    }

    #[test]
    fn le_compteur_de_photos_ne_passe_jamais_sous_zero() {
        reinitialiser_photos_en_cours();
        photo_terminee();
        assert_eq!(photos_en_cours(), 0);

        photo_ajoutee();
        photo_ajoutee();
        photo_terminee();
        assert_eq!(photos_en_cours(), 1);

        // En pause, les photos ne partent pas : la vidéo ne leur cède rien.
        assert!(photos_prioritaires());
        photos_en_pause(true);
        assert!(!photos_prioritaires());
        photos_en_pause(false);
        assert!(photos_prioritaires());

        reinitialiser_photos_en_cours();
        assert_eq!(photos_en_cours(), 0);
        assert!(!photos_prioritaires());
    }
}
