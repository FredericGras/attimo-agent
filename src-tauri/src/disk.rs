// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Espace disque (disk.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// SAAS 430 — Estimation avant captation, surveillance pendant, arrêt propre
// avant saturation.
//
// ─── POURQUOI CE MODULE EXISTE ─────────────────────────────────────────
//
// Une captation qui s'arrête faute de place ne prévient pas : FFmpeg écrit
// un fichier tronqué, puis se tait. Le photographe ne s'en aperçoit qu'au
// retour, quand la moitié de sa course manque. C'est irrattrapable.
//
// Le rôle de ce module n'est donc PAS d'économiser du disque — rien n'est
// supprimé, c'est un choix assumé : le fichier d'origine reste le filet de
// sécurité de tout le reste. Son rôle est d'informer honnêtement, tôt, et
// d'arrêter proprement plutôt que de laisser corrompre.
//
// ─── CE QUI S'ÉCRIT RÉELLEMENT ─────────────────────────────────────────
//
// Quatre flux, tous conservés, pour la même seconde captée :
//
//   1. LE MORCEAU. La matière première, telle que FFmpeg la produit.
//
//   2. LE CLIP. Les morceaux recopiés bout à bout. Le contenu est identique
//      — mais les clips SE CHEVAUCHENT, donc certains morceaux sont écrits
//      deux fois. D'où un facteur supérieur à 1.
//
//   3. LA VERSION LÉGÈRE. Réencodée en 540p, elle subit le même facteur de
//      chevauchement puisqu'elle dérive du clip.
//
//   4. LES IMAGES D'ANALYSE. Des JPEG 1920 px, à intervalle régulier.
//
// L'erreur naturelle est de ne compter que le chevauchement. Ce serait
// oublier que morceaux ET clips coexistent : le facteur réel n'est pas
// 1,25 mais approche 2,7.
//
// ─── ESTIMATION PUIS MESURE ────────────────────────────────────────────
//
// L'estimation théorique donne un ordre de grandeur avant de partir. Elle
// se trompe forcément un peu : le poids d'un JPEG dépend de la scène, et
// x264 en débit variable respecte sa consigne à quelques pour cent près.
//
// Dès que la captation tourne, on ne devine plus : on pèse le dossier de
// travail à intervalle régulier, et le débit réellement observé remplace le
// débit théorique. C'est lui qui décide de l'alerte et de l'arrêt.

use serde::Serialize;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use tauri::{AppHandle, Emitter};

// ═══════════════════════════════════════════════════════════════════════
// CONSTANTES
// ═══════════════════════════════════════════════════════════════════════

const GIO: u64 = 1024 * 1024 * 1024;

/// Réserve intouchable sur le disque système.
///
/// Windows a besoin de respirer : fichier d'échange, mises à jour, fichiers
/// temporaires. Un disque système plein ne se contente pas de gêner l'agent,
/// il rend la machine inutilisable — au milieu d'une course.
const RESERVE_DISQUE_SYSTEME: u64 = 50 * GIO;

/// Réserve minimale sur un disque de données.
///
/// Un disque externe saturé ne casse pas Windows, mais il empêche FFmpeg de
/// clore correctement le morceau en cours — donc un fichier tronqué, donc
/// des secondes de course perdues.
const RESERVE_DISQUE_DONNEES: u64 = 10 * GIO;

/// Poids supposé d'une image d'analyse, avant toute mesure réelle.
///
/// JPEG 1920 px de large, qualité 65, scène de course en extérieur. Valeur
/// de départ uniquement : dès le premier morceau extrait, la mesure prend le
/// relais.
const POIDS_IMAGE_SUPPOSE: u64 = 300_000;

/// Débit audio du flux source, en bits par seconde (AAC 128k).
const DEBIT_AUDIO_SOURCE: u64 = 128_000;

