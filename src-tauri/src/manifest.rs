// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Manifeste de session (manifest.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// SAAS 430 — Contrat d'interface v1.3.
//
// ─── LE MANIFESTE EST L'INTERFACE ──────────────────────────────────────
//
// Ce fichier décrit tout ce que la captation a produit : les clips, leurs
// horodatages absolus, leur composition. C'est lui qui fait foi — jamais les
// noms de fichiers.
//
// Cette règle a l'air anodine, elle est structurante. Les noms sont faits
// pour un humain qui parcourt un dossier ; les interpréter par programme
// revient à figer une convention d'affichage en règle métier. Le jour où l'on
// change le format des noms, tout casse silencieusement.
//
// ─── ÉCRITURE ATOMIQUE ─────────────────────────────────────────────────
//
// Le manifeste est réécrit après CHAQUE clip, pour rester exploitable
// pendant la session et pas seulement à la fin — un envoi peut démarrer
// alors que la course continue.
//
// Il est donc écrit dans un fichier temporaire, puis renommé. Sur les
// systèmes de fichiers usuels, le renommage est atomique : un lecteur voit
// soit l'ancienne version complète, soit la nouvelle, jamais un JSON coupé
// en deux. Écrire directement dans le fichier final exposerait à lire un
// document tronqué au mauvais moment.
//
// ─── L'INDEX DES IMAGES EST À PART ─────────────────────────────────────
//
// Les images d'analyse ne figurent PAS dans le manifeste. À une image toutes
// les deux secondes sur une épreuve de quatre heures, cela ferait 7 200
// entrées dans un fichier réécrit après chaque clip : plusieurs mégaoctets
// réécrits en boucle, pour rien.
//
// Elles vont dans un fichier séparé, en ajout seul — jamais réécrit, donc
// sans limite de taille ni risque de corruption sur une session longue.

use serde::{Deserialize, Serialize};
use std::fs::OpenOptions;
use std::io::Write;
use std::path::{Path, PathBuf};

use crate::assembler::AssembledClip;
use crate::frames::chrono_simple::Instant;
use crate::frames::ExtractedFrame;
use crate::recorder::{RecordingConfig, SegmentPlan};

