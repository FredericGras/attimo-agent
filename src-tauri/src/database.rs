use rusqlite::{Connection, Result, params};
use std::path::PathBuf;
use std::fs;
use serde::Serialize;

/// Retourne le chemin vers la base de données SQLite locale.
/// Stockée dans le dossier AppData de l'utilisateur (Windows: %APPDATA%)
fn db_path() -> PathBuf {
    let mut path = dirs();
    path.push("attimo-agent.db");
    path
}

/// Édition de l'agent, fixée à la COMPILATION (0.3.1).
///
/// Absente pour la production, `dev` pour l'agent de préproduction
/// (build-preprod.bat) : il s'installe à côté de celui de production et doit
/// garder sa propre mémoire — sessions, file vidéo, mot de passe
/// d'application. Sans cela, l'agent de DEV lirait la file de l'agent de
/// production et enverrait ses clips au serveur de préproduction.
pub const EDITION: Option<&str> = option_env!("ATTIMO_EDITION");

/// Nom du dossier de données dans %APPDATA%, calculé par build.rs à partir
/// de l'édition. Celui de la production ne change pas : une mise à jour
/// retrouve ses données.
///
/// Le repli ne sert qu'à rustdoc, qui ne reçoit pas les variables posées par
/// build.rs : toute vraie compilation passe par build.rs, et build-preprod.bat
/// vérifie que le dossier de DEV figure bien dans le binaire.
pub const DOSSIER_DONNEES: &str = match option_env!("ATTIMO_DATA_DIR") {
    Some(dossier) => dossier,
    None => "com.attimo-gallery.agent",
};

/// Résout le dossier de données de l'application
pub fn dirs() -> PathBuf {
    if let Some(data_dir) = std::env::var_os("APPDATA") {
        let mut path = PathBuf::from(data_dir);
        path.push(DOSSIER_DONNEES);
        fs::create_dir_all(&path).ok();
        path
    } else {
        let mut path = std::env::current_dir().unwrap_or_default();
        path.push("data");
        fs::create_dir_all(&path).ok();
        path
    }
}

/// Ouvre (ou crée) la base SQLite et retourne la connexion
pub fn open_db() -> Result<Connection> {
    let path = db_path();
    let conn = Connection::open(path)?;

    // Activer le WAL pour de meilleures performances en écriture concurrente
    conn.execute_batch("PRAGMA journal_mode=WAL;")?;

    Ok(conn)
}

/// Ouvre une connexion à la base locale.
///
/// Exposée pour que le module de file vidéo puisse s'y greffer : une seule
/// base pour tout l'agent, plutôt qu'un second fichier à gérer.
pub fn connexion() -> Result<Connection> {
    open_db()
}

/// Crée les tables si elles n'existent pas encore
pub fn init_db() -> Result<()> {
    let conn = open_db()?;

    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS upload_sessions (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            event_id        INTEGER NOT NULL,
            event_name      TEXT NOT NULL DEFAULT '',
            checkpoint_id   INTEGER,
            checkpoint_name TEXT,
            watch_folder    TEXT NOT NULL,
            include_existing INTEGER NOT NULL DEFAULT 0,
            parallel_uploads INTEGER NOT NULL DEFAULT 2,
            started_at      TEXT NOT NULL DEFAULT (datetime('now')),
            stopped_at      TEXT,
            photos_sent     INTEGER NOT NULL DEFAULT 0,
            photos_failed   INTEGER NOT NULL DEFAULT 0
        );

        CREATE TABLE IF NOT EXISTS upload_files (
            id              INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id      INTEGER NOT NULL REFERENCES upload_sessions(id),
            filename        TEXT NOT NULL,
            file_path       TEXT NOT NULL,
            file_size       INTEGER NOT NULL DEFAULT 0,
            file_modified   TEXT NOT NULL DEFAULT '',
            status          TEXT NOT NULL DEFAULT 'pending',
            attempts        INTEGER NOT NULL DEFAULT 0,
            last_error      TEXT,
            uploaded_at     TEXT,
            server_photo_id INTEGER,
            UNIQUE(session_id, filename)
        );

        CREATE INDEX IF NOT EXISTS idx_upload_files_session
            ON upload_files(session_id);

        CREATE INDEX IF NOT EXISTS idx_upload_files_status
            ON upload_files(session_id, status);

        -- 0.3.2 : empreinte des photos déjà en ligne, par épreuve. Une photo
        -- dont l'empreinte est connue ne repart jamais, quelle que soit la
        -- session ou le dossier d'où elle vient.
        CREATE TABLE IF NOT EXISTS photos_envoyees (
            event_id        INTEGER NOT NULL,
            empreinte       TEXT NOT NULL,
            taille          INTEGER NOT NULL DEFAULT 0,
            nom             TEXT NOT NULL DEFAULT '',
            server_photo_id INTEGER,
            envoyee_le      TEXT NOT NULL DEFAULT (datetime('now')),
            PRIMARY KEY (event_id, empreinte)
        );
        "
    )?;

    Ok(())
}

