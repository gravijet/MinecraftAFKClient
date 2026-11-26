//! Lokaler, token-geschuetzter HTTP-Viewer fuer die texturierte Live-POV bzw. (`--features
//! web-menu`, ohne `pov`) fuer Hotbar und Menue/Inventar ohne Kamerabild.
//!
//! Kein Framework und kein CDN: HTML, CSS und JavaScript kommen aus der Binary, Frames und
//! Zustandsdaten nur aus dem laufenden Client. Dadurch funktioniert die Ansicht offline und ein
//! fremder Webseiten-Tab kann nicht blind das lokale Menue anklicken.

pub mod assets;
pub mod resources;

use crate::client::Shared;
use rand::RngCore;
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::sync::atomic::Ordering;
use std::sync::mpsc::{sync_channel, TrySendError};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const MAX_REQUEST: usize = 16 * 1024;
const WEB_WORKERS: usize = 4;
const CONNECTION_QUEUE: usize = 8;

pub(crate) fn start(shared: &Arc<Shared>, address: SocketAddr) -> Result<(), String> {
    let listener = TcpListener::bind(address)
        .map_err(|error| format!("{} kann nicht geoeffnet werden: {}", address, error))?;
    let token = access_token();
    let shown_host = if address.ip().is_unspecified() {
        if address.is_ipv6() {
            "[::1]".to_string()
        } else {
            "127.0.0.1".to_string()
        }
    } else if address.is_ipv6() {
        // Die Klammern gehoeren in eine URL, `SocketAddr::ip()` liefert sie nicht.
        format!("[{}]", address.ip())
    } else {
        address.ip().to_string()
    };
    let url = format!("http://{}:{}/?token={}", shown_host, address.port(), token);
    shared.console.info(&format!("Browser-POV: {}", url));
    if !address.ip().is_loopback() {
        shared.console.warn(
            "Browser-POV lauscht ausserhalb von localhost. Die URL enthaelt den Zugriffstoken; nicht weitergeben.",
        );
    }
    if let Some(note) = asset_note(shared) {
        shared.console.info(&format!("Browser-POV: {}", note));
    }

    let owned = Arc::clone(shared);
    thread::Builder::new()
        .name("afk-pov-web".into())
        .spawn(move || {
            serve(listener, owned, token);
        })
        .map_err(|error| format!("HTTP-Thread liess sich nicht starten: {}", error))?;
    Ok(())
}

fn access_token() -> String {
    let mut bytes = [0u8; 16];
    rand::thread_rng().fill_bytes(&mut bytes);
    bytes.iter().map(|byte| format!("{:02x}", byte)).collect()
}

fn serve(listener: TcpListener, shared: Arc<Shared>, token: String) {
    let (sender, receiver) = sync_channel::<TcpStream>(CONNECTION_QUEUE);
    let receiver = Arc::new(Mutex::new(receiver));
    let token: Arc<str> = token.into();
    let mut workers = Vec::with_capacity(WEB_WORKERS);
    for index in 0..WEB_WORKERS {
        let receiver = Arc::clone(&receiver);
        let worker_shared = Arc::clone(&shared);
        let token = Arc::clone(&token);
        match thread::Builder::new()
            .name(format!("afk-pov-http-{}", index + 1))
            .spawn(move || loop {
                let stream = {
                    let Ok(receiver) = receiver.lock() else {
                        return;
                    };
                    receiver.recv()
                };
                match stream {
                    Ok(stream) => handle(stream, &worker_shared, &token),
                    Err(_) => return,
                }
            }) {
            Ok(worker) => workers.push(worker),
            Err(error) => shared.console.warn(&format!(
                "Browser-POV: HTTP-Arbeiter konnte nicht starten: {}",
                error
            )),
        }
    }
    if workers.is_empty() {
        shared
            .console
            .error("Browser-POV: Kein HTTP-Arbeiter konnte gestartet werden");
        return;
    }

    // Blockierendes `accept`. Vorher lief der Listener nicht-blockierend in einer 10-ms-Schleife:
    // Der Prozess wachte dadurch hundertmal je Sekunde auf, sein Leben lang, ohne dass jemals
    // etwas passiert wäre – ausgerechnet in einem Client, dessen Aufgabe das Nichtstun ist.
    // Blockierend wartet der Thread stattdessen im Kern und kostet nichts.
    while shared.running.load(Ordering::Relaxed) {
        match listener.accept() {
            Ok((stream, _)) => match sender.try_send(stream) {
                Ok(()) => {}
                Err(TrySendError::Full(mut stream)) => {
                    let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
                    response(
                        &mut stream,
                        503,
                        "text/plain; charset=utf-8",
                        b"Browser-POV ist ausgelastet",
                    );
                }
                Err(TrySendError::Disconnected(_)) => break,
            },
            // Ein unterbrochener Systemaufruf ist kein Fehler, sondern ein Signal – weitermachen.
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => {}
            Err(error) => {
                shared.console.warn(&format!(
                    "Browser-POV: Verbindung fehlgeschlagen: {}",
                    error
                ));
                // Dauerfehler (etwa erschöpfte Dateizeiger) sollen keine heiße Schleife ergeben.
                thread::sleep(Duration::from_millis(50));
            }
        }
    }
    drop(sender);
    for worker in workers {
        let _ = worker.join();
    }
}

