// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Module Watcher (surveillance dossier)
// ═══════════════════════════════════════════════════════════════════════
//
// Ce module est le « guetteur » de l'Agent Terrain.
// Son rôle : surveiller un dossier local sur le PC du photographe
// et détecter chaque nouveau JPEG qui y apparaît.
//
// Flux concret sur le terrain :
//   1. Canon EOS Utility reçoit IMG_1234.JPG via le câble USB/réseau
//   2. Il écrit le fichier dans C:\Photos\Course\ (quelques centaines de ms)
//   3. Windows notifie notre watcher via ReadDirectoryChangesW (natif, pas de polling)
//   4. Le watcher vérifie : JPEG ? pas trop petit ? écriture terminée ?
//   5. Il inscrit le fichier dans la base SQLite locale (déduplication)
//   6. Il l'envoie dans le canal vers l'uploader pour envoi vers l'API Attimo
//
// La crate `notify` utilise les API natives de chaque OS :
//   - Windows : ReadDirectoryChangesW (quasi instantané)
//   - macOS   : FSEvents
//   - Linux   : inotify
//

use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tauri::Emitter;
use tokio::sync::mpsc;
use log::{info, warn, debug};

use crate::database;

// ─── Constantes ─────────────────────────────────────────────

/// Extensions JPEG acceptées (comparaison en minuscules)
const JPEG_EXTENSIONS: &[&str] = &["jpg", "jpeg"];

/// Extensions RAW à ignorer explicitement (tous les constructeurs majeurs)
const RAW_EXTENSIONS: &[&str] = &[
    "cr3", "cr2",   // Canon
    "nef",          // Nikon
    "arw",          // Sony
    "orf",          // Olympus
    "rw2",          // Panasonic
    "raf",          // Fujifilm
    "dng",          // Adobe DNG
];

/// Dossiers à exclure automatiquement en mode récursif
const EXCLUDED_DIRS: &[&str] = &[
    "Rejects",      // Dossier de rejet courant (Capture One, Photo Mechanic)
    "RAW",          // Dossier RAW séparé
    ".thumbnails",  // Thumbnails système
    "@eaDir",       // Synology metadata
    "Thumbs",       // Windows thumbnails
];

/// Taille minimum : 50 Ko. En dessous c'est un fragment ou un thumbnail système.
const MIN_FILE_SIZE: u64 = 50 * 1024;

/// Taille maximum : 20 Mo. Au-delà, le fichier est ignoré avec un avertissement.
const MAX_FILE_SIZE: u64 = 20 * 1024 * 1024;

/// Délai entre les vérifications de stabilité (ms).
/// Le logiciel constructeur peut mettre quelques centaines de ms à écrire le fichier.
const STABILITY_DELAY_MS: u64 = 500;

/// Nombre de vérifications de stabilité avant d'accepter le fichier.
/// 3 vérifications × 500ms = 1,5 secondes maximum d'attente.
const STABILITY_CHECKS: u32 = 3;

/// Vérifications de stabilité menées en même temps (0.3.1).
///
/// Chaque vérification dure au moins 500 ms, et elles se faisaient une par
/// une : la détection plafonnait donc à 2 photos par seconde (1,9 mesuré),
/// moins encore en direct où chaque photo produit plusieurs événements
/// Windows (création puis modifications) traités chacun avec son attente.
/// Menées par 16, elles laissent passer plus de 30 photos par seconde —
/// bien au-delà de ce que 6 envois simultanés consomment.
const VERIFICATIONS_SIMULTANEES: usize = 16;

// ─── Structures ─────────────────────────────────────────────

/// Représente un fichier à uploader.
/// Transmis du watcher vers l'uploader via un canal tokio.
#[derive(Debug, Clone)]
pub struct FileJob {
    /// ID du fichier dans la table SQLite upload_files
    pub db_file_id: i64,
    /// ID de la session d'upload en cours
    pub session_id: i64,
    /// Chemin complet du fichier sur le disque local
    pub file_path: PathBuf,
    /// Nom du fichier seul (ex: "IMG_1234.JPG")
    pub filename: String,
    /// Taille en octets
    pub file_size: u64,
}

// ─── Filtrage des fichiers ──────────────────────────────────

