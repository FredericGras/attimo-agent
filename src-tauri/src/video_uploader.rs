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
use std::time::Duration;

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

/// Nombre d'images envoyées par requête.
///
/// Dix images font environ 3,3 Mo. Au-delà, on s'approche des limites de
/// taille de requête, et une coupure ferait reperdre un lot trop gros.
const FRAMES_PAR_LOT: usize = 10;

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
    #[serde(default)]
    message: Option<String>,
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

fn creer_client(timeout_secs: u64) -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(Duration::from_secs(timeout_secs))
        .build()
        .map_err(|e| format!("Impossible de créer le client réseau : {}", e))
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
    let client = creer_client(TIMEOUT_CLIP_SECS)?;

    let octets = tokio::fs::read(chemin)
        .await
        .map_err(|e| format!("Lecture du clip impossible : {}", e))?;

    if octets.is_empty() {
        return Err("Le clip est vide.".to_string());
    }

    let nom_fichier = chemin
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "clip.mp4".to_string());

    // Identifiant d'envoi : lie les morceaux entre eux côté serveur.
    let upload_id = format!(
        "{}_{}_{}",
        config.session_id,
        variant.as_str(),
        clip_index
    );

    let total_morceaux = octets.len().div_ceil(CHUNK_SIZE);

    // ─── Envoi des morceaux ───

    for (index, tranche) in octets.chunks(CHUNK_SIZE).enumerate() {
        let part = reqwest::multipart::Part::bytes(tranche.to_vec())
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
            .multipart(formulaire)
            .send()
            .await
            .map_err(|e| message_erreur(&e))?;

        if reponse.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err("SESSION_EXPIRED".to_string());
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
        .json(&charge)
        .send()
        .await
        .map_err(|e| message_erreur(&e))?;

    if reponse.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("SESSION_EXPIRED".to_string());
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

/// Envoie un lot d'images d'analyse.
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
    if images.is_empty() {
        return Ok(FrameTotals {
            bibs: 0,
            faces: 0,
            kept: 0,
            discarded: 0,
        });
    }

    let client = creer_client(TIMEOUT_FRAMES_SECS)?;

    let mut cumul = FrameTotals {
        bibs: 0,
        faces: 0,
        kept: 0,
        discarded: 0,
    };

    for lot in images.chunks(FRAMES_PAR_LOT) {
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

        let url = format!(
            "{}/api/sport/events/{}/frames",
            API_BASE_URL, config.event_id
        );

        let reponse = authentifier(client.post(&url), &config.token)
            .multipart(formulaire)
            .send()
            .await
            .map_err(|e| message_erreur(&e))?;

        if reponse.status() == reqwest::StatusCode::UNAUTHORIZED {
            return Err("SESSION_EXPIRED".to_string());
        }

        // Le serveur refuse les images si l'épreuve tourne sans
        // reconnaissance : inutile de continuer à en envoyer.
        if reponse.status() == reqwest::StatusCode::UNPROCESSABLE_ENTITY {
            return Err("ANALYSE_DESACTIVEE".to_string());
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

        if let Some(totaux) = corps.totals {
            cumul.bibs += totaux.bibs;
            cumul.faces += totaux.faces;
            cumul.kept += totaux.kept;
            cumul.discarded += totaux.discarded;
        }
    }

    Ok(cumul)
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
    let client = creer_client(TIMEOUT_FRAMES_SECS)?;

    let url = format!(
        "{}/api/sport/events/{}/clips/status?session_id={}",
        API_BASE_URL, config.event_id, config.session_id
    );

    let reponse = authentifier(client.get(&url), &config.token)
        .send()
        .await
        .map_err(|e| message_erreur(&e))?;

    if reponse.status() == reqwest::StatusCode::UNAUTHORIZED {
        return Err("SESSION_EXPIRED".to_string());
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
    fn lidentifiant_denvoi_distingue_les_variantes() {
        // Deux variantes du même clip ne doivent pas se mélanger côté
        // serveur : leurs morceaux portent des identifiants distincts.
        let session = "test_123";

        let proxy = format!("{}_{}_{}", session, ClipVariant::Proxy.as_str(), 1);
        let hd = format!("{}_{}_{}", session, ClipVariant::Hd.as_str(), 1);

        assert_ne!(proxy, hd);
    }
}
