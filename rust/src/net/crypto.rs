//! Login-Schlüsseltausch ab Minecraft 26.4.
//!
//! Bis 26.3 schickte der Client einen selbst gewürfelten AES-Schlüssel RSA-verschlüsselt an den
//! Server und benutzte denselben Wert als Schlüssel **und** IV für beide Richtungen. 26.4 ersetzt
//! das durch einen echten Diffie-Hellman-Tausch. Nachgebaut aus `SessionKeyExchange`,
//! `SessionKeys`, `SessionCommonInfo`, `PublicPeerInfo` und `StreamCryptoUtils` des
//! unverschleierten 26.4-Clients:
//!
//! * X25519 zwischen dem Serverschlüssel aus `ClientboundKey` und einem frischen eigenen.
//! * HKDF-SHA256, Salz = Server-Salz (8 Byte, big endian) ‖ fester 24-Byte-Wert.
//!   Daraus `a_to_b`/`b_to_a` (je 16 Byte AES), `authenticate` und `verify` (je 32 Byte HMAC).
//!   Seite A ist der Server, Seite B der Client.
//! * Integritätsprüfsumme (HMAC `verify`) über Protokoll, Salz, Auth-Flag und beide
//!   Teilnehmer – geht als Signatur im `ServerboundKey` mit.
//! * Auth-Prüfsumme (HMAC `authenticate`, erste 25 Byte, Base32hex) ersetzt den alten
//!   SHA-1-„Server-Hash“ beim Mojang-Sitzungsserver.
//! * IVs aus `java.util.Random(Arrays.hashCode(signatur))`: erst Server→Client, dann
//!   Client→Server. Der Datenstrom bleibt AES-128-CFB8 (siehe [`super::Cfb8`]).

use hkdf::Hkdf;
use hmac::{Hmac, Mac};
use sha2::Sha256;
use x25519_dalek::{EphemeralSecret, PublicKey};

type HmacSha256 = Hmac<Sha256>;

/// `SessionKeyExchange.SALT`: drei Java-`long`s, big endian aneinandergereiht.
const FIXED_SALT: [i64; 3] = [
    3195254534265565831,
    8302438179253405948,
    6794457759077931664,
];
/// Trennzeichen `+` in allen Prüfsummen (`SessionKeys.DIGEST_SEPARATOR`).
const SEPARATOR: u8 = 43;
/// X.509-`SubjectPublicKeyInfo` für X25519 (OID 1.3.101.110) vor den 32 Schlüsselbytes. Genau
/// so kodiert Java `PublicKey.getEncoded()`, und genau das steht im Paket.
const X25519_DER_PREFIX: [u8; 12] = [
    0x30, 0x2a, 0x30, 0x05, 0x06, 0x03, 0x2b, 0x65, 0x6e, 0x03, 0x21, 0x00,
];

/// Alles, was nach dem Tausch gebraucht wird.
pub struct SessionKeys {
    /// Eigener öffentlicher Schlüssel als X.509 (geht im `ServerboundKey` mit).
    pub client_key_der: Vec<u8>,
    /// Integritätsprüfsumme, 32 Byte (geht im `ServerboundKey` mit).
    pub signature: [u8; 32],
    /// Ersatz für den Server-Hash beim Mojang-Sitzungsserver.
    pub auth_digest: String,
    pub inbound_key: [u8; 16],
    pub outbound_key: [u8; 16],
    pub inbound_iv: [u8; 16],
    pub outbound_iv: [u8; 16],
}

/// Was der Server im `ClientboundKey` schickt, plus das, was beide Seiten schon kennen.
pub struct Offer<'a> {
    pub salt: i64,
    pub server_id: &'a str,
    pub server_key_der: &'a [u8],
    pub authenticate: bool,
    pub client_name: &'a str,
    pub protocol: i32,
}

pub fn exchange(offer: &Offer) -> Result<SessionKeys, String> {
    let secret = EphemeralSecret::random_from_rng(rand::rngs::OsRng);
    let public = PublicKey::from(&secret);
    let server = parse_x25519(offer.server_key_der)?;
    let shared = secret.diffie_hellman(&server);
    if !shared.was_contributory() {
        // Ein Schlüssel kleiner Ordnung ergibt ein bekanntes Geheimnis – Java lehnt ihn ebenso ab.
        return Err("Server-Schlüssel ungültig (kleine Ordnung)".into());
    }
    Ok(derive(offer, public.as_bytes(), shared.as_bytes()))
}

