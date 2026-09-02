// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Assemblage des clips (assembler.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// SAAS 430 — Découpage automatique en clips avec chevauchement.
//
// ─── CE QUE FAIT CE MODULE ─────────────────────────────────────────────
//
// L'enregistreur produit des morceaux courts et réguliers. Ce module les
// colle bout à bout pour former les clips que le coureur achètera.
//
// Avec les réglages de référence — clip de 150 s, chevauchement de 30 s —
// les morceaux durent 30 s, il en faut 5 par clip, et l'on avance de 4 :
//
//   morceaux :  [0][1][2][3][4][5][6][7][8]
//
//   clip 1   :  [0][1][2][3][4]
//   clip 2   :            [4][5][6][7][8]
//                          ↑
//                  présent dans les deux = le chevauchement
//
// ─── COLLER SANS RÉENCODER ─────────────────────────────────────────────
//
// L'assemblage utilise le mode « concat » de FFmpeg avec recopie directe
// des flux. Aucune image n'est recalculée : les octets sont recopiés tels
// quels, à peine plus lentement qu'une copie de fichier.
//
// C'est ce qui rend l'opération tenable sur un portable de terrain, en
// extérieur, sur batterie — pendant que la captation continue en parallèle.
// Réencoder chaque clip aurait saturé le processeur et fait décrocher
// l'enregistrement lui-même.
//
// Cette recopie n'est possible que parce que les morceaux partagent
// exactement les mêmes réglages d'encodage, et qu'ils commencent tous sur
// une image-clé. C'est précisément ce que garantit l'enregistreur.
//
// ─── QUAND ASSEMBLER ───────────────────────────────────────────────────
//
// Un clip n'est assemblable que lorsque TOUS ses morceaux sont clos. Un
// morceau encore en cours d'écriture produirait un fichier tronqué.
//
// L'enregistreur signale chaque morceau terminé ; ce module attend d'en
// avoir assez, puis déclenche l'assemblage.

use serde::Serialize;
use std::path::{Path, PathBuf};
use tauri::{AppHandle, Emitter};
use tauri_plugin_shell::process::CommandEvent;
use tauri_plugin_shell::ShellExt;

use crate::recorder::SegmentPlan;

// ═══════════════════════════════════════════════════════════════════════
// STRUCTURES
// ═══════════════════════════════════════════════════════════════════════

/// Un clip assemblé, prêt à être envoyé.
#[derive(Debug, Clone, Serialize)]
pub struct AssembledClip {
    /// Numéro du clip dans la session, à partir de 1.
    pub index: u32,

    pub filename: String,
    pub path: String,
    pub size_bytes: u64,

    /// Position du premier morceau, en secondes depuis le début de la session.
    pub offset_secs: u32,

    pub duration_secs: u32,

    /// Morceaux qui le composent — utile au diagnostic.
    pub segments: Vec<u32>,
}

/// Suit l'avancement et décide quand un clip est prêt.
///
/// Volontairement séparé de l'enregistreur : un ralentissement de
/// l'assemblage ne doit jamais perturber la captation. Sur le terrain,
/// perdre des images est irrattrapable ; assembler avec quelques secondes
/// de retard ne se voit pas.
pub struct ClipTracker {
    plan: SegmentPlan,
    dossier_morceaux: PathBuf,
    dossier_clips: PathBuf,

    /// Morceaux clos, dans l'ordre d'arrivée.
    morceaux_prets: Vec<u32>,

    /// Numéro du prochain clip à produire, à partir de 1.
    prochain_clip: u32,
}

impl ClipTracker {
    pub fn new(plan: SegmentPlan, dossier_travail: &Path) -> Result<Self, String> {
        let dossier_clips = dossier_travail.join("clips");

        std::fs::create_dir_all(&dossier_clips)
            .map_err(|e| format!("Impossible de créer le dossier des clips : {}", e))?;

        Ok(Self {
            plan,
            dossier_morceaux: dossier_travail.join("_morceaux"),
            dossier_clips,
            morceaux_prets: Vec::new(),
            prochain_clip: 1,
        })
    }

    /// Enregistre un morceau terminé et indique si un clip est désormais
    /// assemblable.
    ///
    /// Renvoie la liste des morceaux à coller, ou rien s'il faut encore
    /// attendre.
    pub fn segment_ready(&mut self, index: u32) -> Option<Vec<u32>> {
        if !self.morceaux_prets.contains(&index) {
            self.morceaux_prets.push(index);
            self.morceaux_prets.sort_unstable();
        }

        self.clip_assemblable()
    }

