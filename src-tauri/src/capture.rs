// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Capture vidéo (capture.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// SAAS 430 — Découpage automatique en clips.
//
// Ce module pilote FFmpeg, embarqué comme binaire compagnon dans
// src-tauri/binaries/. Le photographe n'a rien à installer : c'est un
// enseignement direct de l'outil PowerShell de Fabien, où l'absence de
// FFmpeg sur le poste était la première cause d'échec.
//
// ─── PREMIÈRE BRIQUE : voir le matériel ────────────────────────────────
//
// Aucune liste d'appareils n'est tenue ici, et il ne peut pas y en avoir :
// entre les reflex en mode webcam, les téléphones via application, les
// caméras d'action et les boîtiers d'acquisition, un catalogue serait
// obsolète avant d'être écrit.
//
// C'est WINDOWS qui connaît les périphériques branchés. FFmpeg lui pose la
// question via DirectShow, et nous transmettons la réponse telle quelle. Un
// appareil sorti dans trois ans apparaîtra sans que nous touchions au code,
// du moment que Windows sait le lire.
//
// Deux questions distinctes :
//
//   list_video_devices()      « quels appareils sont branchés ? »
//   probe_device_options()    « que sait faire CET appareil ? »
//
// La seconde interroge le matériel lui-même sur ses résolutions et
// cadences. Là encore, rien n'est deviné.

use serde::{Deserialize, Serialize};
use tauri::AppHandle;
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;

// ═══════════════════════════════════════════════════════════════════════
// STRUCTURES
// ═══════════════════════════════════════════════════════════════════════

/// Un périphérique vu par Windows.
///
/// `name` est le libellé exact retourné par le système : c'est lui qu'il
/// faudra redonner à FFmpeg pour ouvrir le flux, au caractère près.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct VideoDevice {
    pub name: String,
    pub kind: DeviceKind,
}

/// Nature du périphérique.
///
/// Le micro est listé séparément : sur un reflex en mode webcam, l'image et
/// le son se présentent souvent comme deux appareils distincts, et le
/// photographe doit pouvoir les choisir indépendamment.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum DeviceKind {
    Video,
    Audio,
}

/// Un mode de capture supporté par un périphérique donné.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DeviceOption {
    pub width: u32,
    pub height: u32,
    pub min_fps: f32,
    pub max_fps: f32,
    /// Format brut annoncé par le périphérique (mjpeg, yuyv422, nv12…).
    pub pixel_format: String,
}

/// Résultat complet de l'interrogation d'un périphérique.
#[derive(Debug, Clone, Serialize)]
pub struct DeviceCapabilities {
    pub device_name: String,
    pub options: Vec<DeviceOption>,
    /// Meilleur compromis proposé par défaut — voir suggest_best_option().
    pub suggested: Option<DeviceOption>,
}

// ═══════════════════════════════════════════════════════════════════════
// ÉNUMÉRATION DES PÉRIPHÉRIQUES
// ═══════════════════════════════════════════════════════════════════════

/// Demande à Windows la liste des appareils vidéo et audio branchés.
///
/// FFmpeg écrit cette liste sur sa sortie d'erreur, pas sur sa sortie
/// standard — comportement normal de sa part, ce n'est pas un échec. Il
/// termine d'ailleurs avec un code d'erreur, puisqu'on ne lui a donné
/// aucun fichier à traiter : c'est attendu, et le code ne doit surtout pas
/// l'interpréter comme un problème.
pub async fn list_devices(app: &AppHandle) -> Result<Vec<VideoDevice>, String> {
    let sortie = executer_ffmpeg(
        app,
        vec![
            "-hide_banner".into(),
            "-list_devices".into(),
            "true".into(),
            "-f".into(),
            "dshow".into(),
            "-i".into(),
            "dummy".into(),
        ],
    )
    .await?;

    Ok(parser_liste_peripheriques(&sortie))
}

/// Renvoie la sortie brute de FFmpeg, sans interprétation.
///
/// Provisoire, à des fins de diagnostic : permet de lire le texte exact des
/// en-têtes de section, qui varient selon les versions de FFmpeg.
pub async fn debug_raw_devices(app: &AppHandle) -> Result<String, String> {
    executer_ffmpeg(
        app,
        vec![
            "-hide_banner".into(),
            "-list_devices".into(),
            "true".into(),
            "-f".into(),
            "dshow".into(),
            "-i".into(),
            "dummy".into(),
        ],
    )
    .await
}

