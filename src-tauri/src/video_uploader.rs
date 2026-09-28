// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Envoi vidéo (video_uploader.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// SAAS 430 — Contrat d'interface v1.3.
//
// ─── QUATRE FLUX MONTENT DU TERRAIN ────────────────────────────────────
//
//   1. photos boîtier        chaîne existante, traitée par uploader.rs
//   2. images d'analyse      ~330 Ko, 1,3 Mb/s      ← ici
//   3. clips légers 540p     2 Mb/s                 ← ici
//   4. clips pleine qualité  jusqu'à 33 Mb/s        ← ici
//
// L'écart entre le flux 2 et le flux 4 est d'un facteur vingt-cinq. C'est ce
// qui justifie de les traiter séparément : sur une antenne saturée par le
// public d'une course, envoyer les images d'analyse reste possible quand les
// clips pleine qualité ne passent plus du tout.
//
// Et ce sont les images qui portent l'essentiel de la valeur : sans elles,
// aucun dossard n'est lu, donc personne ne retrouve ses vidéos. Un clip qui
// monte le soir au retour ne gêne personne ; une image qui ne monte jamais
// rend le clip introuvable.
//
// ─── DEUX PROTOCOLES, DEUX RAISONS ─────────────────────────────────────
//
// Les CLIPS partent en morceaux, avec une étape de finalisation. Un fichier
// de 500 Mo envoyé d'un bloc est perdu en entier à la moindre coupure ; en
// morceaux, on ne reperd que le dernier.
//
// Les IMAGES partent directement, groupées. À 330 Ko l'unité, le découpage
// serait une complication sans bénéfice — mais les grouper évite de payer un
// aller-retour réseau par image.

use serde::{Deserialize, Serialize};
use std::path::Path;
use std::sync::OnceLock;
use std::time::{Duration, Instant};
use tokio::io::AsyncReadExt;

// ═══════════════════════════════════════════════════════════════════════
// CONSTANTES
// ═══════════════════════════════════════════════════════════════════════

// URL injectée à la COMPILATION (voir auth.rs).
const API_BASE_URL: &str = env!("ATTIMO_API_URL");

/// Taille d'un morceau d'envoi.
///
/// Cinq mégaoctets : assez gros pour limiter les allers-retours, assez petit
/// pour qu'une coupure ne fasse pas reperdre grand-chose.
const CHUNK_SIZE: usize = 5 * 1024 * 1024;

/// Les clips sont volumineux et le réseau de terrain est lent : une minute
/// serait trop court pour un morceau de 5 Mo en 4G dégradée.
const TIMEOUT_CLIP_SECS: u64 = 300;

/// Les images sont légères, une minute suffit largement.
const TIMEOUT_FRAMES_SECS: u64 = 60;

/// Un lot de dix images (0.3.1) : 3,3 Mo à monter, et le serveur analyse
/// chaque image avant de répondre — dossards, visages. Dix analyses dans une
/// même requête ne tiennent pas toujours dans la minute d'une image seule ;
/// un délai dépassé ferait tout réanalyser au nouvel essai.
const TIMEOUT_LOT_IMAGES_SECS: u64 = 180;

/// Nombre d'images envoyées par requête.
///
/// Dix images font environ 3,3 Mo. Au-delà, on s'approche des limites de
/// taille de requête, et une coupure ferait reperdre un lot trop gros.
///
/// C'est aussi le maximum accepté par le serveur
/// (`SportClipFrameController::MAX_FRAMES_PER_REQUEST`).
pub const FRAMES_PAR_LOT: usize = 10;

/// Temps maximal qu'une requête vidéo cède aux photos (0.3.1).
///
/// Les photos passent d'abord : tant qu'il en reste à envoyer, chaque
/// morceau de clip et chaque lot d'images attend. Mais pas indéfiniment —
/// sous un flot continu de photos, la vidéo avance quand même, au rythme
/// d'une requête toutes les vingt secondes par envoi vidéo.
const CEDER_AUX_PHOTOS_MAX_SECS: u64 = 20;

/// Attente par défaut après un 429 ou un 503 sans Retry-After.
const ATTENTE_SATURATION_DEFAUT_SECS: u64 = 10;

