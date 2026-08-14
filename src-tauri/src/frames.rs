// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Images d'analyse (frames.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// SAAS 430 — Découpage automatique en clips avec chevauchement.
//
// ─── LE FLUX INVISIBLE ─────────────────────────────────────────────────
//
// Ce module produit les images qui servent à lire les dossards et
// reconnaître les visages. Le coureur ne les verra jamais : elles ne sont
// ni montrées, ni vendues, et le serveur les supprime après analyse.
//
// C'est pourtant le flux le plus important du dispositif. Sans lui, les
// clips existent mais personne ne les trouve.
//
// ─── EXTRAIRE DEPUIS LES MORCEAUX, JAMAIS DEPUIS LES CLIPS ─────────────
//
// Point contre-intuitif, et coûteux si on se trompe.
//
// Les clips se chevauchent : avec les réglages de référence, le morceau 4
// appartient au clip 1 ET au clip 2. Extraire depuis les clips traiterait
// donc deux fois les mêmes instants — mêmes coureurs, mêmes dossards,
// facturés deux fois par le service de reconnaissance.
//
// Sur un clip de 150 s avec 30 s de chevauchement, cela représente 20 % de
// dépense inutile, pour un résultat identique.
//
// Les morceaux, eux, ne se recouvrent jamais. Chaque instant du flux y est
// présent exactement une fois.
//
// ─── POURQUOI 1920 PIXELS, ET PAS MOINS ────────────────────────────────
//
// Mesuré sur photos réelles, dossards de 5 à 40 mètres :
//
//   540p  → 2 lectures correctes, 1 ERRONÉE
//   720p  → 3 lectures correctes, 1 ERRONÉE
//   1920  → 6 lectures correctes, 0 erronée
//
// Les lectures erronées n'apparaissent QU'EN RÉSOLUTION RÉDUITE. En dessous
// d'un certain seuil, le lecteur de texte reçoit assez de signal pour croire
// voir des chiffres, pas assez pour les distinguer — alors il devine.
//
// Un numéro inventé qui franchit le seuil de confiance envoie les photos au
// mauvais coureur. C'est bien plus grave qu'une absence de lecture, laquelle
// est rattrapée par la reconnaissance faciale.
//
// La compression, en revanche, ne change pas la taille des chiffres en
// pixels : la qualité 65 a été validée sans aucune perte, y compris sur les
// dossards les plus difficiles. On garde donc la définition maximale et on
// économise sur la compression.

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::AppHandle;
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;

// ═══════════════════════════════════════════════════════════════════════
// RÉGLAGES
// ═══════════════════════════════════════════════════════════════════════

/// Largeur des images d'analyse.
///
/// NE PAS DESCENDRE sous cette valeur sans refaire les mesures : toute
/// réduction réintroduit des lectures erronées.
pub const LARGEUR_ANALYSE: u32 = 1920;

/// Qualité JPEG, sur l'échelle inversée de FFmpeg où 2 est le meilleur et
/// 31 le pire. La valeur 6 correspond à environ 65 % sur l'échelle usuelle.
const QUALITE_JPEG: u32 = 6;

/// Ce que le photographe règle pour l'analyse.
#[derive(Debug, Clone)]
pub struct FrameConfig {
    /// Une image toutes les N secondes.
    ///
    /// Curseur direct sur la facture de reconnaissance. Un coureur traverse
    /// le cadre en trois à cinq secondes : à deux secondes d'intervalle il
    /// est capturé deux ou trois fois, ce qui laisse plusieurs chances de
    /// lecture.
    pub interval_secs: f32,
}

impl Default for FrameConfig {
    fn default() -> Self {
        Self { interval_secs: 2.0 }
    }
}

/// Une image extraite, prête à être envoyée.
#[derive(Debug, Clone, Serialize)]
pub struct ExtractedFrame {
    pub filename: String,
    pub path: String,
    pub size_bytes: u64,

    /// Horodatage absolu, au format ISO 8601 avec fuseau.
    ///
    /// C'est le pivot de tout le rattachement : le serveur s'en sert pour
    /// retrouver quels clips couvrent l'instant où un dossard a été lu.
    pub instant_at: String,

    /// Morceau dont elle provient — utile au diagnostic.
    pub segment_index: u32,
}

/// Bilan d'une extraction.
#[derive(Debug, Clone, Serialize)]
pub struct ExtractionResult {
    pub segment_index: u32,
    pub frames: Vec<ExtractedFrame>,
    pub total_bytes: u64,
}

// ═══════════════════════════════════════════════════════════════════════
// EXTRACTION
// ═══════════════════════════════════════════════════════════════════════