/// Original-Ressourcen kommen je nach Bauform aus `pov::Pov` (volle Live-Ansicht, `feature =
/// "pov"`) oder aus `pov_assets::WebAssets` (`--features web-menu`, ohne Weltteil) – hier an
/// einer Stelle entschieden, damit die Endpunkte darunter nicht zweimal geschrieben werden müssen.
#[cfg(feature = "pov")]
fn asset_note(shared: &Arc<Shared>) -> Option<String> {
    shared.extras.pov.asset_note()
}
#[cfg(not(feature = "pov"))]
fn asset_note(shared: &Arc<Shared>) -> Option<String> {
    shared.extras.web.note()
}

#[cfg(feature = "pov")]
fn asset_bytes(shared: &Arc<Shared>, path: &str) -> Option<Vec<u8>> {
    crate::pov::web_asset(shared, path)
}
#[cfg(not(feature = "pov"))]
fn asset_bytes(shared: &Arc<Shared>, path: &str) -> Option<Vec<u8>> {
    crate::web::assets::web_asset(shared, path)
}

#[cfg(feature = "pov")]
fn missing_bytes(shared: &Arc<Shared>) -> Option<Vec<u8>> {
    crate::pov::web_missing(shared)
}
#[cfg(not(feature = "pov"))]
fn missing_bytes(shared: &Arc<Shared>) -> Option<Vec<u8>> {
    crate::web::assets::web_missing(shared)
}

#[cfg(feature = "pov")]
fn item_bytes(shared: &Arc<Shared>, id: i32) -> Option<Vec<u8>> {
    crate::pov::web_item(shared, id)
}
#[cfg(not(feature = "pov"))]
fn item_bytes(shared: &Arc<Shared>, id: i32) -> Option<Vec<u8>> {
    crate::web::assets::web_item(shared, id)
}

/// Zustand für `/api/state.json` ohne den `pov`-Weltteil: nur Verbindung, Position und ob
/// Texturen bereitstehen – keine Chunks/Entities/Dimension, die es in dieser Bauform nicht gibt.
#[cfg(not(feature = "pov"))]
fn menu_state(shared: &Arc<Shared>) -> serde_json::Value {
    let position = shared.position();
    serde_json::json!({
        "connected": shared.in_game.load(Ordering::Relaxed),
        "position": position.map(|value| serde_json::json!({
            "x": value.0, "y": value.1, "z": value.2,
            "yaw": value.3, "pitch": value.4
        })),
        "textures": shared.extras.web.get().is_some(),
        "texture_error": shared.extras.web.note(),
    })
}

