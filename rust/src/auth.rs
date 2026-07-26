//! Microsoft-Login (Device-Code) und Kontoverwaltung – **dateikompatibel zum Java-Client**.
//!
//! Die Konten liegen in denselben Dateien `~/.config/hugoafk/accounts/<name>.json` im Format von
//! MinecraftAuth (`JavaAuthManager.toJson`). Beim Speichern wird die vorhandene JSON-Struktur
//! übernommen und nur das angefasst, was wir wirklich erneuern (MSA-Token, Minecraft-Token,
//! Profil). Die Xbox-Device-/Title-Token des Java-Clients bleiben unangetastet – so kann der
//! Java-Client dieselbe Datei danach weiterverwenden.
//!
//! Token-Kette (der klassische Weg, ohne Xbox-Device-Signaturen):
//!   MSA-Refresh -> XBL-User-Token -> XSTS -> Minecraft-Token -> Profil

use crate::buf::{hex, uuid_from_str, uuid_to_dashed};
use base64::Engine;
use rand::RngCore;
use serde_json::{json, Value};
use sha1::{Digest, Sha1};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Ein einziger HTTP-Agent für alle Aufrufe: hält Verbindungen offen und spart Handshakes.
///
/// TLS kommt unter Windows von SChannel (`native-tls`), sonst von rustls – so braucht der
/// Linux-Build weder OpenSSL-Header noch `pkg-config` (siehe Cargo.toml).
fn agent() -> &'static ureq::Agent {
    static AGENT: OnceLock<ureq::Agent> = OnceLock::new();
    AGENT.get_or_init(|| {
        let builder = ureq::builder()
            .timeout_connect(Duration::from_secs(15))
            .timeout(Duration::from_secs(30));

        #[cfg(windows)]
        let builder = match native_tls::TlsConnector::new() {
            Ok(tls) => builder.tls_connector(std::sync::Arc::new(tls)),
            Err(_) => builder,
        };

        builder.build()
    })
}

/// Öffentliche Client-ID des Minecraft-Launchers (dieselbe, die MinecraftAuth im Java-Client nutzt).
const CLIENT_ID: &str = "00000000402b5328";
const SCOPE: &str = "service::user.auth.xboxlive.com::MBI_SSL";
const DEVICE_CODE_URL: &str = "https://login.live.com/oauth20_connect.srf";
const TOKEN_URL: &str = "https://login.live.com/oauth20_token.srf";

pub type Res<T> = Result<T, String>;

pub struct DeviceCode {
    pub user_code: String,
    pub verification_uri: String,
}

pub struct Account {
    pub name: String,
    pub profile_id: [u8; 16],
    file: PathBuf,
    json: Value,
    token: String,
    expires_at_ms: i64,
}

// ===================== Kontenverwaltung =====================

pub fn accounts_dir(base: &Path) -> PathBuf {
    base.join("accounts")
}

/// Namen aller gespeicherten Konten (liest nur Dateinamen, keine Token).
pub fn list(base: &Path) -> Vec<String> {
    let mut names = Vec::new();
    if let Ok(entries) = std::fs::read_dir(accounts_dir(base)) {
        for entry in entries.flatten() {
            let name = entry.file_name().to_string_lossy().into_owned();
            if let Some(stripped) = name.strip_suffix(".json") {
                names.push(stripped.to_string());
            }
        }
    }
    names.sort_by_key(|n| n.to_lowercase());
    names
}

/// Übernimmt eine alte einzelne `auth.json` als erstes Konto (wie der Java-Client).
pub fn migrate_legacy(base: &Path) -> Option<String> {
    if !list(base).is_empty() {
        return None;
    }
    let legacy = base.join("auth.json");
    let text = std::fs::read_to_string(&legacy).ok()?;
    let json: Value = serde_json::from_str(&text).ok()?;
    let name = json["minecraftProfile"]["name"].as_str()?.to_string();
    std::fs::create_dir_all(accounts_dir(base)).ok()?;
    std::fs::write(account_file(base, &name), &text).ok()?;
    Some(name)
}

