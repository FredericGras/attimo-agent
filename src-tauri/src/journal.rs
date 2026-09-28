// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Journal persistant (journal.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// 0.3.2 — Le journal ne vivait qu'à l'écran, limité à 200 lignes : sur une
// manche de 215 photos, le début était déjà perdu, et aucun fichier ne
// restait sur le disque pour un diagnostic après coup.
//
// Deux sources écrivent désormais dans le même fichier :
//   - le journal technique de l'agent (`log::info!`, `log::error!`…) ;
//   - le journal affiché au photographe, transmis par l'interface.
//
// Le fichier vit dans le dossier de données de l'agent — propre à chaque
// édition, la DEV n'écrit jamais dans celui de la production —, sous
// `journaux\agent.log`. Au-delà de 5 Mo il tourne : agent.1.log,
// agent.2.log… cinq fichiers au plus, 25 Mo en tout.
//
// Le bouton « Exporter le journal » recolle ces fichiers dans l'ordre, du
// plus ancien au plus récent, dans un seul fichier choisi par le
// photographe.

use std::fs::{File, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

/// Taille d'un fichier avant rotation.
const TAILLE_MAX: u64 = 5 * 1024 * 1024;

/// Fichiers conservés, le courant compris.
const FICHIERS_CONSERVES: usize = 5;

/// Nom du fichier courant.
const NOM_COURANT: &str = "agent.log";

/// Le fichier ouvert et sa taille, sous un même verrou : deux fils qui
/// écrivent en même temps ne doivent ni entrelacer leurs lignes, ni
/// déclencher deux rotations.
struct Fichier {
    dossier: PathBuf,
    ouvert: Option<File>,
    taille: u64,
}

impl Fichier {
    fn ecrire(&mut self, ligne: &str) {
        if self.ouvert.is_none() {
            self.ouvrir();
        }

        if self.taille + ligne.len() as u64 > TAILLE_MAX {
            self.tourner();
        }

        if let Some(f) = self.ouvert.as_mut() {
            // Écriture directe, sans tampon : une ligne écrite est une ligne
            // qui survit à un plantage de l'agent.
            if f.write_all(ligne.as_bytes()).is_ok() {
                self.taille += ligne.len() as u64;
            }
        }
    }

    fn ouvrir(&mut self) {
        let _ = std::fs::create_dir_all(&self.dossier);

        let chemin = self.dossier.join(NOM_COURANT);

        self.ouvert = OpenOptions::new().create(true).append(true).open(&chemin).ok();
        self.taille = std::fs::metadata(&chemin).map(|m| m.len()).unwrap_or(0);
    }

    fn tourner(&mut self) {
        self.ouvert = None;

        tourner_fichiers(&self.dossier);

        self.ouvrir();
    }
}

/// Décale les fichiers d'un rang : agent.log devient agent.1.log, et le plus
/// ancien disparaît.
fn tourner_fichiers(dossier: &Path) {
    let rang = |n: usize| {
        if n == 0 {
            dossier.join(NOM_COURANT)
        } else {
            dossier.join(format!("agent.{}.log", n))
        }
    };

    let _ = std::fs::remove_file(rang(FICHIERS_CONSERVES - 1));

    for n in (0..FICHIERS_CONSERVES - 1).rev() {
        let _ = std::fs::rename(rang(n), rang(n + 1));
    }
}

/// Fichiers du journal, du plus ancien au plus récent.
fn fichiers_dans_lordre(dossier: &Path) -> Vec<PathBuf> {
    let mut fichiers = Vec::new();

    for n in (1..FICHIERS_CONSERVES).rev() {
        let chemin = dossier.join(format!("agent.{}.log", n));

        if chemin.exists() {
            fichiers.push(chemin);
        }
    }

    let courant = dossier.join(NOM_COURANT);

    if courant.exists() {
        fichiers.push(courant);
    }

    fichiers
}

fn fichier() -> &'static Mutex<Fichier> {
    static FICHIER: std::sync::OnceLock<Mutex<Fichier>> = std::sync::OnceLock::new();

    FICHIER.get_or_init(|| {
        Mutex::new(Fichier {
            dossier: dossier_journaux(),
            ouvert: None,
            taille: 0,
        })
    })
}

/// Dossier des journaux, dans le dossier de données de l'édition.
pub fn dossier_journaux() -> PathBuf {
    crate::database::dirs().join("journaux")
}

/// Horodatage des lignes : heure universelle, à la milliseconde.
fn horodatage() -> String {
    crate::frames::chrono_simple::Instant::maintenant().to_iso8601()
}

fn ecrire_ligne(ligne: &str) {
    let mut f = fichier().lock().unwrap_or_else(|e| e.into_inner());
    f.ecrire(ligne);
}

// ═══════════════════════════════════════════════════════════════════════
// JOURNAL TECHNIQUE
// ═══════════════════════════════════════════════════════════════════════

/// Enveloppe du journal de `env_logger` : ce qu'il affiche dans la console
/// part aussi dans le fichier, avec le même filtre (RUST_LOG, `info` par
/// défaut).
struct Journal {
    console: env_logger::Logger,
}

impl log::Log for Journal {
    fn enabled(&self, metadata: &log::Metadata) -> bool {
        self.console.enabled(metadata)
    }

