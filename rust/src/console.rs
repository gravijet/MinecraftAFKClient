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
//!
//! **Geschrieben wird nie vom Netz-Thread**, und zwar auf keinem der beiden Ströme. Jeder hat
//! seinen eigenen Schreib-Thread mit gedeckelter Warteschlange (siehe [`Channel`]). Der Grund
//! steht bei [`Console::chat`]: Der Netz-Thread liest die Pakete *und* beantwortet KeepAlive –
//! bleibt er in einer vollen Pipe stecken, fliegt der Client mit `disconnect.timeout` heraus,
//! obwohl die Verbindung völlig in Ordnung war. Für die Standardausgabe galt das schon länger;
//! die Fehlerausgabe schrieb er dagegen weiter selbst, und über sie gehen Beitritts-, Regel- und
//! Ereignismeldungen. Ein Panel, das nur stdout mitliest, reichte damit aus.
//!
//! Zwei getrennte Threads und nicht einer für beides: Sonst hielte eine volle Standardausgabe
//! auch die Fehlerausgabe an – und ausgerechnet `@event disconnect`, mit dem ein Panel erfährt,
//! dass der Client weg ist, käme dann nie an.

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

/// So viele Zeilen dürfen je Strom höchstens auf das Schreiben warten.
///
/// Bewusst klein: Sie liegen als fertige Zeichenketten im Speicher, und wer die Ausgabe wirklich
/// mitliest, hängt nie so weit hinterher. Ist der Puffer voll, fällt die **älteste** Zeile heraus
/// – dieselbe Regel wie in der Sendewarteschlange.
///
/// Warum nicht warten, bis wieder Platz ist? Weil genau das der Fehler wäre, den dieser Puffer
/// verhindern soll: Wartet der Netz-Thread, beantwortet er kein KeepAlive mehr. Und wer die
/// Ausgabe nicht abholt, bekommt die Zeilen ohnehin nicht zu sehen – die Verbindung zu halten
/// ist die eine Aufgabe dieses Clients.
const MAX_QUEUE: usize = 256;

/// Wohin eine Zeile gehört.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Stream {
    /// Standardausgabe – ausschließlich Chat.
    Out,
    /// Standardfehlerausgabe – alles andere.
    Err,
}

impl Stream {
    fn name(self) -> &'static str {
        match self {
            Stream::Out => "Die Standardausgabe",
            Stream::Err => "Die Fehlerausgabe",
        }
    }

    fn thread_name(self) -> &'static str {
        match self {
            Stream::Out => "afk-out",
            Stream::Err => "afk-err",
        }
    }
}

/// Was noch aussteht: die wartenden Zeilen **und** die eine, die der Schreib-Thread gerade in
/// der Hand hat.
///
/// Ohne das zweite Merkmal galt die Ausgabe bereits als leer, sobald die letzte Zeile aus der
/// Warteschlange geholt war – geschrieben war sie da noch nicht. [`Console::flush`] kehrte dann
/// zurück, `std::process::exit` riss den Schreib-Thread mitten im Schreiben weg, und
/// ausgerechnet die letzte Zeile vor einem Kick fehlte. Genau die, wegen der es `flush` gibt.
#[derive(Default)]
struct Waiting {
    lines: VecDeque<String>,
    /// Eine Zeile ist aus der Warteschlange heraus, aber noch nicht draußen.
    writing: bool,
}

impl Waiting {
    fn idle(&self) -> bool {
        self.lines.is_empty() && !self.writing
    }
}

/// Ein Ausgabestrom samt Warteschlange und – bei Bedarf gestartetem – Schreib-Thread.
struct Channel {
    waiting: Mutex<Waiting>,
    /// Weckt den Schreib-Thread, sobald eine Zeile wartet.
    filled: Condvar,
    /// Weckt [`Console::flush`], sobald eine Zeile wirklich draußen ist.
    drained: Condvar,
    /// Läuft die Warteschlange gerade über? Verhindert, dass jede einzelne ausgelassene Zeile
    /// eine eigene Meldung erzeugt – gemeldet wird der Beginn, nicht jede Wiederholung.
    overflowing: AtomicBool,
    /// Sorgt dafür, dass der Schreib-Thread genau einmal startet – und erst dann, wenn wirklich
    /// etwas auf diesem Strom anfällt.
    start: Once,
    /// Läuft der Schreib-Thread? Wenn nicht (Start fehlgeschlagen), wird direkt geschrieben.
    running: AtomicBool,
}

impl Default for Channel {
    fn default() -> Channel {
        Channel {
            waiting: Mutex::new(Waiting::default()),
            filled: Condvar::new(),
            drained: Condvar::new(),
            overflowing: AtomicBool::new(false),
            start: Once::new(),
            running: AtomicBool::new(false),
        }
    }
}

struct Inner {
    color: AtomicBool,
    /// Nur Chat und echte Fehler ausgeben.
    quiet: AtomicBool,
    /// Zusätzliche `@event`-Zeilen für ein Programm davor.
    events: AtomicBool,
    out: Channel,
    err: Channel,
}

