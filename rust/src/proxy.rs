//! Verbindung über einen Proxy – SOCKS5 (mit und ohne Passwort) und HTTP-CONNECT.
//!
//! Bewusst von Hand statt mit einer Bibliothek: beides sind ein paar Dutzend Zeilen, die genau
//! einmal je Verbindungsaufbau laufen. Danach ist der Socket ein ganz normaler TCP-Strom – im
//! laufenden Betrieb kostet der Proxy also nichts.
//!
//! Betrifft **nur** die Spielverbindung. Der Microsoft-Login geht weiter direkt heraus: dort
//! zählt die IP niemand, und ein langsamer Proxy würde den Start unnötig verzögern.

use base64::Engine;
use std::io::{self, BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

/// So lange darf der Proxy für den TCP-Aufbau und für jede Antwort im Handshake brauchen.
///
/// Ohne diese Grenzen hängt ein Proxy, der die Verbindung annimmt und dann schweigt, den
/// Netz-Thread **unbegrenzt** auf: `TcpStream::connect` kennt von sich aus kein Zeitlimit, und
/// ein frischer Socket hat weder Lese- noch Schreib-Zeitablauf. Der Client käme dann weder ins
/// Spiel noch je zu einer Fehlermeldung.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_secs(20);

/// Ein Proxy aus `--proxy`.
#[derive(Clone)]
pub struct Proxy {
    pub kind: Kind,
    pub host: String,
    pub port: u16,
    pub user: Option<(String, String)>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Kind {
    Socks5,
    Http,
}

impl Proxy {
    /// `socks5://[nutzer:passwort@]host:port` – ohne Schema gilt SOCKS5, ohne Port 1080 bzw. 8080.
    /// `socks5h` ist derselbe Fall wie `socks5`: den Namen löst hier ohnehin immer der Proxy auf.
    pub fn parse(input: &str) -> Result<Proxy, String> {
        let text = input.trim();
        let (kind, rest) = match text.split_once("://") {
            Some((scheme, rest)) => {
                let kind = match scheme.to_ascii_lowercase().as_str() {
                    "socks5" | "socks5h" | "socks" => Kind::Socks5,
                    "http" | "https" => Kind::Http,
                    other => {
                        return Err(format!(
                            "Unbekannter Proxy-Typ '{}'. Möglich: socks5:// oder http://",
                            other
                        ))
                    }
                };
                (kind, rest)
            }
            None => (Kind::Socks5, text),
        };

        // Anmeldedaten stehen vor dem letzten '@' – ein Passwort darf selbst eins enthalten.
        let (user, address) = match rest.rsplit_once('@') {
            Some((credentials, address)) => {
                let (name, password) = credentials.split_once(':').unwrap_or((credentials, ""));
                (
                    Some((name.to_string(), password.to_string())),
                    address,
                )
            }
            None => (None, rest),
        };

        let default_port = if kind == Kind::Socks5 { 1080 } else { 8080 };
        let address = address.trim();
        let number = |port: &str| {
            port.trim()
                .parse::<u16>()
                .map_err(|_| format!("Proxy-Port ist keine Zahl: '{}'", port))
        };

        // `[::1]:1080` und eine nackte IPv6-Adresse dürfen nicht am Doppelpunkt zerfallen – und
        // die Klammern müssen weg, sonst findet die Namensauflösung die Adresse nicht.
        let (host, port) = if let Some(rest) = address.strip_prefix('[') {
            let (host, rest) = rest
                .split_once(']')
                .ok_or_else(|| format!("Proxy-Adresse ohne schließende Klammer: '{}'", address))?;
            match rest.strip_prefix(':') {
                Some(port) => (host, number(port)?),
                None => (host, default_port),
            }
        } else if address.matches(':').count() > 1 {
            (address, default_port) // nackte IPv6-Adresse
        } else {
            match address.rsplit_once(':') {
                Some((host, port)) => (host, number(port)?),
                None => (address, default_port),
            }
        };
        if host.trim().is_empty() {
            return Err("Proxy ohne Adresse. Beispiel: --proxy socks5://192.0.2.1:1080".to_string());
        }

        Ok(Proxy {
            kind,
            host: host.trim().to_string(),
            port,
            user,
        })
    }

    /// Für Meldungen: ohne Passwort.
    pub fn describe(&self) -> String {
        let scheme = match self.kind {
            Kind::Socks5 => "socks5",
            Kind::Http => "http",
        };
        match &self.user {
            Some((name, _)) => format!("{}://{}@{}:{}", scheme, name, self.host, self.port),
            None => format!("{}://{}:{}", scheme, self.host, self.port),
        }
    }

    /// Verbindet zum Proxy und lässt ihn zu `host:port` durchstellen.
    ///
    /// Aufbau und Handshake laufen unter [`HANDSHAKE_TIMEOUT`]; die endgültigen Zeitlimits der
    /// Spielverbindung setzt danach [`crate::conn::connect`] auf demselben Socket.
    pub fn connect(&self, host: &str, port: u16) -> io::Result<TcpStream> {
        let stream = self.dial()?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(HANDSHAKE_TIMEOUT))?;
        stream.set_write_timeout(Some(HANDSHAKE_TIMEOUT))?;
        match self.kind {
            Kind::Socks5 => self.socks5(&stream, host, port)?,
            Kind::Http => self.http_connect(&stream, host, port)?,
        }
        Ok(stream)
    }

    /// TCP-Verbindung zum Proxy mit Zeitlimit. `connect_timeout` braucht eine aufgelöste Adresse,
    /// deshalb wird der Name hier selbst aufgelöst; scheitert jeder Kandidat, kommt der letzte
    /// Fehler heraus.
    fn dial(&self) -> io::Result<TcpStream> {
        let mut last = None;
        for address in (self.host.as_str(), self.port).to_socket_addrs()? {
            match TcpStream::connect_timeout(&address, HANDSHAKE_TIMEOUT) {
                Ok(stream) => return Ok(stream),
                Err(e) => last = Some(e),
            }
        }
        Err(last.unwrap_or_else(|| fail("Proxy-Adresse ließ sich nicht auflösen")))
    }

    // ===================== SOCKS5 (RFC 1928) =====================

    fn socks5(&self, mut stream: &TcpStream, host: &str, port: u16) -> io::Result<()> {
        // Begrüßung: welche Anmeldeverfahren wir können.
        let greeting: &[u8] = if self.user.is_some() {
            &[0x05, 0x02, 0x00, 0x02]
        } else {
            &[0x05, 0x01, 0x00]
        };
        stream.write_all(greeting)?;

        let mut answer = [0u8; 2];
        stream.read_exact(&mut answer)?;
        if answer[0] != 0x05 {
            return Err(fail("Proxy antwortet nicht als SOCKS5"));
        }
        match answer[1] {
            0x00 => {}
            0x02 => self.socks5_login(stream)?,
            0xFF => return Err(fail("Proxy lehnt alle Anmeldeverfahren ab")),
            other => return Err(fail(&format!("Proxy verlangt Verfahren {}", other))),
        }

        // CONNECT mit Domänennamen: der Proxy löst den Namen auf, nicht wir.
        let name = host.as_bytes();
        if name.len() > 255 {
            return Err(fail("Serveradresse zu lang für SOCKS5"));
        }
        let mut request = vec![0x05, 0x01, 0x00, 0x03, name.len() as u8];
        request.extend_from_slice(name);
        request.extend_from_slice(&port.to_be_bytes());
        stream.write_all(&request)?;

        let mut head = [0u8; 4];
        stream.read_exact(&mut head)?;
        if head[1] != 0x00 {
            return Err(fail(socks5_error(head[1])));
        }
        // Die gebundene Adresse interessiert uns nicht, muss aber aus dem Strom.
        match head[3] {
            0x01 => stream.read_exact(&mut [0u8; 4])?,
            0x04 => stream.read_exact(&mut [0u8; 16])?,
            0x03 => {
                let mut len = [0u8; 1];
                stream.read_exact(&mut len)?;
                let mut skip = vec![0u8; len[0] as usize];
                stream.read_exact(&mut skip)?;
            }
            _ => return Err(fail("SOCKS5-Antwort mit unbekanntem Adresstyp")),
        }
        stream.read_exact(&mut [0u8; 2])?; // Port
        Ok(())
    }

    /// Benutzername/Passwort nach RFC 1929.
    fn socks5_login(&self, mut stream: &TcpStream) -> io::Result<()> {
        let Some((name, password)) = &self.user else {
            return Err(fail("Proxy verlangt eine Anmeldung, es ist aber keine hinterlegt"));
        };
        if name.len() > 255 || password.len() > 255 {
            return Err(fail("Proxy-Zugangsdaten zu lang"));
        }
        let mut request = vec![0x01, name.len() as u8];
        request.extend_from_slice(name.as_bytes());
        request.push(password.len() as u8);
        request.extend_from_slice(password.as_bytes());
        stream.write_all(&request)?;

        let mut answer = [0u8; 2];
        stream.read_exact(&mut answer)?;
        if answer[1] != 0x00 {
            return Err(fail("Proxy hat die Zugangsdaten abgelehnt"));
        }
        Ok(())
    }

    // ===================== HTTP-CONNECT =====================

    fn http_connect(&self, stream: &TcpStream, host: &str, port: u16) -> io::Result<()> {
        let mut request = format!(
            "CONNECT {host}:{port} HTTP/1.1\r\nHost: {host}:{port}\r\n",
            host = host,
            port = port
        );
        if let Some((name, password)) = &self.user {
            let engine = base64::engine::general_purpose::STANDARD;
            request.push_str(&format!(
                "Proxy-Authorization: Basic {}\r\n",
                engine.encode(format!("{}:{}", name, password))
            ));
        }
        request.push_str("\r\n");
        (&*stream).write_all(request.as_bytes())?;

        // Antwortkopf zeilenweise lesen, bis die Leerzeile kommt. `BufReader` liest womöglich
        // über das Ende hinaus – deshalb ein eigener, der danach wieder verworfen wird und
        // dessen Puffer nur den Kopf enthalten kann (die Gegenstelle schweigt bis dahin).
        let mut reader = BufReader::new(stream);
        let mut status = String::new();
        reader.read_line(&mut status)?;
        // Genau das zweite Feld der Statuszeile ist der Code. `contains(" 200")` ließe sich von
        // einem Ablehnungsgrund täuschen, in dem zufällig „200" steht.
        if status_code(&status) != Some(200) {
            return Err(fail(&format!(
                "Proxy lehnt ab: {}",
                status.trim_end().trim()
            )));
        }
        loop {
            let mut line = String::new();
            if reader.read_line(&mut line)? == 0 {
                return Err(fail("Proxy hat die Verbindung mitten im Kopf beendet"));
            }
            if line == "\r\n" || line == "\n" {
                return Ok(());
            }
        }
    }
}

/// Statuscode aus einer HTTP-Statuszeile (`HTTP/1.1 200 Connection established`).
fn status_code(line: &str) -> Option<u16> {
    line.split_whitespace().nth(1)?.parse().ok()
}

fn socks5_error(code: u8) -> &'static str {
    match code {
        0x01 => "SOCKS5: allgemeiner Fehler des Proxys",
        0x02 => "SOCKS5: Verbindung durch Regelwerk verboten",
        0x03 => "SOCKS5: Netz nicht erreichbar",
        0x04 => "SOCKS5: Rechner nicht erreichbar",
        0x05 => "SOCKS5: Verbindung abgelehnt",
        0x06 => "SOCKS5: Zeitüberschreitung",
        0x07 => "SOCKS5: Befehl nicht unterstützt",
        0x08 => "SOCKS5: Adresstyp nicht unterstützt",
        _ => "SOCKS5: unbekannter Fehler",
    }
}