pub fn remove(base: &Path, name: &str) -> bool {
    std::fs::remove_file(account_file(base, name)).is_ok()
}

fn account_file(base: &Path, name: &str) -> PathBuf {
    accounts_dir(base).join(format!("{}.json", name))
}

// ===================== Laden / Anmelden =====================

/// Lädt ein gespeichertes Konto und frischt es bei Bedarf auf.
pub fn load(base: &Path, name: &str) -> Res<Account> {
    let file = account_file(base, name);
    let text = std::fs::read_to_string(&file).map_err(|e| format!("{}: {}", name, e))?;
    let json: Value = serde_json::from_str(&text).map_err(|e| format!("{}: {}", name, e))?;

    let mut account = Account {
        name: name.to_string(),
        profile_id: [0u8; 16],
        file,
        json,
        token: String::new(),
        expires_at_ms: 0,
    };
    account.adopt_cached();
    account.ensure_fresh()?;
    Ok(account)
}

/// Device-Code-Login für ein neues Konto.
pub fn add(base: &Path, on_code: impl Fn(&DeviceCode)) -> Res<Account> {
    let (device_code, code_info) = request_device_code()?;
    on_code(&code_info);
    let (access, refresh, expires_ms) = poll_for_token(&device_code)?;

    let json = json!({
        "_saveVersion": 1,
        "msaApplicationConfig": {
            "_saveVersion": 1,
            "clientId": CLIENT_ID,
            "scope": SCOPE,
            "environment": "LIVE"
        },
        // Der Java-Client verlangt diese drei Felder beim Laden – wir legen sie gültig an.
        "deviceType": "Win32",
        "deviceKeyPair": device_key_pair()?,
        "deviceId": random_uuid_dashed(),
        "msaToken": {
            "_saveVersion": 1,
            "expireTimeMs": expires_ms,
            "accessToken": access,
            "refreshToken": refresh
        }
    });

    let mut account = Account {
        name: String::new(),
        profile_id: [0u8; 16],
        file: PathBuf::new(),
        json,
        token: String::new(),
        expires_at_ms: 0,
    };
    account.refresh_from_msa()?;
    account.file = account_file(base, &account.name);
    account.save();
    Ok(account)
}

// ===================== Account =====================

impl Account {
    /// Gültiges Minecraft-Token (erneuert sich selbst, wenn es abläuft).
    pub fn token(&mut self) -> Res<&str> {
        self.ensure_fresh()?;
        Ok(&self.token)
    }

    /// Meldet die Sitzung bei Mojang an – Pflicht vor dem verschlüsselten Login.
    pub fn join_server(&mut self, server_hash: &str) -> Res<()> {
        let token = self.token()?.to_string();
        let profile = hex(&self.profile_id);
        let response = agent().post("https://sessionserver.mojang.com/session/minecraft/join")
            .send_json(json!({
                "accessToken": token,
                "selectedProfile": profile,
                "serverId": server_hash
            }));
        match response {
            Ok(_) => Ok(()),
            Err(e) => Err(format!("Sitzungs-Join abgelehnt: {}", short(e))),
        }
    }

    /// Bereits gespeicherte Token übernehmen, solange sie gültig sind (kein Netzverkehr beim Start).
    fn adopt_cached(&mut self) {
        if let Some(id) = self.json["minecraftProfile"]["id"].as_str() {
            if let Some(bytes) = uuid_from_str(id) {
                self.profile_id = bytes;
            }
        }
        if let Some(name) = self.json["minecraftProfile"]["name"].as_str() {
            self.name = name.to_string();
        }
        if let Some(token) = self.json["minecraftToken"]["token"].as_str() {
            self.token = token.to_string();
            self.expires_at_ms = self.json["minecraftToken"]["expireTimeMs"]
                .as_i64()
                .unwrap_or(0);
        }
    }

    fn ensure_fresh(&mut self) -> Res<()> {
        // Eine Minute Sicherheitsabstand: ein Token, das mitten im Login abläuft, hilft niemandem.
        if !self.token.is_empty() && self.expires_at_ms > now_ms() + 60_000 && !self.name.is_empty()
        {
            return Ok(());
        }
        self.refresh_from_msa()?;
        self.save();
        Ok(())
    }