/// Extrait les noms de périphériques de la sortie de FFmpeg.
///
/// Le format ressemble à ceci :
///
///   [dshow @ ...] DirectShow video devices
///   [dshow @ ...]  "Iriun Webcam"
///   [dshow @ ...]     Alternative name "@device_pnp_\\?\usb#vid_..."
///   [dshow @ ...] DirectShow audio devices
///   [dshow @ ...]  "Microphone (Iriun Webcam)"
///
/// Deux pièges traités ici :
///
///   1. Les lignes « Alternative name » contiennent aussi des guillemets,
///      mais ce sont des identifiants système illisibles. On les écarte.
///
///   2. La bascule vidéo → audio se fait sur une ligne d'en-tête. Sans la
///      détecter, tous les micros seraient pris pour des caméras.
fn parser_liste_peripheriques(sortie: &str) -> Vec<VideoDevice> {
    let mut peripheriques = Vec::new();

    for ligne in sortie.lines() {
        // FFmpeg marque le type sur chaque ligne, entre parenthèses :
        //
        //   [in#0 @ ...] "Iriun Webcam" (video)
        //   [in#0 @ ...] "Microphone (Iriun Webcam)" (audio)
        //
        // Attention : le nom du micro contient lui-même une parenthèse.
        // C'est donc la parenthèse FINALE qui porte le type, d'où le test sur
        // la fin de ligne plutôt que sur son contenu.
        let ligne_nettoyee = ligne.trim_end();

        let kind = if ligne_nettoyee.ends_with("(audio)") {
            DeviceKind::Audio
        } else if ligne_nettoyee.ends_with("(video)") {
            DeviceKind::Video
        } else {
            continue;
        };

        // Identifiant système illisible, pas un nom présentable.
        if ligne.contains("Alternative name") {
            continue;
        }

        let Some(nom) = extraire_entre_guillemets(ligne) else {
            continue;
        };

        if nom.is_empty() {
            continue;
        }

        peripheriques.push(VideoDevice { name: nom, kind });
    }

    peripheriques
}

/// Récupère le contenu de la première paire de guillemets d'une ligne.
fn extraire_entre_guillemets(ligne: &str) -> Option<String> {
    let debut = ligne.find('"')?;
    let reste = &ligne[debut + 1..];
    let fin = reste.find('"')?;

    Some(reste[..fin].to_string())
}

// ═══════════════════════════════════════════════════════════════════════
// CAPACITÉS D'UN PÉRIPHÉRIQUE
// ═══════════════════════════════════════════════════════════════════════

/// Interroge un périphérique sur les modes qu'il accepte.
///
/// Indispensable avant de lancer un enregistrement : demander du 1920×1080
/// à un appareil qui ne le propose pas fait échouer FFmpeg avec un message
/// que le photographe ne saura pas interpréter. Mieux vaut ne lui présenter
/// que des réglages réellement disponibles.
pub async fn probe_device(app: &AppHandle, device_name: &str) -> Result<DeviceCapabilities, String> {
    let sortie = executer_ffmpeg(
        app,
        vec![
            "-hide_banner".into(),
            "-list_options".into(),
            "true".into(),
            "-f".into(),
            "dshow".into(),
            "-i".into(),
            format!("video={}", device_name),
        ],
    )
    .await?;

    let options = parser_options(&sortie);
    let suggested = suggest_best_option(&options);

    Ok(DeviceCapabilities {
        device_name: device_name.to_string(),
        options,
        suggested,
    })
}