/// Vérifie si un fichier est un JPEG valide à surveiller.
/// Rejette les fichiers cachés, temporaires, RAW, et non-JPEG.
fn is_valid_jpeg(path: &Path) -> bool {
    // Extraire le nom du fichier
    let filename = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name,
        None => return false,
    };

    // Fichiers cachés Unix/macOS (commencent par .)
    if filename.starts_with('.') {
        return false;
    }

    // Fichiers temporaires Office/éditeurs (commencent par ~)
    if filename.starts_with('~') {
        return false;
    }

    // Fichiers .tmp (écriture en cours par certains logiciels)
    if filename.to_lowercase().ends_with(".tmp") {
        return false;
    }

    // Extraire l'extension en minuscules pour comparaison insensible à la casse
    // (les Canon écrivent .JPG en majuscules, d'autres en minuscules)
    let extension = match path.extension().and_then(|e| e.to_str()) {
        Some(ext) => ext.to_lowercase(),
        None => return false,
    };

    // Rejeter explicitement les fichiers RAW
    if RAW_EXTENSIONS.contains(&extension.as_str()) {
        return false;
    }

    // Accepter uniquement les JPEG
    JPEG_EXTENSIONS.contains(&extension.as_str())
}

/// Vérifie si un nom de dossier doit être exclu en mode récursif
fn is_excluded_dir(dirname: &str) -> bool {
    dirname.starts_with('.') || EXCLUDED_DIRS.contains(&dirname)
}

// ─── Stabilité du fichier ───────────────────────────────────

/// Attend que le fichier soit stable (écriture terminée par le logiciel constructeur).
///
/// Quand Canon EOS Utility reçoit un JPEG via le câble USB, il l'écrit
/// progressivement sur le disque. Si on tente de lire le fichier pendant
/// l'écriture, on aurait un fichier incomplet/corrompu.
///
/// Cette fonction vérifie que la taille du fichier ne change plus pendant
/// 500ms. Si après 3 tentatives la taille change encore, on fait une
/// dernière vérification.
///
/// Retourne `Some(taille)` si le fichier est stable et assez gros,
/// `None` si le fichier a disparu, est trop petit, ou reste instable.
async fn wait_for_stability(path: &Path) -> Option<u64> {
    for attempt in 0..STABILITY_CHECKS {
        // Lecture taille #1
        let size1 = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m.len(),
            _ => return None, // fichier disparu ou n'est pas un fichier
        };

        // Attendre 500ms
        tokio::time::sleep(Duration::from_millis(STABILITY_DELAY_MS)).await;

        // Lecture taille #2
        let size2 = match std::fs::metadata(path) {
            Ok(m) if m.is_file() => m.len(),
            _ => return None, // fichier disparu pendant l'attente
        };

        // Taille identique et suffisante → fichier stable
        if size1 == size2 && size1 >= MIN_FILE_SIZE {
            return Some(size2);
        }

        if attempt < STABILITY_CHECKS - 1 {
            debug!(
                "Fichier en cours d'écriture, tentative {}/{}: {}",
                attempt + 1,
                STABILITY_CHECKS,
                path.display()
            );
        }
    }

    // Dernière chance après toutes les tentatives
    match std::fs::metadata(path) {
        Ok(m) if m.is_file() && m.len() >= MIN_FILE_SIZE => Some(m.len()),
        _ => None,
    }
}

// ─── Traitement d'un fichier détecté ────────────────────────

/// Traite un fichier détecté par le watcher.
///
/// Étapes :
/// 1. Attendre la stabilité (écriture terminée)
/// 2. Inscrire le fichier (voir `inscrire_fichier`)
///
/// Renvoie `false` si le fichier était encore instable ou trop petit : il
/// faudra le revoir s'il bouge encore.
async fn process_detected_file(
    path: PathBuf,
    session_id: i64,
    tx: &mpsc::UnboundedSender<FileJob>,
    app_handle: &tauri::AppHandle,
) -> bool {
    // 1. Attendre la stabilité du fichier
    let file_size = match wait_for_stability(&path).await {
        Some(size) => size,
        None => {
            debug!("Fichier instable ou trop petit, ignoré: {}", path.display());
            return false;
        }
    };

    inscrire_fichier(path, file_size, session_id, tx, app_handle);

    true
}