    /// Volle Kette: MSA-Refresh -> XBL -> XSTS -> Minecraft-Token -> Profil.
    fn refresh_from_msa(&mut self) -> Res<()> {
        let client_id = self.json["msaApplicationConfig"]["clientId"]
            .as_str()
            .unwrap_or(CLIENT_ID)
            .to_string();
        let scope = self.json["msaApplicationConfig"]["scope"]
            .as_str()
            .unwrap_or(SCOPE)
            .to_string();
        let refresh_token = self.json["msaToken"]["refreshToken"]
            .as_str()
            .ok_or("Konto hat kein refreshToken – bitte neu anmelden.")?
            .to_string();

        let (msa_access, msa_refresh, msa_expires) = msa_refresh(&client_id, &scope, &refresh_token)?;
        let (xbl_token, _) = xbl_user_token(&msa_access, &client_id)?;
        let (xsts_token, user_hash) = xsts_token(&xbl_token)?;
        let (mc_token, mc_expires) = minecraft_token(&user_hash, &xsts_token)?;
        let (profile_id, profile_name) = minecraft_profile(&mc_token)?;

        // Nur unsere Felder anfassen – Device-/Title-Token des Java-Clients bleiben stehen.
        self.json["msaToken"] = json!({
            "_saveVersion": 1,
            "expireTimeMs": msa_expires,
            "accessToken": msa_access,
            "refreshToken": msa_refresh
        });
        self.json["minecraftToken"] = json!({
            "_saveVersion": 1,
            "expireTimeMs": mc_expires,
            "type": "Bearer",
            "token": mc_token.clone()
        });
        self.json["minecraftProfile"] = json!({
            "_saveVersion": 1,
            "id": uuid_to_dashed(&profile_id),
            "name": profile_name.clone()
        });

        self.token = mc_token;
        self.expires_at_ms = mc_expires;
        self.profile_id = profile_id;
        self.name = profile_name;
        Ok(())
    }

    fn save(&self) {
        if self.file.as_os_str().is_empty() {
            return;
        }
        if let Some(parent) = self.file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(&self.json) {
            let _ = std::fs::write(&self.file, text);
        }
    }
}

// ===================== Microsoft / Xbox / Minecraft =====================

fn request_device_code() -> Res<(String, DeviceCode)> {
    let response = agent().post(DEVICE_CODE_URL)
        .send_form(&[
            ("client_id", CLIENT_ID),
            ("scope", SCOPE),
            ("response_type", "device_code"),
        ])
        .map_err(|e| format!("Device-Code fehlgeschlagen: {}", short(e)))?;
    let json: Value = response
        .into_json()
        .map_err(|e| format!("Device-Code-Antwort unlesbar: {}", e))?;

    let device_code = field(&json, "device_code")?;
    Ok((
        device_code,
        DeviceCode {
            user_code: field(&json, "user_code")?,
            verification_uri: field(&json, "verification_uri")?,
        },
    ))
}

/// Wartet, bis der Nutzer im Browser bestätigt hat (max. 5 Minuten).
fn poll_for_token(device_code: &str) -> Res<(String, String, i64)> {
    let deadline = SystemTime::now() + Duration::from_secs(300);
    loop {
        let response = agent().post(TOKEN_URL).send_form(&[
            ("client_id", CLIENT_ID),
            ("grant_type", "device_code"),
            ("device_code", device_code),
        ]);
        match response {
            Ok(ok) => {
                let json: Value = ok.into_json().map_err(|e| e.to_string())?;
                return Ok((
                    field(&json, "access_token")?,
                    field(&json, "refresh_token")?,
                    now_ms() + json["expires_in"].as_i64().unwrap_or(3600) * 1000,
                ));
            }
            Err(ureq::Error::Status(400, resp)) => {
                let json: Value = resp.into_json().unwrap_or(Value::Null);
                let error = json["error"].as_str().unwrap_or("");
                if error != "authorization_pending" && error != "slow_down" {
                    return Err(format!("Microsoft-Login abgelehnt: {}", error));
                }
                if SystemTime::now() > deadline {
                    return Err("Zeitüberschreitung beim Microsoft-Login.".to_string());
                }
                std::thread::sleep(Duration::from_secs(5));
            }
            Err(e) => return Err(format!("Microsoft-Login fehlgeschlagen: {}", short(e))),
        }
    }
}