/// Der rechnerische Teil, getrennt vom Zufall – so lässt er sich gegen Java-Referenzwerte testen.
fn derive(offer: &Offer, client_key: &[u8; 32], shared: &[u8; 32]) -> SessionKeys {
    let mut client_key_der = X25519_DER_PREFIX.to_vec();
    client_key_der.extend_from_slice(client_key);

    let mut salt = offer.salt.to_be_bytes().to_vec();
    for part in FIXED_SALT {
        salt.extend_from_slice(&part.to_be_bytes());
    }
    let hkdf = Hkdf::<Sha256>::new(Some(&salt), shared);
    let connection = format!(
        "a={};b={};protocol={}",
        offer.server_id, offer.client_name, offer.protocol
    );
    let peers = format!("a={};b={}", offer.server_id, offer.client_name);
    let expand = |label: &str, out: &mut [u8]| {
        // Länge ≤ 255 · 32 – bei 16/32 Byte kann das nicht scheitern.
        hkdf.expand(label.as_bytes(), out).expect("HKDF-Länge");
    };
    let (mut a_to_b, mut b_to_a) = ([0u8; 16], [0u8; 16]);
    let (mut auth_key, mut verify_key) = ([0u8; 32], [0u8; 32]);
    expand(&format!("a_to_b;{}", connection), &mut a_to_b);
    expand(&format!("b_to_a;{}", connection), &mut b_to_a);
    expand(&format!("authenticate;{}", peers), &mut auth_key);
    expand(&format!("verify;{}", peers), &mut verify_key);

    let mut verify = HmacSha256::new_from_slice(&verify_key).expect("HMAC-Schlüssel");
    verify.update(&offer.protocol.to_be_bytes());
    verify.update(&[SEPARATOR]);
    verify.update(&offer.salt.to_be_bytes());
    verify.update(&[SEPARATOR]);
    verify.update(&[if offer.authenticate { 1 } else { 2 }]);
    verify.update(&[SEPARATOR]);
    verify.update(offer.server_id.as_bytes());
    verify.update(&[SEPARATOR]);
    verify.update(offer.server_key_der);
    verify.update(&[SEPARATOR]);
    verify.update(offer.client_name.as_bytes());
    verify.update(&[SEPARATOR]);
    verify.update(&client_key_der);
    let signature: [u8; 32] = verify.finalize().into_bytes().into();

    let mut auth = HmacSha256::new_from_slice(&auth_key).expect("HMAC-Schlüssel");
    auth.update(offer.server_key_der);
    auth.update(&[SEPARATOR]);
    auth.update(&client_key_der);
    let auth_digest = base32_hex(&auth.finalize().into_bytes()[..25]);

    let mut ivs = JavaRandom::new(java_array_hash(&signature));
    let inbound_iv = ivs.bytes16();
    let outbound_iv = ivs.bytes16();

    SessionKeys {
        client_key_der,
        signature,
        auth_digest,
        // Client = Seite B: hinein kommt a→b, hinaus geht b→a.
        inbound_key: a_to_b,
        outbound_key: b_to_a,
        inbound_iv,
        outbound_iv,
    }
}

fn parse_x25519(der: &[u8]) -> Result<PublicKey, String> {
    match der.strip_prefix(&X25519_DER_PREFIX[..]) {
        Some(key) if key.len() == 32 => {
            let mut bytes = [0u8; 32];
            bytes.copy_from_slice(key);
            Ok(PublicKey::from(bytes))
        }
        _ => Err("Server-Schlüssel ist kein X25519-Schlüssel".into()),
    }
}

/// `java.util.Arrays.hashCode(byte[])` – mit vorzeichenbehafteten Bytes.
fn java_array_hash(bytes: &[u8]) -> i32 {
    bytes.iter().fold(1i32, |hash, b| {
        hash.wrapping_mul(31).wrapping_add(*b as i8 as i32)
    })
}

/// `java.util.Random`, so weit `nextBytes` ihn braucht.
struct JavaRandom(u64);

impl JavaRandom {
    const MULTIPLIER: u64 = 0x5_DEEC_E66D;
    const MASK: u64 = (1 << 48) - 1;

    fn new(seed: i32) -> JavaRandom {
        JavaRandom((seed as i64 as u64 ^ Self::MULTIPLIER) & Self::MASK)
    }

