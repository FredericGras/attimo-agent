// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Enregistrement segmenté (recorder.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// SAAS 430 — Découpage automatique en clips avec chevauchement.
//
// ─── LE PRINCIPE, ET POURQUOI IL EST AINSI ─────────────────────────────
//
// On n'enregistre PAS des clips. On enregistre des MORCEAUX courts, tous de
// la même durée, que l'on assemble ensuite pour former les clips.
//
// Ce détour paraît compliqué ; il est en réalité ce qui rend le reste
// simple. Deux exigences le commandent :
//
//   1. LE CHEVAUCHEMENT. Un coureur qui passe à cheval sur deux clips doit
//      apparaître entier dans au moins l'un des deux. Les clips doivent donc
//      se recouvrir — un même instant appartient à deux clips.
//
//   2. AUCUN RÉENCODAGE. Réencoder une course de quatre heures sur un
//      portable de terrain est hors de question : trop lent, trop chaud,
//      batterie vidée. Les morceaux sont donc recopiés tels quels.
//
// Or on ne peut recopier sans réencoder qu'aux frontières exactes des
// morceaux. D'où la règle centrale :
//
//   ┌──────────────────────────────────────────────────────────────────┐
//   │  La durée d'un morceau doit diviser À LA FOIS la durée du clip   │
//   │  ET le pas entre deux clips.                                     │
//   └──────────────────────────────────────────────────────────────────┘
//
// Le plus grand commun diviseur des deux donne la taille de morceau.
//
// ─── EXEMPLE CONCRET ───────────────────────────────────────────────────
//
// Clip de 150 s, chevauchement de 30 s → pas de 120 s → morceau de 30 s.
//
//   morceaux :  [0][1][2][3][4][5][6][7][8]  (30 s chacun)
//
//   clip 1   :  [0][1][2][3][4]               →   0 s à 150 s
//   clip 2   :            [4][5][6][7][8]     → 120 s à 270 s
//                          ↑
//                    morceau partagé = les 30 s de chevauchement
//
// Chaque clip prend 5 morceaux, et l'on avance de 4 morceaux à chaque fois.
// Le morceau commun aux deux clips EST le chevauchement — il n'y a rien à
// calculer de plus, la géométrie s'en charge.

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicI64, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::process::{CommandChild, CommandEvent};
use tauri_plugin_shell::ShellExt;
use tokio::sync::Notify;

/// Temps laissé à FFmpeg pour refermer son dernier fichier.
///
/// Écrire l'index d'un mp4 de trente secondes prend une fraction de
/// seconde. Au-delà de dix, quelque chose ne répond plus, et mieux vaut un
/// fichier tronqué qu'une interface bloquée sur le terrain.
const DELAI_ARRET_SECS: u64 = 10;

// ═══════════════════════════════════════════════════════════════════════
// RÉGLAGES
// ═══════════════════════════════════════════════════════════════════════

/// Ce que le photographe choisit avant de lancer sa captation.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RecordingConfig {
    /// Nom exact du périphérique, tel que Windows le donne.
    pub device_name: String,

    /// Micro associé. Absent = enregistrement muet.
    pub audio_device: Option<String>,

    pub width: u32,
    pub height: u32,
    pub fps: u32,

    /// Débit vidéo, en mégabits par seconde.
    pub bitrate_mbps: u32,

    pub clip_duration_secs: u32,
    pub overlap_secs: u32,

    /// Dossier de travail. Les morceaux y sont écrits, puis les clips.
    pub output_dir: String,
}

/// Découpage temporel calculé à partir des réglages.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct SegmentPlan {
    /// Durée d'un morceau, en secondes.
    pub segment_secs: u32,

    /// Nombre de morceaux formant un clip.
    pub segments_per_clip: u32,

    /// De combien de morceaux on avance entre deux clips.
    pub segments_step: u32,

    /// Intervalle réel entre deux débuts de clip.
    pub step_secs: u32,
}

