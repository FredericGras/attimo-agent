// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — File d'attente vidéo (video_queue.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// SAAS 430 — Contrat d'interface v1.3, §6.
//
// ─── POURQUOI UNE FILE PERSISTANTE ─────────────────────────────────────
//
// Le photographe décide sur place si la vidéo part pendant la course. Sur
// une antenne saturée par le public, envoyer 33 Mb/s de clips pleine qualité
// ferait décrocher l'envoi des photos — qui, elles, se vendent tout de suite.
//
// Quand il coupe l'envoi vidéo, les fichiers doivent donc ATTENDRE. Et
// attendre vraiment : si l'agent est fermé, si le portable se met en veille,
// si Windows redémarre pour une mise à jour, rien ne doit être perdu.
//
// D'où une file sur disque plutôt qu'en mémoire. Le coût est négligeable,
// la garantie considérable : un photographe qui rentre chez lui retrouve ses
// clips en attente, et il lui suffit de rouvrir l'agent.
//
// ─── PRIORITÉS ─────────────────────────────────────────────────────────
//
// Trois natures de fichiers, trois urgences :
//
//   images d'analyse   1,3 Mb/s   sans elles, aucun dossard n'est lu, donc
//                                 aucune vidéo n'est trouvable
//   clips légers       2 Mb/s     ce que le coureur regarde et commande
//   clips HD          33 Mb/s     ce qu'il télécharge — peut attendre le soir
//
// L'ordre d'envoi suit cette logique. Un clip HD n'a aucune raison de passer
// devant une image d'analyse : il pèse vingt-cinq fois plus pour un besoin
// qui n'est pas urgent.

use rusqlite::{params, Connection};
use serde::{Deserialize, Serialize};

// ═══════════════════════════════════════════════════════════════════════
// TYPES
// ═══════════════════════════════════════════════════════════════════════

/// Nature d'un élément en attente.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum QueueKind {
    /// Images d'analyse — le flux le plus léger, et le plus important.
    Frames,
    /// Clip léger — prévisualisation.
    ClipProxy,
    /// Clip pleine qualité — le produit vendu.
    ClipHd,
}

impl QueueKind {
    fn as_str(&self) -> &'static str {
        match self {
            QueueKind::Frames => "frames",
            QueueKind::ClipProxy => "clip_proxy",
            QueueKind::ClipHd => "clip_hd",
        }
    }

    fn from_str(texte: &str) -> Self {
        match texte {
            "clip_proxy" => QueueKind::ClipProxy,
            "clip_hd" => QueueKind::ClipHd,
            _ => QueueKind::Frames,
        }
    }
}

/// Un élément en attente d'envoi.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct QueueItem {
    pub id: i64,
    pub session_id: String,
    pub kind: QueueKind,
    pub file_path: String,
    pub clip_index: Option<u32>,
    pub started_at: Option<String>,
    pub ended_at: Option<String>,
    /// Horodatage de l'image, pour les éléments de type images d'analyse.
    pub instant_at: Option<String>,
    pub size_bytes: i64,
    pub attempts: i64,
    pub last_error: Option<String>,
}

/// Ce qui attend, par nature.
#[derive(Debug, Clone, Default, Serialize)]
pub struct QueueStats {
    pub frames_pending: i64,
    pub proxy_pending: i64,
    pub hd_pending: i64,
    pub total_bytes: i64,
    pub failed: i64,
}

// ═══════════════════════════════════════════════════════════════════════
// SCHÉMA
// ═══════════════════════════════════════════════════════════════════════