    fn next_int(&mut self) -> i32 {
        self.0 = (self.0.wrapping_mul(Self::MULTIPLIER).wrapping_add(0xB)) & Self::MASK;
        (self.0 >> 16) as u32 as i32
    }

    fn bytes16(&mut self) -> [u8; 16] {
        let mut out = [0u8; 16];
        for chunk in out.chunks_mut(4) {
            let mut value = self.next_int();
            for byte in chunk {
                *byte = value as u8;
                value >>= 8;
            }
        }
        out
    }
}

/// RFC 4648 „base32hex“, Großbuchstaben, ohne Auffüllung (25 Byte ergeben glatt 40 Zeichen).
fn base32_hex(bytes: &[u8]) -> String {
    const ALPHABET: &[u8; 32] = b"0123456789ABCDEFGHIJKLMNOPQRSTUV";
    let mut out = String::with_capacity(bytes.len() * 8 / 5 + 1);
    let (mut buffer, mut bits) = (0u32, 0u32);
    for byte in bytes {
        buffer = (buffer << 8) | *byte as u32;
        bits += 8;
        while bits >= 5 {
            bits -= 5;
            out.push(ALPHABET[((buffer >> bits) & 31) as usize] as char);
        }
    }
    if bits > 0 {
        out.push(ALPHABET[((buffer << (5 - bits)) & 31) as usize] as char);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn hex(bytes: &[u8]) -> String {
        bytes.iter().map(|b| format!("{:02x}", b)).collect()
    }

    #[test]
    fn java_random_folgt_dem_original() {
        // new Random(42).nextInt() == -1170105035, danach 234785527
        let mut random = JavaRandom::new(42);
        assert_eq!(random.next_int(), -1170105035);
        assert_eq!(random.next_int(), 234785527);
    }

    #[test]
    fn array_hash_wie_java() {
        // Arrays.hashCode(new byte[]{1, -1, 127}) == ((31+1)*31-1)*31+127 = 30848
        assert_eq!(java_array_hash(&[1, 0xff, 127]), 30848);
        assert_eq!(java_array_hash(&[]), 1);
    }

    #[test]
    fn base32hex_wie_commons_codec() {
        assert_eq!(base32_hex(b"f"), "CO");
        assert_eq!(base32_hex(b"foobar"), "CPNMUOJ1E8");
        assert_eq!(base32_hex(&[0u8; 25]).len(), 40);
    }

    #[test]
    fn fremde_kurve_wird_abgelehnt() {
        assert!(parse_x25519(&[0u8; 44]).is_err());
        assert!(parse_x25519(&X25519_DER_PREFIX).is_err());
    }

    /// Referenzwerte aus Mojangs eigenen Klassen (`SessionKeyExchange` des 26.4-pre-1-Clients),
    /// berechnet mit festen Schlüsseln: Client-Geheimnis = 32 × 0x11, Server-Geheimnis =
    /// 32 × 0x22, Salz 0x0123456789ABCDEF, Server-ID „server“, Name „Bot“, Protokoll
    /// 0x4000015D, Authentifizierung an. Erzeugt mit `data/crypto-reference/Reference.java`.
    #[test]
    fn schluessel_stimmen_mit_java_ueberein() {
        let client = x25519_dalek::StaticSecret::from([0x11u8; 32]);
        let server = x25519_dalek::StaticSecret::from([0x22u8; 32]);
        let server_public = PublicKey::from(&server);
        let mut server_der = X25519_DER_PREFIX.to_vec();
        server_der.extend_from_slice(server_public.as_bytes());
        let shared = client.diffie_hellman(&server_public);
        let offer = Offer {
            salt: 0x0123_4567_89AB_CDEF,
            server_id: "server",
            server_key_der: &server_der,
            authenticate: true,
            client_name: "Bot",
            protocol: 0x4000_015D,
        };
        let keys = derive(
            &offer,
            PublicKey::from(&client).as_bytes(),
            shared.as_bytes(),
        );
        let expected = include_str!("../../data/crypto-reference/expected.txt");
        let actual = format!(
            "signature={}\nauth={}\ninbound={}\noutbound={}\ninbound_iv={}\noutbound_iv={}\n",
            hex(&keys.signature),
            keys.auth_digest,
            hex(&keys.inbound_key),
            hex(&keys.outbound_key),
            hex(&keys.inbound_iv),
            hex(&keys.outbound_iv),
        );
        assert_eq!(actual.trim(), expected.replace('\r', "").trim());
    }
}
