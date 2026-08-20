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
//! Kein Rohmodus und keine Eingabe-Manipulation: das spart eine Abhängigkeit und macht den Client
//! pipe-fähig. Nur die ausdrücklich gestartete POV-Bauform zeichnet auf stderr ANSI-Frames neu;
//! Chat auf stdout bleibt weiterhin streng zeilenweise.

use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;

pub const RESET: &str = "\x1b[0m";
pub const GRAY: &str = "\x1b[90m";
pub const RED: &str = "\x1b[91m";
pub const GREEN: &str = "\x1b[92m";
pub const YELLOW: &str = "\x1b[93m";
pub const CYAN: &str = "\x1b[96m";
/// Fett setzen nur die Ausbaustufen (Überschriften in `:help`, `:board`, `:menu`, `:pov`) –
/// im schlanken Build ist die Konstante ungenutzt.
#[cfg_attr(not(feature = "local"), allow(dead_code))]
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

    /// Ausgabeformat für Text-Komponenten: eingefärbt oder roh.
    pub fn fmt(&self) -> crate::nbt::Fmt {
        if self.is_color() {
            crate::nbt::Fmt::Ansi
        } else {
            crate::nbt::Fmt::Plain
        }
    }

    /// Einen im Speicher liegenden `§`-Text anzeigefertig machen. Anzeigetafel und
    /// Gegenstandsnamen werden als `§`-Text gehalten, damit sie unverändert an ein Programm
    /// davor weitergereicht werden können – eingefärbt wird erst hier.
    #[cfg(any(feature = "board", feature = "items"))]
    pub fn text(&self, legacy: &str) -> String {
        if self.is_color() {
            crate::nbt::legacy_to_ansi(legacy)
        } else {
            crate::nbt::strip_legacy(legacy)
        }
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
            writeln!(err, "@event {} {}", name, one_line(detail))
        };
        let _ = err.flush();
    }

    /// Einen vollständigen POV-Frame direkt auf die Fehlerausgabe schreiben. Die normale
    /// Ausgabe bleibt zeilenorientiert; nur die ausdrücklich gestartete Live-Ansicht setzt den
    /// Cursor mit ANSI neu. Chat auf stdout bleibt davon vollständig getrennt.
    ///
    /// `prefix` (die Cursor-Steuerung) geht **unter derselben Sperre** raus wie das Bild.
    /// Zwei getrennte Aufrufe ließen einen Zustandshinweis eines anderen Threads dazwischen
    /// rutschen – ein Programm davor sähe dann eine Statuszeile mitten im Bild.
    #[cfg(feature = "pov")]
    pub fn pov_frame(&self, prefix: &str, frame: &str) {
        let mut err = std::io::stderr().lock();
        let _ = err.write_all(prefix.as_bytes());
        let _ = err.write_all(frame.as_bytes());
        let _ = err.flush();
    }

    /// Eingabezeilen, bis die Standardeingabe endet. `None` = Ende (z. B. Strg-D oder eine
    /// Eingabe, die es gar nicht gibt – dann läuft der Client einfach ohne Eingabe weiter).
    ///
    /// Eine einzelne unbrauchbare Zeile beendet die Eingabe **nicht**: Bisher schloss jeder
    /// Fehler die Schleife für immer. Ein Signal (`EINTR`) oder ein einziges Byte, das kein
    /// UTF-8 ist – etwa eine mit falscher Codepage geschriebene Zeile aus einem Panel –, machte
    /// den Client damit dauerhaft taub, obwohl die Standardeingabe noch offen war. Beide Fälle
    /// haben die betroffene Zeile bereits verbraucht, ein erneuter Versuch kommt also voran.
    pub fn read_line(&self) -> Option<String> {
        let mut line = String::new();
        loop {
            line.clear();
            match std::io::stdin().lock().read_line(&mut line) {
                Ok(0) => return None,
                Ok(_) => return Some(line.trim_end_matches(['\r', '\n']).to_string()),
                Err(e)
                    if matches!(
                        e.kind(),
                        std::io::ErrorKind::Interrupted | std::io::ErrorKind::InvalidData
                    ) => {}
                Err(_) => return None,
            }
        }
    }
}

/// Zeilenumbrüche und andere Steuerzeichen in Leerzeichen wandeln.
///
/// `@event` ist eine zugesagte Schnittstelle mit **genau einer Zeile je Ereignis**. Fast alles,
/// was dort als Angabe landet, kommt aber vom Server und darf Umbrüche enthalten: Kick-Gründe
/// sind regelmäßig mehrzeilig, Scoreboard-Zeilen und Gegenstands-Lore ebenfalls. Ohne diese
/// Wandlung riss eine einzige solche Meldung die Zeile auseinander, und alles hinter dem
/// Umbruch sah für ein Programm davor aus wie eine gewöhnliche Statuszeile.
fn one_line(text: &str) -> std::borrow::Cow<'_, str> {
    if !text.chars().any(|c| c.is_control()) {
        return std::borrow::Cow::Borrowed(text);
    }
    std::borrow::Cow::Owned(
        text.chars()
            .map(|c| if c.is_control() { ' ' } else { c })
            .collect(),
    )
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

#[cfg(test)]
mod tests {
    use super::*;

    /// Ein mehrzeiliger Kick-Grund darf die `@event`-Zeile nicht auseinanderreißen – sonst
    /// stünde die zweite Hälfte für ein Programm davor da wie eine gewöhnliche Statuszeile.
    #[test]
    fn ereigniszeile_bleibt_eine_zeile() {
        assert_eq!(one_line("Du wurdest\ngekickt:\r\nSpam"), "Du wurdest gekickt:  Spam");
        assert_eq!(one_line("mit\tTabulator"), "mit Tabulator");
        // Ohne Steuerzeichen wird nichts kopiert.
        assert!(matches!(
            one_line("ganz normal"),
            std::borrow::Cow::Borrowed("ganz normal")
        ));
        // Umlaute und §-Codes bleiben unangetastet.
        assert_eq!(one_line("§cÜberfällig"), "§cÜberfällig");
    }
}