/// Extrait les images d'analyse d'un morceau.
///
/// Appelée dès qu'un morceau est clos, en parallèle de l'assemblage des
/// clips. Les deux opérations sont indépendantes et ne se gênent pas.
///
/// `segment_started_at` est l'instant absolu du début du morceau. Chaque
/// image en dérive : la première à cet instant, la suivante `interval_secs`
/// plus tard, et ainsi de suite. Sans cet ancrage, les images seraient
/// horodatées les unes par rapport aux autres, et l'on perdrait le lien avec
/// l'heure réelle du passage des coureurs.
pub async fn extraire_images(
    app: &AppHandle,
    dossier_travail: &Path,
    segment_index: u32,
    segment_started_at: chrono_simple::Instant,
    config: &FrameConfig,
) -> Result<ExtractionResult, String> {
    let morceau = dossier_travail
        .join("_morceaux")
        .join(format!("m_{:06}.mp4", segment_index));

    if !morceau.exists() {
        return Err(format!(
            "Le morceau {} est introuvable — extraction impossible.",
            segment_index
        ));
    }

    let dossier_images = dossier_travail.join("_analyse");

    std::fs::create_dir_all(&dossier_images)
        .map_err(|e| format!("Impossible de créer le dossier d'analyse : {}", e))?;

    // Motif de nommage. Le numéro de morceau y figure pour éviter toute
    // collision entre extractions successives.
    let motif = dossier_images
        .join(format!("img_{:06}_%03d.jpg", segment_index))
        .to_string_lossy()
        .to_string();

    // Un seul filtre enchaîne les deux opérations :
    //
    //   fps    cadence d'extraction, en images par seconde. Une image toutes
    //          les deux secondes s'écrit 0,5.
    //
    //   scale  ramène à 1920 px de large SI la source est plus grande. Le
    //          « min » évite d'agrandir une source plus petite : cela
    //          n'ajouterait aucune information et gonflerait les fichiers.
    //          Le « -2 » laisse calculer la hauteur en conservant les
    //          proportions, arrondie à un nombre pair.
    let filtre = format!(
        "fps={},scale='min({},iw)':-2",
        1.0 / config.interval_secs,
        LARGEUR_ANALYSE
    );

    let arguments: Vec<String> = vec![
        "-hide_banner".into(),
        "-i".into(),
        morceau.to_string_lossy().to_string(),
        "-vf".into(),
        filtre,
        "-q:v".into(),
        QUALITE_JPEG.to_string(),
        "-y".into(),
        motif,
    ];

    let commande = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| format!("FFmpeg embarqué introuvable : {}", e))?
        .args(arguments);

    let (mut evenements, _enfant) = commande
        .spawn()
        .map_err(|e| format!("Impossible de lancer l'extraction : {}", e))?;

    let mut journal = String::new();

    while let Some(evenement) = evenements.recv().await {
        match evenement {
            CommandEvent::Stderr(donnees) | CommandEvent::Stdout(donnees) => {
                journal.push_str(&String::from_utf8_lossy(&donnees));
            }
            CommandEvent::Terminated(_) => break,
            _ => {}
        }
    }

    // ─── Recensement des images produites ───
    //
    // On lit le dossier plutôt que de se fier au code de retour de FFmpeg :
    // ce qui compte, ce sont les fichiers réellement écrits.

    let mut images = Vec::new();
    let mut total = 0u64;

    let prefixe = format!("img_{:06}_", segment_index);

    let entrees = std::fs::read_dir(&dossier_images)
        .map_err(|e| format!("Impossible de lire le dossier d'analyse : {}", e))?;

    let mut chemins: Vec<PathBuf> = entrees
        .filter_map(|e| e.ok())
        .map(|e| e.path())
        .filter(|p| {
            p.file_name()
                .and_then(|n| n.to_str())
                .map(|n| n.starts_with(&prefixe))
                .unwrap_or(false)
        })
        .collect();

    // L'ordre alphabétique correspond à l'ordre chronologique grâce au
    // numéro sur trois chiffres.
    chemins.sort();

    for (rang, chemin) in chemins.iter().enumerate() {
        let Ok(metadonnees) = std::fs::metadata(chemin) else {
            continue;
        };

        let taille = metadonnees.len();
        total += taille;

        // Instant absolu de cette image : début du morceau, plus le rang
        // multiplié par l'intervalle.
        let decalage_ms = (rang as f32 * config.interval_secs * 1000.0) as i64;
        let instant = segment_started_at.plus_millis(decalage_ms);

        images.push(ExtractedFrame {
            filename: chemin
                .file_name()
                .map(|n| n.to_string_lossy().to_string())
                .unwrap_or_default(),
            path: chemin.to_string_lossy().to_string(),
            size_bytes: taille,
            instant_at: instant.to_iso8601(),
            segment_index,
        });
    }

    if images.is_empty() {
        return Err(format!(
            "Aucune image extraite du morceau {}.\n\nJournal FFmpeg :\n{}",
            segment_index,
            derniers_caracteres(&journal, 400)
        ));
    }

    log::info!(
        "Morceau {} : {} image(s) extraite(s), {} octets",
        segment_index,
        images.len(),
        total
    );

    Ok(ExtractionResult {
        segment_index,
        frames: images,
        total_bytes: total,
    })
}