/// Crée la table si elle n'existe pas.
///
/// Table distincte de celle des photos : les colonnes diffèrent trop —
/// horodatages de clip, numéro dans la session, variante — pour que le
/// partage soit un gain.
pub fn init_schema(conn: &Connection) -> Result<(), String> {
    conn.execute_batch(
        "
        CREATE TABLE IF NOT EXISTS video_queue (
            id           INTEGER PRIMARY KEY AUTOINCREMENT,
            session_id   TEXT NOT NULL,
            event_id     INTEGER,
            kind         TEXT NOT NULL,
            file_path    TEXT NOT NULL,
            clip_index   INTEGER,
            started_at   TEXT,
            ended_at     TEXT,
            instant_at   TEXT,
            size_bytes   INTEGER NOT NULL DEFAULT 0,
            status       TEXT NOT NULL DEFAULT 'pending',
            attempts     INTEGER NOT NULL DEFAULT 0,
            last_error   TEXT,
            queued_at    TEXT NOT NULL DEFAULT (datetime('now')),
            sent_at      TEXT,
            UNIQUE(session_id, kind, file_path)
        );

        CREATE INDEX IF NOT EXISTS idx_video_queue_status
            ON video_queue(status, kind);

        CREATE INDEX IF NOT EXISTS idx_video_queue_session
            ON video_queue(session_id);
        ",
    )
    .map_err(|e| format!("Création de la file vidéo impossible : {}", e))?;

    // Rattrapage des bases antérieures.
    //
    // Sans l'événement d'origine, un clip resté en attente partirait vers
    // l'événement ouvert le lendemain — des vidéos livrées à la mauvaise
    // course, sans moyen de s'en apercevoir.
    //
    // L'erreur est ignorée : elle signifie que la colonne existe déjà.
    let _ = conn.execute("ALTER TABLE video_queue ADD COLUMN event_id INTEGER", []);

    Ok(())
}

// ═══════════════════════════════════════════════════════════════════════
// AJOUT
// ═══════════════════════════════════════════════════════════════════════

/// Met un fichier en attente d'envoi.
///
/// L'unicité porte sur (session, nature, chemin) : réinscrire le même
/// fichier ne crée pas de doublon. Utile lors d'une reprise, où l'on
/// préfère tout réinscrire plutôt que de tenir un état parallèle.
pub fn enfiler(
    conn: &Connection,
    session_id: &str,
    event_id: i64,
    kind: QueueKind,
    file_path: &str,
    clip_index: Option<u32>,
    started_at: Option<&str>,
    ended_at: Option<&str>,
    instant_at: Option<&str>,
) -> Result<i64, String> {
    let taille = std::fs::metadata(file_path)
        .map(|m| m.len() as i64)
        .unwrap_or(0);

    conn.execute(
        "INSERT OR IGNORE INTO video_queue
            (session_id, event_id, kind, file_path, clip_index, started_at, ended_at, instant_at, size_bytes)
         VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
        params![
            session_id,
            event_id,
            kind.as_str(),
            file_path,
            clip_index,
            started_at,
            ended_at,
            instant_at,
            taille
        ],
    )
    .map_err(|e| format!("Mise en file impossible : {}", e))?;

    Ok(conn.last_insert_rowid())
}

// ═══════════════════════════════════════════════════════════════════════
// LECTURE
// ═══════════════════════════════════════════════════════════════════════