/// Extrait les modes supportés de la sortie de FFmpeg.
///
/// Chaque ligne utile ressemble à :
///
///   [dshow @ ...]   pixel_format=yuyv422  min s=640x480 fps=5 max s=1920x1080 fps=30
///
/// Seules les valeurs « max » sont retenues : ce sont les limites hautes du
/// périphérique, et c'est ce qui intéresse le photographe.
fn parser_options(sortie: &str) -> Vec<DeviceOption> {
    let mut options: Vec<DeviceOption> = Vec::new();

    for ligne in sortie.lines() {
        if !ligne.contains("max s=") {
            continue;
        }

        let format = extraire_valeur(ligne, "pixel_format=")
            .or_else(|| extraire_valeur(ligne, "vcodec="))
            .unwrap_or_else(|| "inconnu".to_string());

        let Some(resolution) = extraire_valeur(ligne, "max s=") else {
            continue;
        };

        let Some((largeur, hauteur)) = parser_resolution(&resolution) else {
            continue;
        };

        let fps_min = extraire_valeur(ligne, "min s=")
            .and_then(|_| extraire_apres(ligne, "fps="))
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(1.0);

        let fps_max = extraire_dernier(ligne, "fps=")
            .and_then(|v| v.parse::<f32>().ok())
            .unwrap_or(30.0);

        let option = DeviceOption {
            width: largeur,
            height: hauteur,
            min_fps: fps_min,
            max_fps: fps_max,
            pixel_format: format,
        };

        // Un même mode est souvent annoncé plusieurs fois par le pilote.
        let deja_present = options.iter().any(|o| {
            o.width == option.width
                && o.height == option.height
                && o.pixel_format == option.pixel_format
        });

        if !deja_present {
            options.push(option);
        }
    }

    options
}

/// Choisit le mode à proposer par défaut au photographe.
///
/// La priorité va au 1920×1080 : c'est la résolution retenue par le contrat
/// d'interface pour l'analyse des dossards, et descendre en dessous
/// réintroduit des lectures erronées — mesuré, pas supposé.
///
/// À défaut, on prend la définition la plus haute disponible.
fn suggest_best_option(options: &[DeviceOption]) -> Option<DeviceOption> {
    if options.is_empty() {
        return None;
    }

    let full_hd = options
        .iter()
        .filter(|o| o.width == 1920 && o.height == 1080)
        .max_by(|a, b| a.max_fps.partial_cmp(&b.max_fps).unwrap_or(std::cmp::Ordering::Equal));

    if let Some(o) = full_hd {
        return Some(o.clone());
    }

    options
        .iter()
        .max_by_key(|o| o.width * o.height)
        .cloned()
}

// ═══════════════════════════════════════════════════════════════════════
// EXÉCUTION DE FFMPEG
// ═══════════════════════════════════════════════════════════════════════

/// Lance le FFmpeg embarqué et récupère l'intégralité de sa sortie.
///
/// Les deux flux — standard et erreur — sont fusionnés volontairement :
/// FFmpeg écrit ses informations sur la sortie d'erreur, y compris quand
/// tout se passe bien. Ne lire que la sortie standard ne remonterait rien.
///
/// Le code de retour est ignoré pour la même raison : sur une simple
/// interrogation de périphériques, FFmpeg termine toujours en erreur
/// puisqu'il n'a reçu aucun fichier à traiter. Ce n'est pas un échec.
async fn executer_ffmpeg(app: &AppHandle, arguments: Vec<String>) -> Result<String, String> {
    let commande = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| format!("FFmpeg embarqué introuvable : {}", e))?
        .args(arguments);

    let (mut evenements, _enfant) = commande
        .spawn()
        .map_err(|e| format!("Impossible de lancer FFmpeg : {}", e))?;

    let mut sortie = String::new();

    while let Some(evenement) = evenements.recv().await {
        match evenement {
            CommandEvent::Stdout(donnees) | CommandEvent::Stderr(donnees) => {
                sortie.push_str(&String::from_utf8_lossy(&donnees));
                sortie.push('\n');
            }
            CommandEvent::Terminated(_) => break,
            _ => {}
        }
    }

    Ok(sortie)
}

/// Vérifie que le FFmpeg embarqué répond.
///
/// À appeler au démarrage : mieux vaut un message clair tout de suite qu'un
/// échec incompréhensible au moment où le photographe lance sa course.
pub async fn check_ffmpeg(app: &AppHandle) -> Result<String, String> {
    let sortie = executer_ffmpeg(app, vec!["-version".into()]).await?;

    sortie
        .lines()
        .next()
        .map(|l| l.trim().to_string())
        .ok_or_else(|| "FFmpeg n'a produit aucune réponse.".to_string())
}

// ═══════════════════════════════════════════════════════════════════════
// OUTILS DE LECTURE
// ═══════════════════════════════════════════════════════════════════════

