use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

// ─── URL de base de l'API Attimo ───
// Injectée à la COMPILATION via la variable d'environnement ATTIMO_API_URL.
// Utiliser les scripts build-prod.bat / build-preprod.bat.
// env!() échoue le build si la variable est absente : impossible de produire
// par erreur un binaire pointant sur le mauvais environnement.
pub const API_BASE_URL: &str = env!("ATTIMO_API_URL");

// ─── Préfixe identifiant un App Password Attimo (SAAS 240 — Phase 6) ───
// Doit rester synchronisé avec App\Modules\TwoFactor\Models\AppPassword::TOKEN_PREFIX_BRAND
pub const APP_PASSWORD_PREFIX: &str = "attimo_pat_";

// ═══════════════════════════════════════════════════════════════════════════
// SAAS 240 — Phase 6 (Vague 2C) : authentification par App Password
// ═══════════════════════════════════════════════════════════════════════════
//
// L'agent Tauri n'utilise plus l'authentification email/mot de passe avec
// retour d'un token Sanctum. À la place :
//
//   1. Le photographe génère un "Mot de passe d'application" depuis son
//      profil sécurité Attimo (scope = tauri_agent).
//   2. Il colle ce token dans l'agent : "attimo_pat_xxxxxxxxxxxxxx..."
//   3. L'agent VALIDE le token en appelant /api/sport/auth/me qui renvoie
//      les infos du photographe (id, name, email).
//   4. Toutes les requêtes API utilisent ensuite Authorization: Bearer XXX
//      au lieu de l'ancien header X-API-TOKEN.
//
// Avantages :
//   - Compatible avec la 2FA activée côté tenant (la 2FA empêche le login
//     email/password classique mais pas l'usage d'un app password).
//   - Le photographe peut révoquer le token depuis son dashboard sans
//     toucher au mot de passe principal.
//   - Aucun mot de passe humain n'est jamais stocké côté agent.
// ═══════════════════════════════════════════════════════════════════════════

// ─── Structures de données ───

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct UserInfo {
    pub id: i64,
    pub name: String,
    pub email: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct AuthData {
    pub token: String,
    pub user: UserInfo,
}

/// Réponse du nouvel endpoint /api/sport/auth/me
/// Retourne les infos du tenant authentifié via app password.
#[derive(Debug, Deserialize)]
struct MeApiResponse {
    success: bool,
    user: Option<UserApiData>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct UserApiData {
    id: i64,
    name: String,
    email: String,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct SportEvent {
    pub id: i64,
    pub name: String,
    pub event_date: Option<String>,
    pub sport_type: String,
    pub photo_count: i64,
    pub is_live: bool,
    pub checkpoints: Vec<Checkpoint>,
}

#[derive(Debug, Serialize, Deserialize, Clone)]
pub struct Checkpoint {
    pub id: i64,
    pub name: String,
    pub sort_order: i64,
}

#[derive(Debug, Deserialize)]
struct EventsApiResponse {
    success: bool,
    data: Option<Vec<SportEvent>>,
    message: Option<String>,
}

#[derive(Debug, Deserialize)]
struct CheckpointsApiResponse {
    success: bool,
    data: Option<Vec<Checkpoint>>,
    message: Option<String>,
}

// ─── Stockage du token (fichier JSON dans AppData) ───

fn token_file_path() -> PathBuf {
    let mut path = if let Some(data_dir) = std::env::var_os("APPDATA") {
        let mut p = PathBuf::from(data_dir);
        p.push("com.attimo-gallery.agent");
        p
    } else {
        let mut p = std::env::current_dir().unwrap_or_default();
        p.push("data");
        p
    };
    fs::create_dir_all(&path).ok();
    path.push("auth.json");
    path
}

/// Sauvegarde les données d'authentification sur le disque
pub fn save_auth(auth: &AuthData) -> Result<(), String> {
    let json = serde_json::to_string_pretty(auth)
        .map_err(|e| format!("Erreur sérialisation: {}", e))?;
    fs::write(token_file_path(), json)
        .map_err(|e| format!("Erreur écriture token: {}", e))?;
    Ok(())
}

/// Charge les données d'authentification depuis le disque
pub fn load_auth() -> Option<AuthData> {
    let path = token_file_path();
    if !path.exists() {
        return None;
    }
    let content = fs::read_to_string(path).ok()?;
    serde_json::from_str(&content).ok()
}

/// Supprime les données d'authentification du disque
pub fn clear_auth() {
    let path = token_file_path();
    if path.exists() {
        fs::remove_file(path).ok();
    }
}

// ─── Validation du format de token côté client ───

/// Vérifie que le token a bien le format d'un App Password Attimo.
/// C'est une vérification *locale* : le serveur fera la vraie validation.
/// Ce check évite des appels HTTP inutiles si l'utilisateur colle n'importe quoi.
pub fn is_well_formed_app_password(token: &str) -> bool {
    if !token.starts_with(APP_PASSWORD_PREFIX) {
        return false;
    }
    // 11 caractères de préfixe + 40 caractères aléatoires = 51 caractères au total
    if token.len() != APP_PASSWORD_PREFIX.len() + 40 {
        return false;
    }
    true
}

// ─── Client HTTP réutilisable ───

fn http_client() -> Result<reqwest::Client, String> {
    reqwest::Client::builder()
        .timeout(std::time::Duration::from_secs(30))
        .http1_only()
        .user_agent("Mozilla/5.0 (Windows NT 10.0; Win64; x64) AppleWebKit/537.36 (KHTML, like Gecko) Chrome/120.0.0.0 Safari/537.36")
        .build()
        .map_err(|e| format!("Erreur client HTTP: {}", e))
}

// ─── Appels API ───

/// SAAS 240 — Phase 6 (Vague 2C) : Validation d'un App Password
///
/// Remplace l'ancienne fonction `login(email, password)`.
///
/// Appelle /api/sport/auth/me en passant le token en header Bearer.
/// Si le token est valide :
///   - Le serveur retourne les infos du photographe (id, name, email)
///   - On construit un AuthData avec le token + ces infos
/// Si le token est invalide :
///   - 401 → "Mot de passe d'application invalide ou révoqué"
///   - autre erreur → message générique
///
/// @param app_password Le mot de passe d'application collé par le photographe
/// @return AuthData prêt à être sauvegardé en local
pub async fn validate_app_password(app_password: &str) -> Result<AuthData, String> {
    // Validation locale du format avant d'appeler le serveur (économise un AR réseau)
    if !is_well_formed_app_password(app_password) {
        return Err(
            "Format de mot de passe invalide. Le token doit commencer par 'attimo_pat_'."
                .to_string(),
        );
    }

    let client = http_client()?;
    let url = format!("{}/api/sport/auth/me", API_BASE_URL);

    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .bearer_auth(app_password)
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "Délai d'attente dépassé. Vérifiez votre connexion internet.".to_string()
            } else if e.is_connect() {
                "Impossible de contacter le serveur Attimo. Vérifiez votre connexion internet."
                    .to_string()
            } else {
                format!("Erreur réseau: {}", e)
            }
        })?;

    let status = response.status();

    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("Mot de passe d'application invalide ou révoqué.".to_string());
    }

    if !status.is_success() {
        return Err(format!("Erreur serveur ({})", status.as_u16()));
    }

    let api_response: MeApiResponse = response
        .json()
        .await
        .map_err(|e| format!("Erreur lecture réponse: {}", e))?;

    if !api_response.success {
        return Err(api_response
            .message
            .unwrap_or_else(|| "Échec de validation.".to_string()));
    }

    let user_data = api_response
        .user
        .ok_or("Données utilisateur manquantes dans la réponse.")?;

    Ok(AuthData {
        token: app_password.to_string(),
        user: UserInfo {
            id: user_data.id,
            name: user_data.name,
            email: user_data.email,
        },
    })
}

