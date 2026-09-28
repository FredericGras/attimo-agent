// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Vérification des empreintes auprès du serveur
// (verification.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// 0.3.3 — La mémoire locale des photos envoyées (0.3.2) a une limite : une
// photo supprimée de la galerie n'est jamais renvoyée depuis ce poste, et
// un autre poste, lui, ne sait rien. Le serveur fait foi.
//
// Au scan, avant l'envoi, l'agent demande au serveur lesquelles de ces
// empreintes il détient déjà :
//
//   POST /api/sport/events/{eventId}/hash-check
//   entrée  : {"hashes": ["<md5>", …]}   (2 000 au plus par requête)
//   sortie  : {"success": true, "existing": ["<md5>", …]}
//
// - une photo que le serveur détient ne part pas, même inconnue localement
//   (la mémoire locale est complétée) ;
// - une photo absente du serveur part, même si la mémoire locale la
//   connaît (la mémoire locale est corrigée).
//
// LA ROUTE N'EXISTE PAS ENCORE côté serveur. Sur 404, 405 ou erreur réseau,
// l'agent garde exactement le comportement 0.3.2 — mémoire locale seule —,
// sans rien afficher à l'écran : une seule ligne au journal technique. Une
// route absente n'est plus redemandée de la session.

use std::collections::HashSet;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::time::Duration;

const API_BASE_URL: &str = env!("ATTIMO_API_URL");

/// Empreintes au plus par requête (limite du serveur).
pub const EMPREINTES_PAR_REQUETE: usize = 2000;

/// Une vérification ne doit jamais retarder longtemps l'envoi.
const DELAI_SECS: u64 = 15;

const ROUTE_INCONNUE: u8 = 0;
const ROUTE_DISPONIBLE: u8 = 1;
const ROUTE_ABSENTE: u8 = 2;

/// État de la route pour la session en cours.
static ROUTE: AtomicU8 = AtomicU8::new(ROUTE_INCONNUE);

/// Une ligne au journal technique, pas davantage.
static SIGNALEE: AtomicBool = AtomicBool::new(false);

/// Nouvelle session : la route est redemandée (le serveur a pu être mis à
/// jour entre deux sessions).
pub fn reinitialiser() {
    ROUTE.store(ROUTE_INCONNUE, Ordering::Relaxed);
    SIGNALEE.store(false, Ordering::Relaxed);
}

/// La route est-elle connue pour absente ? Inutile alors de calculer des
/// empreintes pour elle.
pub fn route_absente() -> bool {
    ROUTE.load(Ordering::Relaxed) == ROUTE_ABSENTE
}

/// Pourquoi le serveur n'a pas pu répondre.
#[derive(Debug, PartialEq)]
pub enum Indisponible {
    /// 404 ou 405 : la route n'existe pas (encore).
    RouteAbsente(u16),
    /// Réseau, délai, autre statut, réponse illisible.
    Autre(String),
}

fn client() -> Option<&'static reqwest::Client> {
    static CLIENT: OnceLock<Option<reqwest::Client>> = OnceLock::new();

    CLIENT
        .get_or_init(|| {
            reqwest::Client::builder()
                .timeout(Duration::from_secs(DELAI_SECS))
                .build()
                .ok()
        })
        .as_ref()
}

/// Une requête de vérification, vers un serveur donné (isolé pour les tests).
pub(crate) async fn verifier_sur(
    client: &reqwest::Client,
    base: &str,
    token: &str,
    event_id: i64,
    empreintes: &[String],
) -> Result<HashSet<String>, Indisponible> {
    let url = format!("{}/api/sport/events/{}/hash-check", base, event_id);

    let reponse = client
        .post(&url)
        .header("Accept", "application/json")
        .bearer_auth(token)
        .json(&serde_json::json!({ "hashes": empreintes }))
        .send()
        .await
        .map_err(|e| Indisponible::Autre(format!("réseau : {}", e)))?;

    let statut = reponse.status().as_u16();

    if statut == 404 || statut == 405 {
        return Err(Indisponible::RouteAbsente(statut));
    }

    if !reponse.status().is_success() {
        return Err(Indisponible::Autre(format!("statut {}", statut)));
    }

    let corps: serde_json::Value = reponse
        .json()
        .await
        .map_err(|e| Indisponible::Autre(format!("réponse illisible : {}", e)))?;

    lire_existantes(&corps).ok_or_else(|| Indisponible::Autre("réponse sans « existing »".into()))
}

