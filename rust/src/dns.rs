//! Minimaler SRV-Lookup (`_minecraft._tcp.<host>`).
//!
//! Ohne den geht es nicht: Viele Server – auch `hugosmp.net` – haben **gar keinen** A-Record,
//! sondern nur einen SRV-Eintrag, der auf einen anderen Host/Port zeigt. Der Java-Client bekommt
//! das über MCProtocolLib geschenkt; hier ist es ein knapper, abhängigkeitsfreier DNS-Client.
//!
//! Bewusst nur UDP, ein Versuch pro Resolver, kurze Timeouts: Schlägt alles fehl, wird schlicht
//! `host:port` direkt verwendet (genau wie ein Vanilla-Client, dessen SRV-Abfrage ins Leere läuft).
//!
//! Gefragt wird **zuerst der Resolver des Rechners**, danach erst die drei öffentlichen als
//! Ersatz. Das ist kein Detail: In vielen Netzen ist ausgehendes UDP/53 nach draußen gesperrt
//! (Firmennetz, Schulnetz, mancher Hoster). Dort lief vorher jeder einzelne Verbindungsversuch
//! erst in drei Zeitabläufe – viereinhalb Sekunden, bevor überhaupt ein TCP-SYN hinausging.
//! Zudem findet nur der Systemresolver einen Server, dessen SRV-Eintrag in einer internen Zone
//! steht.

use std::collections::HashMap;
use std::net::UdpSocket;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

/// Ersatz, wenn der Rechner keinen brauchbaren Resolver nennt.
const FALLBACK_RESOLVERS: [&str; 3] = ["192.0.2.1:53", "192.0.2.1:53", "192.0.2.1:53"];
/// Zeitlimit einer einzelnen Abfrage.
const TIMEOUT: Duration = Duration::from_millis(1200);
/// Gesamtbudget der SRV-Auflösung. Sie sitzt vor dem Verbindungsaufbau und blockiert den
/// Netz-Thread; ohne Budget summierten sich sechs erfolglose Abfragen auf über sieben Sekunden.
const TOTAL_TIMEOUT: Duration = Duration::from_secs(3);
/// So lange gilt eine einmal geholte Auskunft (auch ein „gibt es nicht").
///
/// Der Client löst nur beim Verbindungsaufbau auf, und der kommt selten – außer bei einem Server,
/// der uns im Kreis weiterreicht. Genau dort verhindert dieser kurze Speicher, dass jede Runde
/// erneut das volle Zeitbudget kostet. Kurz genug, dass eine echte Umstellung im DNS binnen einer
/// Minute ankommt.
const CACHE_TTL: Duration = Duration::from_secs(60);
const TYPE_SRV: u16 = 33;

/// Was eine einzelne Resolver-Abfrage ergeben hat.
enum Reply {
    /// Ein SRV-Eintrag – fertig.
    Found(String, u16),
    /// Der Resolver hat gültig geantwortet, es gibt aber keinen SRV-Eintrag. Das ist eine
    /// **Auskunft**, keine Störung: weitere Resolver zu fragen kann nichts anderes ergeben.
    Empty,
    /// Keine oder keine brauchbare Antwort (Zeitablauf, SERVFAIL, gesperrter Port). Erst hier
    /// lohnt der nächste Resolver.
    Unusable,
}

/// Liefert (Zielhost, Port), falls ein SRV-Eintrag existiert.
///
/// Wichtig fürs Tempo: Die allermeisten Server **haben** keinen SRV-Eintrag. Früher lief die
/// Schleife dann durch alle drei Resolver, obwohl schon der erste verbindlich „gibt es nicht"
/// gesagt hatte – bei blockiertem UDP kostete das vor jedem Verbindungsversuch 4,5 Sekunden.
/// Jetzt endet die Suche mit der ersten gültigen Auskunft.
pub fn resolve_srv(host: &str) -> Option<(String, u16)> {
    if host.parse::<std::net::IpAddr>().is_ok() {
        return None; // IP-Adresse: nichts aufzulösen
    }
    if let Some(cached) = cache_get(host) {
        return cached;
    }
    let found = query_srv(host);
    cache_put(host, found.clone());
    found
}