// ═══════════════════════════════════════════════════════════════════════
// STRUCTURE DU MANIFESTE
// ═══════════════════════════════════════════════════════════════════════

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Manifest {
    /// Version du format. Permettra au serveur de s'adapter si la structure
    /// évolue, sans casser les sessions déjà enregistrées.
    pub version: String,

    pub session: SessionInfo,
    pub capture: CaptureInfo,
    pub analyse: AnalyseInfo,
    pub clips: Vec<ClipEntry>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SessionInfo {
    /// Identifiant de la session de capture.
    ///
    /// Forme la clé d'appariement avec le numéro de clip : c'est ce couple
    /// qui permet au serveur de réunir la version légère et la version haute
    /// qualité d'un même clip, montées séparément.
    pub identifiant: String,

    pub debut: String,
    pub fin: Option<String>,

    /// Rattachement à l'épreuve. Recopié depuis la configuration pour qu'un
    /// manifeste retrouvé seul reste exploitable.
    pub attimo: AttimoInfo,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AttimoInfo {
    #[serde(rename = "tenantId")]
    pub tenant_id: i64,
    #[serde(rename = "eventId")]
    pub event_id: i64,
    #[serde(rename = "checkpointId")]
    pub checkpoint_id: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CaptureInfo {
    pub peripherique: String,
    pub resolution: String,
    pub fps: u32,
    #[serde(rename = "debitMbps")]
    pub debit_mbps: u32,
    #[serde(rename = "dureeClipSecondes")]
    pub duree_clip_secondes: u32,
    #[serde(rename = "chevauchementSecondes")]
    pub chevauchement_secondes: u32,
    #[serde(rename = "dureeMorceauSecondes")]
    pub duree_morceau_secondes: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AnalyseInfo {
    pub actif: bool,
    pub dossier: String,
    pub index: String,
    #[serde(rename = "intervalleSecondes")]
    pub intervalle_secondes: f32,
    #[serde(rename = "largeurPixels")]
    pub largeur_pixels: u32,
    #[serde(rename = "imagesProduites")]
    pub images_produites: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ClipEntry {
    pub numero: u32,
    pub fichier: String,

    /// Horodatages ABSOLUS, avec fuseau.
    ///
    /// C'est le seul lien entre les images d'analyse, les clips et les
    /// participants. Le serveur cherche les clips vérifiant
    /// `debut <= instant < fin` pour savoir lesquels couvrent le passage
    /// d'un coureur. Une date sans fuseau décalerait tout d'une heure en été.
    pub debut: String,
    pub fin: String,

    pub octets: u64,

    /// Vrai si tous les morceaux attendus étaient présents.
    ///
    /// Un clip incomplet reste exploitable mais peut ne pas contenir le
    /// passage annoncé : le serveur doit pouvoir le savoir.
    pub complet: bool,

    /// Morceaux qui le composent. Deux clips consécutifs en partagent
    /// toujours un — c'est le chevauchement.
    pub morceaux: Vec<u32>,
}

// ═══════════════════════════════════════════════════════════════════════
// RÉDACTION
// ═══════════════════════════════════════════════════════════════════════

/// Tient le manifeste à jour au fil de la captation.
pub struct ManifestWriter {
    chemin: PathBuf,
    chemin_index: PathBuf,
    manifeste: Manifest,
    debut_session: Instant,
    duree_morceau: u32,
}

impl ManifestWriter {
    pub fn new(
        dossier_travail: &Path,
        session_id: String,
        config: &RecordingConfig,
        plan: &SegmentPlan,
        attimo: AttimoInfo,
        debut_session: Instant,
        intervalle_analyse: f32,
    ) -> Result<Self, String> {
        std::fs::create_dir_all(dossier_travail)
            .map_err(|e| format!("Impossible de créer le dossier de session : {}", e))?;

        let manifeste = Manifest {
            version: "1.3".into(),
            session: SessionInfo {
                identifiant: session_id,
                debut: debut_session.to_iso8601(),
                fin: None,
                attimo,
            },
            capture: CaptureInfo {
                peripherique: config.device_name.clone(),
                resolution: format!("{}x{}", config.width, config.height),
                fps: config.fps,
                debit_mbps: config.bitrate_mbps,
                duree_clip_secondes: config.clip_duration_secs,
                chevauchement_secondes: config.overlap_secs,
                duree_morceau_secondes: plan.segment_secs,
            },
            analyse: AnalyseInfo {
                actif: true,
                dossier: "_analyse".into(),
                index: "_analyse/images.jsonl".into(),
                intervalle_secondes: intervalle_analyse,
                largeur_pixels: crate::frames::LARGEUR_ANALYSE,
                images_produites: 0,
            },
            clips: Vec::new(),
        };

        let writer = Self {
            chemin: dossier_travail.join("manifest.json"),
            chemin_index: dossier_travail.join("_analyse").join("images.jsonl"),
            manifeste,
            debut_session,
            duree_morceau: plan.segment_secs,
        };

        writer.ecrire()?;

        Ok(writer)
    }

    /// Ajoute un clip et réécrit le manifeste.
    pub fn ajouter_clip(&mut self, clip: &AssembledClip, complet: bool) -> Result<(), String> {
        // Les horodatages se déduisent de la position du premier morceau.
        // Aucune horloge n'est relue : tout dérive du départ de la session,
        // ce qui garantit que les clips s'enchaînent exactement.
        let debut = self
            .debut_session
            .plus_millis(clip.offset_secs as i64 * 1000);

        let fin = debut.plus_millis(clip.duration_secs as i64 * 1000);

        self.manifeste.clips.push(ClipEntry {
            numero: clip.index,
            fichier: format!("clips/{}", clip.filename),
            debut: debut.to_iso8601(),
            fin: fin.to_iso8601(),
            octets: clip.size_bytes,
            complet,
            morceaux: clip.segments.clone(),
        });

        self.ecrire()
    }

    /// Inscrit les images d'un morceau dans l'index.
    ///
    /// Ajout seul : le fichier n'est jamais relu ni réécrit. Une session de
    /// plusieurs heures peut ainsi produire des dizaines de milliers de
    /// lignes sans coût croissant.
    pub fn ajouter_images(&mut self, images: &[ExtractedFrame]) -> Result<(), String> {
        if images.is_empty() {
            return Ok(());
        }

        if let Some(parent) = self.chemin_index.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| format!("Impossible de créer le dossier d'analyse : {}", e))?;
        }

        let mut fichier = OpenOptions::new()
            .create(true)
            .append(true)
            .open(&self.chemin_index)
            .map_err(|e| format!("Impossible d'ouvrir l'index des images : {}", e))?;

        for image in images {
            // Une ligne JSON par image, sans indentation : format prévu pour
            // être lu ligne à ligne, y compris sur un fichier volumineux.
            let ligne = format!(
                r#"{{"fichier":"{}","instant":"{}","octets":{}}}"#,
                image.filename, image.instant_at, image.size_bytes
            );

            writeln!(fichier, "{}", ligne)
                .map_err(|e| format!("Écriture impossible dans l'index : {}", e))?;
        }

        self.manifeste.analyse.images_produites += images.len() as u64;

        self.ecrire()
    }

    /// Clôt la session.
    pub fn cloturer(&mut self, fin: Instant) -> Result<(), String> {
        self.manifeste.session.fin = Some(fin.to_iso8601());
        self.ecrire()
    }

    pub fn manifeste(&self) -> &Manifest {
        &self.manifeste
    }

    /// Écrit le manifeste de façon atomique.
    ///
    /// Le passage par un fichier temporaire puis un renommage garantit qu'un
    /// lecteur ne tombera jamais sur un JSON incomplet — cas très réel, le
    /// manifeste étant réécrit après chaque clip pendant que l'envoi le lit.
    fn ecrire(&self) -> Result<(), String> {
        let json = serde_json::to_string_pretty(&self.manifeste)
            .map_err(|e| format!("Sérialisation du manifeste impossible : {}", e))?;

        let temporaire = self.chemin.with_extension("json.tmp");

        std::fs::write(&temporaire, json)
            .map_err(|e| format!("Écriture du manifeste impossible : {}", e))?;

        std::fs::rename(&temporaire, &self.chemin)
            .map_err(|e| format!("Renommage du manifeste impossible : {}", e))?;

        Ok(())
    }

    /// Durée d'un morceau — utile aux appelants pour situer un instant.
    pub fn duree_morceau(&self) -> u32 {
        self.duree_morceau
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn config_type() -> RecordingConfig {
        RecordingConfig {
            device_name: "Iriun Webcam".into(),
            audio_device: None,
            width: 1920,
            height: 1080,
            fps: 30,
            bitrate_mbps: 12,
            clip_duration_secs: 150,
            overlap_secs: 30,
            output_dir: String::new(),
        }
    }

    fn attimo_type() -> AttimoInfo {
        AttimoInfo {
            tenant_id: 3,
            event_id: 128,
            checkpoint_id: Some(412),
        }
    }

    fn writer_de_test(dossier: &Path) -> ManifestWriter {
        let config = config_type();
        let plan = config.plan().unwrap();

        ManifestWriter::new(
            dossier,
            "2026-08-13_21h30".into(),
            &config,
            &plan,
            attimo_type(),
            Instant::depuis_millis(1_767_268_800_000),
            2.0,
        )
        .unwrap()
    }

    #[test]
    fn cree_un_manifeste_exploitable_des_le_depart() {
        let dossier = std::env::temp_dir().join("attimo_test_manifest_1");
        let _ = std::fs::remove_dir_all(&dossier);

        let writer = writer_de_test(&dossier);

        assert!(writer.chemin.exists());
        assert_eq!(writer.manifeste().version, "1.3");
        assert_eq!(writer.manifeste().clips.len(), 0);
        assert_eq!(writer.manifeste().capture.duree_morceau_secondes, 30);

        let _ = std::fs::remove_dir_all(&dossier);
    }

    #[test]
    fn horodate_les_clips_par_rapport_au_depart() {
        let dossier = std::env::temp_dir().join("attimo_test_manifest_2");
        let _ = std::fs::remove_dir_all(&dossier);

        let mut writer = writer_de_test(&dossier);

        let clip = AssembledClip {
            index: 1,
            filename: "clip_0001.mp4".into(),
            path: String::new(),
            size_bytes: 12_000_000,
            offset_secs: 0,
            duration_secs: 150,
            segments: vec![0, 1, 2, 3, 4],
        };

        writer.ajouter_clip(&clip, true).unwrap();

        let entree = &writer.manifeste().clips[0];

        assert_eq!(entree.debut, "2026-01-01T12:00:00.000Z");
        assert_eq!(entree.fin, "2026-01-01T12:02:30.000Z");

        let _ = std::fs::remove_dir_all(&dossier);
    }

    #[test]
    fn les_clips_consecutifs_se_chevauchent_vraiment() {
        let dossier = std::env::temp_dir().join("attimo_test_manifest_3");
        let _ = std::fs::remove_dir_all(&dossier);

        let mut writer = writer_de_test(&dossier);

        let premier = AssembledClip {
            index: 1,
            filename: "clip_0001.mp4".into(),
            path: String::new(),
            size_bytes: 1,
            offset_secs: 0,
            duration_secs: 150,
            segments: vec![0, 1, 2, 3, 4],
        };

        let second = AssembledClip {
            index: 2,
            filename: "clip_0002.mp4".into(),
            path: String::new(),
            size_bytes: 1,
            offset_secs: 120,
            duration_secs: 150,
            segments: vec![4, 5, 6, 7, 8],
        };

        writer.ajouter_clip(&premier, true).unwrap();
        writer.ajouter_clip(&second, true).unwrap();

        // Le second commence AVANT la fin du premier : c'est le
        // chevauchement, et c'est ce qui garantit qu'un coureur passant à
        // cette frontière apparaît entier quelque part.
        assert_eq!(writer.manifeste().clips[0].fin, "2026-01-01T12:02:30.000Z");
        assert_eq!(writer.manifeste().clips[1].debut, "2026-01-01T12:02:00.000Z");

        // Le morceau 4 appartient aux deux.
        assert!(writer.manifeste().clips[0].morceaux.contains(&4));
        assert!(writer.manifeste().clips[1].morceaux.contains(&4));

        let _ = std::fs::remove_dir_all(&dossier);
    }

    #[test]
    fn lindex_des_images_sajoute_sans_reecriture() {
        let dossier = std::env::temp_dir().join("attimo_test_manifest_4");
        let _ = std::fs::remove_dir_all(&dossier);

        let mut writer = writer_de_test(&dossier);

        let images = vec![
            ExtractedFrame {
                filename: "img_000000_001.jpg".into(),
                path: String::new(),
                size_bytes: 330_000,
                instant_at: "2026-01-01T12:00:00.000Z".into(),
                segment_index: 0,
            },
            ExtractedFrame {
                filename: "img_000000_002.jpg".into(),
                path: String::new(),
                size_bytes: 328_000,
                instant_at: "2026-01-01T12:00:02.000Z".into(),
                segment_index: 0,
            },
        ];

        writer.ajouter_images(&images).unwrap();
        writer.ajouter_images(&images).unwrap();

        let contenu = std::fs::read_to_string(&writer.chemin_index).unwrap();
        let lignes: Vec<&str> = contenu.lines().filter(|l| !l.is_empty()).collect();

        // Quatre lignes : le second appel s'ajoute, il ne remplace pas.
        assert_eq!(lignes.len(), 4);
        assert_eq!(writer.manifeste().analyse.images_produites, 4);

        let _ = std::fs::remove_dir_all(&dossier);
    }

    #[test]
    fn le_manifeste_reste_lisible_apres_chaque_ecriture() {
        let dossier = std::env::temp_dir().join("attimo_test_manifest_5");
        let _ = std::fs::remove_dir_all(&dossier);

        let mut writer = writer_de_test(&dossier);

        for i in 1..=5 {
            let clip = AssembledClip {
                index: i,
                filename: format!("clip_{:04}.mp4", i),
                path: String::new(),
                size_bytes: 1_000_000,
                offset_secs: (i - 1) * 120,
                duration_secs: 150,
                segments: vec![],
            };

            writer.ajouter_clip(&clip, true).unwrap();

            // À chaque étape, le fichier doit être un JSON valide et complet.
            let contenu = std::fs::read_to_string(&writer.chemin).unwrap();
            let relu: Manifest = serde_json::from_str(&contenu).unwrap();

            assert_eq!(relu.clips.len(), i as usize);
        }

        let _ = std::fs::remove_dir_all(&dossier);
    }

    #[test]
    fn la_cloture_renseigne_la_fin() {
        let dossier = std::env::temp_dir().join("attimo_test_manifest_6");
        let _ = std::fs::remove_dir_all(&dossier);

        let mut writer = writer_de_test(&dossier);

        assert!(writer.manifeste().session.fin.is_none());

        writer
            .cloturer(Instant::depuis_millis(1_767_272_400_000))
            .unwrap();

        assert_eq!(
            writer.manifeste().session.fin.as_deref(),
            Some("2026-01-01T13:00:00.000Z")
        );

        let _ = std::fs::remove_dir_all(&dossier);
    }
}