/// Débit audio de la version légère, en bits par seconde (AAC 96k).
const DEBIT_AUDIO_PROXY: u64 = 96_000;

/// Réglages par défaut de la version légère, alignés sur ce que
/// l'interface demande aujourd'hui à `generate_proxy`.
const PROXY_DEBIT_MBPS_DEFAUT: u32 = 2;

/// Intervalle par défaut entre deux images d'analyse, en secondes.
const INTERVALLE_IMAGES_DEFAUT: f32 = 2.0;

/// Cadence de la surveillance, en secondes.
///
/// Quinze secondes : assez souvent pour ne pas rater une chute brutale
/// d'espace, assez rare pour que peser le dossier reste sans effet sur la
/// captation.
const PERIODE_SURVEILLANCE_SECS: u64 = 15;

/// Durée minimale avant de se fier au débit mesuré, en secondes.
///
/// Avant cela, l'échantillon est trop court : le premier clip n'est pas
/// encore assemblé, et le débit apparent serait sous-estimé de moitié.
const DELAI_AVANT_MESURE_SECS: u64 = 120;

/// Seuils d'autonomie restante, en secondes.
const SEUIL_ALERTE_SECS: u64 = 30 * 60;
const SEUIL_ALERTE_FORTE_SECS: u64 = 15 * 60;
const SEUIL_ARRET_SECS: u64 = 5 * 60;

// ═══════════════════════════════════════════════════════════════════════
// PARAMÈTRES D'ÉCRITURE
// ═══════════════════════════════════════════════════════════════════════

/// Tout ce qui détermine la quantité écrite par seconde de captation.
///
/// Rassemblé ici plutôt que dispersé : l'estimation d'avant-course et la
/// surveillance d'en-course doivent raisonner sur exactement les mêmes
/// chiffres, sans quoi le photographe verrait deux prévisions divergentes.
#[derive(Debug, Clone, Copy)]
pub struct FluxParams {
    /// Débit vidéo de la source, en mégabits par seconde.
    pub bitrate_mbps: u32,

    /// Un micro est-il branché ? Sans lui, aucun octet d'audio.
    pub avec_audio: bool,

    /// Durée d'un clip, en secondes.
    pub clip_duration_secs: u32,

    /// Intervalle entre deux débuts de clip, en secondes.
    pub step_secs: u32,

    /// Débit de la version légère, en mégabits par seconde.
    pub proxy_bitrate_mbps: u32,

    /// Intervalle entre deux images d'analyse, en secondes.
    pub frame_interval_secs: f32,

    /// Poids moyen d'une image d'analyse, en octets.
    pub frame_avg_bytes: u64,
}

impl FluxParams {
    /// Déduit les paramètres d'écriture des réglages de captation.
    pub fn depuis_config(
        config: &crate::recorder::RecordingConfig,
        plan: &crate::recorder::SegmentPlan,
        proxy_bitrate_mbps: Option<u32>,
        frame_interval_secs: Option<f32>,
    ) -> Self {
        Self {
            bitrate_mbps: config.bitrate_mbps,
            avec_audio: config.audio_device.is_some(),
            clip_duration_secs: config.clip_duration_secs,
            step_secs: plan.step_secs,
            proxy_bitrate_mbps: proxy_bitrate_mbps.unwrap_or(PROXY_DEBIT_MBPS_DEFAUT),
            frame_interval_secs: frame_interval_secs.unwrap_or(INTERVALLE_IMAGES_DEFAUT),
            frame_avg_bytes: POIDS_IMAGE_SUPPOSE,
        }
    }

    /// Facteur de chevauchement.
    ///
    /// Un clip de 150 s qui redémarre toutes les 120 s réécrit 30 s déjà
    /// écrites : on produit 1,25 seconde de clip par seconde captée.
    pub fn facteur_chevauchement(&self) -> f64 {
        if self.step_secs == 0 {
            return 1.0;
        }

        self.clip_duration_secs as f64 / self.step_secs as f64
    }