/// Renvoie les prochains éléments à envoyer, dans l'ordre de priorité.
///
/// `inclure_hd` reflète le toggle du photographe. Quand il est éteint, les
/// clips pleine qualité restent en file sans être proposés : ils partiront
/// au retour, sur un réseau qui les supporte.
pub fn prochains(
    conn: &Connection,
    event_id: i64,
    inclure_hd: bool,
    limite: usize,
) -> Result<Vec<QueueItem>, String> {
    // L'ordre suit la priorité puis l'ancienneté : à nature égale, le plus
    // ancien part en premier, ce qui évite qu'un élément reste indéfiniment
    // au fond de la file.
    //
    // Le filtre par événement empêche qu'un clip d'hier parte vers la course
    // ouverte aujourd'hui. Les lignes sans événement viennent d'une base
    // antérieure au correctif : on les rattache à l'événement courant, ce
    // qui reproduit le comportement d'avant sans rien perdre.
    let requete = if inclure_hd {
        "SELECT id, session_id, kind, file_path, clip_index, started_at, ended_at,
                instant_at, size_bytes, attempts, last_error
         FROM video_queue
         WHERE status = 'pending' AND (event_id = ?1 OR event_id IS NULL)
         ORDER BY CASE kind
                    WHEN 'frames' THEN 1
                    WHEN 'clip_proxy' THEN 2
                    ELSE 3
                  END,
                  id
         LIMIT ?2"
    } else {
        "SELECT id, session_id, kind, file_path, clip_index, started_at, ended_at,
                instant_at, size_bytes, attempts, last_error
         FROM video_queue
         WHERE status = 'pending' AND kind != 'clip_hd'
           AND (event_id = ?1 OR event_id IS NULL)
         ORDER BY CASE kind
                    WHEN 'frames' THEN 1
                    ELSE 2
                  END,
                  id
         LIMIT ?2"
    };

    let mut requete = conn
        .prepare(requete)
        .map_err(|e| format!("Lecture de la file impossible : {}", e))?;

    let lignes = requete
        .query_map(params![event_id, limite as i64], |ligne| {
            Ok(QueueItem {
                id: ligne.get(0)?,
                session_id: ligne.get(1)?,
                kind: QueueKind::from_str(&ligne.get::<_, String>(2)?),
                file_path: ligne.get(3)?,
                clip_index: ligne.get::<_, Option<i64>>(4)?.map(|v| v as u32),
                started_at: ligne.get(5)?,
                ended_at: ligne.get(6)?,
                instant_at: ligne.get(7)?,
                size_bytes: ligne.get(8)?,
                attempts: ligne.get(9)?,
                last_error: ligne.get(10)?,
            })
        })
        .map_err(|e| format!("Lecture de la file impossible : {}", e))?;

    let mut elements = Vec::new();

    for ligne in lignes {
        elements.push(ligne.map_err(|e| format!("Lecture d'une ligne impossible : {}", e))?);
    }

    Ok(elements)
}

/// Ce qui attend, par nature.
///
/// Sert à l'affichage : le photographe doit voir ce qui reste avant de
/// décider d'activer l'envoi vidéo. Sans ce chiffre, il bascule à l'aveugle.
pub fn statistiques(conn: &Connection, event_id: i64) -> Result<QueueStats, String> {
    let mut stats = QueueStats::default();

    let mut requete = conn
        .prepare(
            "SELECT kind, COUNT(*), COALESCE(SUM(size_bytes), 0)
             FROM video_queue
             WHERE status = 'pending' AND (event_id = ?1 OR event_id IS NULL)
             GROUP BY kind",
        )
        .map_err(|e| format!("Statistiques impossibles : {}", e))?;

    let lignes = requete
        .query_map(params![event_id], |ligne| {
            Ok((
                ligne.get::<_, String>(0)?,
                ligne.get::<_, i64>(1)?,
                ligne.get::<_, i64>(2)?,
            ))
        })
        .map_err(|e| format!("Statistiques impossibles : {}", e))?;

    for ligne in lignes {
        let (nature, nombre, octets) =
            ligne.map_err(|e| format!("Lecture impossible : {}", e))?;

        match QueueKind::from_str(&nature) {
            QueueKind::Frames => stats.frames_pending = nombre,
            QueueKind::ClipProxy => stats.proxy_pending = nombre,
            QueueKind::ClipHd => stats.hd_pending = nombre,
        }

        stats.total_bytes += octets;
    }

    stats.failed = conn
        .query_row(
            "SELECT COUNT(*) FROM video_queue
             WHERE status = 'failed' AND (event_id = ?1 OR event_id IS NULL)",
            params![event_id],
            |l| l.get(0),
        )
        .unwrap_or(0);

    Ok(stats)
}

// ═══════════════════════════════════════════════════════════════════════
// MISE À JOUR
// ═══════════════════════════════════════════════════════════════════════

/// Réserve un élément avant de l'envoyer.
///
/// Sans cette réservation, l'ouvrier qui tourne chaque seconde reprendrait
/// un élément dont l'envoi est déjà en cours : un clip de 500 Mo qui met
/// trente secondes à monter serait relancé trente fois.
///
/// Le passage en « sending » est ATOMIQUE : la condition sur le statut fait
/// partie de la mise à jour. Si deux appels se présentent en même temps, un
/// seul modifie une ligne, l'autre en modifie zéro et repart.
pub fn reserver(conn: &Connection, id: i64) -> Result<bool, String> {
    let modifiees = conn
        .execute(
            "UPDATE video_queue
             SET status = 'sending'
             WHERE id = ?1 AND status = 'pending'",
            params![id],
        )
        .map_err(|e| format!("Réservation impossible : {}", e))?;

    Ok(modifiees == 1)
}