/// Préfixe des erreurs de saturation : `SATURE:<secondes>`.
///
/// La file le reconnaît : l'élément repart en attente sans consommer de
/// tentative, et l'envoi vidéo patiente le délai demandé.
pub const PREFIXE_SATURE: &str = "SATURE:";

/// Préfixe des erreurs définitives (0.3.2) : `DEFINITIF:<message>`.
///
/// Le serveur a refusé la requête elle-même (4xx) : la renvoyer telle quelle
/// donnerait la même réponse. L'élément sort de la file sans nouvel essai.
pub const PREFIXE_DEFINITIF: &str = "DEFINITIF:";

/// Le serveur refuse les images : l'épreuve est sans identification
/// (mode « Aucune »). Erreur définitive, propre à toute l'épreuve.
pub const ANALYSE_DESACTIVEE: &str = "ANALYSE_DESACTIVEE";

/// Message exact du serveur dans ce cas
/// (`SportClipFrameController::resolveEvent`).
const MESSAGE_ANALYSE_DESACTIVEE: &str = "Recognition is disabled";

// ═══════════════════════════════════════════════════════════════════════
// CONFIGURATION
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone)]
pub struct VideoUploadConfig {
    pub token: String,
    pub event_id: i64,
    pub checkpoint_id: Option<i64>,
    pub session_id: String,
}

/// Variante d'un clip.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ClipVariant {
    /// Version légère, envoyée pendant la course : ce que le coureur regarde.
    Proxy,
    /// Version pleine qualité : ce qu'il achète.
    Hd,
}

impl ClipVariant {
    fn as_str(&self) -> &'static str {
        match self {
            ClipVariant::Proxy => "proxy",
            ClipVariant::Hd => "hd",
        }
    }
}

