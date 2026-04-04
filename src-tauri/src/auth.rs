use serde::{Deserialize, Serialize};
use std::fs;
use std::path::PathBuf;

// ─── URL de base de l'API Attimo ───
// TODO: passer en "https://attimo-gallery.com" pour la production
const API_BASE_URL: &str = "https://dev-saas.attimo-gallery.com";

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

#[derive(Debug, Deserialize)]
struct LoginApiResponse {
    success: bool,
    token: Option<String>,
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

/// Authentification : POST /api/sport/auth/login
pub async fn login(email: &str, password: &str) -> Result<AuthData, String> {
    let client = http_client()?;
    let url = format!("{}/api/sport/auth/login", API_BASE_URL);

    let response = client
        .post(&url)
        .header("Accept", "application/json")
        .header("Content-Type", "application/json")
        .json(&serde_json::json!({
            "email": email,
            "password": password
        }))
        .send()
        .await
        .map_err(|e| {
            if e.is_timeout() {
                "Délai d'attente dépassé. Vérifiez votre connexion internet.".to_string()
            } else if e.is_connect() {
                "Impossible de contacter le serveur Attimo. Vérifiez votre connexion internet.".to_string()
            } else {
                format!("Erreur réseau: {}", e)
            }
        })?;

    let status = response.status();

    if status == reqwest::StatusCode::UNAUTHORIZED {
        return Err("Identifiants invalides.".to_string());
    }

    if !status.is_success() {
        return Err(format!("Erreur serveur ({})", status.as_u16()));
    }

    let api_response: LoginApiResponse = response
        .json()
        .await
        .map_err(|e| format!("Erreur lecture réponse: {}", e))?;

    if !api_response.success {
        return Err(api_response.message.unwrap_or_else(|| "Échec de connexion.".to_string()));
    }

    let token = api_response.token.ok_or("Token manquant dans la réponse.")?;
    let user_data = api_response.user.ok_or("Données utilisateur manquantes.")?;

    let auth = AuthData {
        token,
        user: UserInfo {
            id: user_data.id,
            name: user_data.name,
            email: user_data.email,
        },
    };

    Ok(auth)
}

/// Récupère les événements sport du photographe : GET /api/sport/events
pub async fn fetch_events(token: &str) -> Result<Vec<SportEvent>, String> {
    let client = http_client()?;
    let url = format!("{}/api/sport/events", API_BASE_URL);

    let response = client
        .get(&url)
        .header("Accept", "application/json")
        .header("X-API-TOKEN", token)
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
        .header("X-API-TOKEN", token)
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