/// Valeur suivant une clé, jusqu'à l'espace suivant.
fn extraire_valeur(ligne: &str, cle: &str) -> Option<String> {
    let position = ligne.find(cle)? + cle.len();
    let reste = &ligne[position..];

    let valeur: String = reste
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();

    if valeur.is_empty() {
        None
    } else {
        Some(valeur)
    }
}

/// Première occurrence d'une clé — utilisée pour la cadence minimale.
fn extraire_apres(ligne: &str, cle: &str) -> Option<String> {
    extraire_valeur(ligne, cle)
}

/// Dernière occurrence d'une clé.
///
/// La cadence apparaît deux fois par ligne, une pour le minimum et une pour
/// le maximum. C'est la seconde qui nous intéresse.
fn extraire_dernier(ligne: &str, cle: &str) -> Option<String> {
    let position = ligne.rfind(cle)? + cle.len();
    let reste = &ligne[position..];

    let valeur: String = reste
        .chars()
        .take_while(|c| !c.is_whitespace())
        .collect();

    if valeur.is_empty() {
        None
    } else {
        Some(valeur)
    }
}

/// Découpe une chaîne « 1920x1080 » en deux nombres.
fn parser_resolution(texte: &str) -> Option<(u32, u32)> {
    let (largeur, hauteur) = texte.split_once('x')?;

    Some((largeur.parse().ok()?, hauteur.parse().ok()?))
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn separe_bien_video_et_audio() {
        let sortie = r#"
[dshow @ 000] DirectShow video devices (some may be both video and audio devices)
[dshow @ 000]  "Iriun Webcam"
[dshow @ 000]     Alternative name "@device_pnp_\\?\usb#vid_046d"
[dshow @ 000] DirectShow audio devices
[dshow @ 000]  "Microphone (Iriun Webcam)"
[dshow @ 000]     Alternative name "@device_cm_{33D9A762}"
"#;

        let peripheriques = parser_liste_peripheriques(sortie);

        assert_eq!(peripheriques.len(), 2);
        assert_eq!(peripheriques[0].name, "Iriun Webcam");
        assert_eq!(peripheriques[0].kind, DeviceKind::Video);
        assert_eq!(peripheriques[1].kind, DeviceKind::Audio);
    }

    #[test]
    fn ignore_les_noms_techniques() {
        let sortie = r#"
[dshow @ 000] DirectShow video devices
[dshow @ 000]  "Canon EOS Webcam Utility"
[dshow @ 000]     Alternative name "@device_pnp_\\?\usb#vid_04a9#pid_32c1"
"#;

        let peripheriques = parser_liste_peripheriques(sortie);

        assert_eq!(peripheriques.len(), 1);
        assert_eq!(peripheriques[0].name, "Canon EOS Webcam Utility");
    }

    #[test]
    fn lit_les_resolutions_annoncees() {
        let sortie = r#"
[dshow @ 000]   pixel_format=yuyv422  min s=640x480 fps=5 max s=1920x1080 fps=30
[dshow @ 000]   pixel_format=mjpeg  min s=640x480 fps=5 max s=1280x720 fps=60
"#;

        let options = parser_options(sortie);

        assert_eq!(options.len(), 2);
        assert_eq!(options[0].width, 1920);
        assert_eq!(options[0].height, 1080);
        assert_eq!(options[0].pixel_format, "yuyv422");
    }

    #[test]
    fn propose_le_full_hd_en_priorite() {
        let options = vec![
            DeviceOption { width: 640, height: 480, min_fps: 5.0, max_fps: 30.0, pixel_format: "yuyv422".into() },
            DeviceOption { width: 1920, height: 1080, min_fps: 5.0, max_fps: 30.0, pixel_format: "mjpeg".into() },
            DeviceOption { width: 1280, height: 720, min_fps: 5.0, max_fps: 60.0, pixel_format: "mjpeg".into() },
        ];

        let choix = suggest_best_option(&options).unwrap();

        assert_eq!(choix.width, 1920);
    }

    #[test]
    fn se_rabat_sur_la_plus_haute_definition() {
        let options = vec![
            DeviceOption { width: 640, height: 480, min_fps: 5.0, max_fps: 30.0, pixel_format: "yuyv422".into() },
            DeviceOption { width: 1280, height: 720, min_fps: 5.0, max_fps: 60.0, pixel_format: "mjpeg".into() },
        ];

        let choix = suggest_best_option(&options).unwrap();

        assert_eq!(choix.width, 1280);
    }
}