impl RecordingConfig {
    /// Calcule le découpage, ou explique pourquoi les réglages ne vont pas.
    ///
    /// Les refus sont volontairement explicites : un message clair ici évite
    /// une captation lancée sur des réglages absurdes, et découverte trop
    /// tard.
    pub fn plan(&self) -> Result<SegmentPlan, String> {
        if self.clip_duration_secs == 0 {
            return Err("La durée d'un clip ne peut pas être nulle.".into());
        }

        if self.overlap_secs >= self.clip_duration_secs {
            return Err(
                "Le chevauchement doit rester inférieur à la durée du clip, \
                 sans quoi les clips se répéteraient sans jamais avancer."
                    .into(),
            );
        }

        let step_secs = self.clip_duration_secs - self.overlap_secs;
        let segment_secs = pgcd(self.clip_duration_secs, step_secs);

        // Un morceau trop court multiplie les fichiers et les ouvertures
        // disque ; trop long, il rend le premier clip très tardif.
        if segment_secs < 1 {
            return Err("Ces réglages ne permettent pas un découpage propre.".into());
        }

        Ok(SegmentPlan {
            segment_secs,
            segments_per_clip: self.clip_duration_secs / segment_secs,
            segments_step: step_secs / segment_secs,
            step_secs,
        })
    }
}

