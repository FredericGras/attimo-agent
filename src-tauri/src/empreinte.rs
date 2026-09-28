// ═══════════════════════════════════════════════════════════════════════
// ATTIMO AGENT TERRAIN — Empreinte des photos (empreinte.rs)
// ═══════════════════════════════════════════════════════════════════════
//
// 0.3.2 — Anti-doublon AVANT l'envoi.
//
// Le serveur écarte déjà les doublons, mais après les avoir reçus : un
// renvoi du dossier refaisait partir tous les fichiers (585 Mo mesurés en
// 4G). L'agent garde désormais, par épreuve, l'empreinte de chaque photo
// envoyée, et ne renvoie jamais une photo dont l'empreinte est connue.
//
// L'empreinte est un MD5 du fichier entier : exactement celle que calcule
// le serveur (`SportUploadController::upload`, `md5_file`, colonne
// `content_hash`). Le jour où le serveur exposera la vérification d'une
// liste d'empreintes à l'agent, les mêmes valeurs serviront telles quelles.
//
// MD5 n'est pas utilisé ici pour la sécurité, seulement pour reconnaître un
// fichier identique : ses faiblesses cryptographiques sont sans objet.
// Implémentation locale (RFC 1321) : aucune dépendance de plus à télécharger.

/// Empreinte MD5 d'un contenu, en hexadécimal minuscule (32 caractères).
pub fn md5_hexa(donnees: &[u8]) -> String {
    md5(donnees).iter().map(|o| format!("{:02x}", o)).collect()
}

const S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

const K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

fn md5(donnees: &[u8]) -> [u8; 16] {
    let mut a0: u32 = 0x67452301;
    let mut b0: u32 = 0xefcdab89;
    let mut c0: u32 = 0x98badcfe;
    let mut d0: u32 = 0x10325476;

    // Bourrage : un bit à 1, des zéros, puis la longueur en bits sur 64 bits,
    // pour atteindre un multiple de 64 octets.
    let longueur_bits = (donnees.len() as u64).wrapping_mul(8);

    let mut fin = Vec::with_capacity(128);
    let reste = donnees.len() % 64;
    fin.extend_from_slice(&donnees[donnees.len() - reste..]);
    fin.push(0x80);

    while fin.len() % 64 != 56 {
        fin.push(0);
    }

    fin.extend_from_slice(&longueur_bits.to_le_bytes());

    let complets = &donnees[..donnees.len() - reste];

    for bloc in complets.chunks_exact(64).chain(fin.chunks_exact(64)) {
        let mut m = [0u32; 16];

        for (i, mot) in m.iter_mut().enumerate() {
            *mot = u32::from_le_bytes([bloc[i * 4], bloc[i * 4 + 1], bloc[i * 4 + 2], bloc[i * 4 + 3]]);
        }

        let (mut a, mut b, mut c, mut d) = (a0, b0, c0, d0);

        for i in 0..64 {
            let (f, g) = match i {
                0..=15 => ((b & c) | (!b & d), i),
                16..=31 => ((d & b) | (!d & c), (5 * i + 1) % 16),
                32..=47 => (b ^ c ^ d, (3 * i + 5) % 16),
                _ => (c ^ (b | !d), (7 * i) % 16),
            };

            let f = f.wrapping_add(a).wrapping_add(K[i]).wrapping_add(m[g]);

            a = d;
            d = c;
            c = b;
            b = b.wrapping_add(f.rotate_left(S[i]));
        }

        a0 = a0.wrapping_add(a);
        b0 = b0.wrapping_add(b);
        c0 = c0.wrapping_add(c);
        d0 = d0.wrapping_add(d);
    }

    let mut sortie = [0u8; 16];
    sortie[0..4].copy_from_slice(&a0.to_le_bytes());
    sortie[4..8].copy_from_slice(&b0.to_le_bytes());
    sortie[8..12].copy_from_slice(&c0.to_le_bytes());
    sortie[12..16].copy_from_slice(&d0.to_le_bytes());
    sortie
}

// ═══════════════════════════════════════════════════════════════════════
// TESTS
// ═══════════════════════════════════════════════════════════════════════

#[cfg(test)]
mod tests {
    use super::*;

    /// Jeu d'essai officiel de la RFC 1321 (annexe A.5).
    #[test]
    fn suit_la_rfc_1321() {
        let cas = [
            ("", "d41d8cd98f00b204e9800998ecf8427e"),
            ("a", "0cc175b9c0f1b6a831c399e269772661"),
            ("abc", "900150983cd24fb0d6963f7d28e17f72"),
            ("message digest", "f96b697d7cb7938d525a2f31aaf161d0"),
            ("abcdefghijklmnopqrstuvwxyz", "c3fcd3d76192e4007dfb496cca67e13b"),
            (
                "ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789",
                "d174ab98d277d9f5a5611c2c9f419d9f",
            ),
            (
                "12345678901234567890123456789012345678901234567890123456789012345678901234567890",
                "57edf4a22be3c955ac49da2e2107b67a",
            ),
        ];

        for (texte, attendu) in cas {
            assert_eq!(md5_hexa(texte.as_bytes()), attendu, "« {} »", texte);
        }
    }

    #[test]
    fn traite_les_longueurs_limites_du_bourrage() {
        // 55, 56 et 64 octets : les trois cas où le bourrage déborde ou non
        // sur un bloc supplémentaire.
        assert_eq!(md5_hexa(&[b'a'; 55]), "ef1772b6dff9a122358552954ad0df65");
        assert_eq!(md5_hexa(&[b'a'; 56]), "3b0c8ac703f828b04c6c197006d17218");
        assert_eq!(md5_hexa(&[b'a'; 64]), "014842d480b571495a4a0363793f7367");
    }
}
