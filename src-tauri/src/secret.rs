// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Stockage sécurisé du jeton (secret.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// 0.3.2 — `auth.json` contenait le mot de passe d'application en clair :
// quiconque copiait le dossier de l'agent pouvait envoyer des photos au nom
// du photographe.
//
// Windows : DPAPI (CryptProtectData). Le jeton est chiffré avec une clé liée
// à la session Windows de l'utilisateur ; `auth.json` n'en garde que la
// version chiffrée. Copié sur un autre poste, ou lu par un autre compte
// Windows, il est inutilisable.
//
// macOS : le trousseau de la session (Keychain) ; `auth.json` ne garde plus
// que les informations d'affichage (nom, adresse).
//
// Ailleurs : aucun coffre prévu, le fichier reste comme avant.

/// Où vit le jeton.
pub enum Rangement {
    /// Chiffré, dans auth.json (Windows).
    #[cfg_attr(not(windows), allow(dead_code))]
    Chiffre(Vec<u8>),
    /// Dans le coffre du système (macOS) : rien à écrire dans auth.json.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    Coffre,
    /// En clair : aucun coffre sur ce système.
    #[cfg_attr(any(windows, target_os = "macos"), allow(dead_code))]
    Clair,
}

// ═══════════════════════════════════════════════════════════════════════
// WINDOWS — DPAPI
// ═══════════════════════════════════════════════════════════════════════

#[cfg(windows)]
mod dpapi {
    use windows_sys::Win32::Foundation::LocalFree;
    use windows_sys::Win32::Security::Cryptography::{
        CryptProtectData, CryptUnprotectData, CRYPTPROTECT_UI_FORBIDDEN, CRYPT_INTEGER_BLOB,
    };

    /// Chaîne propre à l'agent, mêlée à la clé : un autre programme de la
    /// même session ne déchiffre pas le jeton par simple appel à DPAPI.
    const ENTROPIE: &[u8] = b"attimo-agent-terrain/jeton";

    fn blob(octets: &[u8]) -> CRYPT_INTEGER_BLOB {
        CRYPT_INTEGER_BLOB {
            cbData: octets.len() as u32,
            pbData: octets.as_ptr() as *mut u8,
        }
    }

    /// Recopie la sortie de DPAPI puis la rend au système.
    unsafe fn recuperer(sortie: CRYPT_INTEGER_BLOB) -> Vec<u8> {
        let octets = std::slice::from_raw_parts(sortie.pbData, sortie.cbData as usize).to_vec();
        LocalFree(sortie.pbData as _);
        octets
    }

    pub fn chiffrer(clair: &[u8]) -> Result<Vec<u8>, String> {
        let entree = blob(clair);
        let entropie = blob(ENTROPIE);
        let mut sortie = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };

        // SAFETY : les blobs d'entrée pointent sur des tranches vivantes
        // pendant tout l'appel ; la sortie est allouée par le système et
        // rendue par `recuperer`.
        let ok = unsafe {
            CryptProtectData(
                &entree,
                std::ptr::null(),
                &entropie,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut sortie,
            )
        };

        if ok == 0 || sortie.pbData.is_null() {
            return Err("Chiffrement du jeton impossible (DPAPI).".to_string());
        }

        Ok(unsafe { recuperer(sortie) })
    }

    pub fn dechiffrer(chiffre: &[u8]) -> Result<Vec<u8>, String> {
        let entree = blob(chiffre);
        let entropie = blob(ENTROPIE);
        let mut sortie = CRYPT_INTEGER_BLOB { cbData: 0, pbData: std::ptr::null_mut() };

        // SAFETY : mêmes garanties que pour le chiffrement.
        let ok = unsafe {
            CryptUnprotectData(
                &entree,
                std::ptr::null_mut(),
                &entropie,
                std::ptr::null(),
                std::ptr::null(),
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut sortie,
            )
        };

        if ok == 0 || sortie.pbData.is_null() {
            return Err("Déchiffrement du jeton impossible (autre poste ou autre compte Windows ?).".to_string());
        }

        Ok(unsafe { recuperer(sortie) })
    }
}

// ═══════════════════════════════════════════════════════════════════════
// macOS — TROUSSEAU
// ═══════════════════════════════════════════════════════════════════════