fn query_srv(host: &str) -> Option<(String, u16)> {
    // Zufällige Transaktions-ID und Prüfung derselben in der Antwort: eine feste Kennung wäre
    // für jeden, der auf dem Weg mitliest, eine Einladung, eine eigene Antwort vorzulegen.
    let mut id = [0u8; 2];
    rand::RngCore::fill_bytes(&mut rand::rngs::OsRng, &mut id);
    let query = build_query(&format!("_minecraft._tcp.{}", host), id)?;
    let deadline = Instant::now() + TOTAL_TIMEOUT;
    for resolver in resolvers() {
        let left = deadline.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return None;
        }
        match ask(&resolver, &query, id, TIMEOUT.min(left)) {
            Reply::Found(target, port) => return Some((target, port)),
            Reply::Empty => return None,
            Reply::Unusable => {}
        }
    }
    None
}

/// Zu fragende Resolver: erst die des Rechners, dahinter die öffentlichen als Ersatz.
fn resolvers() -> Vec<String> {
    let mut list = system_resolvers();
    for fallback in FALLBACK_RESOLVERS {
        if !list.iter().any(|known| known == fallback) {
            list.push(fallback.to_string());
        }
    }
    list
}

/// Die Resolver des Systems. Unter Unix stehen sie in `/etc/resolv.conf`; unter Windows gibt es
/// dafür keinen Weg ohne Systembibliothek, dort bleiben die öffentlichen die erste Wahl.
#[cfg(unix)]
fn system_resolvers() -> Vec<String> {
    std::fs::read_to_string("/etc/resolv.conf")
        .map(|text| parse_resolv_conf(&text))
        .unwrap_or_default()
}

#[cfg(not(unix))]
fn system_resolvers() -> Vec<String> {
    Vec::new()
}

/// `nameserver`-Zeilen aus `resolv.conf` lesen.
///
/// Bewusst als reine Funktion über den Text: nur so lässt sich das Format prüfen, ohne dass der
/// Test von der Datei des Rechners abhängt, auf dem er gerade läuft.
#[cfg(any(unix, test))]
fn parse_resolv_conf(text: &str) -> Vec<String> {
    let mut out = Vec::new();
    for line in text.lines() {
        // Kommentare beginnen mit '#' oder ';'.
        let line = line.split(['#', ';']).next().unwrap_or("").trim();
        let Some(rest) = line.strip_prefix("nameserver") else {
            continue;
        };
        // Ohne Trennzeichen wäre „nameserverX" ein Treffer.
        if !rest.starts_with(char::is_whitespace) {
            continue;
        }
        let Ok(address) = rest.trim().parse::<std::net::IpAddr>() else {
            continue;
        };
        let formatted = match address {
            std::net::IpAddr::V4(v4) => format!("{}:53", v4),
            std::net::IpAddr::V6(v6) => format!("[{}]:53", v6),
        };
        if !out.contains(&formatted) {
            out.push(formatted);
        }
        // `resolv.conf` erlaubt selbst höchstens drei Einträge (MAXNS).
        if out.len() == 3 {
            break;
        }
    }
    out
}

/// Kurzzeitspeicher der Auskünfte, siehe [`CACHE_TTL`].
#[allow(clippy::type_complexity)]
fn cache() -> &'static Mutex<HashMap<String, (Instant, Option<(String, u16)>)>> {
    static CACHE: OnceLock<Mutex<HashMap<String, (Instant, Option<(String, u16)>)>>> =
        OnceLock::new();
    CACHE.get_or_init(|| Mutex::new(HashMap::new()))
}

/// `None` = nichts (mehr) gespeichert; `Some(auskunft)` = gespeicherte Auskunft, auch ein „nein".
fn cache_get(host: &str) -> Option<Option<(String, u16)>> {
    let mut cache = cache().lock().ok()?;
    let (at, value) = cache.get(host)?;
    if at.elapsed() >= CACHE_TTL {
        cache.remove(host);
        return None;
    }
    Some(value.clone())
}

fn cache_put(host: &str, value: Option<(String, u16)>) {
    let Ok(mut cache) = cache().lock() else {
        return;
    };
    // Abgelaufene Einträge mitnehmen: Die Ablage wächst sonst mit jedem je gefragten Namen.
    cache.retain(|_, (at, _)| at.elapsed() < CACHE_TTL);
    cache.insert(host.to_string(), (Instant::now(), value));
}