/// Inscrit un fichier stable et le confie à l'uploader.
///
/// Étapes :
/// 1. Vérifier la taille max (20 Mo)
/// 2. Insérer dans SQLite (déduplication automatique via UNIQUE)
/// 3. Notifier le frontend JS (événement `file_detected`)
/// 4. Envoyer au canal vers l'uploader
fn inscrire_fichier(
    path: PathBuf,
    file_size: u64,
    session_id: i64,
    tx: &mpsc::UnboundedSender<FileJob>,
    app_handle: &tauri::AppHandle,
) {
    // Extraire le nom du fichier
    let filename = match path.file_name().and_then(|n| n.to_str()) {
        Some(name) => name.to_string(),
        None => return,
    };

    // 1. Vérifier la taille maximale
    if file_size > MAX_FILE_SIZE {
        warn!(
            "Fichier trop volumineux ({:.1} Mo), ignoré: {}",
            file_size as f64 / 1_048_576.0,
            filename
        );
        let _ = app_handle.emit(
            "file_skipped",
            serde_json::json!({
                "filename": filename,
                "reason": format!(
                    "Trop volumineux ({:.1} Mo, max 20 Mo)",
                    file_size as f64 / 1_048_576.0
                ),
            }),
        );
        return;
    }

    // 2. Date de modification du fichier (timestamp Unix comme chaîne)
    let file_modified = std::fs::metadata(&path)
        .and_then(|m| m.modified())
        .map(|t| {
            t.duration_since(std::time::UNIX_EPOCH)
                .unwrap_or_default()
                .as_secs()
                .to_string()
        })
        .unwrap_or_default();

    // 3. Insérer dans SQLite
    //    INSERT OR IGNORE + vérification changes() → déduplication automatique.
    //    Si le fichier existe déjà (même session + même nom), insert_file retourne None.
    let db_file_id = match database::insert_file(
        session_id,
        &filename,
        path.to_str().unwrap_or(""),
        file_size as i64,
        &file_modified,
    ) {
        Ok(Some(id)) => id,
        Ok(None) => {
            // Fichier déjà connu dans cette session (pending, uploading, success, ou failed)
            debug!("Fichier déjà en base, ignoré: {}", filename);
            return;
        }
        Err(e) => {
            warn!("Erreur insertion fichier en base: {}", e);
            return;
        }
    };

    // 4. Notifier le frontend (met à jour le compteur "En attente" et le journal)
    let _ = app_handle.emit(
        "file_detected",
        serde_json::json!({
            "filename": filename,
            "size": file_size,
        }),
    );

    info!(
        "Nouveau fichier détecté: {} ({:.1} Mo)",
        filename,
        file_size as f64 / 1_048_576.0
    );

    // 5. Envoyer dans le canal vers l'uploader
    let job = FileJob {
        db_file_id,
        session_id,
        file_path: path,
        filename,
        file_size,
    };

    match tx.send(job) {
        // Compté pour la priorité des photos sur la vidéo.
        Ok(()) => crate::uploader::photo_ajoutee(),
        Err(e) => warn!("Erreur envoi vers uploader: {}", e),
    }
}

// ─── Scan des fichiers existants ────────────────────────────

/// Scanne les fichiers JPEG déjà présents dans le dossier au démarrage.
///
/// Appelé quand le photographe choisit "Inclure les fichiers existants" dans
/// l'écran de configuration. Utile quand le photographe lance l'Agent Terrain
/// après avoir déjà commencé à photographier — les photos déjà dans le dossier
/// seront uploadées aussi.
pub async fn scan_existing_files(
    folder: &Path,
    recursive: bool,
    session_id: i64,
    tx: &mpsc::UnboundedSender<FileJob>,
    app_handle: &tauri::AppHandle,
    running: &Arc<AtomicBool>,
) {
    info!("Scan des fichiers existants dans: {}", folder.display());

    let files = if recursive {
        collect_files_recursive(folder)
    } else {
        collect_files_flat(folder)
    };

    let mut jpegs: Vec<PathBuf> = files.into_iter().filter(|p| is_valid_jpeg(p)).collect();

    // Ordre du nom de fichier = ordre de prise de vue sur les boîtiers :
    // les premières photos de la course partent les premières.
    jpegs.sort();

    let total = jpegs.len();
    info!("JPEG existants trouvés: {}", total);

    // Notifier le frontend du début du scan
    let _ = app_handle.emit(
        "scan_started",
        serde_json::json!({ "total": total }),
    );

    // Les vérifications de stabilité d'un paquet tournent ensemble, puis les
    // fichiers sont inscrits dans l'ordre : même garantie qu'avant (taille
    // inchangée pendant 500 ms), sans payer ces 500 ms une photo après
    // l'autre.
    for paquet in jpegs.chunks(VERIFICATIONS_SIMULTANEES) {
        // Vérifier si l'arrêt a été demandé
        if !running.load(Ordering::Relaxed) {
            info!("Scan interrompu (arrêt demandé)");
            break;
        }

        let verifications: Vec<_> = paquet
            .iter()
            .map(|path| {
                let path = path.clone();
                tokio::spawn(async move { wait_for_stability(&path).await })
            })
            .collect();

        for (path, verification) in paquet.iter().zip(verifications) {
            match verification.await.ok().flatten() {
                Some(taille) => {
                    inscrire_fichier(path.clone(), taille, session_id, tx, app_handle)
                }
                None => debug!("Fichier instable ou trop petit, ignoré: {}", path.display()),
            }
        }
    }

    let _ = app_handle.emit("scan_complete", serde_json::json!({}));
}