fn msa_refresh(client_id: &str, scope: &str, refresh_token: &str) -> Res<(String, String, i64)> {
    let json: Value = agent().post(TOKEN_URL)
        .send_form(&[
            ("client_id", client_id),
            ("scope", scope),
            ("grant_type", "refresh_token"),
            ("refresh_token", refresh_token),
        ])
        .map_err(|e| format!("MSA-Refresh fehlgeschlagen: {}", short(e)))?
        .into_json()
        .map_err(|e| e.to_string())?;

    Ok((
        field(&json, "access_token")?,
        // Manche Antworten enthalten kein neues Refresh-Token: dann das alte behalten.
        json["refresh_token"]
            .as_str()
            .unwrap_or(refresh_token)
            .to_string(),
        now_ms() + json["expires_in"].as_i64().unwrap_or(3600) * 1000,
    ))
}

fn xbl_user_token(msa_access: &str, client_id: &str) -> Res<(String, String)> {
    // Title-Client-IDs (keine UUID) liefern den Ticket mit "t=", MSAL-Anwendungen mit "d=".
    let prefix = if uuid_from_str(client_id).is_some() {
        "d="
    } else {
        "t="
    };
    let json: Value = agent().post("https://user.auth.xboxlive.com/user/authenticate")
        .set("x-xbl-contract-version", "1")
        .set("Accept", "application/json")
        .send_json(json!({
            "Properties": {
                "AuthMethod": "RPS",
                "SiteName": "user.auth.xboxlive.com",
                "RpsTicket": format!("{}{}", prefix, msa_access)
            },
            "RelyingParty": "http://auth.xboxlive.com",
            "TokenType": "JWT"
        }))
        .map_err(|e| format!("Xbox-Login fehlgeschlagen: {}", short(e)))?
        .into_json()
        .map_err(|e| e.to_string())?;

    Ok((field(&json, "Token")?, user_hash(&json)?))
}

fn xsts_token(xbl_token: &str) -> Res<(String, String)> {
    let response = agent().post("https://xsts.auth.xboxlive.com/xsts/authorize")
        .set("x-xbl-contract-version", "1")
        .set("Accept", "application/json")
        .send_json(json!({
            "Properties": { "SandboxId": "RETAIL", "UserTokens": [xbl_token] },
            "RelyingParty": "rp://api.minecraftservices.com/",
            "TokenType": "JWT"
        }));

    let json: Value = match response {
        Ok(ok) => ok.into_json().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(401, resp)) => {
            let body: Value = resp.into_json().unwrap_or(Value::Null);
            let reason = match body["XErr"].as_i64().unwrap_or(0) {
                2148916233 => "Das Microsoft-Konto hat kein Xbox-Profil.",
                2148916235 => "Xbox Live ist im Land des Kontos nicht verfügbar.",
                2148916238 => "Kinderkonto – es muss einer Familie hinzugefügt werden.",
                _ => "XSTS hat die Anmeldung abgelehnt.",
            };
            return Err(reason.to_string());
        }
        Err(e) => return Err(format!("XSTS fehlgeschlagen: {}", short(e))),
    };

    Ok((field(&json, "Token")?, user_hash(&json)?))
}

fn user_hash(json: &Value) -> Res<String> {
    json["DisplayClaims"]["xui"][0]["uhs"]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| "Xbox-Antwort ohne Benutzer-Hash".to_string())
}

