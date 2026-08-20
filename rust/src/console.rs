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

use std::collections::VecDeque;
use std::io::{BufRead, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, Once};
use std::time::{Duration, Instant};

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

/// So viele Chatzeilen dürfen höchstens auf das Schreiben warten.
///
/// Bewusst klein: Sie liegen als fertige Zeichenketten im Speicher, und wer den Chat wirklich
/// mitliest, hängt nie so weit hinterher. Ist der Puffer voll, fällt die **älteste** Zeile heraus
/// – dieselbe Regel wie in der Sendewarteschlange.
///
/// Warum nicht warten, bis wieder Platz ist? Weil genau das der Fehler wäre, den dieser Puffer
/// verhindern soll: Wartet der Netz-Thread, beantwortet er kein KeepAlive mehr. Und wer die
/// Ausgabe nicht abholt, bekommt die Zeilen ohnehin nicht zu sehen – die Verbindung zu halten
/// ist die eine Aufgabe dieses Clients.
const MAX_CHAT_QUEUE: usize = 256;

struct Inner {
    color: AtomicBool,
    /// Nur Chat und echte Fehler ausgeben.
    quiet: AtomicBool,
    /// Zusätzliche `@event`-Zeilen für ein Programm davor.
    events: AtomicBool,
    /// Wartende Chatzeilen, siehe [`Console::chat`].
    chat: Mutex<VecDeque<String>>,
    /// Weckt den Schreib-Thread, sobald eine Zeile wartet.
    filled: Condvar,
    /// Weckt [`Console::flush_chat`], sobald eine Zeile draußen ist.
    drained: Condvar,
    /// Läuft die Warteschlange gerade über? Verhindert, dass jede einzelne ausgelassene Zeile
    /// eine eigene Meldung erzeugt – gemeldet wird der Beginn, nicht jede Wiederholung.
    overflowing: AtomicBool,
    /// Sorgt dafür, dass der Schreib-Thread genau einmal startet – und erst dann, wenn wirklich
    /// Chat anfällt. `--accounts` und `--login` bekommen dadurch keinen zusätzlichen Thread.
    start: Once,
    /// Läuft der Schreib-Thread? Wenn nicht (Start fehlgeschlagen), wird direkt geschrieben.
    writing: AtomicBool,
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
                chat: Mutex::new(VecDeque::new()),
                filled: Condvar::new(),
                drained: Condvar::new(),
                start: Once::new(),
                writing: AtomicBool::new(false),
                overflowing: AtomicBool::new(false),
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
    #[cfg(any(feature = "board", feature = "items", feature = "menu"))]
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
    ///
    /// Geschrieben wird sie von einem eigenen Thread, nicht hier. Der Grund ist der Netz-Thread:
    /// Er liest den Chat aus dem Paket **und** beantwortet KeepAlive. Schrieb er die Zeile selbst
    /// und las das Programm davor gerade nicht mit, lief die Pipe voll und der Schreibvorgang
    /// blockierte – der Client antwortete dann nicht mehr und flog mit `disconnect.timeout`
    /// heraus. Genau das ist im Java-Client schon passiert; dort steht derselbe Thread seither
    /// aus demselben Grund.
    ///
    /// Gestartet wird der Thread erst bei der ersten Chatzeile: `--accounts` und `--login`
    /// bekommen dadurch keinen.
    pub fn chat(&self, text: &str) {
        if !self.start_writer() {
            return write_chat(text); // kein Thread zu bekommen: dann eben wie bisher direkt
        }
        let mut queue = self.inner.chat.lock().unwrap();
        let dropped = queue.len() >= MAX_CHAT_QUEUE;
        if dropped {
            queue.pop_front();
        }
        queue.push_back(text.to_string());
        drop(queue);
        self.inner.filled.notify_one();

        // Nur der Übergang wird gemeldet. Läuft die Ausgabe voll, kämen sonst tausende
        // Warnungen – und die gingen auf denselben Weg wie das, was gerade nicht abfließt.
        if dropped && !self.inner.overflowing.swap(true, Ordering::SeqCst) {
            self.warn("Die Standardausgabe wird nicht gelesen – Chatzeilen fallen heraus.");
            // Auch als Ereignis: Mit `--quiet` gäbe es sonst überhaupt keinen Hinweis darauf,
            // dass gerade Zeilen fehlen – und ein Panel soll das erfahren können.
            self.event("output", "ausgelassen");
        }
    }

    /// Wartende Chatzeilen noch hinausschreiben. Vor dem Beenden aufrufen – sonst gingen genau
    /// die letzten Zeilen verloren, und das sind die mit dem Grund.
    ///
    /// Mit Zeitlimit: Liest niemand mehr mit, soll das Beenden daran nicht hängen bleiben.
    pub fn flush_chat(&self, limit: Duration) {
        if !self.inner.writing.load(Ordering::SeqCst) {
            return;
        }
        let until = Instant::now() + limit;
        let mut queue = self.inner.chat.lock().unwrap();
        while !queue.is_empty() {
            let Some(left) = until.checked_duration_since(Instant::now()) else {
                return;
            };
            queue = self.inner.drained.wait_timeout(queue, left).unwrap().0;
        }
    }

    /// `true`, sobald der Schreib-Thread läuft.
    fn start_writer(&self) -> bool {
        // Der Regelfall ist „läuft längst": dann nicht einmal den Arc anfassen.
        if self.inner.start.is_completed() {
            return self.inner.writing.load(Ordering::SeqCst);
        }
        let inner = Arc::clone(&self.inner);
        self.inner.start.call_once(move || {
            let running = std::thread::Builder::new()
                .name("afk-chat".into())
                .spawn({
                    let inner = Arc::clone(&inner);
                    move || write_loop(&inner)
                })
                .is_ok();
            inner.writing.store(running, Ordering::SeqCst);
        });
        self.inner.writing.load(Ordering::SeqCst)
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

/// Der Schreib-Thread: nimmt Zeilen aus der Warteschlange und schreibt sie hinaus. Blockiert er
/// dabei (volle Pipe), betrifft das nur ihn – der Netz-Thread arbeitet weiter.
fn write_loop(inner: &Inner) {
    loop {
        let (line, empty) = {
            let mut queue = inner.chat.lock().unwrap();
            loop {
                match queue.pop_front() {
                    Some(line) => break (line, queue.is_empty()),
                    None => queue = inner.filled.wait(queue).unwrap(),
                }
            }
        };
        if empty {
            // Wieder aufgeholt: Die nächste Überlaufmeldung darf wieder kommen.
            inner.overflowing.store(false, Ordering::SeqCst);
        }
        // Erst wecken, dann schreiben: Der Platz ist ja schon frei.
        inner.drained.notify_all();
        write_chat(&line);
    }
}

fn write_chat(text: &str) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{}", text);
    let _ = out.flush();
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