impl Inner {
    fn channel(&self, stream: Stream) -> &Channel {
        match stream {
            Stream::Out => &self.out,
            Stream::Err => &self.err,
        }
    }
}

/// Beliebig oft klonbar (alle Klone teilen sich denselben Zustand).
#[derive(Clone)]
pub struct Console {
    inner: Arc<Inner>,
}

impl Console {
    pub fn new(color: bool, quiet: bool, events: bool) -> Console {
        prepare_terminal();
        Console {
            inner: Arc::new(Inner {
                color: AtomicBool::new(color),
                quiet: AtomicBool::new(quiet),
                events: AtomicBool::new(events),
                out: Channel::default(),
                err: Channel::default(),
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
        self.queue(Stream::Out, text);
    }

    /// Eine Zeile in die Warteschlange ihres Stroms stellen.
    ///
    /// Läuft sie über, fällt die **älteste** heraus – und das wird genau einmal gemeldet, nicht
    /// je ausgelassener Zeile: Die Meldung ginge sonst denselben Weg wie das, was gerade nicht
    /// abfließt.
    fn queue(&self, stream: Stream, text: &str) {
        if !self.start_writer(stream) {
            // Kein Thread zu bekommen: dann eben direkt, wie ohne diese Warteschlange.
            return write_direct(stream, text);
        }
        let channel = self.inner.channel(stream);
        let mut waiting = channel.waiting.lock().unwrap();
        let dropped = waiting.lines.len() >= MAX_QUEUE;
        if dropped {
            waiting.lines.pop_front();
        }
        waiting.lines.push_back(text.to_string());
        drop(waiting);
        channel.filled.notify_one();

        if dropped && !channel.overflowing.swap(true, Ordering::SeqCst) {
            // Die Meldung geht selbst wieder durch diese Funktion; der Merker verhindert, dass
            // sie sich dabei endlos selbst auslöst.
            self.warn(&format!(
                "{} wird nicht gelesen – Zeilen fallen heraus.",
                stream.name()
            ));
            // Auch als Ereignis: Mit `--quiet` gäbe es sonst überhaupt keinen Hinweis darauf,
            // dass gerade Zeilen fehlen – und ein Panel soll das erfahren können.
            self.event("output", "ausgelassen");
        }
    }

    /// Wartende Zeilen beider Ströme noch hinausschreiben. Vor **jedem** `exit` aufrufen – sonst
    /// gingen genau die letzten Zeilen verloren, und das sind die mit dem Grund.
    ///
    /// Mit Zeitlimit: Liest niemand mehr mit, soll das Beenden daran nicht hängen bleiben.
    pub fn flush(&self, limit: Duration) {
        let until = Instant::now() + limit;
        for stream in [Stream::Out, Stream::Err] {
            let channel = self.inner.channel(stream);
            if !channel.running.load(Ordering::SeqCst) {
                continue;
            }
            let mut waiting = channel.waiting.lock().unwrap();
            while !waiting.idle() {
                let Some(left) = until.checked_duration_since(Instant::now()) else {
                    return;
                };
                waiting = channel.drained.wait_timeout(waiting, left).unwrap().0;
            }
        }
    }

    /// `true`, sobald der Schreib-Thread dieses Stroms läuft.
    fn start_writer(&self, stream: Stream) -> bool {
        let channel = self.inner.channel(stream);
        // Der Regelfall ist „läuft längst": dann nicht einmal den Arc anfassen.
        if channel.start.is_completed() {
            return channel.running.load(Ordering::SeqCst);
        }
        let inner = Arc::clone(&self.inner);
        channel.start.call_once(move || {
            let running = std::thread::Builder::new()
                .name(stream.thread_name().into())
                .spawn({
                    let inner = Arc::clone(&inner);
                    move || write_loop(&inner, stream)
                })
                .is_ok();
            inner
                .channel(stream)
                .running
                .store(running, Ordering::SeqCst);
        });
        channel.running.load(Ordering::SeqCst)
    }

    /// Zustandsmeldung (Standardfehlerausgabe, mit `--quiet` unterdrückt).
    pub fn print(&self, text: &str) {
        if !self.inner.quiet.load(Ordering::Relaxed) {
            self.queue(Stream::Err, text);
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
        self.queue(Stream::Err, &self.paint(RED, text));
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
        self.queue(
            Stream::Err,
            &if detail.is_empty() {
                format!("@event {}", name)
            } else {
                format!("@event {} {}", name, one_line(detail))
            },
        );
    }

    /// Einen vollständigen POV-Frame direkt auf die Fehlerausgabe schreiben. Die normale
    /// Ausgabe bleibt zeilenorientiert; nur die ausdrücklich gestartete Live-Ansicht setzt den
    /// Cursor mit ANSI neu. Chat auf stdout bleibt davon vollständig getrennt.
    ///
    /// `prefix` (die Cursor-Steuerung) geht **unter derselben Sperre** raus wie das Bild.
    /// Zwei getrennte Aufrufe ließen einen Zustandshinweis eines anderen Threads dazwischen
    /// rutschen – ein Programm davor sähe dann eine Statuszeile mitten im Bild.
    ///
    /// Bewusst ohne Warteschlange: Ein Bild ist bei 160x80 gut 300 KB, das gehört nicht in einen
    /// Zeilenpuffer. Blockiert es, betrifft das nur den Zeichen-Thread – und genau dafür gibt es
    /// ihn.
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
        let stdin = std::io::stdin();
        let mut input = stdin.lock();
        let mut line = String::new();
        loop {
            line.clear();
            match input.read_line(&mut line) {
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

/// Der Schreib-Thread eines Stroms: nimmt Zeilen aus seiner Warteschlange und schreibt sie
/// hinaus. Blockiert er dabei (volle Pipe), betrifft das nur ihn – der Netz-Thread arbeitet
/// weiter, und der jeweils andere Strom ebenfalls.
fn write_loop(inner: &Inner, stream: Stream) {
    let channel = inner.channel(stream);
    loop {
        let line = {
            let mut waiting = channel.waiting.lock().unwrap();
            loop {
                match waiting.lines.pop_front() {
                    Some(line) => {
                        // Ab hier gilt die Zeile als „unterwegs", nicht als erledigt.
                        waiting.writing = true;
                        break line;
                    }
                    None => waiting = channel.filled.wait(waiting).unwrap(),
                }
            }
        };
        write_direct(stream, &line);
        // Erst **nach** dem Schreiben abmelden: Sonst hielte `flush` die Ausgabe schon für leer,
        // während die letzte Zeile noch im Puffer steht.
        let mut waiting = channel.waiting.lock().unwrap();
        waiting.writing = false;
        if waiting.lines.is_empty() {
            // Wieder aufgeholt: Die nächste Überlaufmeldung darf wieder kommen.
            channel.overflowing.store(false, Ordering::SeqCst);
        }
        drop(waiting);
        channel.drained.notify_all();
    }
}

/// Eine Zeile in **einem** Schreibvorgang hinausgeben.
///
/// Der Zeilenumbruch wird vorher angehängt statt über `writeln!` geschrieben: Die Fehlerausgabe
/// ist ungepuffert, `writeln!(err, "{}", text)` wären also zwei Systemaufrufe je Zeile. Eine
/// kurze Zeichenkette anzulegen ist billiger als der zweite Aufruf.
fn write_direct(stream: Stream, text: &str) {
    let mut line = String::with_capacity(text.len() + 1);
    line.push_str(text);
    line.push('\n');
    match stream {
        Stream::Out => {
            let mut out = std::io::stdout().lock();
            let _ = out.write_all(line.as_bytes());
            let _ = out.flush();
        }
        Stream::Err => {
            let mut err = std::io::stderr().lock();
            let _ = err.write_all(line.as_bytes());
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

/// Das Terminal auf UTF-8 stellen. Ohne das zerlegt die Windows-Konsole jeden Umlaut.
///
/// Aufzurufen, **bevor** irgendetwas geschrieben wird – auch vor `--help` und `--accounts`. Die
/// beiden legen keine [`Console`] an, geben aber einen Pfad aus; steht im Benutzernamen ein
/// Umlaut, kam er bisher als Zeichensalat heraus.
#[cfg(windows)]
pub fn prepare_terminal() {
    use windows_sys::Win32::System::Console::{SetConsoleCP, SetConsoleOutputCP};
    const UTF8: u32 = 65001;
    unsafe {
        SetConsoleOutputCP(UTF8);
        SetConsoleCP(UTF8);
    }
}

#[cfg(not(windows))]
pub fn prepare_terminal() {}

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

    /// Beide Ströme haben ihre eigene Warteschlange. Läuft die eine über, muss die andere
    /// unberührt bleiben – sonst hielte eine volle Standardausgabe auch `@event disconnect` auf,
    /// und ein Panel erführe nie, dass der Client weg ist.
    #[test]
    fn die_stroeme_haben_getrennte_warteschlangen() {
        let console = Console::new(false, false, true);
        let inner = &console.inner;
        // Von Hand gefüllt: Der Test soll nichts wirklich hinausschreiben.
        inner.out.waiting.lock().unwrap().lines.push_back("chat".into());
        assert_eq!(inner.out.waiting.lock().unwrap().lines.len(), 1);
        assert_eq!(inner.err.waiting.lock().unwrap().lines.len(), 0);
        assert!(!inner.channel(Stream::Out).waiting.lock().unwrap().idle());
        assert!(inner.channel(Stream::Err).waiting.lock().unwrap().idle());
    }

    /// `flush` darf erst zurückkehren, wenn die Zeile wirklich draußen ist – nicht schon,
    /// sobald sie aus der Warteschlange geholt wurde.
    #[test]
    fn eine_zeile_in_arbeit_gilt_nicht_als_erledigt() {
        let mut waiting = Waiting::default();
        assert!(waiting.idle());
        waiting.lines.push_back("x".into());
        assert!(!waiting.idle());
        waiting.lines.pop_front();
        waiting.writing = true;
        assert!(!waiting.idle(), "in Arbeit ist nicht dasselbe wie erledigt");
        waiting.writing = false;
        assert!(waiting.idle());
    }
}