/// Plus grand commun diviseur — algorithme d'Euclide.
fn pgcd(a: u32, b: u32) -> u32 {
    if b == 0 {
        a
    } else {
        pgcd(b, a % b)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ÉTAT D'UNE CAPTATION
// ═══════════════════════════════════════════════════════════════════════

/// Poignée sur un enregistrement en cours.
pub struct RecordingHandle {
    pub session_id: String,
    pub running: Arc<AtomicBool>,
    /// Découpage retenu — l'assembleur en a besoin pour savoir quels
    /// morceaux forment quel clip.
    pub plan: SegmentPlan,
    /// Dossier de travail, où se trouvent les morceaux et les clips.
    pub work_dir: PathBuf,
    /// Instant absolu du début de la captation.
    ///
    /// Ancre de tous les horodatages d'images : sans lui, on saurait qu'un
    /// coureur passe à la trentième seconde d'un morceau, mais pas à quelle
    /// heure réelle — et le rattachement aux clips serait impossible.
    pub started_at: crate::frames::chrono_simple::Instant,

    /// Intervalle d'extraction des images, retenu au démarrage.
    ///
    /// En course, c'est l'interface qui le passe à chaque morceau. À
    /// l'arrêt, l'agent traite le dernier morceau tout seul et doit donc le
    /// connaître sans rien demander à personne.
    pub interval_secs: f32,

    /// Dernier morceau OUVERT par FFmpeg.
    ///
    /// Partagé avec la tâche d'écoute. C'est le seul morceau qui ne sera
    /// jamais annoncé : l'annonce d'un morceau est déclenchée par
    /// l'ouverture du suivant, et il n'y en aura pas.
    pub dernier_morceau: Arc<AtomicI64>,

    /// Signalé quand FFmpeg est parti ET que tout ce qu'il a écrit a été lu.
    fin_ffmpeg: Arc<Notify>,

    /// Un arrêt est déjà en cours (0.3.1).
    ///
    /// Posé sous le verrou de la captation par le premier arrêt : un second
    /// clic, ou l'arrêt de fin de session qui suit de près celui du bouton
    /// vidéo, repart aussitôt au lieu d'attendre dix secondes un FFmpeg déjà
    /// parti, puis de rattraper une deuxième fois le même morceau.
    pub arret_demande: bool,

    child: Option<CommandChild>,
}

impl RecordingHandle {
    /// Arrête l'enregistrement et attend que FFmpeg ait refermé son fichier.
    ///
    /// La touche « q » sur l'entrée standard est la seule demande d'arrêt
    /// que FFmpeg comprend : il finit son image en cours, écrit l'index du
    /// mp4, puis rend la main. Le tuer à la place laisse un fichier sans
    /// index, donc illisible — et c'est précisément celui qui contient les
    /// dernières secondes filmées.
    ///
    /// La version précédente faisait l'inverse de ce que son commentaire
    /// annonçait : `kill()` est un `TerminateProcess` sous Windows.
    ///
    /// Renvoie le numéro du morceau resté ouvert, s'il y en a un.
    pub async fn stop(&mut self) -> Option<u32> {
        // Le drapeau tombe en premier : il coupe la surveillance disque, et
        // il dit à la tâche d'écoute que la sortie de FFmpeg est attendue.
        // FFmpeg rend 255 quand il quitte sur « q » ; sans ce drapeau, son
        // départ normal serait journalisé comme une panne.
        self.running.store(false, Ordering::Relaxed);

        // Déjà arrêté : FFmpeg est parti et le signal de fin a été consommé.
        // L'attendre encore bloquerait dix secondes pour rien.
        if self.child.is_none() {
            let dernier = self.dernier_morceau.load(Ordering::Relaxed);
            return u32::try_from(dernier).ok();
        }

        if let Some(child) = self.child.as_mut() {
            let _ = child.write(b"q");
        }

        let rendu = tokio::time::timeout(
            Duration::from_secs(DELAI_ARRET_SECS),
            self.fin_ffmpeg.notified(),
        )
        .await
        .is_ok();

        if let Some(child) = self.child.take() {
            if !rendu {
                log::warn!(
                    "FFmpeg n'a pas rendu la main en {} s : arrêt forcé, \
                     le dernier morceau restera tronqué.",
                    DELAI_ARRET_SECS
                );

                let _ = child.kill();
            }
        }

        let dernier = self.dernier_morceau.load(Ordering::Relaxed);

        if dernier >= 0 {
            Some(dernier as u32)
        } else {
            None
        }
    }
}

/// Signalé au frontend à chaque morceau terminé.
#[derive(Debug, Clone, Serialize)]
pub struct SegmentEvent {
    pub session_id: String,
    pub index: u32,
    pub filename: String,
    pub started_at: String,
}

// ═══════════════════════════════════════════════════════════════════════
// LANCEMENT DE L'ENREGISTREMENT
// ═══════════════════════════════════════════════════════════════════════

/// Démarre la captation.
///
/// FFmpeg écrit des morceaux numérotés dans le dossier de travail. Il tourne
/// jusqu'à ce qu'on l'arrête : c'est un enregistrement continu, pas une série
/// d'appels successifs.
///
/// L'assemblage en clips est traité ailleurs, en surveillant l'apparition des
/// morceaux. Séparer les deux évite qu'un ralentissement de l'assemblage ne
/// perturbe la captation elle-même — sur le terrain, perdre des images est
/// irrattrapable.
pub async fn start_recording(
    app: &AppHandle,
    config: RecordingConfig,
    session_id: String,
    interval_secs: f32,
) -> Result<RecordingHandle, String> {
    let plan = config.plan()?;

    let dossier_morceaux = PathBuf::from(&config.output_dir).join("_morceaux");

    std::fs::create_dir_all(&dossier_morceaux)
        .map_err(|e| format!("Impossible de créer le dossier de travail : {}", e))?;

    let motif = dossier_morceaux
        .join("m_%06d.mp4")
        .to_string_lossy()
        .to_string();

    let arguments = construire_arguments(&config, &plan, &motif);

    log::info!(
        "Captation {} : morceaux de {} s, {} par clip, avance de {}",
        session_id,
        plan.segment_secs,
        plan.segments_per_clip,
        plan.segments_step
    );

    let commande = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| format!("FFmpeg embarqué introuvable : {}", e))?
        .args(arguments);

    let (mut evenements, enfant) = commande
        .spawn()
        .map_err(|e| format!("Impossible de lancer l'enregistrement : {}", e))?;

    let running = Arc::new(AtomicBool::new(true));

    // FFmpeg parle en continu sur sa sortie d'erreur : progression, débit,
    // avertissements. On l'écoute dans une tâche séparée pour repérer les
    // morceaux terminés sans bloquer l'appelant.
    let running_ecoute = running.clone();
    let app_ecoute = app.clone();
    let session_ecoute = session_id.clone();

    let dernier_morceau = Arc::new(AtomicI64::new(-1));
    let dernier_ecoute = dernier_morceau.clone();

    let fin_ffmpeg = Arc::new(Notify::new());
    let fin_ecoute = fin_ffmpeg.clone();

    tauri::async_runtime::spawn(async move {
        let mut dernier_index: i64 = -1;
        let mut dernieres_lignes: Vec<String> = Vec::new();

        // La boucle va jusqu'à la FERMETURE DU CANAL, et non jusqu'au
        // premier signe d'arrêt. Deux raisons, apprises à nos dépens :
        //
        //   — sortir sur `running` jetait les dernières lignes de FFmpeg,
        //     donc l'annonce du dernier morceau, sans même les lire ;
        //   — sortir sur `Terminated` n'est pas plus sûr : la sortie
        //     d'erreur et l'état de fin remontent par deux fils différents,
        //     rien ne garantit leur ordre d'arrivée.
        //
        // Le canal se ferme quand FFmpeg est parti et que tout ce qu'il a
        // écrit a été lu. C'est le seul instant où l'on sait vraiment quel
        // était le dernier morceau.
        while let Some(evenement) = evenements.recv().await {
            match evenement {
                CommandEvent::Stderr(donnees) | CommandEvent::Stdout(donnees) => {
                    let texte = String::from_utf8_lossy(&donnees);

                    // On garde les dernières lignes sous le coude. Quand
                    // FFmpeg refuse un périphérique, il le dit ici et nulle
                    // part ailleurs : sans cette trace, un photographe qui
                    // n'enregistre rien n'a aucun moyen de savoir pourquoi.
                    for ligne in texte.lines() {
                        let ligne = ligne.trim();

                        if !ligne.is_empty() {
                            dernieres_lignes.push(ligne.to_string());
                        }
                    }

                    while dernieres_lignes.len() > 25 {
                        dernieres_lignes.remove(0);
                    }

                    // FFmpeg annonce chaque ouverture de fichier. C'est le
                    // signal que le PRÉCÉDENT vient d'être clos, donc
                    // complet et exploitable.
                    if let Some(index) = detecter_ouverture_morceau(&texte) {
                        if dernier_index >= 0 {
                            let termine = dernier_index as u32;

                            let _ = app_ecoute.emit(
                                "segment-ready",
                                SegmentEvent {
                                    session_id: session_ecoute.clone(),
                                    index: termine,
                                    filename: format!("m_{:06}.mp4", termine),
                                    started_at: chrono_maintenant(),
                                },
                            );
                        }

                        dernier_index = index as i64;

                        // Lisible depuis l'arrêt de captation : c'est ce
                        // morceau-là qu'il faudra rattraper à la main.
                        dernier_ecoute.store(dernier_index, Ordering::Relaxed);
                    }
                }
                CommandEvent::Terminated(statut) => {
                    // Un arrêt demandé par le photographe est normal. Un
                    // arrêt spontané, non : il faut alors dire ce que
                    // FFmpeg a répondu.
                    let normal = !running_ecoute.load(Ordering::Relaxed)
                        || statut.code == Some(0);

                    if normal {
                        log::info!("Captation {} terminée : {:?}", session_ecoute, statut);
                    } else {
                        log::error!(
                            "Captation {} interrompue par FFmpeg : {:?}",
                            session_ecoute,
                            statut
                        );

                        for ligne in &dernieres_lignes {
                            log::error!("  ffmpeg | {}", ligne);
                        }
                    }
                }
                _ => {}
            }
        }

        // Plus rien à lire : le numéro du dernier morceau est définitif.
        fin_ecoute.notify_one();
    });

    Ok(RecordingHandle {
        session_id,
        running,
        plan,
        work_dir: PathBuf::from(&config.output_dir),
        started_at: crate::frames::chrono_simple::Instant::maintenant(),
        interval_secs,
        dernier_morceau,
        fin_ffmpeg,
        arret_demande: false,
        child: Some(enfant),
    })
}

// ═══════════════════════════════════════════════════════════════════════
// CONSTRUCTION DE LA COMMANDE
// ═══════════════════════════════════════════════════════════════════════

/// Assemble les arguments passés à FFmpeg.
///
/// Isolé dans sa propre fonction pour être lisible et testable : c'est la
/// partie la plus délicate à régler, et celle qu'on relira le plus souvent.
fn construire_arguments(
    config: &RecordingConfig,
    plan: &SegmentPlan,
    motif_sortie: &str,
) -> Vec<String> {
    let mut args: Vec<String> = Vec::new();

    args.push("-hide_banner".into());

    // ─── Entrée ───

    args.push("-f".into());
    args.push("dshow".into());

    args.push("-video_size".into());
    args.push(format!("{}x{}", config.width, config.height));

    args.push("-framerate".into());
    args.push(config.fps.to_string());

    // Mémoire tampon : absorbe les micro-décrochages du périphérique sans
    // perdre d'images. Indispensable sur un flux réseau comme un téléphone.
    args.push("-rtbufsize".into());
    args.push("512M".into());

    let entree = match &config.audio_device {
        Some(micro) => format!("video={}:audio={}", config.device_name, micro),
        None => format!("video={}", config.device_name),
    };

    args.push("-i".into());
    args.push(entree);

    // ─── Encodage vidéo ───

    args.push("-c:v".into());
    args.push("libx264".into());

    // « veryfast » plutôt que « ultrafast » : le gain de vitesse d'ultrafast
    // se paie par un fichier nettement plus lourd à qualité égale, ce qui
    // pèse ensuite sur l'envoi en 4G — le vrai goulot d'étranglement.
    args.push("-preset".into());
    args.push("veryfast".into());

    args.push("-b:v".into());
    args.push(format!("{}M", config.bitrate_mbps));

    // Une image-clé à chaque seconde.
    //
    // POINT CRITIQUE : la découpe en morceaux ne peut se faire QUE sur une
    // image-clé. Sans cette contrainte, FFmpeg en place où il veut et les
    // morceaux ne tombent plus à la bonne durée — le chevauchement dérive,
    // et l'assemblage produit des clips de longueur variable.
    args.push("-g".into());
    args.push(config.fps.to_string());

    args.push("-keyint_min".into());
    args.push(config.fps.to_string());

    // Interdit à l'encodeur d'insérer des images-clés supplémentaires sur
    // les changements de scène : elles casseraient la régularité.
    args.push("-sc_threshold".into());
    args.push("0".into());

    args.push("-pix_fmt".into());
    args.push("yuv420p".into());

    // ─── Audio ───

    if config.audio_device.is_some() {
        args.push("-c:a".into());
        args.push("aac".into());
        args.push("-b:a".into());
        args.push("128k".into());
    } else {
        args.push("-an".into());
    }

    // ─── Découpage en morceaux ───

    args.push("-f".into());
    args.push("segment".into());

    args.push("-segment_time".into());
    args.push(plan.segment_secs.to_string());

    // Chaque morceau redémarre à zéro : indispensable pour qu'ils
    // s'assemblent ensuite sans décalage temporel.
    args.push("-reset_timestamps".into());
    args.push("1".into());

    // Force la coupe exactement à la durée demandée, sans attendre la
    // prochaine image-clé « naturelle ».
    args.push("-force_key_frames".into());
    args.push(format!("expr:gte(t,n_forced*{})", plan.segment_secs));

    args.push("-segment_format".into());
    args.push("mp4".into());

    args.push("-y".into());
    args.push(motif_sortie.to_string());

    args
}

// ═══════════════════════════════════════════════════════════════════════
// LECTURE DE LA SORTIE DE FFMPEG
// ═══════════════════════════════════════════════════════════════════════

/// Repère l'ouverture d'un nouveau morceau dans le bavardage de FFmpeg.
///
/// La ligne ressemble à :
///
///   [segment @ 000] Opening 'C:\...\_morceaux\m_000003.mp4' for writing
///
/// L'ouverture du morceau N signifie que le morceau N-1 vient d'être clos :
/// c'est à ce moment qu'il devient exploitable, et pas avant.
fn detecter_ouverture_morceau(texte: &str) -> Option<u32> {
    for ligne in texte.lines() {
        if !ligne.contains("Opening") || !ligne.contains("for writing") {
            continue;
        }

        // On isole le nom de fichier entre les apostrophes.
        let debut = ligne.find("m_")?;
        let reste = &ligne[debut + 2..];

        let numero: String = reste.chars().take_while(|c| c.is_ascii_digit()).collect();

        if numero.is_empty() {
            continue;
        }

        if let Ok(index) = numero.parse::<u32>() {
            return Some(index);
        }
    }

    None
}

/// Horodatage absolu, avec fuseau.
///
/// Le fuseau n'est pas décoratif : c'est le seul lien entre les images
/// d'analyse, les clips et les participants. Une heure sans fuseau décalerait
/// tout le rattachement d'une heure en été.
fn chrono_maintenant() -> String {
    use std::time::{SystemTime, UNIX_EPOCH};

    let secondes = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);

    format!("@{}", secondes)
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn config_type(duree: u32, chevauchement: u32) -> RecordingConfig {
        RecordingConfig {
            device_name: "Test".into(),
            audio_device: None,
            width: 1920,
            height: 1080,
            fps: 30,
            bitrate_mbps: 12,
            clip_duration_secs: duree,
            overlap_secs: chevauchement,
            output_dir: "C:\\tmp".into(),
        }
    }

    #[test]
    fn decoupe_le_reglage_de_reference() {
        // 150 s de clip, 30 s de chevauchement — le défaut du contrat.
        let plan = config_type(150, 30).plan().unwrap();

        assert_eq!(plan.step_secs, 120);
        assert_eq!(plan.segment_secs, 30);
        assert_eq!(plan.segments_per_clip, 5);
        assert_eq!(plan.segments_step, 4);
    }

    #[test]
    fn decoupe_des_clips_longs() {
        let plan = config_type(300, 30).plan().unwrap();

        assert_eq!(plan.segment_secs, 30);
        assert_eq!(plan.segments_per_clip, 10);
        assert_eq!(plan.segments_step, 9);
    }

    #[test]
    fn decoupe_des_clips_courts() {
        // Réglage de test rapide : 20 s de clip, 5 s de chevauchement.
        let plan = config_type(20, 5).plan().unwrap();

        assert_eq!(plan.segment_secs, 5);
        assert_eq!(plan.segments_per_clip, 4);
        assert_eq!(plan.segments_step, 3);
    }

    #[test]
    fn refuse_un_chevauchement_trop_grand() {
        assert!(config_type(60, 60).plan().is_err());
        assert!(config_type(60, 90).plan().is_err());
    }

    #[test]
    fn refuse_une_duree_nulle() {
        assert!(config_type(0, 0).plan().is_err());
    }

    #[test]
    fn le_chevauchement_reel_correspond_au_demande() {
        // Vérifie la géométrie : les morceaux communs à deux clips
        // consécutifs doivent totaliser exactement le chevauchement voulu.
        for (duree, chevauchement) in [(150, 30), (300, 30), (120, 20), (60, 15)] {
            let plan = config_type(duree, chevauchement).plan().unwrap();

            let morceaux_communs = plan.segments_per_clip - plan.segments_step;
            let chevauchement_reel = morceaux_communs * plan.segment_secs;

            assert_eq!(
                chevauchement_reel, chevauchement,
                "clip de {} s avec {} s de chevauchement",
                duree, chevauchement
            );
        }
    }

    #[test]
    fn lit_le_numero_du_morceau_ouvert() {
        let texte = "[segment @ 0000] Opening 'C:\\Video\\_morceaux\\m_000003.mp4' for writing";

        assert_eq!(detecter_ouverture_morceau(texte), Some(3));
    }

    #[test]
    fn ignore_le_bavardage_ordinaire() {
        let texte = "frame= 120 fps= 30 q=28.0 size=  1024kB time=00:00:04.00 bitrate=2097.2kbits/s";

        assert_eq!(detecter_ouverture_morceau(texte), None);
    }

    #[test]
    fn place_bien_les_arguments_de_decoupe() {
        let config = config_type(150, 30);
        let plan = config.plan().unwrap();
        let args = construire_arguments(&config, &plan, "C:\\tmp\\m_%06d.mp4");

        let position = args.iter().position(|a| a == "-segment_time").unwrap();
        assert_eq!(args[position + 1], "30");

        assert!(args.contains(&"-reset_timestamps".to_string()));
        assert!(args.contains(&"segment".to_string()));
    }

    #[test]
    fn coupe_le_son_quand_aucun_micro_nest_choisi() {
        let config = config_type(150, 30);
        let plan = config.plan().unwrap();
        let args = construire_arguments(&config, &plan, "C:\\tmp\\m_%06d.mp4");

        assert!(args.contains(&"-an".to_string()));
    }
}