/// Remet en file un élément réservé dont l'envoi a échoué.
///
/// Distinct de l'échec définitif : ici on rend simplement l'élément
/// disponible pour une nouvelle tentative.
pub fn liberer(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE video_queue SET status = 'pending' WHERE id = ?1 AND status = 'sending'",
        params![id],
    )
    .map_err(|e| format!("Libération impossible : {}", e))?;

    Ok(())
}

/// Libère les éléments restés réservés au démarrage.
///
/// Si l'agent a été fermé pendant un envoi, des éléments sont figés en
/// « sending » et ne repartiraient jamais. On les remet en file au
/// lancement : au pire, un fichier est envoyé deux fois, ce qui est sans
/// conséquence côté serveur.
pub fn liberer_orphelins(conn: &Connection) -> Result<usize, String> {
    let nombre = conn
        .execute(
            "UPDATE video_queue SET status = 'pending' WHERE status = 'sending'",
            [],
        )
        .map_err(|e| format!("Libération impossible : {}", e))?;

    Ok(nombre)
}

/// Marque un élément comme envoyé.
pub fn marquer_envoye(conn: &Connection, id: i64) -> Result<(), String> {
    conn.execute(
        "UPDATE video_queue
         SET status = 'sent', sent_at = datetime('now'), last_error = NULL
         WHERE id = ?1",
        params![id],
    )
    .map_err(|e| format!("Mise à jour impossible : {}", e))?;

    Ok(())
}

/// Enregistre un échec.
///
/// Après trois tentatives, l'élément passe en échec définitif plutôt que de
/// bloquer la file. Le photographe peut le relancer manuellement — mieux
/// vaut un fichier signalé qu'une file figée sur un cas insoluble.
pub fn marquer_echec(conn: &Connection, id: i64, erreur: &str) -> Result<bool, String> {
    // L'élément repasse en attente : il était réservé le temps de l'envoi.
    conn.execute(
        "UPDATE video_queue
         SET attempts = attempts + 1, last_error = ?2, status = 'pending'
         WHERE id = ?1",
        params![id, erreur],
    )
    .map_err(|e| format!("Mise à jour impossible : {}", e))?;

    let tentatives: i64 = conn
        .query_row(
            "SELECT attempts FROM video_queue WHERE id = ?1",
            params![id],
            |l| l.get(0),
        )
        .unwrap_or(0);

    if tentatives >= 3 {
        conn.execute(
            "UPDATE video_queue SET status = 'failed' WHERE id = ?1",
            params![id],
        )
        .map_err(|e| format!("Mise à jour impossible : {}", e))?;

        return Ok(true);
    }

    Ok(false)
}

/// Remet les éléments en échec dans la file.
pub fn relancer_echecs(conn: &Connection) -> Result<usize, String> {
    let nombre = conn
        .execute(
            "UPDATE video_queue
             SET status = 'pending', attempts = 0, last_error = NULL
             WHERE status = 'failed'",
            [],
        )
        .map_err(|e| format!("Relance impossible : {}", e))?;

    Ok(nombre)
}

/// Purge les éléments envoyés d'une session.
///
/// Ne touche QUE la file : les fichiers restent sur le disque. Le contrat
/// interdit toute suppression automatique sur le poste du photographe — un
/// fichier effacé tout seul, c'est potentiellement une vente perdue sans
/// recours.
///
/// Conservée sans appelant : c'est le SEUL effacement de `video_queue` de
/// tout le code. Sans elle, les lignes `sent` s'accumulent course après
/// course. Elle attend son point d'appel — la brancher est un changement de
/// comportement, décidé à part.
#[allow(dead_code)]
pub fn purger_envoyes(conn: &Connection, session_id: &str) -> Result<usize, String> {
    let nombre = conn
        .execute(
            "DELETE FROM video_queue WHERE session_id = ?1 AND status = 'sent'",
            params![session_id],
        )
        .map_err(|e| format!("Purge impossible : {}", e))?;

    Ok(nombre)
}