fn handle(mut stream: TcpStream, shared: &Arc<Shared>, token: &str) {
    let _ = stream.set_read_timeout(Some(Duration::from_secs(1)));
    let _ = stream.set_write_timeout(Some(Duration::from_secs(5)));
    let request = match read_request(&mut stream) {
        Ok(request) => request,
        Err(()) => {
            return response(
                &mut stream,
                400,
                "text/plain; charset=utf-8",
                b"Ungueltige oder zu grosse HTTP-Anfrage",
            )
        }
    };
    let Some(first) = request.lines().next() else {
        return;
    };
    let mut parts = first.split_whitespace();
    let method = parts.next().unwrap_or("");
    let target = parts.next().unwrap_or("/");
    let version = parts.next().unwrap_or("");
    if parts.next().is_some() || !matches!(version, "HTTP/1.0" | "HTTP/1.1") {
        return response(
            &mut stream,
            400,
            "text/plain; charset=utf-8",
            b"Ungueltige HTTP-Anfrage",
        );
    }
    let (path, query) = target.split_once('?').unwrap_or((target, ""));

    let authorized = query_value(query, "token") == Some(token);
    if !authorized {
        return response(
            &mut stream,
            403,
            "text/plain; charset=utf-8",
            b"Zugriffstoken fehlt. Die vollstaendige URL steht im Terminal des AFK-Clients.",
        );
    }

    if method == "GET" && path.starts_with("/assets/") {
        let resource = path.trim_start_matches('/');
        if !resource.ends_with(".png") || resource.contains("..") || resource.contains('\\') {
            return response(
                &mut stream,
                400,
                "text/plain; charset=utf-8",
                b"Ungueltiger Ressourcenpfad",
            );
        }
        let Some(bytes) = asset_bytes(shared, resource).or_else(|| missing_bytes(shared)) else {
            return response(
                &mut stream,
                503,
                "text/plain; charset=utf-8",
                b"Keine Minecraft-Ressourcen geladen",
            );
        };
        return immutable_response(&mut stream, 200, "image/png", &bytes);
    }

    match (method, path) {
        ("GET", "/") => {
            let page = page(token);
            response(
                &mut stream,
                200,
                "text/html; charset=utf-8",
                page.as_bytes(),
            );
        }
        ("GET", "/api/state.json") => {
            #[cfg(feature = "pov")]
            let mut value = crate::pov::web_world(shared);
            #[cfg(not(feature = "pov"))]
            let mut value = menu_state(shared);
            if let Some(object) = value.as_object_mut() {
                object.insert("menu".to_string(), crate::menu::web_state(shared));
                object.insert(
                    "selected_hotbar".to_string(),
                    shared.extras.selected_hotbar.load(Ordering::Relaxed).into(),
                );
                // Die Seite wählt damit die Texturpfade: 1.8.9 hat noch keine HUD-Sprites.
                object.insert("version".to_string(), shared.proto.name.into());
            }
            match serde_json::to_vec(&value) {
                Ok(bytes) => response(&mut stream, 200, "application/json", &bytes),
                Err(_) => response(&mut stream, 500, "application/json", b"{}"),
            }
        }
        #[cfg(feature = "pov")]
        ("GET", "/api/frame.png") => {
            let width = query_value(query, "w")
                .and_then(|value| value.parse().ok())
                .unwrap_or(426);
            let height = query_value(query, "h")
                .and_then(|value| value.parse().ok())
                .unwrap_or(240);
            match crate::pov::web_frame(shared, width, height) {
                Ok(bytes) => response(&mut stream, 200, "image/png", &bytes),
                Err(error) => response(
                    &mut stream,
                    503,
                    "text/plain; charset=utf-8",
                    error.as_bytes(),
                ),
            }
        }
        ("GET", "/api/item.png") => {
            let id = query_value(query, "id")
                .and_then(|value| value.parse().ok())
                .unwrap_or(-1);
            let Some(bytes) = item_bytes(shared, id).or_else(|| missing_bytes(shared)) else {
                return response(
                    &mut stream,
                    503,
                    "text/plain; charset=utf-8",
                    b"Keine Minecraft-Ressourcen geladen",
                );
            };
            immutable_response(&mut stream, 200, "image/png", &bytes);
        }
        ("POST", "/api/click") => {
            let slot = query_value(query, "slot").unwrap_or("");
            let Some(action) = click_action(query) else {
                return response(
                    &mut stream,
                    400,
                    "text/plain; charset=utf-8",
                    b"Ungueltige Klickart",
                );
            };
            let status = if crate::menu::click_command(shared, &format!("{} {}", slot, action)) {
                204
            } else {
                400
            };
            response(&mut stream, status, "text/plain", b"");
        }
        ("POST", "/api/close") => {
            let status = if crate::menu::close_command(shared) {
                204
            } else {
                400
            };
            response(&mut stream, status, "text/plain", b"");
        }
        ("POST", "/api/hotbar") => {
            let slot = query_value(query, "slot").and_then(|value| value.parse::<usize>().ok());
            let status = match slot {
                Some(slot) if crate::extras::web_hotbar(shared, slot) => 204,
                _ => 400,
            };
            response(&mut stream, status, "text/plain", b"");
        }
        _ => response(
            &mut stream,
            404,
            "text/plain; charset=utf-8",
            b"Nicht gefunden",
        ),
    }
}

fn read_request(stream: &mut TcpStream) -> Result<String, ()> {
    let mut bytes = Vec::with_capacity(1024);
    let mut chunk = [0u8; 1024];
    while bytes.len() < MAX_REQUEST {
        let remaining = MAX_REQUEST - bytes.len();
        let read_len = remaining.min(chunk.len());
        let count = stream.read(&mut chunk[..read_len]).map_err(|_| ())?;
        if count == 0 {
            return Err(());
        }
        bytes.extend_from_slice(&chunk[..count]);
        if bytes.windows(4).any(|part| part == b"\r\n\r\n") {
            return String::from_utf8(bytes).map_err(|_| ());
        }
    }
    Err(())
}