/// Liste les fichiers d'un dossier (non récursif, un seul niveau)
fn collect_files_flat(folder: &Path) -> Vec<PathBuf> {
    std::fs::read_dir(folder)
        .into_iter()
        .flatten()
        .filter_map(|entry| entry.ok())
        .filter(|entry| {
            entry.file_type().map(|ft| ft.is_file()).unwrap_or(false)
        })
        .map(|entry| entry.path())
        .collect()
}

/// Liste les fichiers d'un dossier et de tous ses sous-dossiers,
/// en excluant les dossiers dans EXCLUDED_DIRS
fn collect_files_recursive(folder: &Path) -> Vec<PathBuf> {
    let mut files = Vec::new();
    collect_recursive_inner(folder, &mut files);
    files
}

/// Fonction récursive interne pour parcourir l'arborescence
fn collect_recursive_inner(dir: &Path, files: &mut Vec<PathBuf>) {
    let entries = match std::fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return, // dossier inaccessible, on continue
    };

    for entry in entries.filter_map(|e| e.ok()) {
        let path = entry.path();
        if path.is_dir() {
            let dirname = path
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or("");
            // Ignorer les dossiers exclus
            if !is_excluded_dir(dirname) {
                collect_recursive_inner(&path, files);
            }
        } else {
            files.push(path);
        }
    }
}

// ─── Démarrage de la surveillance ───────────────────────────