#[cfg(target_os = "macos")]
mod trousseau {
    use security_framework::passwords::{
        delete_generic_password, get_generic_password, set_generic_password,
    };

    /// Le service porte le dossier de données : l'édition DEV a son propre
    /// jeton, jamais celui de la production.
    fn service() -> String {
        crate::database::DOSSIER_DONNEES.to_string()
    }

    const COMPTE: &str = "app-password";

    pub fn ranger(jeton: &str) -> Result<(), String> {
        set_generic_password(&service(), COMPTE, jeton.as_bytes())
            .map_err(|e| format!("Trousseau indisponible : {}", e))
    }

    pub fn lire() -> Option<String> {
        let octets = get_generic_password(&service(), COMPTE).ok()?;
        String::from_utf8(octets).ok()
    }

    pub fn oublier() {
        let _ = delete_generic_password(&service(), COMPTE);
    }
}

// ═══════════════════════════════════════════════════════════════════════
// INTERFACE COMMUNE
// ═══════════════════════════════════════════════════════════════════════

/// Met le jeton à l'abri. Renvoie ce qu'il faut garder dans auth.json.
#[cfg(windows)]
pub fn proteger(jeton: &str) -> Result<Rangement, String> {
    dpapi::chiffrer(jeton.as_bytes()).map(Rangement::Chiffre)
}

#[cfg(target_os = "macos")]
pub fn proteger(jeton: &str) -> Result<Rangement, String> {
    trousseau::ranger(jeton).map(|_| Rangement::Coffre)
}

#[cfg(not(any(windows, target_os = "macos")))]
pub fn proteger(_jeton: &str) -> Result<Rangement, String> {
    Ok(Rangement::Clair)
}

/// Relit un jeton chiffré (Windows).
#[cfg(windows)]
pub fn dechiffrer(chiffre: &[u8]) -> Result<String, String> {
    let clair = dpapi::dechiffrer(chiffre)?;
    String::from_utf8(clair).map_err(|_| "Jeton illisible.".to_string())
}

#[cfg(not(windows))]
pub fn dechiffrer(_chiffre: &[u8]) -> Result<String, String> {
    Err("Aucun coffre de chiffrement sur ce système.".to_string())
}

/// Relit le jeton rangé dans le coffre du système (macOS).
#[cfg(target_os = "macos")]
pub fn lire_coffre() -> Option<String> {
    trousseau::lire()
}

#[cfg(not(target_os = "macos"))]
pub fn lire_coffre() -> Option<String> {
    None
}

/// Efface le jeton du coffre du système, s'il y en a un.
#[cfg(target_os = "macos")]
pub fn oublier_coffre() {
    trousseau::oublier();
}

#[cfg(not(target_os = "macos"))]
pub fn oublier_coffre() {}

/// Hexadécimal, pour ranger des octets chiffrés dans un fichier JSON.
pub fn en_hexa(octets: &[u8]) -> String {
    octets.iter().map(|o| format!("{:02x}", o)).collect()
}

pub fn depuis_hexa(texte: &str) -> Option<Vec<u8>> {
    if texte.len() % 2 != 0 {
        return None;
    }

    (0..texte.len())
        .step_by(2)
        .map(|i| u8::from_str_radix(texte.get(i..i + 2)?, 16).ok())
        .collect()
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lhexadecimal_fait_laller_retour() {
        let octets = vec![0u8, 1, 127, 128, 255];

        assert_eq!(en_hexa(&octets), "00017f80ff");
        assert_eq!(depuis_hexa("00017f80ff"), Some(octets));
        assert_eq!(depuis_hexa("0"), None);
        assert_eq!(depuis_hexa("zz"), None);
    }

    #[cfg(windows)]
    #[test]
    fn dpapi_chiffre_et_dechiffre() {
        let jeton = "attimo_pat_0123456789012345678901234567890123456789";

        let chiffre = dpapi::chiffrer(jeton.as_bytes()).unwrap();

        // Le jeton n'apparaît nulle part dans la version chiffrée.
        assert!(!en_hexa(&chiffre).contains(&en_hexa(b"attimo_pat_")));

        assert_eq!(dechiffrer(&chiffre).unwrap(), jeton);
    }
}