// ─── Structures sérialisables pour le frontend ───

#[derive(Debug, Serialize, Clone)]
pub struct UploadFile {
    pub id: i64,
    pub session_id: i64,
    pub filename: String,
    pub file_path: String,
    pub file_size: i64,
    pub status: String,
    pub attempts: i64,
    pub last_error: Option<String>,
    pub uploaded_at: Option<String>,
    pub server_photo_id: Option<i64>,
}

// ─── Fonctions CRUD ───

/// Crée une nouvelle session d'upload et retourne son ID
pub fn create_session(
    event_id: i64,
    event_name: &str,
    checkpoint_id: Option<i64>,
    checkpoint_name: Option<&str>,
    watch_folder: &str,
    include_existing: bool,
    parallel_uploads: i32,
) -> Result<i64> {
    let conn = open_db()?;
    conn.execute(
        "INSERT INTO upload_sessions (event_id, event_name, checkpoint_id, checkpoint_name, watch_folder, include_existing, parallel_uploads)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)",
        params![
            event_id,
            event_name,
            checkpoint_id,
            checkpoint_name,
            watch_folder,
            include_existing as i32,
            parallel_uploads,
        ],
    )?;
    Ok(conn.last_insert_rowid())
}

/// Insère un nouveau fichier dans la file d'attente.
/// Retourne Some(id) si inséré, None si le fichier existait déjà (déduplication).
pub fn insert_file(
    session_id: i64,
    filename: &str,
    file_path: &str,
    file_size: i64,
    file_modified: &str,
) -> Result<Option<i64>> {
    let conn = open_db()?;
    conn.execute(
        "INSERT OR IGNORE INTO upload_files (session_id, filename, file_path, file_size, file_modified, status)
         VALUES (?1, ?2, ?3, ?4, ?5, 'pending')",
        params![session_id, filename, file_path, file_size, file_modified],
    )?;
    // Si OR IGNORE a ignoré le doublon, changes() == 0
    if conn.changes() == 0 {
        return Ok(None);
    }
    Ok(Some(conn.last_insert_rowid()))
}

/// Met à jour le statut d'un fichier après upload
pub fn update_file_status(
    file_id: i64,
    status: &str,
    error: Option<&str>,
    server_photo_id: Option<i64>,
) -> Result<()> {
    let conn = open_db()?;
    let uploaded_at = if status == "success" || status == "already" {
        Some(chrono_now())
    } else {
        None
    };

    conn.execute(
        "UPDATE upload_files SET status = ?1, last_error = ?2, server_photo_id = ?3, uploaded_at = ?4, attempts = attempts + 1 WHERE id = ?5",
        params![status, error, server_photo_id, uploaded_at, file_id],
    )?;

    // Mettre à jour les compteurs de la session. Une photo déjà en ligne
    // (0.3.2) compte comme envoyée : elle est sur le serveur.
    if status == "success" || status == "already" {
        conn.execute(
            "UPDATE upload_sessions SET photos_sent = photos_sent + 1 WHERE id = (SELECT session_id FROM upload_files WHERE id = ?1)",
            params![file_id],
        )?;
    } else if status == "failed" {
        conn.execute(
            "UPDATE upload_sessions SET photos_failed = photos_failed + 1 WHERE id = (SELECT session_id FROM upload_files WHERE id = ?1)",
            params![file_id],
        )?;
    }

    Ok(())
}