fn fail(message: &str) -> io::Error {
    io::Error::other(message.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn schema_nutzer_und_port_werden_erkannt() {
        let p = Proxy::parse("socks5://hugo:geheim@192.0.2.1:1080").unwrap();
        assert!(p.kind == Kind::Socks5);
        assert_eq!(p.host, "192.0.2.1");
        assert_eq!(p.port, 1080);
        assert_eq!(p.user, Some(("hugo".into(), "geheim".into())));

        // Ohne Schema gilt SOCKS5, ohne Port der Standardport.
        let p = Proxy::parse("proxy.example.net").unwrap();
        assert!(p.kind == Kind::Socks5);
        assert_eq!(p.port, 1080);
        assert!(p.user.is_none());

        let p = Proxy::parse("http://proxy.example.net").unwrap();
        assert!(p.kind == Kind::Http);
        assert_eq!(p.port, 8080);
    }

    /// Ein '@' im Passwort darf die Zerlegung nicht zerreißen.
    #[test]
    fn passwort_mit_sonderzeichen() {
        let p = Proxy::parse("socks5://hugo:a@b@192.0.2.1:1080").unwrap();
        assert_eq!(p.host, "192.0.2.1");
        assert_eq!(p.user, Some(("hugo".into(), "a@b".into())));
    }

    #[test]
    fn das_passwort_steht_in_keiner_meldung() {
        let p = Proxy::parse("socks5://hugo:geheim@192.0.2.1:1080").unwrap();
        assert!(!p.describe().contains("geheim"));
    }

    #[test]
    fn unsinn_wird_abgelehnt() {
        assert!(Proxy::parse("ftp://192.0.2.1:21").is_err());
        assert!(Proxy::parse("socks5://192.0.2.1:abc").is_err());
        assert!(Proxy::parse("socks5://").is_err());
        assert!(Proxy::parse("socks5://[::1").is_err());
    }

    /// Eine IPv6-Adresse darf weder am Doppelpunkt zerfallen noch ihre Klammern behalten –
    /// mit Klammern findet die Namensauflösung sie nicht.
    #[test]
    fn ipv6_adressen_bleiben_heil() {
        let p = Proxy::parse("socks5://[::1]:1080").unwrap();
        assert_eq!(p.host, "::1");
        assert_eq!(p.port, 1080);

        // Ohne Port gilt der Standardport der jeweiligen Art.
        let p = Proxy::parse("socks5://[fe80::1]").unwrap();
        assert_eq!(p.host, "fe80::1");
        assert_eq!(p.port, 1080);
        assert_eq!(Proxy::parse("http://[::1]").unwrap().port, 8080);

        // Auch ohne Klammern darf eine nackte IPv6-Adresse nicht zerschnitten werden.
        let p = Proxy::parse("socks5://fe80::1").unwrap();
        assert_eq!(p.host, "fe80::1");
        assert_eq!(p.port, 1080);
    }

    /// „200" irgendwo im Ablehnungsgrund darf nicht als Erfolg durchgehen.
    #[test]
    fn nur_der_echte_statuscode_zaehlt() {
        assert_eq!(status_code("HTTP/1.1 200 Connection established\r\n"), Some(200));
        assert_eq!(status_code("HTTP/1.1 403 Forbidden (rule 200)\r\n"), Some(403));
        assert_eq!(status_code("HTTP/1.1 407 Proxy Authentication Required"), Some(407));
        assert_eq!(status_code(""), None);
    }
}