fn query_value<'a>(query: &'a str, name: &str) -> Option<&'a str> {
    query.split('&').find_map(|pair| {
        let (key, value) = pair.split_once('=')?;
        (key == name).then_some(value)
    })
}

fn click_action(query: &str) -> Option<&'static str> {
    match query_value(query, "action") {
        None | Some("left") => Some("links"),
        Some("right") => Some("rechts"),
        Some("shift") => Some("shift"),
        Some(_) => None,
    }
}

fn response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    response_with_cache(stream, status, content_type, body, "no-store");
}

fn immutable_response(stream: &mut TcpStream, status: u16, content_type: &str, body: &[u8]) {
    response_with_cache(
        stream,
        status,
        content_type,
        body,
        "private, max-age=31536000, immutable",
    );
}

fn response_with_cache(
    stream: &mut TcpStream,
    status: u16,
    content_type: &str,
    body: &[u8],
    cache_control: &str,
) {
    let reason = match status {
        200 => "OK",
        204 => "No Content",
        400 => "Bad Request",
        403 => "Forbidden",
        404 => "Not Found",
        503 => "Service Unavailable",
        _ => "Internal Server Error",
    };
    let header = format!(
        "HTTP/1.1 {} {}\r\nContent-Type: {}\r\nContent-Length: {}\r\nCache-Control: {}\r\nX-Content-Type-Options: nosniff\r\nX-Frame-Options: DENY\r\nReferrer-Policy: no-referrer\r\nCross-Origin-Resource-Policy: same-origin\r\nPermissions-Policy: camera=(), microphone=(), geolocation=()\r\nContent-Security-Policy: default-src 'self'; img-src 'self' blob:; style-src 'unsafe-inline'; script-src 'unsafe-inline'; connect-src 'self'; frame-ancestors 'none'; base-uri 'none'; form-action 'none'\r\nConnection: close\r\n\r\n",
        status,
        reason,
        content_type,
        body.len(),
        cache_control
    );
    let _ = stream.write_all(header.as_bytes());
    let _ = stream.write_all(body);
    let _ = stream.flush();
}

/// Die Seite des Viewers, bewusst ohne Bibliothek oder fremde Assets. Alle Texturen kommen aus
/// der vom Nutzer bereitgestellten Original-JAR. Beide Bauformen teilen dieselbe Datei; `web-menu`
/// ohne `pov` hat kein Kamerabild und schaltet es über `__CAMERA__` ab.
const PAGE: &str = include_str!("viewer.html");

fn page(token: &str) -> String {
    PAGE.replace("__TOKEN__", token).replace(
        "__CAMERA__",
        if cfg!(feature = "pov") {
            "true"
        } else {
            "false"
        },
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn query_liest_nur_ganze_schluessel() {
        assert_eq!(query_value("token=abc&slot=4", "slot"), Some("4"));
        assert_eq!(query_value("not_token=x", "token"), None);
    }

    #[test]
    fn unbekannte_klickart_wird_nicht_als_linksklick_ausgefuehrt() {
        assert_eq!(click_action("slot=4&action=right"), Some("rechts"));
        assert_eq!(click_action("slot=4"), Some("links"));
        assert_eq!(click_action("slot=4&action=double"), None);
    }

    #[test]
    fn zugriffstoken_hat_128_bit() {
        let token = access_token();
        assert_eq!(token.len(), 32);
        assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));
    }

    #[test]
    fn seite_hat_keine_fremden_abhaengigkeiten() {
        let page = page("abc123");
        assert!(!page.contains("https://") && !page.contains("http://"));
        assert!(!page.contains("__TOKEN__") && !page.contains("__CAMERA__"));
        assert!(page.contains("const TOKEN = 'abc123';"));
        // Jede JAR-Ressource läuft über `texture()`, das den Zugriffstoken anhängt.
        assert_eq!(page.matches("/assets/").count(), 1);
        assert!(
            page.contains("const texture = (path) => api('/assets/minecraft/textures/' + path);")
        );
        #[cfg(feature = "pov")]
        assert!(page.contains("const CAMERA = true;"));
        #[cfg(not(feature = "pov"))]
        assert!(page.contains("const CAMERA = false;"));
    }
}