/// Lit la liste `existing` d'une réponse réussie.
fn lire_existantes(corps: &serde_json::Value) -> Option<HashSet<String>> {
    if corps.get("success").and_then(|v| v.as_bool()) != Some(true) {
        return None;
    }

    Some(
        corps
            .get("existing")?
            .as_array()?
            .iter()
            .filter_map(|v| v.as_str())
            .map(|s| s.trim().to_lowercase())
            .collect(),
    )
}

/// Demande au serveur lesquelles de ces empreintes il détient déjà.
///
/// `None` : le serveur ne peut pas répondre (route absente, réseau…) —
/// l'appelant garde alors le comportement 0.3.2.
pub async fn verifier_empreintes(
    token: &str,
    event_id: i64,
    empreintes: &[String],
) -> Option<HashSet<String>> {
    if empreintes.is_empty() || route_absente() {
        return None;
    }

    let client = client()?;
    let mut existantes = HashSet::new();

    for lot in empreintes.chunks(EMPREINTES_PAR_REQUETE) {
        match verifier_sur(client, API_BASE_URL, token, event_id, lot).await {
            Ok(trouvees) => {
                ROUTE.store(ROUTE_DISPONIBLE, Ordering::Relaxed);
                existantes.extend(trouvees);
            }
            Err(raison) => {
                if let Indisponible::RouteAbsente(_) = raison {
                    ROUTE.store(ROUTE_ABSENTE, Ordering::Relaxed);
                }

                if !SIGNALEE.swap(true, Ordering::Relaxed) {
                    log::info!(
                        "Vérification des empreintes par le serveur indisponible ({:?}) : \
                         mémoire locale seule, comme en 0.3.2",
                        raison
                    );
                }

                return None;
            }
        }
    }

    Some(existantes)
}

/// Ce que l'agent fait d'une photo, d'après la mémoire locale et, s'il a
/// répondu, le serveur.
#[derive(Debug, PartialEq, Clone, Copy)]
pub enum Decision {
    /// Elle part. `oublier_local` : la mémoire locale la croyait en ligne à
    /// tort (photo supprimée de la galerie) — à corriger.
    Envoyer { oublier_local: bool },
    /// Elle ne part pas. `retenir_local` : le serveur l'a, la mémoire
    /// locale ne le savait pas — à compléter.
    Ecarter { retenir_local: bool },
}

/// Le serveur fait foi ; sans réponse de sa part, la mémoire locale décide.
pub fn decider(connue_localement: bool, sur_le_serveur: Option<bool>) -> Decision {
    match sur_le_serveur {
        Some(true) => Decision::Ecarter { retenir_local: !connue_localement },
        Some(false) => Decision::Envoyer { oublier_local: connue_localement },
        None if connue_localement => Decision::Ecarter { retenir_local: false },
        None => Decision::Envoyer { oublier_local: false },
    }
}