// ═══════════════════════════════════════════════════════════════════════
// RÉPONSES DU SERVEUR
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Deserialize)]
struct ChunkResponse {
    success: bool,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FinalizeResponse {
    success: bool,
    #[serde(default)]
    clip: Option<ClipInfo>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClipInfo {
    pub video_id: i64,
    pub index: u32,
    pub has_proxy: bool,
    pub has_hd: bool,
    pub status: String,
}

#[derive(Debug, Deserialize)]
struct FramesResponse {
    success: bool,
    #[serde(default)]
    totals: Option<FrameTotals>,
    /// Résultat image par image : une image en échec ne fait pas perdre le
    /// lot, seule elle est à renvoyer.
    #[serde(default)]
    frames: Vec<FrameResult>,
    #[serde(default)]
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct FrameResult {
    index: usize,
    success: bool,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct FrameTotals {
    #[serde(default)]
    pub bibs: u32,
    #[serde(default)]
    pub faces: u32,
    #[serde(default)]
    pub kept: u32,
    #[serde(default)]
    pub discarded: u32,
}

// ═══════════════════════════════════════════════════════════════════════
// CLIENT
// ═══════════════════════════════════════════════════════════════════════

/// Client réseau partagé par tous les envois vidéo (0.3.1).
///
/// Il en naissait un par clip, par lot d'images et par interrogation d'état :
/// autant de connexions TCP et de poignées de main TLS. Partagé, il garde ses
/// connexions ouvertes d'une requête à l'autre (keep-alive). Le délai est
/// posé requête par requête, puisqu'il diffère entre clips et images.
fn client() -> Result<&'static reqwest::Client, String> {
    static CLIENT: OnceLock<reqwest::Client> = OnceLock::new();

    if let Some(c) = CLIENT.get() {
        return Ok(c);
    }

    let nouveau = reqwest::Client::builder()
        .connect_timeout(Duration::from_secs(20))
        .pool_idle_timeout(Duration::from_secs(90))
        .build()
        .map_err(|e| format!("Impossible de créer le client réseau : {}", e))?;

    Ok(CLIENT.get_or_init(|| nouveau))
}

/// Laisse passer les photos d'abord.
///
/// Appelé avant chaque requête vidéo. Borné : voir `CEDER_AUX_PHOTOS_MAX_SECS`.
async fn ceder_aux_photos() {
    let debut = Instant::now();

    while crate::uploader::photos_prioritaires()
        && debut.elapsed() < Duration::from_secs(CEDER_AUX_PHOTOS_MAX_SECS)
    {
        tokio::time::sleep(Duration::from_millis(500)).await;
    }
}

/// Traduit les statuts qui ne concernent pas le fichier lui-même.
///
/// 401 : reconnexion nécessaire. 429 et 503 : le serveur demande de
/// ralentir — l'élément repartira sans consommer de tentative.
fn statut_bloquant(reponse: &reqwest::Response) -> Option<String> {
    let statut = reponse.status();

    if statut == reqwest::StatusCode::UNAUTHORIZED {
        return Some("SESSION_EXPIRED".to_string());
    }

    if statut == reqwest::StatusCode::TOO_MANY_REQUESTS
        || statut == reqwest::StatusCode::SERVICE_UNAVAILABLE
    {
        let attente = crate::uploader::lire_retry_after(
            reponse
                .headers()
                .get(reqwest::header::RETRY_AFTER)
                .and_then(|v| v.to_str().ok()),
        )
        .unwrap_or(ATTENTE_SATURATION_DEFAUT_SECS)
        .clamp(1, 300);

        return Some(format!("{}{}", PREFIXE_SATURE, attente));
    }

    None
}

/// Délai demandé par une erreur de saturation, s'il s'agit de cela.
pub fn attente_si_sature(erreur: &str) -> Option<u64> {
    erreur.strip_prefix(PREFIXE_SATURE)?.parse().ok()
}

/// Une erreur que le serveur répéterait à l'identique ?
pub fn est_definitive(erreur: &str) -> bool {
    erreur.starts_with(PREFIXE_DEFINITIF) || erreur == ANALYSE_DESACTIVEE
}

/// Le message d'une erreur, sans son préfixe technique.
pub fn message_lisible(erreur: &str) -> &str {
    erreur.strip_prefix(PREFIXE_DEFINITIF).unwrap_or(erreur)
}

/// Ce que dit une réponse d'erreur : son message, et si elle détaille des
/// champs refusés (erreur de validation Laravel).
async fn lire_refus(reponse: reqwest::Response) -> (u16, String, bool) {
    let code = reponse.status().as_u16();

    let corps: serde_json::Value = reponse.json().await.unwrap_or(serde_json::Value::Null);

    let message = corps
        .get("message")
        .and_then(|m| m.as_str())
        .unwrap_or("")
        .to_string();

    (code, message, corps.get("errors").is_some())
}

/// Traduit un refus du serveur (4xx) en erreur définitive.
fn refus_definitif(code: u16, message: &str) -> String {
    if message.is_empty() {
        format!("{}Requête refusée par le serveur ({})", PREFIXE_DEFINITIF, code)
    } else {
        format!("{}{} ({})", PREFIXE_DEFINITIF, message, code)
    }
}

/// Pose les en-têtes d'authentification.
///
/// Les deux formes sont envoyées : l'agent photo utilise historiquement
/// `X-API-TOKEN`, tandis que les routes vidéo sont documentées avec
/// `Authorization: Bearer`. Envoyer les deux évite de dépendre d'un détail
/// d'implémentation du middleware, sans aucun coût.
fn authentifier(requete: reqwest::RequestBuilder, token: &str) -> reqwest::RequestBuilder {
    requete
        .header("Accept", "application/json")
        .header("X-API-TOKEN", token)
        .header("Authorization", format!("Bearer {}", token))
}

/// Traduit les erreurs réseau en messages compréhensibles.
///
/// Un photographe en pleine course n'a que faire du détail technique : il
/// doit savoir s'il attend, s'il se reconnecte, ou s'il rappelle.
fn message_erreur(e: &reqwest::Error) -> String {
    if e.is_timeout() {
        "Délai dépassé — réseau trop lent ou saturé.".to_string()
    } else if e.is_connect() {
        "Serveur injoignable — vérifier la connexion.".to_string()
    } else {
        format!("Erreur réseau : {}", e)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ENVOI D'UN CLIP
// ═══════════════════════════════════════════════════════════════════════

/// Envoie un clip au serveur, en morceaux puis finalisation.
///
/// L'appariement entre la version légère et la version pleine qualité se
/// fait côté serveur sur le couple (session, numéro de clip). L'ordre
/// d'arrivée est donc indifférent : le premier fichier crée la fiche, le
/// second la complète.
pub async fn envoyer_clip(
    config: &VideoUploadConfig,
    chemin: &Path,
    variant: ClipVariant,
    clip_index: u32,
    debut: &str,
    fin: &str,
) -> Result<ClipInfo, String> {
    let client = client()?;

    // Lu morceau par morceau (0.3.1) : un clip HD de plusieurs centaines de
    // mégaoctets n'est plus chargé en entier en mémoire, et deux envois
    // simultanés ne doublent plus cette charge.
    let mut fichier = tokio::fs::File::open(chemin)
        .await
        .map_err(|e| format!("Lecture du clip impossible : {}", e))?;

    let taille = fichier
        .metadata()
        .await
        .map_err(|e| format!("Lecture du clip impossible : {}", e))?
        .len() as usize;

    if taille == 0 {
        return Err("Le clip est vide.".to_string());
    }

    let nom_fichier = chemin
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "clip.mp4".to_string());

    // Identifiant d'envoi : lie les morceaux entre eux côté serveur.
    //
    // 0.3.2 — Il porte l'épreuve : le serveur range les morceaux par
    // identifiant seul, tous comptes confondus. Deux captations lancées à la
    // même milliseconde sur deux épreuves ne mélangent plus leurs morceaux.
    let upload_id = identifiant_envoi(config.event_id, &config.session_id, variant, clip_index);

    let total_morceaux = taille.div_ceil(CHUNK_SIZE);

    // ─── Envoi des morceaux ───

    for index in 0..total_morceaux {
        let longueur = CHUNK_SIZE.min(taille - index * CHUNK_SIZE);
        let mut tranche = vec![0u8; longueur];

        fichier
            .read_exact(&mut tranche)
            .await
            .map_err(|e| format!("Lecture du clip impossible : {}", e))?;

        // Les photos d'abord : un clip HD de 500 Mo ne doit pas monopoliser
        // la liaison pendant qu'une rafale attend, d'où une pause possible à
        // chaque morceau. Une version légère (quelques Mo, 0.3.2) ne cède
        // qu'une fois, avant son premier morceau : elle doit partir au fil
        // de la captation.
        if variant == ClipVariant::Hd || index == 0 {
            ceder_aux_photos().await;
        }

        let part = reqwest::multipart::Part::bytes(tranche)
            .file_name(nom_fichier.clone())
            .mime_str("application/octet-stream")
            .map_err(|e| format!("Type de contenu invalide : {}", e))?;

        let formulaire = reqwest::multipart::Form::new()
            .part("chunk", part)
            .text("upload_id", upload_id.clone())
            .text("chunk_index", index.to_string())
            .text("total_chunks", total_morceaux.to_string())
            .text("filename", nom_fichier.clone());

        let url = format!(
            "{}/api/sport/events/{}/clips/chunk",
            API_BASE_URL, config.event_id
        );

        let reponse = authentifier(client.post(&url), &config.token)
            .timeout(Duration::from_secs(TIMEOUT_CLIP_SECS))
            .multipart(formulaire)
            .send()
            .await
            .map_err(|e| message_erreur(&e))?;

        if let Some(erreur) = statut_bloquant(&reponse) {
            return Err(erreur);
        }

        // Morceau refusé (validation) : le renvoyer n'y changerait rien.
        if reponse.status().is_client_error() {
            let (code, message, _) = lire_refus(reponse).await;
            return Err(refus_definitif(code, &message));
        }

        let corps: ChunkResponse = reponse
            .json()
            .await
            .map_err(|e| format!("Réponse inattendue du serveur : {}", e))?;

        if !corps.success {
            return Err(corps
                .message
                .unwrap_or_else(|| "Le serveur a refusé le morceau.".to_string()));
        }
    }

    // ─── Finalisation ───
    //
    // C'est cette étape qui déclenche l'assemblage côté serveur et
    // l'appariement avec l'autre variante du même clip.

    let mut charge = serde_json::json!({
        "upload_id": upload_id,
        "variant": variant.as_str(),
        "session_id": config.session_id,
        "clip_index": clip_index,
        "started_at": debut,
        "ended_at": fin,
    });

    if let Some(checkpoint) = config.checkpoint_id {
        charge["checkpoint_id"] = serde_json::json!(checkpoint);
    }

    let url = format!(
        "{}/api/sport/events/{}/clips/finalize",
        API_BASE_URL, config.event_id
    );

    let reponse = authentifier(client.post(&url), &config.token)
        .timeout(Duration::from_secs(TIMEOUT_CLIP_SECS))
        .json(&charge)
        .send()
        .await
        .map_err(|e| message_erreur(&e))?;

    if let Some(erreur) = statut_bloquant(&reponse) {
        return Err(erreur);
    }

    // Finalisation refusée. Un 422 dit « envoi introuvable ou incomplet » :
    // un nouvel essai renvoie tous les morceaux, il peut réussir. Les autres
    // refus ne changeront pas.
    if reponse.status().is_client_error() {
        let (code, message, _) = lire_refus(reponse).await;

        if code == 422 {
            return Err(if message.is_empty() {
                "Le serveur n'a pas pu assembler le clip.".to_string()
            } else {
                message
            });
        }

        return Err(refus_definitif(code, &message));
    }

    let corps: FinalizeResponse = reponse
        .json()
        .await
        .map_err(|e| format!("Réponse inattendue du serveur : {}", e))?;

    if !corps.success {
        return Err(corps
            .message
            .unwrap_or_else(|| "Le serveur a refusé le clip.".to_string()));
    }

    corps
        .clip
        .ok_or_else(|| "Le serveur n'a pas renvoyé les informations du clip.".to_string())
}

// ═══════════════════════════════════════════════════════════════════════
// ENVOI DES IMAGES D'ANALYSE
// ═══════════════════════════════════════════════════════════════════════

/// Une image à envoyer, avec son horodatage absolu.
#[derive(Debug, Clone)]
pub struct FrameToUpload {
    pub path: String,
    pub instant_at: String,
}

/// Résultat d'un lot d'images.
#[derive(Debug, Clone)]
pub struct LotImages {
    pub totaux: FrameTotals,

    /// Rangs, dans le lot, des images que le serveur n'a pas pu analyser.
    /// Elles seules sont à renvoyer.
    pub echecs: Vec<usize>,
}

/// Envoie des images d'analyse, par lots de dix.
///
/// Les images sont groupées par dix pour limiter les allers-retours réseau,
/// mais chaque lot reste assez petit pour qu'une coupure ne fasse pas
/// reperdre grand-chose.
///
/// Le serveur les analyse à réception : il lit les dossards, reconnaît les
/// visages, puis supprime les images qui ne contiennent personne. Elles ne
/// sont ni montrées ni vendues — c'est un consommable technique.
pub async fn envoyer_images(
    config: &VideoUploadConfig,
    images: &[FrameToUpload],
) -> Result<FrameTotals, String> {
    let mut cumul = FrameTotals {
        bibs: 0,
        faces: 0,
        kept: 0,
        discarded: 0,
    };

    for lot in images.chunks(FRAMES_PAR_LOT) {
        let resultat = envoyer_lot_images(config, lot).await?;

        cumul.bibs += resultat.totaux.bibs;
        cumul.faces += resultat.totaux.faces;
        cumul.kept += resultat.totaux.kept;
        cumul.discarded += resultat.totaux.discarded;
    }

    Ok(cumul)
}

/// Envoie un lot d'au plus dix images en une seule requête.
///
/// Utilisé par la file d'envoi (0.3.1) : une requête pour dix images au lieu
/// de dix requêtes, donc dix fois moins d'allers-retours et de vérifications
/// du jeton côté serveur.
pub async fn envoyer_lot_images(
    config: &VideoUploadConfig,
    lot: &[FrameToUpload],
) -> Result<LotImages, String> {
    let vide = FrameTotals {
        bibs: 0,
        faces: 0,
        kept: 0,
        discarded: 0,
    };

    if lot.is_empty() {
        return Ok(LotImages {
            totaux: vide,
            echecs: Vec::new(),
        });
    }

    if lot.len() > FRAMES_PAR_LOT {
        return Err(format!("Lot trop gros : {} images, {} au plus.", lot.len(), FRAMES_PAR_LOT));
    }

    let client = client()?;

    let mut formulaire = reqwest::multipart::Form::new()
        .text("session_id", config.session_id.clone());

    if let Some(checkpoint) = config.checkpoint_id {
        formulaire = formulaire.text("checkpoint_id", checkpoint.to_string());
    }

    for (rang, image) in lot.iter().enumerate() {
        let octets = tokio::fs::read(&image.path)
            .await
            .map_err(|e| format!("Lecture de l'image impossible : {}", e))?;

        let nom = Path::new(&image.path)
            .file_name()
            .map(|n| n.to_string_lossy().to_string())
            .unwrap_or_else(|| format!("img_{}.jpg", rang));

        let part = reqwest::multipart::Part::bytes(octets)
            .file_name(nom)
            .mime_str("image/jpeg")
            .map_err(|e| format!("Type de contenu invalide : {}", e))?;

        formulaire = formulaire
            .part(format!("frames[{}][image]", rang), part)
            .text(
                format!("frames[{}][instant_at]", rang),
                image.instant_at.clone(),
            );
    }

    ceder_aux_photos().await;

    let url = format!(
        "{}/api/sport/events/{}/frames",
        API_BASE_URL, config.event_id
    );

    let reponse = authentifier(client.post(&url), &config.token)
        .timeout(Duration::from_secs(TIMEOUT_LOT_IMAGES_SECS))
        .multipart(formulaire)
        .send()
        .await
        .map_err(|e| message_erreur(&e))?;

    if let Some(erreur) = statut_bloquant(&reponse) {
        return Err(erreur);
    }

    // Refus du serveur. Aucun ne se règle en renvoyant les mêmes images.
    //
    // 0.3.2 — Avant, tout 422 valait « analyse désactivée », et comptait
    // pour une tentative ordinaire : chaque lot était renvoyé trois fois, et
    // « Envoi vidéo abandonné après 3 essais : ANALYSE_DESACTIVEE » revenait
    // en boucle. Le cas se reconnaît désormais au message exact du serveur.
    if reponse.status().is_client_error() {
        let (code, message, _) = lire_refus(reponse).await;

        if message.contains(MESSAGE_ANALYSE_DESACTIVEE) {
            return Err(ANALYSE_DESACTIVEE.to_string());
        }

        return Err(refus_definitif(code, &message));
    }

    let corps: FramesResponse = reponse
        .json()
        .await
        .map_err(|e| format!("Réponse inattendue du serveur : {}", e))?;

    if !corps.success {
        return Err(corps
            .message
            .unwrap_or_else(|| "Le serveur a refusé les images.".to_string()));
    }

    Ok(LotImages {
        totaux: corps.totals.unwrap_or(vide),
        echecs: rangs_en_echec(&corps.frames, lot.len()),
    })
}

/// Rangs des images refusées, d'après le détail renvoyé par le serveur.
///
/// Un serveur qui ne détaille pas (réponse ancienne) vaut réussite pour tout
/// le lot, comme avant.
fn rangs_en_echec(resultats: &[FrameResult], taille_lot: usize) -> Vec<usize> {
    let mut echecs: Vec<usize> = resultats
        .iter()
        .filter(|r| !r.success && r.index < taille_lot)
        .map(|r| r.index)
        .collect();

    echecs.sort_unstable();
    echecs.dedup();
    echecs
}

/// L'épreuve accepte-t-elle les images d'analyse ? (0.3.2)
///
/// Interrogé au démarrage de la captation, pour le dire au photographe une
/// fois, en clair, et ne pas extraire d'images pour rien. La sonde est une
/// requête d'images VIDE : le serveur contrôle l'épreuve avant le contenu,
/// il répond donc « analyse désactivée » s'il y a lieu, et sinon refuse la
/// requête faute d'image. Rien n'est analysé, rien n'est facturé.
///
/// Réponses : `Some(false)` analyse désactivée, `Some(true)` analyse
/// active, `None` impossible à savoir (réseau, réponse inattendue) — les
/// envois diront alors ce qu'il en est.
pub async fn sonder_analyse(config: &VideoUploadConfig) -> Result<Option<bool>, String> {
    let client = client()?;

    let url = format!(
        "{}/api/sport/events/{}/frames",
        API_BASE_URL, config.event_id
    );

    let formulaire = reqwest::multipart::Form::new().text("session_id", config.session_id.clone());

    let reponse = authentifier(client.post(&url), &config.token)
        .timeout(Duration::from_secs(TIMEOUT_FRAMES_SECS))
        .multipart(formulaire)
        .send()
        .await
        .map_err(|e| message_erreur(&e))?;

    if let Some(erreur) = statut_bloquant(&reponse) {
        return Err(erreur);
    }

    Ok(interpreter_sonde(lire_refus(reponse).await))
}

/// Lecture de la réponse à la sonde, isolée pour être testée.
fn interpreter_sonde((code, message, champs_refuses): (u16, String, bool)) -> Option<bool> {
    if code == 422 && message.contains(MESSAGE_ANALYSE_DESACTIVEE) {
        return Some(false);
    }

    // Refus de validation (« frames » manquant) : l'épreuve accepte les
    // images, seule la requête vide est refusée.
    if code == 422 && champs_refuses {
        return Some(true);
    }

    None
}

/// Identifiant d'envoi d'un clip.
fn identifiant_envoi(event_id: i64, session_id: &str, variant: ClipVariant, clip_index: u32) -> String {
    format!("e{}_{}_{}_{}", event_id, session_id, variant.as_str(), clip_index)
}

// ═══════════════════════════════════════════════════════════════════════
// ÉTAT D'AVANCEMENT
// ═══════════════════════════════════════════════════════════════════════

/// Ce que le serveur connaît déjà d'une session.
///
/// Interrogé avant de réémettre après une coupure : sans cela, une reprise
/// reposterait des gigaoctets déjà reçus.
#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct SessionStatus {
    pub success: bool,
    pub session_id: String,
    pub clips: Vec<ClipStatus>,
}

#[derive(Debug, Clone, Deserialize, Serialize)]
pub struct ClipStatus {
    pub index: u32,
    pub has_proxy: bool,
    pub has_hd: bool,
    pub status: String,
}

/// Demande au serveur ce qu'il a déjà reçu.
pub async fn etat_session(config: &VideoUploadConfig) -> Result<SessionStatus, String> {
    let client = client()?;

    let url = format!(
        "{}/api/sport/events/{}/clips/status?session_id={}",
        API_BASE_URL, config.event_id, config.session_id
    );

    let reponse = authentifier(client.get(&url), &config.token)
        .timeout(Duration::from_secs(TIMEOUT_FRAMES_SECS))
        .send()
        .await
        .map_err(|e| message_erreur(&e))?;

    if let Some(erreur) = statut_bloquant(&reponse) {
        return Err(erreur);
    }

    reponse
        .json()
        .await
        .map_err(|e| format!("Réponse inattendue du serveur : {}", e))
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn les_variantes_ont_les_bons_libelles() {
        assert_eq!(ClipVariant::Proxy.as_str(), "proxy");
        assert_eq!(ClipVariant::Hd.as_str(), "hd");
    }

    #[test]
    fn le_decoupage_en_morceaux_couvre_tout_le_fichier() {
        // Un fichier de 12 Mo doit se découper en 3 morceaux de 5 Mo.
        // Le type suit celui de la production : CHUNK_SIZE est un usize,
        // et le decoupage porte sur octets.len().
        let taille: usize = 12 * 1024 * 1024;
        let morceaux = taille.div_ceil(CHUNK_SIZE);

        assert_eq!(morceaux, 3);
    }

    #[test]
    fn un_fichier_plus_petit_quun_morceau_tient_en_un_seul() {
        let taille: usize = 2 * 1024 * 1024;
        let morceaux = taille.div_ceil(CHUNK_SIZE);

        assert_eq!(morceaux, 1);
    }

    #[test]
    fn les_images_partent_par_lots_de_dix() {
        let images: Vec<FrameToUpload> = (0..25)
            .map(|i| FrameToUpload {
                path: format!("img_{}.jpg", i),
                instant_at: "2026-08-13T20:00:00.000Z".into(),
            })
            .collect();

        let lots: Vec<_> = images.chunks(FRAMES_PAR_LOT).collect();

        assert_eq!(lots.len(), 3);
        assert_eq!(lots[0].len(), 10);
        assert_eq!(lots[2].len(), 5);
    }

    #[test]
    fn reconnait_une_erreur_de_saturation() {
        assert_eq!(attente_si_sature("SATURE:30"), Some(30));
        assert_eq!(attente_si_sature("SESSION_EXPIRED"), None);
        assert_eq!(attente_si_sature("Délai dépassé"), None);
    }

    #[test]
    fn seules_les_images_refusees_sont_a_renvoyer() {
        let resultats = vec![
            FrameResult { index: 0, success: true },
            FrameResult { index: 3, success: false },
            FrameResult { index: 1, success: false },
            // Rang hors du lot : ignoré plutôt que de marquer une autre image.
            FrameResult { index: 12, success: false },
        ];

        assert_eq!(rangs_en_echec(&resultats, 10), vec![1, 3]);

        // Un serveur qui ne détaille pas : tout le lot est réputé passé.
        assert!(rangs_en_echec(&[], 10).is_empty());
    }

    #[test]
    fn le_detail_des_images_est_lu_dans_la_reponse() {
        let json = r#"{"success":true,"totals":{"bibs":2,"faces":1,"kept":1,"discarded":1},
                       "frames":[{"index":0,"success":true,"bibs":2},{"index":1,"success":false,"message":"x"}]}"#;

        let corps: FramesResponse = serde_json::from_str(json).unwrap();

        assert_eq!(corps.totals.unwrap().bibs, 2);
        assert_eq!(rangs_en_echec(&corps.frames, 2), vec![1]);
    }

    #[test]
    fn lidentifiant_denvoi_distingue_les_variantes() {
        // Deux variantes du même clip ne doivent pas se mélanger côté
        // serveur : leurs morceaux portent des identifiants distincts.
        let session = "video_1727520000000";

        let proxy = identifiant_envoi(42, session, ClipVariant::Proxy, 1);
        let hd = identifiant_envoi(42, session, ClipVariant::Hd, 1);

        assert_ne!(proxy, hd);

        // Ni deux épreuves, ni deux numéros.
        assert_ne!(proxy, identifiant_envoi(43, session, ClipVariant::Proxy, 1));
        assert_ne!(proxy, identifiant_envoi(42, session, ClipVariant::Proxy, 11));

        // Le serveur limite l'identifiant à 64 caractères.
        assert!(identifiant_envoi(9_999_999, session, ClipVariant::Proxy, 99_999).len() <= 64);
    }

    #[test]
    fn la_sonde_reconnait_une_galerie_sans_identification() {
        let desactivee = (422, "Recognition is disabled for this event.".to_string(), false);
        let active = (422, "The frames field is required.".to_string(), true);
        let sans_galerie = (422, "Event has no gallery.".to_string(), false);

        assert_eq!(interpreter_sonde(desactivee), Some(false));
        assert_eq!(interpreter_sonde(active), Some(true));
        assert_eq!(interpreter_sonde(sans_galerie), None);
        assert_eq!(interpreter_sonde((500, String::new(), false)), None);
    }

    #[test]
    fn distingue_les_erreurs_definitives() {
        assert!(est_definitive(&refus_definitif(404, "Event not found.")));
        assert!(est_definitive(ANALYSE_DESACTIVEE));
        assert!(!est_definitive("Délai dépassé — réseau trop lent ou saturé."));
        assert!(!est_definitive("SATURE:30"));

        assert_eq!(
            message_lisible(&refus_definitif(404, "Event not found.")),
            "Event not found. (404)"
        );
    }
}