/// Récupère les événements sport du photographe : GET /api/sport/events
pub async fn fetch_events(token: &str) -> Result<Vec<SportEvent>, String> {
    let client = http_client()?;
    let url = format!("{}/api/sport/events", API_BASE_URL);

    let response = client
        .get(&url)
        .header("Accept", "application/json")
        // SAAS 240 — Phase 6 : Bearer token au lieu de X-API-TOKEN
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("Erreur réseau: {}", e))?;

    let status = response.status();

    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("SESSION_EXPIRED".to_string());
    }

    if !status.is_success() {
        return Err(format!("Erreur serveur ({})", status.as_u16()));
    }

    let api_response: EventsApiResponse = response
        .json()
        .await
        .map_err(|e| format!("Erreur lecture réponse: {}", e))?;

    if !api_response.success {
        return Err(api_response.message.unwrap_or_else(|| "Erreur inconnue.".to_string()));
    }

    Ok(api_response.data.unwrap_or_default())
}

/// Récupère les checkpoints d'un événement : GET /api/sport/events/{id}/checkpoints
pub async fn fetch_checkpoints(token: &str, event_id: i64) -> Result<Vec<Checkpoint>, String> {
    let client = http_client()?;
    let url = format!("{}/api/sport/events/{}/checkpoints", API_BASE_URL, event_id);

    let response = client
        .get(&url)
        .header("Accept", "application/json")
        // SAAS 240 — Phase 6 : Bearer token au lieu de X-API-TOKEN
        .bearer_auth(token)
        .send()
        .await
        .map_err(|e| format!("Erreur réseau: {}", e))?;

    let status = response.status();

    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("SESSION_EXPIRED".to_string());
    }

    if !status.is_success() {
        return Err(format!("Erreur serveur ({})", status.as_u16()));
    }

    let api_response: CheckpointsApiResponse = response
        .json()
        .await
        .map_err(|e| format!("Erreur lecture réponse: {}", e))?;

    if !api_response.success {
        return Err(api_response.message.unwrap_or_else(|| "Erreur inconnue.".to_string()));
    }

    Ok(api_response.data.unwrap_or_default())
}