    /// Octets écrits par seconde de captation, flux par flux.
    fn debits(&self) -> Debits {
        let video = (self.bitrate_mbps as u64 * 1_000_000) / 8;

        let audio_source = if self.avec_audio {
            DEBIT_AUDIO_SOURCE / 8
        } else {
            0
        };

        let audio_proxy = if self.avec_audio {
            DEBIT_AUDIO_PROXY / 8
        } else {
            0
        };

        let source = video + audio_source;
        let proxy = (self.proxy_bitrate_mbps as u64 * 1_000_000) / 8 + audio_proxy;

        let facteur = self.facteur_chevauchement();

        // Une image toutes les N secondes. Un intervalle nul ou négatif
        // n'aurait pas de sens : on le neutralise plutôt que de diviser par
        // zéro.
        let images = if self.frame_interval_secs > 0.0 {
            (self.frame_avg_bytes as f64 / self.frame_interval_secs as f64) as u64
        } else {
            0
        };

        Debits {
            // Le morceau est écrit une seule fois : c'est la source.
            morceaux: source,
            // Le clip, lui, subit le chevauchement.
            clips: (source as f64 * facteur) as u64,
            // La version légère en dérive, donc même facteur.
            proxies: (proxy as f64 * facteur) as u64,
            images,
        }
    }
}

/// Octets par seconde de captation, ventilés par flux.
#[derive(Debug, Clone, Copy)]
struct Debits {
    morceaux: u64,
    clips: u64,
    proxies: u64,
    images: u64,
}

impl Debits {
    fn total(&self) -> u64 {
        self.morceaux + self.clips + self.proxies + self.images
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ESTIMATION AVANT CAPTATION
// ═══════════════════════════════════════════════════════════════════════

/// Ce que l'interface montre au photographe avant qu'il ne lance sa course.
#[derive(Debug, Clone, Serialize)]
pub struct DiskEstimate {
    /// Écriture par heure, flux par flux, en octets.
    pub segments_per_hour: u64,
    pub clips_per_hour: u64,
    pub proxies_per_hour: u64,
    pub frames_per_hour: u64,
    pub total_per_hour: u64,

    /// Facteur de chevauchement appliqué aux clips et aux versions légères.
    pub overlap_factor: f64,

    /// Espace réellement disponible sur le volume visé, en octets.
    pub free_bytes: u64,

    /// Part mise de côté et jamais consommée, en octets.
    pub reserve_bytes: u64,

    /// Ce qui reste après réserve — le seul chiffre qui compte vraiment.
    pub usable_bytes: u64,

    /// Durée de captation possible, en secondes.
    pub autonomy_secs: u64,

    /// Le dossier de travail est-il sur le disque système ?
    pub on_system_disk: bool,

    /// Chemin effectivement interrogé.
    ///
    /// Peut différer du dossier demandé si celui-ci n'existe pas encore : on
    /// remonte alors au premier parent existant, qui est sur le même volume.
    pub measured_path: String,
}

/// Estime l'occupation disque d'une captation, sans rien enregistrer.
pub fn estimer(dossier: &Path, flux: &FluxParams) -> Result<DiskEstimate, String> {
    let chemin = premier_parent_existant(dossier)?;

    let libre = fs4::available_space(&chemin)
        .map_err(|e| format!("Impossible de lire l'espace disque : {}", e))?;

    let systeme = sur_disque_systeme(&chemin);
    let reserve = reserve_pour(&chemin, flux, systeme);

    let utilisable = libre.saturating_sub(reserve);

    let debits = flux.debits();
    let total_par_sec = debits.total();

    let autonomie = if total_par_sec > 0 {
        utilisable / total_par_sec
    } else {
        0
    };

    Ok(DiskEstimate {
        segments_per_hour: debits.morceaux * 3600,
        clips_per_hour: debits.clips * 3600,
        proxies_per_hour: debits.proxies * 3600,
        frames_per_hour: debits.images * 3600,
        total_per_hour: total_par_sec * 3600,
        overlap_factor: flux.facteur_chevauchement(),
        free_bytes: libre,
        reserve_bytes: reserve,
        usable_bytes: utilisable,
        autonomy_secs: autonomie,
        on_system_disk: systeme,
        measured_path: chemin.to_string_lossy().to_string(),
    })
}

/// Réserve à ne jamais entamer sur ce volume.
///
/// Sur un disque de données, le plancher de 10 Go suffit dans l'immense
/// majorité des cas. Il ne cède la place au calcul par clips que sur des
/// réglages inhabituels — clips très longs ou débit très élevé — où trois
/// clips pèsent davantage. C'est précisément là que la troncature guette :
/// il faut de quoi contenir le morceau en cours d'écriture, le clip en cours
/// d'assemblage et sa version légère, tous trois vivants au même instant.
fn reserve_pour(chemin: &Path, flux: &FluxParams, systeme: bool) -> u64 {
    let _ = chemin;

    if systeme {
        return RESERVE_DISQUE_SYSTEME;
    }

    let debits = flux.debits();
    let taille_clip = debits.morceaux * flux.clip_duration_secs as u64;

    RESERVE_DISQUE_DONNEES.max(taille_clip * 3)
}

// ═══════════════════════════════════════════════════════════════════════
// SURVEILLANCE PENDANT LA CAPTATION
// ═══════════════════════════════════════════════════════════════════════

/// Gravité de la situation disque.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum DiskLevel {
    /// Rien à signaler.
    Ok,
    /// Moins de trente minutes d'autonomie.
    Warning,
    /// Moins de quinze minutes.
    Critical,
    /// Sous le seuil d'arrêt : la captation a été close.
    Stopped,
}

/// État instantané, émis vers l'interface à chaque passage.
#[derive(Debug, Clone, Serialize)]
pub struct DiskStatus {
    pub level: DiskLevel,