fn ask(resolver: &str, query: &[u8], id: [u8; 2], timeout: Duration) -> Reply {
    // Zum Ziel passende Adressfamilie binden – an einen IPv6-Resolver kommt ein IPv4-Socket nicht.
    let bind = if resolver.starts_with('[') {
        "[::]:0"
    } else {
        "0.0.0.0:0"
    };
    let Ok(socket) = UdpSocket::bind(bind) else {
        return Reply::Unusable;
    };
    if socket.set_read_timeout(Some(timeout)).is_err()
        || socket.set_write_timeout(Some(timeout)).is_err()
        || socket.send_to(query, resolver).is_err()
    {
        return Reply::Unusable;
    }

    let mut buf = [0u8; 512];
    let Ok((len, from)) = socket.recv_from(&mut buf) else {
        return Reply::Unusable;
    };
    // Nur die Antwort des gefragten Resolvers zählt.
    if from.to_string() != resolver {
        return Reply::Unusable;
    }
    parse_answer(&buf[..len], id)
}

fn build_query(name: &str, id: [u8; 2]) -> Option<Vec<u8>> {
    let mut q = Vec::with_capacity(64);
    q.extend_from_slice(&id); // Transaktions-ID
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

fn parse_answer(msg: &[u8], id: [u8; 2]) -> Reply {
    if msg.len() < 12 || msg[0] != id[0] || msg[1] != id[1] {
        return Reply::Unusable;
    }
    // Antwortbit muss gesetzt sein, sonst ist es überhaupt keine Antwort auf unsere Frage.
    if msg[2] & 0x80 == 0 {
        return Reply::Unusable;
    }
    match msg[3] & 0x0F {
        // NOERROR: weiterlesen. NXDOMAIN: den Namen gibt es nicht – das ist eine verbindliche
        // Auskunft und heißt schlicht „kein SRV-Eintrag".
        0 => {}
        3 => return Reply::Empty,
        // FORMERR/SERVFAIL/REFUSED: eine Störung dieses Resolvers, der nächste darf es versuchen.
        _ => return Reply::Unusable,
    }
    // Abgeschnittene Antworten (TC) enthalten womöglich nicht alle Einträge; dann lieber den
    // nächsten Resolver fragen, statt aus einem Bruchstück zu schließen.
    let truncated = msg[2] & 0x02 != 0;
    let questions = u16::from_be_bytes([msg[4], msg[5]]);
    let answers = u16::from_be_bytes([msg[6], msg[7]]);
    if answers == 0 {
        return if truncated {
            Reply::Unusable
        } else {
            Reply::Empty
        };
    }

    let mut pos = 12;
    for _ in 0..questions {
        let Some(next) = skip_name(msg, pos) else {
            return Reply::Unusable;
        };
        pos = next + 4; // Typ + Klasse
    }

    let mut best: Option<(u16, String, u16)> = None; // (Priorität, Ziel, Port)
    for _ in 0..answers {
        let Some(next) = skip_name(msg, pos) else {
            return Reply::Unusable;
        };
        pos = next;
        if pos + 10 > msg.len() {
            return Reply::Unusable;
        }
        let rtype = u16::from_be_bytes([msg[pos], msg[pos + 1]]);
        let rdlen = u16::from_be_bytes([msg[pos + 8], msg[pos + 9]]) as usize;
        pos += 10;
        if pos + rdlen > msg.len() {
            return Reply::Unusable;
        }
        if rtype == TYPE_SRV && rdlen >= 7 {
            let priority = u16::from_be_bytes([msg[pos], msg[pos + 1]]);
            let port = u16::from_be_bytes([msg[pos + 4], msg[pos + 5]]);
            // Ein Ziel „." (leer) heißt im SRV-Standard ausdrücklich „Dienst nicht verfügbar";
            // dann gilt wie ohne Eintrag die ursprüngliche Adresse.
            if let Some((target, _)) = read_name(msg, pos + 6, 0) {
                if !target.is_empty() && best.as_ref().is_none_or(|(p, _, _)| priority < *p) {
                    best = Some((priority, target, port));
                }
            }
        }
        pos += rdlen;
    }
    match best {
        Some((_, target, port)) => Reply::Found(target, port),
        None if truncated => Reply::Unusable,
        None => Reply::Empty,
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    const ID: [u8; 2] = [0x12, 0x34];

    fn name(labels: &[&str]) -> Vec<u8> {
        let mut out = Vec::new();
        for label in labels {
            out.push(label.len() as u8);
            out.extend_from_slice(label.as_bytes());
        }
        out.push(0);
        out
    }

    /// Eine Antwort mit einer Frage und beliebig vielen SRV-Einträgen.
    fn response(rcode: u8, flags: u8, srv: &[(u16, u16, &[&str])]) -> Vec<u8> {
        let mut msg = Vec::new();
        msg.extend_from_slice(&ID);
        msg.push(0x80 | flags); // Antwortbit (+ ggf. abgeschnitten)
        msg.push(rcode);
        msg.extend_from_slice(&1u16.to_be_bytes()); // Fragen
        msg.extend_from_slice(&(srv.len() as u16).to_be_bytes()); // Antworten
        msg.extend_from_slice(&[0, 0, 0, 0]);
        msg.extend_from_slice(&name(&["_minecraft", "_tcp", "example", "net"]));
        msg.extend_from_slice(&TYPE_SRV.to_be_bytes());
        msg.extend_from_slice(&[0, 1]); // Klasse IN

        for (priority, port, target) in srv {
            msg.extend_from_slice(&[0xC0, 0x0C]); // Name als Zeiger auf die Frage
            msg.extend_from_slice(&TYPE_SRV.to_be_bytes());
            msg.extend_from_slice(&[0, 1]); // Klasse IN
            msg.extend_from_slice(&300u32.to_be_bytes()); // TTL
            let mut rdata = Vec::new();
            rdata.extend_from_slice(&priority.to_be_bytes());
            rdata.extend_from_slice(&0u16.to_be_bytes()); // Gewicht
            rdata.extend_from_slice(&port.to_be_bytes());
            rdata.extend_from_slice(&name(target));
            msg.extend_from_slice(&(rdata.len() as u16).to_be_bytes());
            msg.extend_from_slice(&rdata);
        }
        msg
    }

    #[test]
    fn srv_eintrag_wird_gelesen() {
        let msg = response(0, 0, &[(10, 25577, &["mc", "example", "net"])]);
        match parse_answer(&msg, ID) {
            Reply::Found(target, port) => {
                assert_eq!(target, "mc.example.net");
                assert_eq!(port, 25577);
            }
            _ => panic!("SRV-Eintrag nicht erkannt"),
        }
    }

    /// Mehrere Einträge: der mit der kleinsten Priorität gewinnt – egal, in welcher Reihenfolge
    /// der Resolver sie liefert.
    #[test]
    fn kleinste_prioritaet_gewinnt() {
        for order in [
            vec![
                (30u16, 1u16, &["c", "example", "net"][..]),
                (5, 2, &["a", "example", "net"][..]),
            ],
            vec![
                (5, 2, &["a", "example", "net"][..]),
                (30, 1, &["c", "example", "net"][..]),
            ],
        ] {
            let msg = response(0, 0, &order);
            match parse_answer(&msg, ID) {
                Reply::Found(target, port) => {
                    assert_eq!(target, "a.example.net");
                    assert_eq!(port, 2);
                }
                _ => panic!("kein Eintrag"),
            }
        }
    }

    /// Der Kern der Beschleunigung: Sagt ein Resolver verbindlich „kein SRV-Eintrag", ist die
    /// Suche zu Ende. Vorher liefen dafür noch zwei weitere Abfragen samt Zeitfenster – vor
    /// **jedem** Verbindungsversuch.
    #[test]
    fn verbindliche_auskunft_beendet_die_suche() {
        assert!(matches!(
            parse_answer(&response(0, 0, &[]), ID),
            Reply::Empty
        ));
        assert!(matches!(
            parse_answer(&response(3, 0, &[]), ID),
            Reply::Empty
        )); // NXDOMAIN
            // Nur A-Records, kein SRV: ebenfalls eine gültige Auskunft.
        let mut msg = response(0, 0, &[(1, 25565, &["mc", "example", "net"])]);
        msg[response(0, 0, &[]).len() + 2] = 0; // Typ des Antworteintrags auf A (1) ...
        msg[response(0, 0, &[]).len() + 3] = 1;
        assert!(matches!(parse_answer(&msg, ID), Reply::Empty));
    }

    /// Eine Störung dieses Resolvers ist keine Auskunft – dann darf der nächste ran.
    #[test]
    fn stoerungen_gehen_an_den_naechsten_resolver() {
        assert!(matches!(
            parse_answer(&response(2, 0, &[]), ID),
            Reply::Unusable
        )); // SERVFAIL
        assert!(matches!(
            parse_answer(&response(5, 0, &[]), ID),
            Reply::Unusable
        )); // REFUSED
            // Abgeschnitten: das „keine Antworten" könnte am Kürzen liegen.
        assert!(matches!(
            parse_answer(&response(0, 0x02, &[]), ID),
            Reply::Unusable
        ));
        // Falsche Transaktions-ID (untergeschobene Antwort) und Müll.
        assert!(matches!(
            parse_answer(&response(0, 0, &[]), [9, 9]),
            Reply::Unusable
        ));
        assert!(matches!(parse_answer(&[], ID), Reply::Unusable));
    }

    /// Der Resolver des Rechners wird zuerst gefragt – in Netzen mit gesperrtem UDP/53 nach
    /// draußen ist er der einzige, der überhaupt antwortet.
    #[test]
    fn resolv_conf_wird_gelesen() {
        let text = "\
# Kommentar
domain example.net
nameserver 192.0.2.1
nameserver 192.0.2.1  ; noch ein Kommentar
nameserver fd00::1
options edns0
";
        assert_eq!(
            parse_resolv_conf(text),
            vec!["192.0.2.1:53", "192.0.2.1:53", "[fd00::1]:53"]
        );

        // Unsinn darf nicht als Resolver durchgehen.
        assert!(parse_resolv_conf("nameserver\nnameserver keine-ip\n").is_empty());
        assert!(parse_resolv_conf("nameserverX 192.0.2.1").is_empty());
        assert!(parse_resolv_conf("# nameserver 192.0.2.1").is_empty());
        // Doppelte Einträge nur einmal, und höchstens drei (MAXNS).
        assert_eq!(
            parse_resolv_conf("nameserver 192.0.2.1\nnameserver 192.0.2.1").len(),
            1
        );
        assert_eq!(
            parse_resolv_conf(
                "nameserver 192.0.2.1\nnameserver 192.0.2.1\nnameserver 192.0.2.1\nnameserver 192.0.2.1"
            )
            .len(),
            3
        );
    }

    /// Die öffentlichen Resolver bleiben als Ersatz erhalten – aber hinter denen des Systems und
    /// ohne Doppelung, falls der Rechner ohnehin einen davon benutzt.
    #[test]
    fn oeffentliche_resolver_bleiben_der_ersatz() {
        let list = resolvers();
        for fallback in FALLBACK_RESOLVERS {
            assert_eq!(
                list.iter().filter(|entry| *entry == fallback).count(),
                1,
                "{} fehlt oder steht doppelt in {:?}",
                fallback,
                list
            );
        }
        let system = system_resolvers();
        assert_eq!(&list[..system.len()], &system[..], "System zuerst");
    }

    /// Eine einmal geholte Auskunft gilt kurz – auch ein „gibt es nicht". Sonst kostet ein Server,
    /// der uns im Kreis weiterreicht, in jeder Runde erneut das volle Zeitbudget.
    #[test]
    fn auskuenfte_werden_kurz_behalten() {
        let host = "test.beispiel.invalid";
        assert!(cache_get(host).is_none());
        cache_put(host, Some(("mc.beispiel.invalid".to_string(), 25577)));
        assert_eq!(
            cache_get(host),
            Some(Some(("mc.beispiel.invalid".to_string(), 25577)))
        );
        // Auch das „nein" wird behalten, nicht nur der Treffer.
        cache_put(host, None);
        assert_eq!(cache_get(host), Some(None));
    }

    /// Ein Ziel „." heißt im SRV-Standard ausdrücklich „Dienst nicht verfügbar".
    #[test]
    fn leeres_ziel_zaehlt_nicht() {
        let msg = response(0, 0, &[(0, 25565, &[])]);
        assert!(matches!(parse_answer(&msg, ID), Reply::Empty));
    }

    /// Beliebige Bytes dürfen den Parser nicht in Panik bringen – die Antwort kommt aus dem Netz.
    #[test]
    fn beliebige_bytes_stuerzen_nicht_ab() {
        let mut seed = 0x9E37_79B9_7F4A_7C15u64;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..2000 {
            let len = (random() % 512) as usize;
            let mut data: Vec<u8> = (0..len).map(|_| random() as u8).collect();
            if data.len() >= 2 {
                data[0] = ID[0];
                data[1] = ID[1];
            }
            let _ = parse_answer(&data, ID);
        }
    }
}