// ═══════════════════════════════════════════════════════════════════════
// REPRISE APRÈS COUPURE
// ═══════════════════════════════════════════════════════════════════════

/// Ce qu'une réconciliation a évité d'envoyer.
#[derive(Debug, Clone, Default, Serialize)]
pub struct Reconciliation {
    /// Nombre de fichiers écartés parce que le serveur les détient déjà.
    pub skipped: usize,

    /// Octets que l'on n'aura pas à réémettre.
    pub bytes_saved: i64,
}

/// Aligne la file locale sur ce que le serveur possède réellement.
///
/// La file survit à tout — fermeture de l'agent, veille, redémarrage — et
/// c'est sa raison d'être. Mais cette mémoire est aveugle : elle sait ce
/// qu'elle a tenté d'envoyer, pas ce qui est arrivé à destination. Un envoi
/// coupé au dernier octet est indiscernable d'un envoi jamais commencé.
///
/// Sans ce recoupement, un photographe qui rouvre son agent le soir
/// reposterait l'intégralité de sa course. Avec, il n'envoie que ce qui
/// manque.
///
/// Les images d'analyse ne sont pas concernées : elles n'ont pas de numéro
/// de clip, et leur poids ne justifie pas un mécanisme de plus.
pub fn reconcilier(
    conn: &Connection,
    session_id: &str,
    deja_recus: &[(u32, bool, bool)],
) -> Result<Reconciliation, String> {
    let mut bilan = Reconciliation::default();

    for (index, a_proxy, a_hd) in deja_recus {
        if *a_proxy {
            let (nombre, octets) = ecarter(conn, session_id, QueueKind::ClipProxy, *index)?;
            bilan.skipped += nombre;
            bilan.bytes_saved += octets;
        }

        if *a_hd {
            let (nombre, octets) = ecarter(conn, session_id, QueueKind::ClipHd, *index)?;
            bilan.skipped += nombre;
            bilan.bytes_saved += octets;
        }
    }

    Ok(bilan)
}