/// Renvoie la fin d'une chaîne, pour ne pas noyer un message d'erreur.
fn derniers_caracteres(texte: &str, combien: usize) -> String {
    let caracteres: Vec<char> = texte.chars().collect();

    if caracteres.len() <= combien {
        return texte.to_string();
    }

    caracteres[caracteres.len() - combien..].iter().collect()
}

// ═══════════════════════════════════════════════════════════════════════
// HORODATAGE
// ═══════════════════════════════════════════════════════════════════════

/// Petit utilitaire de date, pour éviter une dépendance supplémentaire.
///
/// Le fuseau n'est pas décoratif : c'est le seul lien entre les images, les
/// clips et les participants. Une heure sans fuseau décalerait tout le
/// rattachement d'une heure en été.
pub mod chrono_simple {
    use std::time::{SystemTime, UNIX_EPOCH};

    #[derive(Debug, Clone, Copy)]
    pub struct Instant {
        millis: i64,
    }

    impl Instant {
        pub fn maintenant() -> Self {
            let millis = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|d| d.as_millis() as i64)
                .unwrap_or(0);

            Self { millis }
        }

        pub fn depuis_millis(millis: i64) -> Self {
            Self { millis }
        }

        pub fn plus_millis(&self, ajout: i64) -> Self {
            Self {
                millis: self.millis + ajout,
            }
        }

        pub fn millis(&self) -> i64 {
            self.millis
        }

        /// Format ISO 8601 en temps universel.
        ///
        /// Le suffixe Z indique explicitement le fuseau, ce qui lève toute
        /// ambiguïté côté serveur, quel que soit son propre réglage.
        pub fn to_iso8601(&self) -> String {
            let secondes = self.millis / 1000;
            let millisecondes = self.millis % 1000;

            let (annee, mois, jour, heure, minute, seconde) = decomposer(secondes);

            format!(
                "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}.{:03}Z",
                annee, mois, jour, heure, minute, seconde, millisecondes
            )
        }
    }

    /// Convertit un nombre de secondes depuis 1970 en date civile.
    fn decomposer(mut secondes: i64) -> (i64, i64, i64, i64, i64, i64) {
        let seconde = secondes % 60;
        secondes /= 60;

        let minute = secondes % 60;
        secondes /= 60;

        let heure = secondes % 24;
        let mut jours = secondes / 24;

        let mut annee = 1970;

        loop {
            let dans_annee = if bissextile(annee) { 366 } else { 365 };

            if jours < dans_annee {
                break;
            }

            jours -= dans_annee;
            annee += 1;
        }

        let longueurs = [
            31,
            if bissextile(annee) { 29 } else { 28 },
            31, 30, 31, 30, 31, 31, 30, 31, 30, 31,
        ];

        let mut mois = 1;

        for longueur in longueurs {
            if jours < longueur {
                break;
            }

            jours -= longueur;
            mois += 1;
        }

        (annee, mois, jours + 1, heure, minute, seconde)
    }

    fn bissextile(annee: i64) -> bool {
        (annee % 4 == 0 && annee % 100 != 0) || annee % 400 == 0
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::chrono_simple::Instant;
    use super::*;

    #[test]
    fn formate_une_date_connue() {
        // 1er janvier 2026 à midi pile, temps universel.
        let instant = Instant::depuis_millis(1_767_268_800_000);

        assert_eq!(instant.to_iso8601(), "2026-01-01T12:00:00.000Z");
    }

    #[test]
    fn ajoute_correctement_les_millisecondes() {
        let depart = Instant::depuis_millis(1_767_268_800_000);
        let deux_secondes_plus_tard = depart.plus_millis(2000);

        assert_eq!(
            deux_secondes_plus_tard.to_iso8601(),
            "2026-01-01T12:00:02.000Z"
        );
    }

    #[test]
    fn gere_le_passage_a_lheure_suivante() {
        // 12h59min59s
        let instant = Instant::depuis_millis(1_767_272_399_000);
        let apres = instant.plus_millis(1000);

        assert_eq!(apres.to_iso8601(), "2026-01-01T13:00:00.000Z");
    }

    #[test]
    fn reconnait_les_annees_bissextiles() {
        // 29 février 2024 — année bissextile.
        let instant = Instant::depuis_millis(1_709_208_000_000);
        let texte = instant.to_iso8601();

        assert!(texte.starts_with("2024-02-29"), "obtenu : {}", texte);
    }

    #[test]
    fn lintervalle_par_defaut_est_de_deux_secondes() {
        let config = FrameConfig::default();

        assert_eq!(config.interval_secs, 2.0);
    }

    #[test]
    fn tronque_les_journaux_trop_longs() {
        let long = "x".repeat(1000);
        let court = derniers_caracteres(&long, 100);

        assert_eq!(court.len(), 100);
    }

    #[test]
    fn conserve_les_journaux_courts_entiers() {
        let texte = "erreur brève";

        assert_eq!(derniers_caracteres(texte, 400), texte);
    }
}