    fn log(&self, record: &log::Record) {
        if !self.console.matches(record) {
            return;
        }

        self.console.log(record);

        ecrire_ligne(&format!(
            "{} {:<5} [{}] {}\n",
            horodatage(),
            record.level(),
            record.target(),
            record.args()
        ));
    }

    fn flush(&self) {
        self.console.flush();
    }
}

/// Installe le journal. À appeler une fois, au tout début du lancement.
pub fn initialiser() {
    let console = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("info"),
    )
    .build();

    let niveau = console.filter();

    if log::set_boxed_logger(Box::new(Journal { console })).is_ok() {
        log::set_max_level(niveau);
    }

    ecrire_ligne(&format!(
        "\n{} ===== Attimo Agent Terrain {}{} — démarrage =====\n",
        horodatage(),
        env!("CARGO_PKG_VERSION"),
        crate::database::EDITION
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(|e| format!(" {}", e.to_uppercase()))
            .unwrap_or_default()
    ));
}

// ═══════════════════════════════════════════════════════════════════════
// JOURNAL DE L'ÉCRAN
// ═══════════════════════════════════════════════════════════════════════

/// Une ligne du journal affiché au photographe.
#[derive(Debug, Clone, serde::Deserialize)]
pub struct LigneEcran {
    /// Heure affichée à l'écran, dans le format local du poste.
    pub heure: String,
    pub fichier: String,
    pub statut: String,
    pub message: String,
}

/// Recopie des lignes de l'écran dans le fichier.
///
/// L'interface les envoie par paquets, une fois par seconde au plus : une
/// rafale de photos ne doit pas coûter un appel par ligne.
pub fn ecrire_lignes_ecran(lignes: &[LigneEcran]) {
    if lignes.is_empty() {
        return;
    }

    let horo = horodatage();
    let mut bloc = String::new();

    for l in lignes {
        bloc.push_str(&format!(
            "{} ECRAN [{} {}] {} — {}\n",
            horo,
            l.heure,
            l.statut,
            l.fichier,
            l.message.replace('\n', " ")
        ));
    }

    ecrire_ligne(&bloc);
}

/// Recolle tous les fichiers du journal dans `destination`.
///
/// Renvoie le nombre d'octets écrits.
pub fn exporter(destination: &Path) -> Result<u64, String> {
    // Le fichier courant est relâché le temps de la copie : rien ne s'écrit
    // pendant qu'on le lit, et il se rouvre à la ligne suivante.
    let mut f = fichier().lock().unwrap_or_else(|e| e.into_inner());
    f.ouvert = None;

    let dossier = f.dossier.clone();

    let mut sortie = File::create(destination)
        .map_err(|e| format!("Impossible de créer le fichier d'export : {}", e))?;

    let entete = format!(
        "Attimo Agent Terrain {}{} — journal exporté le {} (heures techniques en temps universel)\n\n",
        env!("CARGO_PKG_VERSION"),
        crate::database::EDITION
            .map(str::trim)
            .filter(|e| !e.is_empty())
            .map(|e| format!(" {}", e.to_uppercase()))
            .unwrap_or_default(),
        horodatage()
    );

    sortie
        .write_all(entete.as_bytes())
        .map_err(|e| format!("Écriture impossible : {}", e))?;

    let mut total = entete.len() as u64;

    for chemin in fichiers_dans_lordre(&dossier) {
        let mut source = File::open(&chemin)
            .map_err(|e| format!("Lecture impossible de {} : {}", chemin.display(), e))?;

        total += std::io::copy(&mut source, &mut sortie)
            .map_err(|e| format!("Copie impossible : {}", e))?;
    }

    Ok(total)
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn dossier_de_test(nom: &str) -> PathBuf {
        let d = std::env::temp_dir().join(nom);
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn la_rotation_garde_cinq_fichiers_au_plus() {
        let d = dossier_de_test("attimo_journal_rotation");

        for n in 0..8 {
            std::fs::write(d.join(NOM_COURANT), format!("generation {}", n)).unwrap();
            tourner_fichiers(&d);
        }

        std::fs::write(d.join(NOM_COURANT), "courant").unwrap();

        let fichiers = fichiers_dans_lordre(&d);

        assert_eq!(fichiers.len(), FICHIERS_CONSERVES);

        // Du plus ancien au plus récent : le courant ferme la marche.
        assert_eq!(std::fs::read_to_string(fichiers.last().unwrap()).unwrap(), "courant");
        assert_eq!(std::fs::read_to_string(&fichiers[0]).unwrap(), "generation 4");

        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn un_fichier_plein_tourne_avant_decrire() {
        let d = dossier_de_test("attimo_journal_plein");

        let mut f = Fichier { dossier: d.clone(), ouvert: None, taille: 0 };

        f.ecrire("premiere\n");
        f.taille = TAILLE_MAX;
        f.ecrire("seconde\n");

        assert_eq!(std::fs::read_to_string(d.join("agent.1.log")).unwrap(), "premiere\n");
        assert_eq!(std::fs::read_to_string(d.join(NOM_COURANT)).unwrap(), "seconde\n");

        f.ouvert = None;
        let _ = std::fs::remove_dir_all(&d);
    }
}