    /// Espace disponible sur le volume, en octets.
    pub free_bytes: u64,

    /// Espace disponible après réserve, en octets.
    pub usable_bytes: u64,

    /// Écrit par la captation depuis son démarrage, en octets.
    pub written_bytes: u64,

    /// Débit retenu pour le calcul, en octets par seconde.
    pub rate_bytes_per_sec: u64,

    /// Ce débit vient-il d'une mesure, ou encore de la théorie ?
    ///
    /// Distinction utile à l'affichage : une prévision mesurée mérite plus
    /// de confiance qu'une prévision calculée, et le photographe doit savoir
    /// laquelle il regarde.
    pub rate_measured: bool,

    /// Temps de captation encore possible, en secondes.
    pub autonomy_secs: u64,

    /// Temps écoulé depuis le démarrage, en secondes.
    pub elapsed_secs: u64,
}

/// Lance la surveillance de l'espace disque pour une captation.
///
/// Tourne dans sa propre tâche, et s'éteint d'elle-même quand la captation
/// s'arrête — le drapeau `running` est celui du `RecordingHandle`, donc la
/// surveillance ne peut pas survivre à ce qu'elle surveille.
///
/// Volontairement en Rust et non pilotée par l'interface : une fenêtre qui
/// rame ou un onglet en arrière-plan ne doit pas suspendre le seul garde-fou
/// qui protège l'enregistrement.
pub fn surveiller(
    app: AppHandle,
    dossier: PathBuf,
    flux: FluxParams,
    running: Arc<AtomicBool>,
) {
    tauri::async_runtime::spawn(async move {
        // Le dossier peut déjà contenir des fichiers d'une session
        // précédente. On mesure ce qui s'y ajoute, pas ce qu'il pèse.
        let reference = taille_dossier(&dossier);
        let depart = std::time::Instant::now();

        let debits_theoriques = flux.debits();
        let systeme = sur_disque_systeme(&dossier);
        let reserve = reserve_pour(&dossier, &flux, systeme);

        loop {
            tokio::time::sleep(std::time::Duration::from_secs(PERIODE_SURVEILLANCE_SECS)).await;

            if !running.load(Ordering::Relaxed) {
                break;
            }

            let ecoule = depart.elapsed().as_secs();

            let libre = match fs4::available_space(&dossier) {
                Ok(v) => v,
                Err(e) => {
                    // Un volume momentanément indisponible — disque externe
                    // qui se réveille, partage réseau qui hoquette — ne doit
                    // pas déclencher un arrêt. On note et on repassera.
                    log::warn!("Espace disque illisible : {}", e);
                    continue;
                }
            };

            let ecrit = taille_dossier(&dossier).saturating_sub(reference);

            // Le débit mesuré ne remplace le théorique qu'une fois
            // l'échantillon assez large. Trop tôt, il serait faussé : le
            // premier clip n'est pas encore assemblé, ni sa version légère.
            let mesure_fiable = ecoule >= DELAI_AVANT_MESURE_SECS && ecrit > 0;

            let debit = if mesure_fiable {
                ecrit / ecoule.max(1)
            } else {
                debits_theoriques.total()
            };

            let utilisable = libre.saturating_sub(reserve);

            let autonomie = if debit > 0 { utilisable / debit } else { u64::MAX };

            let niveau = if autonomie <= SEUIL_ARRET_SECS {
                DiskLevel::Stopped
            } else if autonomie <= SEUIL_ALERTE_FORTE_SECS {
                DiskLevel::Critical
            } else if autonomie <= SEUIL_ALERTE_SECS {
                DiskLevel::Warning
            } else {
                DiskLevel::Ok
            };

            let etat = DiskStatus {
                level: niveau,
                free_bytes: libre,
                usable_bytes: utilisable,
                written_bytes: ecrit,
                rate_bytes_per_sec: debit,
                rate_measured: mesure_fiable,
                autonomy_secs: if autonomie == u64::MAX { 0 } else { autonomie },
                elapsed_secs: ecoule,
            };

            let _ = app.emit("disk-status", &etat);

            if niveau == DiskLevel::Stopped {
                log::error!(
                    "Espace disque insuffisant : {} Mo utilisables, {} Mo/min écrits. \
                     Arrêt de la captation.",
                    utilisable / 1_048_576,
                    (debit * 60) / 1_048_576
                );

                // Arrêt propre, pas brutal : FFmpeg clôt le morceau en cours,
                // le manifeste reçoit son heure de fin. Ce qui a été capté
                // reste exploitable — c'est tout l'intérêt de s'arrêter avant
                // la saturation plutôt qu'après.
                if let Err(e) = crate::commands::arreter_captation(&app).await {
                    log::error!("Arrêt automatique impossible : {}", e);
                }

                break;
            }
        }

        log::info!("Surveillance disque terminée.");
    });
}

// ═══════════════════════════════════════════════════════════════════════
// OUTILS SYSTÈME
// ═══════════════════════════════════════════════════════════════════════

/// Poids total d'un dossier, sous-dossiers compris.
///
/// Parcours complet plutôt que suivi de l'espace libre : sur une machine de
/// terrain, d'autres programmes écrivent aussi. Ce qu'on veut connaître ici,
/// c'est le débit de l'agent, pas celui du poste.
///
/// Les erreurs de lecture sont ignorées — un fichier verrouillé par FFmpeg
/// pendant qu'on le pèse est normal, et pas une raison d'abandonner la
/// mesure.
fn taille_dossier(dossier: &Path) -> u64 {
    let mut total = 0u64;

    let Ok(entrees) = std::fs::read_dir(dossier) else {
        return 0;
    };

    for entree in entrees.flatten() {
        let Ok(infos) = entree.metadata() else {
            continue;
        };

        if infos.is_dir() {
            total += taille_dossier(&entree.path());
        } else {
            total += infos.len();
        }
    }

    total
}

/// Premier ancêtre existant d'un chemin.
///
/// L'espace libre se lit sur un chemin qui existe. Or le photographe choisit
/// son dossier de travail avant qu'il ne soit créé. On remonte donc jusqu'au
/// premier parent réel : il est sur le même volume, donc la mesure est
/// juste.
fn premier_parent_existant(dossier: &Path) -> Result<PathBuf, String> {
    let mut courant = dossier.to_path_buf();

    loop {
        if courant.exists() {
            return Ok(courant);
        }

        match courant.parent() {
            Some(parent) if parent != courant => courant = parent.to_path_buf(),
            _ => {
                return Err(format!(
                    "Aucun dossier accessible sur ce chemin : {}",
                    dossier.display()
                ))
            }
        }
    }
}

/// Le chemin est-il sur le volume où le système est installé ?
///
/// Sur Windows, on compare la lettre de lecteur à celle du système. Ailleurs,
/// on compare l'identifiant de périphérique à celui de la racine : deux
/// chemins sur le même volume le partagent, ce qui est exact quel que soit
/// le point de montage.
#[cfg(windows)]
fn sur_disque_systeme(chemin: &Path) -> bool {
    let Ok(systeme) = std::env::var("SystemDrive") else {
        // Sans certitude, on suppose le disque système : cela mène à la
        // réserve la plus prudente, jamais à la moins prudente.
        return true;
    };

    let Ok(absolu) = std::fs::canonicalize(chemin) else {
        return true;
    };

    let texte = absolu.to_string_lossy().to_uppercase();
    let lettre = systeme.to_uppercase();

    // `canonicalize` produit un chemin étendu, de la forme \\?\C:\...
    texte.trim_start_matches(r"\\?\").starts_with(&lettre)
}

#[cfg(not(windows))]
fn sur_disque_systeme(chemin: &Path) -> bool {
    use std::os::unix::fs::MetadataExt;

    let Ok(cible) = std::fs::metadata(chemin) else {
        return true;
    };

    let Ok(racine) = std::fs::metadata("/") else {
        return true;
    };

    cible.dev() == racine.dev()
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn flux_reference() -> FluxParams {
        // Les réglages réellement utilisés aujourd'hui : 1080p à 8 Mb/s,
        // clips de 150 s avec 30 s de chevauchement, une image toutes les
        // 2 s, sans micro.
        FluxParams {
            bitrate_mbps: 8,
            avec_audio: false,
            clip_duration_secs: 150,
            step_secs: 120,
            proxy_bitrate_mbps: 2,
            frame_interval_secs: 2.0,
            frame_avg_bytes: POIDS_IMAGE_SUPPOSE,
        }
    }

    #[test]
    fn calcule_le_facteur_de_chevauchement() {
        assert_eq!(flux_reference().facteur_chevauchement(), 1.25);
    }

    #[test]
    fn un_clip_sans_chevauchement_ne_coute_rien_de_plus() {
        let mut flux = flux_reference();
        flux.step_secs = flux.clip_duration_secs;

        assert_eq!(flux.facteur_chevauchement(), 1.0);
    }

    #[test]
    fn compte_les_morceaux_au_debit_source() {
        // 8 Mb/s = 1 000 000 octets par seconde.
        assert_eq!(flux_reference().debits().morceaux, 1_000_000);
    }

    #[test]
    fn applique_le_chevauchement_aux_clips_et_aux_proxies() {
        let debits = flux_reference().debits();

        assert_eq!(debits.clips, 1_250_000);
        assert_eq!(debits.proxies, 312_500);
    }

    #[test]
    fn compte_les_images_a_leur_cadence() {
        // Une image toutes les 2 s, 300 000 octets pièce.
        assert_eq!(flux_reference().debits().images, 150_000);
    }

    #[test]
    fn le_total_horaire_correspond_a_la_prevision() {
        let total = flux_reference().debits().total();

        // Environ 9,7 gigaoctets par heure.
        let par_heure = total * 3600;

        assert!(par_heure > 9_000_000_000, "trop bas : {}", par_heure);
        assert!(par_heure < 10_500_000_000, "trop haut : {}", par_heure);
    }

    #[test]
    fn le_son_alourdit_la_captation() {
        let muet = flux_reference().debits().total();

        let mut avec_son = flux_reference();
        avec_son.avec_audio = true;

        assert!(avec_son.debits().total() > muet);
    }

    #[test]
    fn un_intervalle_nul_ne_fait_pas_diviser_par_zero() {
        let mut flux = flux_reference();
        flux.frame_interval_secs = 0.0;

        assert_eq!(flux.debits().images, 0);
    }

    #[test]
    fn un_pas_nul_ne_fait_pas_diviser_par_zero() {
        let mut flux = flux_reference();
        flux.step_secs = 0;

        assert_eq!(flux.facteur_chevauchement(), 1.0);
    }

    #[test]
    fn reserve_cinquante_giga_sur_le_disque_systeme() {
        let reserve = reserve_pour(Path::new("/tmp"), &flux_reference(), true);

        assert_eq!(reserve, RESERVE_DISQUE_SYSTEME);
    }

    #[test]
    fn reserve_au_moins_dix_giga_sur_un_disque_de_donnees() {
        let reserve = reserve_pour(Path::new("/tmp"), &flux_reference(), false);

        assert_eq!(reserve, RESERVE_DISQUE_DONNEES);
    }

    #[test]
    fn la_reserve_suit_la_taille_des_clips_quand_ils_sont_enormes() {
        // Clips d'une heure à 50 Mb/s : trois clips dépassent largement les
        // dix gigaoctets du plancher.
        let mut flux = flux_reference();
        flux.bitrate_mbps = 50;
        flux.clip_duration_secs = 3600;
        flux.step_secs = 3000;

        let reserve = reserve_pour(Path::new("/tmp"), &flux, false);

        assert!(
            reserve > RESERVE_DISQUE_DONNEES,
            "la réserve devrait suivre la taille des clips : {}",
            reserve
        );
    }

    #[test]
    fn remonte_au_premier_parent_existant() {
        let inexistant = std::env::temp_dir().join("attimo_absent_xyz/encore/plus/loin");

        let trouve = premier_parent_existant(&inexistant).unwrap();

        assert!(trouve.exists());
    }

    #[test]
    fn un_chemin_existant_est_rendu_tel_quel() {
        let temp = std::env::temp_dir();

        assert_eq!(premier_parent_existant(&temp).unwrap(), temp);
    }

    #[test]
    fn pese_un_dossier_et_ses_sous_dossiers() {
        let base = std::env::temp_dir().join("attimo_test_taille");
        let sous = base.join("_morceaux");

        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&sous).unwrap();

        std::fs::write(base.join("a.bin"), vec![0u8; 1000]).unwrap();
        std::fs::write(sous.join("b.bin"), vec![0u8; 2000]).unwrap();

        assert_eq!(taille_dossier(&base), 3000);

        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn un_dossier_absent_ne_pese_rien() {
        let absent = std::env::temp_dir().join("attimo_dossier_qui_nexiste_pas");

        assert_eq!(taille_dossier(&absent), 0);
    }

    #[test]
    fn estime_sur_un_dossier_reel() {
        let estimation = estimer(&std::env::temp_dir(), &flux_reference()).unwrap();

        assert!(estimation.total_per_hour > 0);
        assert_eq!(estimation.overlap_factor, 1.25);
        assert_eq!(
            estimation.usable_bytes,
            estimation.free_bytes.saturating_sub(estimation.reserve_bytes)
        );
    }
}