// ═══════════════════════════════════════════════════════════════════════
// FAUX SERVEUR POUR LES TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
pub(crate) mod serveur_factice {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    /// Lance un serveur HTTP local qui répond une fois `statut` et `corps`.
    /// Renvoie son adresse (http://127.0.0.1:port) et, à la fin, la requête
    /// reçue.
    pub async fn une_reponse(
        statut: u16,
        corps: &str,
    ) -> (String, tokio::task::JoinHandle<String>) {
        let ecoute = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let adresse = format!("http://{}", ecoute.local_addr().unwrap());
        let corps = corps.to_string();

        let tache = tokio::spawn(async move {
            let (mut flux, _) = ecoute.accept().await.unwrap();
            let mut recu = Vec::new();
            let mut tampon = [0u8; 4096];

            // En-têtes, puis le corps annoncé par Content-Length.
            loop {
                let n = flux.read(&mut tampon).await.unwrap();
                if n == 0 {
                    break;
                }
                recu.extend_from_slice(&tampon[..n]);

                let texte = String::from_utf8_lossy(&recu).to_string();

                if let Some(fin) = texte.find("\r\n\r\n") {
                    let longueur = texte[..fin]
                        .lines()
                        .find_map(|l| {
                            let l = l.to_ascii_lowercase();
                            l.strip_prefix("content-length:").map(|v| v.trim().parse::<usize>().unwrap_or(0))
                        })
                        .unwrap_or(0);

                    if recu.len() >= fin + 4 + longueur {
                        break;
                    }
                }
            }

            let reponse = format!(
                "HTTP/1.1 {} X\r\nContent-Type: application/json\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                statut,
                corps.len(),
                corps
            );

            flux.write_all(reponse.as_bytes()).await.unwrap();
            let _ = flux.shutdown().await;

            String::from_utf8_lossy(&recu).to_string()
        });

        (adresse, tache)
    }
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    fn empreintes() -> Vec<String> {
        vec!["aaaa".into(), "bbbb".into()]
    }

    #[tokio::test]
    async fn route_presente_le_serveur_dit_ce_quil_detient() {
        let (base, requete) = serveur_factice::une_reponse(
            200,
            r#"{"success":true,"existing":["AAAA"]}"#,
        )
        .await;

        let client = reqwest::Client::new();
        let existantes = verifier_sur(&client, &base, "jeton", 42, &empreintes()).await.unwrap();

        assert!(existantes.contains("aaaa"));
        assert!(!existantes.contains("bbbb"));

        let requete = requete.await.unwrap();
        assert!(requete.starts_with("POST /api/sport/events/42/hash-check"));
        assert!(requete.contains(r#""hashes":["aaaa","bbbb"]"#));
        assert!(requete.to_lowercase().contains("authorization: bearer jeton"));
    }

    #[tokio::test]
    async fn route_absente_404_ou_405() {
        for statut in [404, 405] {
            let (base, _) = serveur_factice::une_reponse(statut, "{}").await;

            let resultat = verifier_sur(&reqwest::Client::new(), &base, "j", 1, &empreintes()).await;

            assert_eq!(resultat, Err(Indisponible::RouteAbsente(statut)));
        }
    }

    #[tokio::test]
    async fn serveur_injoignable() {
        // Un port fermé : connexion refusée.
        let ecoute = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let base = format!("http://{}", ecoute.local_addr().unwrap());
        drop(ecoute);

        let resultat = verifier_sur(&reqwest::Client::new(), &base, "j", 1, &empreintes()).await;

        assert!(matches!(resultat, Err(Indisponible::Autre(_))));
    }

    #[tokio::test]
    async fn reponse_sans_liste_indisponible() {
        let (base, _) = serveur_factice::une_reponse(200, r#"{"success":false}"#).await;

        let resultat = verifier_sur(&reqwest::Client::new(), &base, "j", 1, &empreintes()).await;

        assert!(matches!(resultat, Err(Indisponible::Autre(_))));
    }

    #[test]
    fn le_serveur_fait_foi() {
        // Supprimée de la galerie : elle repart, la mémoire locale est corrigée.
        assert_eq!(decider(true, Some(false)), Decision::Envoyer { oublier_local: true });

        // En ligne (autre poste) : elle ne part pas, la mémoire est complétée.
        assert_eq!(decider(false, Some(true)), Decision::Ecarter { retenir_local: true });
        assert_eq!(decider(true, Some(true)), Decision::Ecarter { retenir_local: false });
        assert_eq!(decider(false, Some(false)), Decision::Envoyer { oublier_local: false });
    }

    #[test]
    fn sans_serveur_comportement_0_3_2() {
        assert_eq!(decider(true, None), Decision::Ecarter { retenir_local: false });
        assert_eq!(decider(false, None), Decision::Envoyer { oublier_local: false });
    }

    #[test]
    fn une_route_absente_nest_plus_redemandee() {
        reinitialiser();
        assert!(!route_absente());

        ROUTE.store(ROUTE_ABSENTE, Ordering::Relaxed);
        assert!(route_absente());

        reinitialiser();
        assert!(!route_absente());
    }
}
