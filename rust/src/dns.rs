//! Minimaler SRV-Lookup (`_minecraft._tcp.<host>`).
//!
//! Ohne den geht es nicht: Viele Server – auch `hugosmp.net` – haben **gar keinen** A-Record,
//! sondern nur einen SRV-Eintrag, der auf einen anderen Host/Port zeigt. Der Java-Client bekommt
//! das über MCProtocolLib geschenkt; hier ist es ein knapper, abhängigkeitsfreier DNS-Client.
//!
//! Bewusst nur UDP, ein Versuch pro Resolver, kurze Timeouts: Schlägt alles fehl, wird schlicht
//! `host:port` direkt verwendet (genau wie ein Vanilla-Client, dessen SRV-Abfrage ins Leere läuft).

use std::net::UdpSocket;
use std::time::Duration;

const RESOLVERS: [&str; 3] = ["192.0.2.1:53", "192.0.2.1:53", "192.0.2.1:53"];
const TIMEOUT: Duration = Duration::from_millis(1500);
const TYPE_SRV: u16 = 33;

/// Liefert (Zielhost, Port), falls ein SRV-Eintrag existiert.
pub fn resolve_srv(host: &str) -> Option<(String, u16)> {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return None; // IP-Adresse: nichts aufzulösen
    }
    let query = build_query(&format!("_minecraft._tcp.{}", host))?;
    for resolver in RESOLVERS {
        if let Some(answer) = ask(resolver, &query) {
            return Some(answer);
        }
    }
    None
}

fn ask(resolver: &str, query: &[u8]) -> Option<(String, u16)> {
    let socket = UdpSocket::bind("0.0.0.0:0").ok()?;
    socket.set_read_timeout(Some(TIMEOUT)).ok()?;
    socket.set_write_timeout(Some(TIMEOUT)).ok()?;
    socket.send_to(query, resolver).ok()?;

    let mut buf = [0u8; 512];
    let (len, _) = socket.recv_from(&mut buf).ok()?;
    parse_answer(&buf[..len])
}

fn build_query(name: &str) -> Option<Vec<u8>> {
    let mut q = Vec::with_capacity(64);
    q.extend_from_slice(&[0x13, 0x37]); // Transaktions-ID (fest, wir prüfen nur eine Antwort)
    q.extend_from_slice(&[0x01, 0x00]); // Standardabfrage, Rekursion erwünscht
    q.extend_from_slice(&[0x00, 0x01]); // 1 Frage
    q.extend_from_slice(&[0x00, 0x00, 0x00, 0x00, 0x00, 0x00]); // keine Antworten/Autoritäten

    for label in name.split('.') {
        if label.is_empty() || label.len() > 63 {
            return None;
        }
        q.push(label.len() as u8);
        q.extend_from_slice(label.as_bytes());
    }
    q.push(0);
    q.extend_from_slice(&TYPE_SRV.to_be_bytes());
    q.extend_from_slice(&[0x00, 0x01]); // Klasse IN
    Some(q)
}

fn parse_answer(msg: &[u8]) -> Option<(String, u16)> {
    if msg.len() < 12 || msg[0] != 0x13 || msg[1] != 0x37 {
        return None;
    }
    let questions = u16::from_be_bytes([msg[4], msg[5]]);
    let answers = u16::from_be_bytes([msg[6], msg[7]]);
    if answers == 0 {
        return None;
    }

    let mut pos = 12;
    for _ in 0..questions {
        pos = skip_name(msg, pos)?;
        pos += 4; // Typ + Klasse
    }

    let mut best: Option<(u16, String, u16)> = None; // (Priorität, Ziel, Port)
    for _ in 0..answers {
        pos = skip_name(msg, pos)?;
        if pos + 10 > msg.len() {
            return None;
        }
        let rtype = u16::from_be_bytes([msg[pos], msg[pos + 1]]);
        let rdlen = u16::from_be_bytes([msg[pos + 8], msg[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlen > msg.len() {
            return None;
        }
        if rtype == TYPE_SRV && rdlen >= 7 {
            let priority = u16::from_be_bytes([msg[pos], msg[pos + 1]]);
            let port = u16::from_be_bytes([msg[pos + 4], msg[pos + 5]]);
            let (target, _) = read_name(msg, pos + 6, 0)?;
            if !target.is_empty() && best.as_ref().map_or(true, |(p, _, _)| priority < *p) {
                best = Some((priority, target, port));
            }
        }
        pos += rdlen;
    }
    best.map(|(_, target, port)| (target, port))
}

fn skip_name(msg: &[u8], mut pos: usize) -> Option<usize> {
    loop {
        let len = *msg.get(pos)? as usize;
        if len == 0 {
            return Some(pos + 1);
        }
        if len & 0xC0 == 0xC0 {
            return Some(pos + 2); // Zeiger beendet den Namen
        }
        pos += 1 + len;
    }
}

/// Namen lesen – inklusive der DNS-Zeigerkompression (0xC0-Präfix).
fn read_name(msg: &[u8], mut pos: usize, depth: u32) -> Option<(String, usize)> {
    if depth > 8 {
        return None; // Zeigerschleife
    }
    let mut out = String::new();
    loop {
        let len = *msg.get(pos)? as usize;
        if len == 0 {
            return Some((out, pos + 1));
        }
        if len & 0xC0 == 0xC0 {
            let ptr = ((len & 0x3F) << 8) | *msg.get(pos + 1)? as usize;
            let (tail, _) = read_name(msg, ptr, depth + 1)?;
            if !out.is_empty() && !tail.is_empty() {
                out.push('.');
            }
            out.push_str(&tail);
            return Some((out, pos + 2));
        }
        let label = msg.get(pos + 1..pos + 1 + len)?;
        if !out.is_empty() {
            out.push('.');
        }
        out.push_str(&String::from_utf8_lossy(label));
        pos += 1 + len;
    }
}