/// Récupère les statistiques d'une session
#[derive(Debug, Serialize, Clone)]
pub struct SessionStats {
    pub total: i64,
    /// Sur le serveur : envoyées, ou déjà en ligne avant cette session.
    pub sent: i64,
    pub pending: i64,
    pub uploading: i64,
    pub failed: i64,
    /// Dont déjà en ligne, écartées sans être renvoyées (0.3.2).
    pub already: i64,
}

pub fn get_session_stats(session_id: i64) -> Result<SessionStats> {
    let conn = open_db()?;

    let total: i64 = conn.query_row(
        "SELECT COUNT(*) FROM upload_files WHERE session_id = ?1",
        params![session_id], |row| row.get(0),
    )?;
    let sent: i64 = conn.query_row(
        "SELECT COUNT(*) FROM upload_files WHERE session_id = ?1 AND status IN ('success', 'already')",
        params![session_id], |row| row.get(0),
    )?;
    let already: i64 = conn.query_row(
        "SELECT COUNT(*) FROM upload_files WHERE session_id = ?1 AND status = 'already'",
        params![session_id], |row| row.get(0),
    )?;
    let pending: i64 = conn.query_row(
        "SELECT COUNT(*) FROM upload_files WHERE session_id = ?1 AND status = 'pending'",
        params![session_id], |row| row.get(0),
    )?;
    let uploading: i64 = conn.query_row(
        "SELECT COUNT(*) FROM upload_files WHERE session_id = ?1 AND status = 'uploading'",
        params![session_id], |row| row.get(0),
    )?;
    let failed: i64 = conn.query_row(
        "SELECT COUNT(*) FROM upload_files WHERE session_id = ?1 AND status = 'failed'",
        params![session_id], |row| row.get(0),
    )?;

    Ok(SessionStats { total, sent, pending, uploading, failed, already })
}

/// Marque la session comme arrêtée
pub fn stop_session(session_id: i64) -> Result<()> {
    let conn = open_db()?;
    conn.execute(
        "UPDATE upload_sessions SET stopped_at = datetime('now') WHERE id = ?1",
        params![session_id],
    )?;
    Ok(())
}

/// Remet les fichiers échoués en attente pour retry
pub fn reset_failed_files(session_id: i64) -> Result<u64> {
    let conn = open_db()?;
    let count = conn.execute(
        "UPDATE upload_files SET status = 'pending', attempts = 0, last_error = NULL WHERE session_id = ?1 AND status = 'failed'",
        params![session_id],
    )?;
    Ok(count as u64)
}

/// Remet un fichier spécifique en attente pour retry
pub fn reset_file(file_id: i64) -> Result<()> {
    let conn = open_db()?;
    conn.execute(
        "UPDATE upload_files SET status = 'pending', last_error = NULL WHERE id = ?1 AND status = 'failed'",
        params![file_id],
    )?;
    Ok(())
}

/// Récupère les fichiers en échec pour une session (avant reset)
pub fn get_failed_files(session_id: i64) -> Result<Vec<UploadFile>> {
    let conn = open_db()?;
    let mut stmt = conn.prepare(
        "SELECT id, session_id, filename, file_path, file_size, status, attempts, last_error, uploaded_at, server_photo_id
         FROM upload_files WHERE session_id = ?1 AND status = 'failed' ORDER BY id ASC"
    )?;

    let files = stmt.query_map(params![session_id], |row| {
        Ok(UploadFile {
            id: row.get(0)?,
            session_id: row.get(1)?,
            filename: row.get(2)?,
            file_path: row.get(3)?,
            file_size: row.get(4)?,
            status: row.get(5)?,
            attempts: row.get(6)?,
            last_error: row.get(7)?,
            uploaded_at: row.get(8)?,
            server_photo_id: row.get(9)?,
        })
    })?.collect::<Result<Vec<_>>>()?;

    Ok(files)
}