    /// Détermine si le prochain clip attendu peut être formé.
    ///
    /// Le premier morceau du clip N se déduit du pas : le clip 1 commence au
    /// morceau 0, le clip 2 au morceau `segments_step`, et ainsi de suite.
    fn clip_assemblable(&self) -> Option<Vec<u32>> {
        let premier = (self.prochain_clip - 1) * self.plan.segments_step;
        let dernier = premier + self.plan.segments_per_clip - 1;

        // Tous les morceaux de l'intervalle doivent être clos. Un seul
        // manquant, et le clip serait tronqué.
        let attendus: Vec<u32> = (premier..=dernier).collect();

        let tous_presents = attendus
            .iter()
            .all(|index| self.morceaux_prets.contains(index));

        if tous_presents {
            Some(attendus)
        } else {
            None
        }
    }

    /// Position du clip courant depuis le début de la session.
    fn offset_du_clip_courant(&self) -> u32 {
        (self.prochain_clip - 1) * self.plan.step_secs
    }

    /// Passe au clip suivant. À appeler après un assemblage réussi.
    pub fn clip_termine(&mut self) {
        self.prochain_clip += 1;

        // Les morceaux devenus inutiles peuvent être oubliés du suivi : un
        // morceau situé avant le début du prochain clip ne servira plus.
        //
        // Ils ne sont PAS supprimés du disque pour autant — les images
        // d'analyse en sont extraites, et le contrat interdit toute
        // suppression automatique sur le poste du photographe.
        let premier_utile = (self.prochain_clip - 1) * self.plan.segments_step;
        self.morceaux_prets.retain(|index| *index >= premier_utile);
    }

    pub fn clip_courant(&self) -> u32 {
        self.prochain_clip
    }