fn minecraft_token(user_hash: &str, xsts: &str) -> Res<(String, i64)> {
    let json: Value = agent().post("https://api.minecraftservices.com/authentication/login_with_xbox")
        .send_json(json!({ "identityToken": format!("XBL3.0 x={};{}", user_hash, xsts) }))
        .map_err(|e| format!("Minecraft-Login fehlgeschlagen: {}", short(e)))?
        .into_json()
        .map_err(|e| e.to_string())?;

    Ok((
        field(&json, "access_token")?,
        now_ms() + json["expires_in"].as_i64().unwrap_or(86400) * 1000,
    ))
}

fn minecraft_profile(token: &str) -> Res<([u8; 16], String)> {
    let response = agent().get("https://api.minecraftservices.com/minecraft/profile")
        .set("Authorization", &format!("Bearer {}", token))
        .call();

    let json: Value = match response {
        Ok(ok) => ok.into_json().map_err(|e| e.to_string())?,
        Err(ureq::Error::Status(404, _)) => {
            return Err("Dieses Konto besitzt kein Minecraft-Java-Profil.".to_string())
        }
        Err(e) => return Err(format!("Profil-Abruf fehlgeschlagen: {}", short(e))),
    };

    let id = uuid_from_str(&field(&json, "id")?).ok_or("Profil-UUID unlesbar")?;
    Ok((id, field(&json, "name")?))
}

// ===================== Kryptografie-Hilfen =====================

/// Mojangs „Server-Hash": SHA-1 über serverId + Shared Secret + Server-Public-Key,
/// ausgegeben als vorzeichenbehaftete Hex-Zahl (negative Werte im Zweierkomplement mit „-").
pub fn server_hash(server_id: &str, secret: &[u8; 16], public_key_der: &[u8]) -> String {
    let mut sha = Sha1::new();
    sha.update(server_id.as_bytes());
    sha.update(secret);
    sha.update(public_key_der);
    let mut digest: [u8; 20] = sha.finalize().into();

    let negative = digest[0] & 0x80 != 0;
    if negative {
        let mut carry = true;
        for byte in digest.iter_mut().rev() {
            *byte = !*byte;
            if carry {
                let (value, overflow) = byte.overflowing_add(1);
                *byte = value;
                carry = overflow;
            }
        }
    }
    let text = hex(&digest);
    let trimmed = text.trim_start_matches('0');
    format!("{}{}", if negative { "-" } else { "" }, trimmed)
}

/// Erzeugt ein EC-P-256-Schlüsselpaar im selben Format, das der Java-Client erwartet
/// (Base64 von X.509-SubjectPublicKeyInfo bzw. PKCS#8).
fn device_key_pair() -> Res<Value> {
    use p256::pkcs8::{EncodePrivateKey, EncodePublicKey};
    let secret = p256::SecretKey::random(&mut rand::rngs::OsRng);
    let private_der = secret
        .to_pkcs8_der()
        .map_err(|e| format!("Geräteschlüssel: {}", e))?;
    let public_der = secret
        .public_key()
        .to_public_key_der()
        .map_err(|e| format!("Geräteschlüssel: {}", e))?;

    let engine = base64::engine::general_purpose::STANDARD;
    Ok(json!({
        "algorithm": "EC",
        "publicKey": engine.encode(public_der.as_bytes()),
        "privateKey": engine.encode(private_der.as_bytes())
    }))
}

fn random_uuid_dashed() -> String {
    let mut bytes = [0u8; 16];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    bytes[6] = (bytes[6] & 0x0F) | 0x40; // Version 4
    bytes[8] = (bytes[8] & 0x3F) | 0x80; // Variante
    uuid_to_dashed(&bytes)
}

// ===================== Kleinkram =====================

fn field(json: &Value, key: &str) -> Res<String> {
    json[key]
        .as_str()
        .map(|s| s.to_string())
        .ok_or_else(|| format!("Antwort ohne Feld '{}'", key))
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// ureq-Fehler kurz und lesbar machen (der Volltext enthält teils die ganze URL samt Token).
fn short(e: ureq::Error) -> String {
    match e {
        ureq::Error::Status(code, _) => format!("HTTP {}", code),
        ureq::Error::Transport(t) => format!("Netzwerkfehler ({})", t.kind()),
    }
}
