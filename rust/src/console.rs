//! Ein-/Ausgabe – bewusst zeilenweise statt Terminal-Rohmodus.
//!
//! Die Aufteilung ist die ganze Bedienung des Clients:
//!
//! * **Standardausgabe**: ausschließlich Chat. Nichts anderes landet dort, damit ein Programm
//!   davor (z. B. die spätere Website) die Zeilen unverändert weiterreichen kann.
//! * **Standardfehlerausgabe**: Verbindungszustand, Fehler, Hinweise. Mit `--quiet` bleibt davon
//!   nur noch, was wirklich schiefgeht.
//! * **Standardeingabe**: jede Zeile geht als Chat-Nachricht bzw. – mit `/` vorn – als
//!   Serverbefehl raus.
//!
//! Kein Rohmodus, keine Statuszeile, kein Neuzeichnen: das spart eine Abhängigkeit, einen Thread
//! und macht den Client pipe-fähig.

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const RESET: &str = "\x1b[0m";
pub const GRAY: &str = "\x1b[90m";
pub const RED: &str = "\x1b[91m";
pub const GREEN: &str = "\x1b[92m";
pub const YELLOW: &str = "\x1b[93m";
pub const CYAN: &str = "\x1b[96m";
/// Nur die Bewegung setzt fett – im schlanken Build ist die Konstante ungenutzt.
#[cfg_attr(not(feature = "movement"), allow(dead_code))]
pub const BOLD: &str = "\x1b[1m";

struct Inner {
    color: AtomicBool,
    /// Nur Chat und echte Fehler ausgeben.
    quiet: AtomicBool,
    /// Zusätzliche `@event`-Zeilen für ein Programm davor.
    events: AtomicBool,
}

/// Beliebig oft klonbar (alle Klone teilen sich denselben Zustand).
#[derive(Clone)]
pub struct Console {
    inner: Arc<Inner>,
}

impl Console {
    pub fn new(color: bool, quiet: bool, events: bool) -> Console {
        #[cfg(windows)]
        enable_windows_utf8();
        Console {
            inner: Arc::new(Inner {
                color: AtomicBool::new(color),
                quiet: AtomicBool::new(quiet),
                events: AtomicBool::new(events),
            }),
        }
    }

    pub fn is_color(&self) -> bool {
        self.inner.color.load(Ordering::Relaxed)
    }

    /// Text einfärben – ohne Farbe unverändert zurück.
    pub fn paint(&self, code: &str, text: &str) -> String {
        if self.is_color() {
            format!("{}{}{}", code, text, RESET)
        } else {
            text.to_string()
        }
    }

    /// Eine Chat-Zeile. Das Einzige, was auf der Standardausgabe erscheint.
    pub fn chat(&self, text: &str) {
        let mut out = std::io::stdout().lock();
        let _ = writeln!(out, "{}", text);
        let _ = out.flush();
    }

    /// Zustandsmeldung (Standardfehlerausgabe, mit `--quiet` unterdrückt).
    pub fn print(&self, text: &str) {
        if !self.inner.quiet.load(Ordering::Relaxed) {
            let mut err = std::io::stderr().lock();
            let _ = writeln!(err, "{}", text);
        }
    }

    pub fn info(&self, text: &str) {
        self.print(&self.paint(GRAY, text));
    }

    pub fn note(&self, text: &str) {
        self.print(&self.paint(CYAN, text));
    }

    pub fn ok(&self, text: &str) {
        self.print(&self.paint(GREEN, text));
    }

    #[cfg_attr(not(feature = "movement"), allow(dead_code))]
    pub fn warn(&self, text: &str) {
        self.print(&self.paint(YELLOW, text));
    }

    /// Fehler kommen auch mit `--quiet` durch – sonst stünde man ratlos vor einem stummen Client.
    pub fn error(&self, text: &str) {
        let mut err = std::io::stderr().lock();
        let _ = writeln!(err, "{}", self.paint(RED, text));
    }

    /// Maschinenlesbare Zustandszeile für ein Programm davor (`--events`):
    /// `@event <name> <angaben>`, immer ohne Farbe und **auch mit `--quiet`**.
    ///
    /// Ohne `--events` kostet das genau einen atomaren Ladevorgang – der Aufrufer darf die
    /// Zeile also bedenkenlos an jeder interessanten Stelle setzen.
    pub fn event(&self, name: &str, detail: &str) {
        if !self.inner.events.load(Ordering::Relaxed) {
            return;
        }
        let mut err = std::io::stderr().lock();
        let _ = if detail.is_empty() {
            writeln!(err, "@event {}", name)
        } else {
            writeln!(err, "@event {} {}", name, detail)
        };
        let _ = err.flush();
    }

    /// Eingabezeilen, bis die Standardeingabe endet. `None` = Ende (z. B. Strg-D oder eine
    /// Eingabe, die es gar nicht gibt – dann läuft der Client einfach ohne Eingabe weiter).
    pub fn read_line(&self) -> Option<String> {
        let mut line = String::new();
        match std::io::stdin().lock().read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end_matches(['\r', '\n']).to_string()),
        }
    }
}

/// Ohne das zerlegt die Windows-Konsole jeden Umlaut.
#[cfg(windows)]
fn enable_windows_utf8() {
    use windows_sys::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};
    const UTF8: u32 = 65001;
    unsafe {
        SetConsoleOutputCP(UTF8);
        SetConsoleCP(UTF8);
    }
}