    /// Morceaux disponibles pour un DERNIER clip, forcément plus court.
    ///
    /// Appelé une seule fois, à l'arrêt de la captation. La géométrie ne
    /// tombe presque jamais juste : une captation s'arrête au milieu d'un
    /// clip, et tout ce qui a été filmé après le dernier clip complet
    /// n'appartient à aucun clip. Sans ce rattrapage, ces secondes-là sont
    /// perdues — c'est-à-dire les coureurs qui y passent.
    ///
    /// Ne renvoie rien si ces morceaux n'apportent aucune vidéo neuve : le
    /// chevauchement fait qu'un clip final trop court serait déjà contenu
    /// tout entier dans le précédent. Le seuil se lit dans le plan — il faut
    /// dépasser le nombre de morceaux de chevauchement.
    pub fn clip_final(&self) -> Option<Vec<u32>> {
        let premier = (self.prochain_clip - 1) * self.plan.segments_step;

        // On s'arrête au premier trou : un clip à trous serait un montage,
        // pas un extrait.
        let mut morceaux = Vec::new();

        for index in premier..(premier + self.plan.segments_per_clip) {
            if !self.morceaux_prets.contains(&index) {
                break;
            }

            morceaux.push(index);
        }

        if morceaux.is_empty() {
            return None;
        }

        let morceaux_de_chevauchement = self.plan.segments_per_clip - self.plan.segments_step;

        if self.prochain_clip > 1 && morceaux.len() as u32 <= morceaux_de_chevauchement {
            return None;
        }

        Some(morceaux)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// ASSEMBLAGE
// ═══════════════════════════════════════════════════════════════════════

/// Colle les morceaux indiqués en un clip.
pub async fn assembler_clip(
    app: &AppHandle,
    tracker: &ClipTracker,
    morceaux: &[u32],
    session_id: &str,
) -> Result<AssembledClip, String> {
    let index_clip = tracker.clip_courant();

    // ─── Liste des fichiers à coller ───
    //
    // FFmpeg lit cette liste depuis un fichier texte plutôt que depuis la
    // ligne de commande : au-delà de quelques morceaux, la commande
    // dépasserait la longueur maximale acceptée par Windows.
    let mut liste = String::new();

    for index in morceaux {
        let chemin = tracker
            .dossier_morceaux
            .join(format!("m_{:06}.mp4", index));

        if !chemin.exists() {
            return Err(format!(
                "Le morceau {} est introuvable — le clip ne peut pas être assemblé.",
                index
            ));
        }

        // Les antislash de Windows doivent être doublés, et les apostrophes
        // protégées : un dossier nommé « Course d'été » casserait la liste.
        let chemin_texte = chemin.to_string_lossy().replace('\\', "/").replace('\'', "'\\''");

        liste.push_str(&format!("file '{}'\n", chemin_texte));
    }

    let fichier_liste = tracker
        .dossier_clips
        .join(format!(".liste_{}.txt", index_clip));

    std::fs::write(&fichier_liste, &liste)
        .map_err(|e| format!("Impossible d'écrire la liste d'assemblage : {}", e))?;

    // ─── Nom du clip ───

    let nom_clip = format!("clip_{:04}.mp4", index_clip);
    let chemin_clip = tracker.dossier_clips.join(&nom_clip);

    // ─── Assemblage ───

    let arguments: Vec<String> = vec![
        "-hide_banner".into(),
        "-f".into(),
        "concat".into(),
        // Autorise les chemins absolus dans la liste. Sans cela, FFmpeg
        // refuse par précaution — il craint qu'un fichier de liste hostile
        // ne fasse lire des fichiers arbitraires. Ici la liste est écrite
        // par nous, pas reçue de l'extérieur.
        "-safe".into(),
        "0".into(),
        "-i".into(),
        fichier_liste.to_string_lossy().to_string(),
        // Recopie directe : aucun réencodage.
        "-c".into(),
        "copy".into(),
        // Réécrit les horodatages internes pour que le clip démarre à zéro.
        "-avoid_negative_ts".into(),
        "make_zero".into(),
        "-y".into(),
        chemin_clip.to_string_lossy().to_string(),
    ];

    let commande = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| format!("FFmpeg embarqué introuvable : {}", e))?
        .args(arguments);

    let (mut evenements, _enfant) = commande
        .spawn()
        .map_err(|e| format!("Impossible de lancer l'assemblage : {}", e))?;

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

    // Le fichier de liste ne sert plus.
    let _ = std::fs::remove_file(&fichier_liste);

    // ─── Contrôle du résultat ───
    //
    // On ne se fie pas au code de retour de FFmpeg : c'est la présence d'un
    // fichier de taille plausible qui fait foi.

    let metadonnees = std::fs::metadata(&chemin_clip).map_err(|_| {
        format!(
            "L'assemblage n'a produit aucun fichier.\n\nJournal FFmpeg :\n{}",
            journal.chars().rev().take(500).collect::<String>().chars().rev().collect::<String>()
        )
    })?;

    let taille = metadonnees.len();

    if taille < 1024 {
        return Err(format!(
            "Le clip produit est vide ({} octets).\n\nJournal FFmpeg :\n{}",
            taille,
            journal.chars().rev().take(500).collect::<String>().chars().rev().collect::<String>()
        ));
    }

    let clip = AssembledClip {
        index: index_clip,
        filename: nom_clip,
        path: chemin_clip.to_string_lossy().to_string(),
        size_bytes: taille,
        offset_secs: tracker.offset_du_clip_courant(),
        duration_secs: tracker.plan.segments_per_clip * tracker.plan.segment_secs,
        segments: morceaux.to_vec(),
    };

    log::info!(
        "Clip {} assemblé : {} morceaux, {} octets",
        clip.index,
        morceaux.len(),
        taille
    );

    let _ = app.emit("clip-ready", &clip);

    let _ = session_id;

    Ok(clip)
}

// ═══════════════════════════════════════════════════════════════════════
// MESURE
// ═══════════════════════════════════════════════════════════════════════

/// Durée réelle d'un fichier vidéo, en secondes.
///
/// Sert à deux choses, et la seconde est la plus importante :
///
///   1. Donner sa vraie longueur au dernier clip, plus court que les
///      autres. Le manifeste en déduit l'heure de fin, et le serveur s'en
///      sert pour rattacher les coureurs : une fin annoncée trop tard
///      vendrait à un coureur un clip où il n'apparaît pas.
///
///   2. Vérifier qu'un morceau est exploitable. Un mp4 dont l'index n'a pas
///      été écrit — FFmpeg tué avant la fin — n'a aucune durée lisible.
///      C'est un test bien plus honnête qu'un code de retour.
pub async fn duree_reelle(app: &AppHandle, chemin: &Path) -> Option<f64> {
    if !chemin.exists() {
        return None;
    }

    let arguments: Vec<String> = vec![
        "-v".into(),
        "error".into(),
        "-show_entries".into(),
        "format=duration".into(),
        "-of".into(),
        "default=noprint_wrappers=1:nokey=1".into(),
        chemin.to_string_lossy().to_string(),
    ];

    let commande = app.shell().sidecar("ffprobe").ok()?.args(arguments);

    let (mut evenements, _enfant) = commande.spawn().ok()?;

    let mut sortie = String::new();

    // Lecture jusqu'à la fermeture du canal, comme pour l'enregistreur :
    // sortir sur `Terminated` risquerait de perdre la ligne attendue.
    while let Some(evenement) = evenements.recv().await {
        if let CommandEvent::Stdout(donnees) = evenement {
            sortie.push_str(&String::from_utf8_lossy(&donnees));
        }
    }

    sortie.trim().parse::<f64>().ok().filter(|duree| *duree > 0.0)
}

// ═══════════════════════════════════════════════════════════════════════
// VERSION LÉGÈRE
// ═══════════════════════════════════════════════════════════════════════

/// Produit la version légère d'un clip.
///
/// C'est la seule opération de tout le module qui réencode réellement — et
/// elle est indispensable : le coureur doit pouvoir regarder sa vidéo
/// pendant la course, sur un réseau où le fichier pleine qualité ne passerait
/// jamais.
///
/// Vingt-cinq mégabits par seconde contre deux : c'est ce rapport qui décide
/// si le coureur voit sa vidéo le jour même ou le lendemain.
///
/// Le réencodage coûte du temps processeur, mais reste tenable : « veryfast »
/// sur du 540p va nettement plus vite que le temps réel, même sur un portable
/// de terrain. La captation continue pendant ce temps, sur un autre fil.
pub async fn generer_proxy(
    app: &AppHandle,
    chemin_clip: &Path,
    hauteur: u32,
    debit_mbps: u32,
) -> Result<PathBuf, String> {
    let dossier = chemin_clip
        .parent()
        .ok_or_else(|| "Chemin de clip invalide.".to_string())?
        .join("proxy");

    std::fs::create_dir_all(&dossier)
        .map_err(|e| format!("Impossible de créer le dossier des versions légères : {}", e))?;

    let nom = chemin_clip
        .file_name()
        .map(|n| n.to_string_lossy().to_string())
        .unwrap_or_else(|| "clip.mp4".to_string());

    let chemin_proxy = dossier.join(&nom);

    let arguments: Vec<String> = vec![
        "-hide_banner".into(),
        "-i".into(),
        chemin_clip.to_string_lossy().to_string(),
        // Réduction en hauteur, largeur calculée pour conserver les
        // proportions. Le « -2 » arrondit à un nombre pair, exigence des
        // encodeurs.
        "-vf".into(),
        format!("scale=-2:{}", hauteur),
        "-c:v".into(),
        "libx264".into(),
        "-preset".into(),
        "veryfast".into(),
        "-b:v".into(),
        format!("{}M", debit_mbps),
        "-pix_fmt".into(),
        "yuv420p".into(),
        // Place l'index de lecture en tête de fichier : la vidéo démarre
        // immédiatement au lieu d'attendre le téléchargement complet.
        // Détail invisible en local, déterminant en lecture depuis un
        // navigateur.
        "-movflags".into(),
        "+faststart".into(),
        // Le son est conservé mais fortement compressé : il pèse peu et
        // l'ambiance de course fait partie du souvenir.
        "-c:a".into(),
        "aac".into(),
        "-b:a".into(),
        "96k".into(),
        "-y".into(),
        chemin_proxy.to_string_lossy().to_string(),
    ];

    let commande = app
        .shell()
        .sidecar("ffmpeg")
        .map_err(|e| format!("FFmpeg embarqué introuvable : {}", e))?
        .args(arguments);

    let (mut evenements, _enfant) = commande
        .spawn()
        .map_err(|e| format!("Impossible de lancer la conversion : {}", e))?;

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

    let metadonnees = std::fs::metadata(&chemin_proxy)
        .map_err(|_| "La conversion n'a produit aucun fichier.".to_string())?;

    if metadonnees.len() < 1024 {
        return Err(format!(
            "La version légère est vide ({} octets).",
            metadonnees.len()
        ));
    }

    log::info!(
        "Version légère produite : {} octets ({} % du clip d'origine)",
        metadonnees.len(),
        std::fs::metadata(chemin_clip)
            .map(|m| metadonnees.len() * 100 / m.len().max(1))
            .unwrap_or(0)
    );

    Ok(chemin_proxy)
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn plan_reference() -> SegmentPlan {
        // Clip de 150 s, chevauchement de 30 s.
        SegmentPlan {
            segment_secs: 30,
            segments_per_clip: 5,
            segments_step: 4,
            step_secs: 120,
        }
    }

    fn tracker_de_test() -> ClipTracker {
        ClipTracker {
            plan: plan_reference(),
            dossier_morceaux: PathBuf::from("/tmp/m"),
            dossier_clips: PathBuf::from("/tmp/c"),
            morceaux_prets: Vec::new(),
            prochain_clip: 1,
        }
    }

    #[test]
    fn attend_davoir_tous_les_morceaux() {
        let mut t = tracker_de_test();

        assert!(t.segment_ready(0).is_none());
        assert!(t.segment_ready(1).is_none());
        assert!(t.segment_ready(2).is_none());
        assert!(t.segment_ready(3).is_none());

        // Le cinquième déclenche l'assemblage.
        let morceaux = t.segment_ready(4).unwrap();
        assert_eq!(morceaux, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn le_second_clip_reprend_le_dernier_morceau_du_premier() {
        let mut t = tracker_de_test();

        for i in 0..=4 {
            t.segment_ready(i);
        }
        t.clip_termine();

        for i in 5..=7 {
            assert!(t.segment_ready(i).is_none());
        }

        let morceaux = t.segment_ready(8).unwrap();

        // Le morceau 4 appartient aux deux clips : c'est le chevauchement.
        assert_eq!(morceaux, vec![4, 5, 6, 7, 8]);
    }

    #[test]
    fn le_chevauchement_dure_bien_ce_qui_est_demande() {
        let mut t = tracker_de_test();

        for i in 0..=4 {
            t.segment_ready(i);
        }
        let premier = t.clip_assemblable().unwrap();
        t.clip_termine();

        for i in 5..=8 {
            t.segment_ready(i);
        }
        let second = t.clip_assemblable().unwrap();

        let communs: Vec<u32> = premier
            .iter()
            .filter(|i| second.contains(i))
            .copied()
            .collect();

        assert_eq!(communs.len(), 1);
        assert_eq!(communs[0] * 0 + t.plan.segment_secs, 30);
    }

    #[test]
    fn oublie_les_morceaux_devenus_inutiles() {
        let mut t = tracker_de_test();

        for i in 0..=4 {
            t.segment_ready(i);
        }
        t.clip_termine();

        // Les morceaux 0 à 3 ne serviront plus à aucun clip.
        assert!(!t.morceaux_prets.contains(&0));
        assert!(!t.morceaux_prets.contains(&3));

        // Le 4 reste : il ouvre le clip suivant.
        assert!(t.morceaux_prets.contains(&4));
    }

    #[test]
    fn supporte_un_ordre_darrivee_desordonne() {
        let mut t = tracker_de_test();

        // Les morceaux peuvent être signalés dans le désordre si le disque
        // prend du retard.
        t.segment_ready(2);
        t.segment_ready(0);
        t.segment_ready(4);
        t.segment_ready(1);

        let morceaux = t.segment_ready(3).unwrap();
        assert_eq!(morceaux, vec![0, 1, 2, 3, 4]);
    }

    #[test]
    fn propose_un_clip_final_avec_ce_qui_reste() {
        let mut t = tracker_de_test();

        for i in 0..=4 {
            t.segment_ready(i);
        }
        t.clip_termine();

        // Le clip 2 attend les morceaux 4 à 8 ; la captation s'arrête à 6.
        t.segment_ready(5);
        t.segment_ready(6);

        assert_eq!(t.clip_final(), Some(vec![4, 5, 6]));
    }

    #[test]
    fn refuse_un_clip_final_deja_contenu_dans_le_precedent() {
        let mut t = tracker_de_test();

        for i in 0..=4 {
            t.segment_ready(i);
        }
        t.clip_termine();

        // Seul le morceau de chevauchement est là : il est déjà tout entier
        // dans le clip 1, un clip final ne montrerait rien de neuf.
        assert_eq!(t.clip_final(), None);
    }

    #[test]
    fn accepte_un_premier_clip_final_meme_tres_court() {
        let mut t = tracker_de_test();

        // Captation arrêtée avant le premier clip complet : ce qui a été
        // filmé n'est nulle part ailleurs.
        t.segment_ready(0);
        t.segment_ready(1);

        assert_eq!(t.clip_final(), Some(vec![0, 1]));
    }

    #[test]
    fn le_clip_final_sarrete_au_premier_trou() {
        let mut t = tracker_de_test();

        t.segment_ready(0);
        t.segment_ready(1);
        t.segment_ready(3);

        assert_eq!(t.clip_final(), Some(vec![0, 1]));
    }

    #[test]
    fn calcule_la_position_de_chaque_clip() {
        let mut t = tracker_de_test();

        assert_eq!(t.offset_du_clip_courant(), 0);

        t.clip_termine();
        assert_eq!(t.offset_du_clip_courant(), 120);

        t.clip_termine();
        assert_eq!(t.offset_du_clip_courant(), 240);
    }
}