// ─── Empreintes des photos en ligne (0.3.2) ───

/// La photo de cette empreinte est-elle déjà en ligne pour l'épreuve ?
///
/// Renvoie l'identifiant serveur retenu, s'il est connu.
pub fn empreinte_connue(conn: &Connection, event_id: i64, empreinte: &str) -> Result<Option<Option<i64>>> {
    let mut requete = conn.prepare(
        "SELECT server_photo_id FROM photos_envoyees WHERE event_id = ?1 AND empreinte = ?2",
    )?;

    let mut lignes = requete.query(params![event_id, empreinte])?;

    match lignes.next()? {
        Some(ligne) => Ok(Some(ligne.get(0)?)),
        None => Ok(None),
    }
}

/// Oublie une photo que la mémoire locale croyait en ligne (0.3.3) : le
/// serveur dit ne pas l'avoir — supprimée de la galerie.
pub fn oublier_empreinte(conn: &Connection, event_id: i64, empreinte: &str) -> Result<()> {
    conn.execute(
        "DELETE FROM photos_envoyees WHERE event_id = ?1 AND empreinte = ?2",
        params![event_id, empreinte],
    )?;
    Ok(())
}

/// Retient qu'une photo est en ligne pour l'épreuve.
pub fn retenir_empreinte(
    conn: &Connection,
    event_id: i64,
    empreinte: &str,
    taille: i64,
    nom: &str,
    server_photo_id: Option<i64>,
) -> Result<()> {
    conn.execute(
        "INSERT OR IGNORE INTO photos_envoyees (event_id, empreinte, taille, nom, server_photo_id)
         VALUES (?1, ?2, ?3, ?4, ?5)",
        params![event_id, empreinte, taille, nom, server_photo_id],
    )?;
    Ok(())
}

/// Génère un timestamp ISO 8601 courant
fn chrono_now() -> String {
    // Format simplifié sans dépendance chrono
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    // Retourne le timestamp Unix comme fallback — suffisant pour le tri
    format!("{}", now)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn une_empreinte_retenue_est_reconnue_pour_la_meme_epreuve_seulement() {
        let conn = Connection::open_in_memory().unwrap();
        conn.execute_batch(
            "CREATE TABLE photos_envoyees (
                event_id INTEGER NOT NULL, empreinte TEXT NOT NULL,
                taille INTEGER NOT NULL DEFAULT 0, nom TEXT NOT NULL DEFAULT '',
                server_photo_id INTEGER, envoyee_le TEXT NOT NULL DEFAULT (datetime('now')),
                PRIMARY KEY (event_id, empreinte));",
        )
        .unwrap();

        assert_eq!(empreinte_connue(&conn, 7, "abc").unwrap(), None);

        retenir_empreinte(&conn, 7, "abc", 100, "IMG_1.JPG", Some(42)).unwrap();

        // Deux fois la même : sans erreur, sans doublon.
        retenir_empreinte(&conn, 7, "abc", 100, "IMG_1.JPG", Some(42)).unwrap();

        assert_eq!(empreinte_connue(&conn, 7, "abc").unwrap(), Some(Some(42)));

        // Une autre épreuve n'en sait rien : la photo doit y partir.
        assert_eq!(empreinte_connue(&conn, 8, "abc").unwrap(), None);

        // Le serveur dit ne plus l'avoir : la mémoire est corrigée.
        oublier_empreinte(&conn, 7, "abc").unwrap();
        assert_eq!(empreinte_connue(&conn, 7, "abc").unwrap(), None);
    }

    #[test]
    fn la_production_garde_son_dossier_de_donnees() {
        // Compilé sans ATTIMO_EDITION (production, ou test.bat) : le dossier
        // historique, pour qu'une mise à jour retrouve ses sessions.
        match EDITION.map(str::trim).filter(|e| !e.is_empty()) {
            None => assert_eq!(DOSSIER_DONNEES, "com.attimo-gallery.agent"),
            Some(edition) => assert_eq!(
                DOSSIER_DONNEES,
                format!("com.attimo-gallery.agent.{}", edition.to_lowercase())
            ),
        }
    }
}