/// Démarre la surveillance du dossier.
///
/// Cette fonction crée deux choses :
///
/// 1. Un `RecommendedWatcher` — c'est lui qui reçoit les notifications
///    de l'OS quand un fichier est créé ou modifié dans le dossier.
///    Il tourne sur son propre thread interne (géré par la crate notify).
///
/// 2. Une tâche tokio async — elle reçoit les chemins des fichiers
///    détectés par le watcher, les vérifie, et les envoie à l'uploader.
///
/// Le watcher retourné DOIT être gardé en vie (stocké quelque part).
/// S'il est droppé (libéré), la surveillance s'arrête.
///
/// # Arguments
/// * `folder` — Chemin du dossier à surveiller
/// * `recursive` — `true` pour surveiller aussi les sous-dossiers
/// * `session_id` — ID de la session dans la base SQLite
/// * `include_existing` — Scanner les fichiers déjà présents dans le dossier
/// * `tx` — Canal d'envoi vers l'uploader (unbounded = pas de limite)
/// * `running` — Flag partagé : mis à `false` pour tout stopper
/// * `app_handle` — Handle Tauri pour émettre des événements vers le JS
///
/// # Retour
/// Le `RecommendedWatcher` à stocker. Erreur si le dossier est inaccessible.
pub fn start_watching(
    folder: PathBuf,
    recursive: bool,
    session_id: i64,
    include_existing: bool,
    tx: mpsc::UnboundedSender<FileJob>,
    running: Arc<AtomicBool>,
    app_handle: tauri::AppHandle,
) -> Result<RecommendedWatcher, String> {
    // ── Canal intermédiaire ──
    // notify fonctionne en synchrone (callback sur un thread).
    // On bridge vers l'async tokio via un canal unbounded.
    let (notify_tx, mut notify_rx) = mpsc::unbounded_channel::<PathBuf>();

    // ── Créer le watcher ──
    // Le callback est appelé par notify sur son thread interne
    // à chaque événement filesystem.
    let mut watcher = RecommendedWatcher::new(
        move |result: Result<Event, notify::Error>| {
            if let Ok(event) = result {
                match event.kind {
                    // Nouveau fichier créé OU fichier existant modifié
                    EventKind::Create(_) | EventKind::Modify(_) => {
                        for path in event.paths {
                            if is_valid_jpeg(&path) {
                                // Envoyer le chemin vers la tâche async
                                // (unbounded → ne bloque jamais)
                                let _ = notify_tx.send(path);
                            }
                        }
                    }
                    _ => {} // Ignorer les autres événements (Delete, Access, etc.)
                }
            }
        },
        Config::default(),
    )
    .map_err(|e| format!("Erreur création watcher: {}", e))?;

    // ── Démarrer la surveillance du dossier ──
    let mode = if recursive {
        RecursiveMode::Recursive
    } else {
        RecursiveMode::NonRecursive
    };

    watcher
        .watch(folder.as_ref(), mode)
        .map_err(|e| format!("Erreur démarrage surveillance de {}: {}", folder.display(), e))?;

    info!(
        "Surveillance démarrée: {} (récursif: {})",
        folder.display(),
        recursive
    );

    // ── Tâche async de traitement ──
    // Elle tourne en boucle, reçoit les chemins du watcher,
    // et les traite (stabilité, déduplication, envoi à l'uploader).
    // Note : pendant une PAUSE, cette tâche continue de tourner.
    // Les fichiers sont détectés et inscrits en base même pendant la pause.
    // C'est l'uploader qui s'arrête, pas le watcher.
    let tx_processor = tx.clone();
    let running_processor = running.clone();
    let app_processor = app_handle.clone();

    // Chaque fichier est vérifié dans sa propre tâche (0.3.1), au plus 16 à
    // la fois : une photo n'attend plus la fin des 500 ms de la précédente.
    let places = Arc::new(tokio::sync::Semaphore::new(VERIFICATIONS_SIMULTANEES));

    // Fichiers en cours de vérification. Windows envoie une création puis
    // plusieurs modifications pour une même photo : sans ce registre, chacune
    // relançait une vérification complète. La valeur dit si un nouvel
    // événement est arrivé pendant la vérification — auquel cas, si le
    // fichier était encore instable, on le revoit.
    let en_cours: Arc<std::sync::Mutex<HashMap<PathBuf, bool>>> =
        Arc::new(std::sync::Mutex::new(HashMap::new()));

    tokio::spawn(async move {
        while let Some(path) = notify_rx.recv().await {
            // Si l'arrêt a été demandé, on sort de la boucle
            if !running_processor.load(Ordering::Relaxed) {
                break;
            }

            {
                let mut registre = en_cours.lock().unwrap_or_else(|e| e.into_inner());

                if let Some(a_revoir) = registre.get_mut(&path) {
                    *a_revoir = true;
                    continue;
                }

                registre.insert(path.clone(), false);
            }

            let Ok(place) = places.clone().acquire_owned().await else {
                break;
            };

            let tx_tache = tx_processor.clone();
            let app_tache = app_processor.clone();
            let en_cours_tache = en_cours.clone();
            let running_tache = running_processor.clone();

            tokio::spawn(async move {
                let _place = place;

                loop {
                    if !running_tache.load(Ordering::Relaxed) {
                        break;
                    }

                    // Traiter le fichier (stabilité + dédup + envoi)
                    let traite =
                        process_detected_file(path.clone(), session_id, &tx_tache, &app_tache)
                            .await;

                    let mut registre = en_cours_tache.lock().unwrap_or_else(|e| e.into_inner());

                    let a_revoir = registre.get(&path).copied().unwrap_or(false);

                    if !traite && a_revoir {
                        registre.insert(path.clone(), false);
                        continue;
                    }

                    registre.remove(&path);
                    break;
                }
            });
        }
        debug!("Tâche de traitement des événements watcher terminée");
    });

    // ── Scanner les fichiers existants si demandé ──
    // Lance un scan en tâche de fond (ne bloque pas le démarrage)
    if include_existing {
        let tx_scan = tx.clone();
        let folder_scan = folder.clone();
        let running_scan = running.clone();
        let app_scan = app_handle.clone();

        tokio::spawn(async move {
            scan_existing_files(
                &folder_scan,
                recursive,
                session_id,
                &tx_scan,
                &app_scan,
                &running_scan,
            )
            .await;
        });
    }

    Ok(watcher)
}