/// Sort de la file un fichier que le serveur détient déjà.
///
/// Il est marqué comme envoyé, pas supprimé : la file garde ainsi la trace
/// complète de la session, et le fichier reste sur le disque du photographe.
///
/// Les éléments réservés — un envoi en cours à cet instant précis — sont
/// laissés tranquilles : le passage qui les traite doit conclure lui-même.
fn ecarter(
    conn: &Connection,
    session_id: &str,
    kind: QueueKind,
    clip_index: u32,
) -> Result<(usize, i64), String> {
    let octets: i64 = conn
        .query_row(
            "SELECT COALESCE(SUM(size_bytes), 0) FROM video_queue
             WHERE session_id = ?1 AND kind = ?2 AND clip_index = ?3
               AND status IN ('pending', 'failed')",
            params![session_id, kind.as_str(), clip_index],
            |ligne| ligne.get(0),
        )
        .unwrap_or(0);

    let nombre = conn
        .execute(
            "UPDATE video_queue
             SET status = 'sent', sent_at = datetime('now'), last_error = NULL
             WHERE session_id = ?1 AND kind = ?2 AND clip_index = ?3
               AND status IN ('pending', 'failed')",
            params![session_id, kind.as_str(), clip_index],
        )
        .map_err(|e| format!("Réconciliation impossible : {}", e))?;

    Ok((nombre, octets))
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn base_de_test() -> Connection {
        let conn = Connection::open_in_memory().unwrap();
        init_schema(&conn).unwrap();
        conn
    }

    fn enfiler_simple(conn: &Connection, kind: QueueKind, chemin: &str) -> i64 {
        enfiler(conn, "session_1", kind, chemin, Some(1), None, None, None).unwrap()
    }

    #[test]
    fn les_images_passent_avant_les_clips() {
        let conn = base_de_test();

        // Ajoutés dans l'ordre inverse de la priorité.
        enfiler_simple(&conn, QueueKind::ClipHd, "hd.mp4");
        enfiler_simple(&conn, QueueKind::ClipProxy, "proxy.mp4");
        enfiler_simple(&conn, QueueKind::Frames, "img.jpg");

        let file = prochains(&conn, true, 10).unwrap();

        assert_eq!(file[0].kind, QueueKind::Frames);
        assert_eq!(file[1].kind, QueueKind::ClipProxy);
        assert_eq!(file[2].kind, QueueKind::ClipHd);
    }

    #[test]
    fn le_toggle_eteint_retient_la_pleine_qualite() {
        let conn = base_de_test();

        enfiler_simple(&conn, QueueKind::Frames, "img.jpg");
        enfiler_simple(&conn, QueueKind::ClipProxy, "proxy.mp4");
        enfiler_simple(&conn, QueueKind::ClipHd, "hd.mp4");

        let file = prochains(&conn, false, 10).unwrap();

        assert_eq!(file.len(), 2);
        assert!(file.iter().all(|e| e.kind != QueueKind::ClipHd));

        // Elle n'est pas perdue pour autant : elle reste en attente.
        let stats = statistiques(&conn).unwrap();
        assert_eq!(stats.hd_pending, 1);
    }

    #[test]
    fn le_meme_fichier_nest_pas_enfile_deux_fois() {
        let conn = base_de_test();

        enfiler_simple(&conn, QueueKind::ClipHd, "clip.mp4");
        enfiler_simple(&conn, QueueKind::ClipHd, "clip.mp4");

        let file = prochains(&conn, true, 10).unwrap();

        assert_eq!(file.len(), 1);
    }

    #[test]
    fn un_element_envoye_quitte_la_file() {
        let conn = base_de_test();

        let id = enfiler_simple(&conn, QueueKind::Frames, "img.jpg");
        marquer_envoye(&conn, id).unwrap();

        assert_eq!(prochains(&conn, true, 10).unwrap().len(), 0);
    }

    #[test]
    fn trois_echecs_sortent_lelement_de_la_file() {
        let conn = base_de_test();

        let id = enfiler_simple(&conn, QueueKind::ClipHd, "clip.mp4");

        assert!(!marquer_echec(&conn, id, "réseau").unwrap());
        assert!(!marquer_echec(&conn, id, "réseau").unwrap());

        // Le troisième échec est définitif : mieux vaut un fichier signalé
        // qu'une file bloquée sur un cas insoluble.
        assert!(marquer_echec(&conn, id, "réseau").unwrap());

        assert_eq!(prochains(&conn, true, 10).unwrap().len(), 0);
        assert_eq!(statistiques(&conn).unwrap().failed, 1);
    }

    #[test]
    fn les_echecs_peuvent_etre_relances() {
        let conn = base_de_test();

        let id = enfiler_simple(&conn, QueueKind::ClipHd, "clip.mp4");

        for _ in 0..3 {
            let _ = marquer_echec(&conn, id, "réseau");
        }

        assert_eq!(relancer_echecs(&conn).unwrap(), 1);
        assert_eq!(prochains(&conn, true, 10).unwrap().len(), 1);
    }

    #[test]
    fn les_statistiques_distinguent_les_natures() {
        let conn = base_de_test();

        enfiler_simple(&conn, QueueKind::Frames, "a.jpg");
        enfiler_simple(&conn, QueueKind::Frames, "b.jpg");
        enfiler_simple(&conn, QueueKind::ClipProxy, "p.mp4");
        enfiler_simple(&conn, QueueKind::ClipHd, "h1.mp4");
        enfiler_simple(&conn, QueueKind::ClipHd, "h2.mp4");

        let stats = statistiques(&conn).unwrap();

        assert_eq!(stats.frames_pending, 2);
        assert_eq!(stats.proxy_pending, 1);
        assert_eq!(stats.hd_pending, 2);
    }

    #[test]
    fn la_purge_ne_touche_que_les_envoyes() {
        let conn = base_de_test();

        let envoye = enfiler_simple(&conn, QueueKind::Frames, "a.jpg");
        enfiler_simple(&conn, QueueKind::ClipHd, "b.mp4");

        marquer_envoye(&conn, envoye).unwrap();

        assert_eq!(purger_envoyes(&conn, "session_1").unwrap(), 1);
        assert_eq!(prochains(&conn, true, 10).unwrap().len(), 1);
    }

    #[test]
    fn la_reprise_ecarte_ce_que_le_serveur_detient() {
        let conn = base_de_test();

        enfiler(&conn, "session_1", EVT, QueueKind::ClipProxy, "p1.mp4", Some(1), None, None, None).unwrap();
        enfiler(&conn, "session_1", EVT, QueueKind::ClipHd, "h1.mp4", Some(1), None, None, None).unwrap();
        enfiler(&conn, "session_1", EVT, QueueKind::ClipProxy, "p2.mp4", Some(2), None, None, None).unwrap();

        // Le serveur a le clip 1 en entier, et rien du clip 2.
        let bilan = reconcilier(&conn, "session_1", &[(1, true, true)]).unwrap();

        assert_eq!(bilan.skipped, 2);

        let reste = prochains(&conn, EVT, true, 10).unwrap();
        assert_eq!(reste.len(), 1);
        assert_eq!(reste[0].clip_index, Some(2));
    }

    #[test]
    fn la_reprise_distingue_les_deux_variantes() {
        let conn = base_de_test();

        enfiler(&conn, "session_1", EVT, QueueKind::ClipProxy, "p1.mp4", Some(1), None, None, None).unwrap();
        enfiler(&conn, "session_1", EVT, QueueKind::ClipHd, "h1.mp4", Some(1), None, None, None).unwrap();

        // Cas courant : la version légère est passée pendant la course, la
        // pleine qualité attendait le retour.
        let bilan = reconcilier(&conn, "session_1", &[(1, true, false)]).unwrap();

        assert_eq!(bilan.skipped, 1);

        let reste = prochains(&conn, EVT, true, 10).unwrap();
        assert_eq!(reste.len(), 1);
        assert_eq!(reste[0].kind, QueueKind::ClipHd);
    }

    #[test]
    fn la_reprise_ne_touche_pas_les_autres_sessions() {
        let conn = base_de_test();

        enfiler(&conn, "session_1", EVT, QueueKind::ClipHd, "a.mp4", Some(1), None, None, None).unwrap();
        enfiler(&conn, "session_2", EVT, QueueKind::ClipHd, "b.mp4", Some(1), None, None, None).unwrap();

        reconcilier(&conn, "session_1", &[(1, true, true)]).unwrap();

        let reste = prochains(&conn, EVT, true, 10).unwrap();
        assert_eq!(reste.len(), 1);
        assert_eq!(reste[0].session_id, "session_2");
    }

    #[test]
    fn la_reprise_recupere_aussi_les_abandonnes() {
        let conn = base_de_test();

        let id = enfiler(&conn, "session_1", EVT, QueueKind::ClipHd, "a.mp4", Some(1), None, None, None).unwrap();

        // Trois échecs réseau l'ont sorti de la file. Mais le serveur l'avait
        // bel et bien reçu : il ne doit pas rester marqué en échec.
        for _ in 0..3 {
            let _ = marquer_echec(&conn, id, "réseau");
        }

        assert_eq!(statistiques(&conn, EVT).unwrap().failed, 1);

        reconcilier(&conn, "session_1", &[(1, false, true)]).unwrap();

        assert_eq!(statistiques(&conn, EVT).unwrap().failed, 0);
    }

    #[test]
    fn un_serveur_vide_ne_retire_rien() {
        let conn = base_de_test();

        enfiler_simple(&conn, QueueKind::ClipHd, "a.mp4");

        let bilan = reconcilier(&conn, "session_1", &[]).unwrap();

        assert_eq!(bilan.skipped, 0);
        assert_eq!(prochains(&conn, EVT, true, 10).unwrap().len(), 1);
    }
}
