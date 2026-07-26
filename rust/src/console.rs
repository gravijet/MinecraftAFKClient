//! Terminal-Oberfläche im Rohmodus: unten ein gerahmtes Eingabefeld mit Statuszeile,
//! darüber laufen Chat und Meldungen durch.
//!
//! Der Kniff ist derselbe wie bei JLine im Java-Client, nur mehrzeilig: Vor jeder Ausgabe wird
//! der Eingabeblock gelöscht, die Zeile geschrieben und der Block darunter neu gezeichnet.
//! Es läuft kein eigener Ausgabe-Thread – Ausgaben sind kurz und blockieren den Netz-Thread
//! nicht spürbar, da sie nur in den Terminalpuffer schreiben.

use crate::clock;
use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::io::{self, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const RESET: &str = "\x1b[0m";
pub const GRAY: &str = "\x1b[90m";
pub const RED: &str = "\x1b[91m";
pub const GREEN: &str = "\x1b[92m";
pub const YELLOW: &str = "\x1b[93m";
pub const CYAN: &str = "\x1b[96m";
pub const BOLD: &str = "\x1b[1m";

/// Unter dieser Terminalbreite wird auf eine schlichte einzeilige Eingabe zurückgefallen.
const MIN_BOX_WIDTH: u16 = 46;

/// Einzeilige Eingabe entfernen: Zeile leeren, Cursor an den Anfang.
const ERASE_LINE: &str = "\r\x1b[2K";
/// Rahmen entfernen. Der Cursor steht in der Eingabezeile, also eine Zeile unter der oberen
/// Rahmenlinie: erst von dort alles bis zum Schirmende löschen, dann eine Zeile hoch und auch
/// die Rahmenlinie leeren. Danach beginnt genau dort die nächste Ausgabe.
const ERASE_BOX: &str = "\r\x1b[J\x1b[1A\r\x1b[2K";
/// So viele Eingaben hält der Verlauf (Pfeiltasten) – mehr braucht niemand, kostet nur RAM.
const HISTORY_LIMIT: usize = 50;

/// Verbindungszustand für den Punkt in der Statuszeile.
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Link {
    Offline,
    Connecting,
    Online,
}

impl Link {
    fn dot(self) -> (&'static str, &'static str) {
        match self {
            Link::Offline => (RED, "●"),
            Link::Connecting => (YELLOW, "●"),
            Link::Online => (GREEN, "●"),
        }
    }
}

/// Ausgabekanal: im Betrieb die Standardausgabe, im Test ein Puffer – so lassen sich die
/// Steuersequenzen überprüfen, statt sie nur zu vermuten.
type Sink = Box<dyn Write + Send>;

struct Io {
    out: Sink,
    color: Arc<AtomicBool>,

    /// Eingabepuffer als Zeichen – so zählt „ä" als ein Zeichen und der Cursor bleibt korrekt.
    line: Vec<char>,
    cursor: usize,
    /// Verschiebung des Sichtfensters, wenn die Zeile breiter ist als der Rahmen.
    scroll: usize,
    prompt: String,

    link: Link,
    status: String,

    history: Vec<String>,
    /// Position im Verlauf beim Blättern; `None` = aktuelle (noch nicht abgeschickte) Zeile.
    history_pos: Option<usize>,

    width: u16,
    /// true, solange eine Eingabe läuft (der Block wird nach jeder Ausgabe wieder gezeichnet).
    active: bool,
    /// Steht der Block gerade auf dem Schirm? `Some(true)` = als Rahmen, `Some(false)` = als
    /// Einzeiler, `None` = nichts zu löschen. Die beim Zeichnen benutzte Form muss beim Löschen
    /// wieder herhalten – sonst räumt ein Größenwechsel die falsche Zeilenzahl auf.
    drawn: Option<bool>,
    /// Im Rohmodus braucht jede Zeile ein CR; headless nicht.
    newline: &'static str,
    interactive: bool,
}

impl Io {
    fn color(&self) -> bool {
        self.color.load(Ordering::Relaxed)
    }

    fn paint(&self, code: &str, text: &str) -> String {
        if self.color() {
            format!("{}{}{}", code, text, RESET)
        } else {
            text.to_string()
        }
    }

    /// Rahmen nur, wenn das Terminal breit genug ist.
    fn boxed(&self) -> bool {
        self.interactive && self.width >= MIN_BOX_WIDTH
    }

    // ---- Zeichnen ----

    /// Den kompletten Block zeichnen. Erwartet den Cursor am Anfang einer freien Zeile und
    /// parkt ihn danach an der Schreibstelle in der Eingabezeile.
    fn draw(&mut self) {
        if !self.active {
            return;
        }
        let boxed = self.boxed();
        let text = if boxed {
            self.render_box()
        } else {
            self.render_plain()
        };
        let _ = self.out.write_all(text.as_bytes());
        let _ = self.out.flush();
        self.drawn = Some(boxed);
    }

    /// Block an derselben Stelle neu aufbauen (nach Tastendruck, Statuswechsel, Größenwechsel).
    /// Ohne das vorherige Löschen würde der Rahmen bei jedem Tastendruck eine Zeile nach unten
    /// wandern, weil der Cursor in der Eingabezeile geparkt steht.
    fn redraw(&mut self) {
        self.erase();
        self.draw();
    }

    fn render_plain(&self) -> String {
        let visible: String = self.line.iter().collect();
        format!(
            "\r\x1b[2K{}{}\r\x1b[{}C",
            self.paint(CYAN, &self.prompt),
            visible,
            self.prompt.chars().count() + self.cursor
        )
    }

    /// ```text
    ///  ╭──────────────────────────────╮
    ///  │ ❯ hallo welt                 │
    ///  ╰──────────────────────────────╯
    ///    ● verbunden · host · Konto
    /// ```
    fn render_box(&self) -> String {
        let outer = self.width.saturating_sub(2) as usize; // je 1 Zeichen Rand links/rechts
        let bar = "─".repeat(outer.saturating_sub(2));
        let prompt_len = self.prompt.chars().count();
        let inner = outer.saturating_sub(4); // Rahmen + je ein Füllzeichen
        let room = inner.saturating_sub(prompt_len);

        let visible: String = self.line.iter().skip(self.scroll).take(room).collect();
        let used = prompt_len + visible.chars().count();
        let pad = " ".repeat(inner.saturating_sub(used));

        let edge = |left: &str, right: &str| {
            self.paint(GRAY, &format!(" {}{}{}", left, bar, right))
        };
        let (dot_color, dot) = self.link.dot();

        // Die Statuszeile darf NICHT umbrechen: eine zusätzliche Zeile würde beim Löschen
        // nicht mitgezählt und der Rahmen bliebe als Rest stehen.
        let status: String = self
            .status
            .chars()
            .take((self.width as usize).saturating_sub(6))
            .collect();

        format!(
            "\r{top}{nl} {bar_l} {prompt}{visible}{pad} {bar_r}{nl}{bottom}{nl}   {dot} {status}\
             \x1b[2A\r\x1b[{col}C",
            top = edge("╭", "╮"),
            bottom = edge("╰", "╯"),
            bar_l = self.paint(GRAY, "│"),
            bar_r = self.paint(GRAY, "│"),
            prompt = self.paint(CYAN, &self.prompt),
            visible = visible,
            pad = pad,
            dot = self.paint(dot_color, dot),
            status = self.paint(GRAY, &status),
            col = 3 + prompt_len + self.cursor.saturating_sub(self.scroll),
            nl = self.newline,
        )
    }

    /// Block wieder entfernen. Danach steht der Cursor am Anfang der Zeile, in der zuvor die
    /// obere Rahmenlinie stand – dort kann die nächste Ausgabe beginnen.
    fn erase(&mut self) {
        let sequence = match self.drawn.take() {
            Some(true) => ERASE_BOX,
            Some(false) => ERASE_LINE,
            None => return,
        };
        let _ = self.out.write_all(sequence.as_bytes());
    }

    /// Eine oder mehrere Zeilen oberhalb des Eingabeblocks ausgeben.
    fn print_above(&mut self, text: &str) {
        let nl = self.newline;
        self.erase();
        for line in text.split('\n') {
            let _ = self.out.write_all(line.trim_end_matches('\r').as_bytes());
            let _ = self.out.write_all(nl.as_bytes());
        }
        self.draw();
        let _ = self.out.flush();
    }

    /// Sichtfenster so verschieben, dass der Cursor immer im Rahmen liegt.
    fn follow_cursor(&mut self) {
        if !self.boxed() {
            self.scroll = 0;
            return;
        }
        let inner = (self.width.saturating_sub(2) as usize).saturating_sub(4);
        let room = inner.saturating_sub(self.prompt.chars().count()).max(1);
        if self.cursor < self.scroll {
            self.scroll = self.cursor;
        } else if self.cursor >= self.scroll + room {
            self.scroll = self.cursor + 1 - room;
        }
    }

    fn set_line(&mut self, text: &str) {
        self.line = text.chars().collect();
        self.cursor = self.line.len();
        self.scroll = 0;
        self.follow_cursor();
    }
}

#[derive(Clone)]
pub struct Console {
    io: Arc<Mutex<Io>>,
    color: Arc<AtomicBool>,
    /// false = headless: keine Tastatureingabe, keine Steuersequenzen (Dienst-/Hintergrundbetrieb).
    interactive: bool,
}

impl Console {
    pub fn new() -> io::Result<Console> {
        enable_raw_mode()?;
        // Windows: VT-Verarbeitung anschalten, sonst erscheinen die ANSI-Codes als Text.
        #[cfg(windows)]
        let _ = crossterm::ansi_support::supports_ansi();

        // Ohne das bliebe das Terminal nach einer Panik im Rohmodus stehen (v. a. auf Linux
        // ist die Shell danach unbenutzbar, bis man `reset` tippt).
        let previous = std::panic::take_hook();
        std::panic::set_hook(Box::new(move |info| {
            let _ = disable_raw_mode();
            print!("\r\n");
            let _ = io::stdout().flush();
            previous(info);
        }));

        Ok(Console::build(true))
    }

    /// Ohne Rohmodus und ohne Eingabe – für `--headless` (z. B. im Hintergrund oder in einem
    /// Terminal-Multiplexer).
    pub fn headless() -> Console {
        Console::build(false)
    }

    fn build(interactive: bool) -> Console {
        // Windows-Konsolen laufen standardmäßig auf einer OEM-Codepage – dann wird aus „ä" Müll.
        #[cfg(windows)]
        unsafe {
            windows_sys::Win32::System::Console::SetConsoleOutputCP(65001);
            windows_sys::Win32::System::Console::SetConsoleCP(65001);
        }

        let color = Arc::new(AtomicBool::new(true));
        Console {
            io: Arc::new(Mutex::new(Io {
                out: Box::new(io::stdout()),
                color: Arc::clone(&color),
                line: Vec::new(),
                cursor: 0,
                scroll: 0,
                prompt: String::new(),
                link: Link::Offline,
                status: String::new(),
                history: Vec::new(),
                history_pos: None,
                width: terminal_width(),
                active: false,
                drawn: None,
                newline: if interactive { "\r\n" } else { "\n" },
                interactive,
            })),
            color,
            interactive,
        }
    }

    pub fn set_color(&self, enabled: bool) {
        self.color.store(enabled, Ordering::Relaxed);
    }

    pub fn is_color(&self) -> bool {
        self.color.load(Ordering::Relaxed)
    }

    pub fn paint(&self, code: &str, text: &str) -> String {
        if self.is_color() {
            format!("{}{}{}", code, text, RESET)
        } else {
            text.to_string()
        }
    }

    /// Zustand und Text der Statuszeile unter dem Eingabefeld setzen.
    pub fn set_status(&self, link: Link, text: &str) {
        if let Ok(mut io) = self.io.lock() {
            io.link = link;
            io.status = text.to_string();
            io.redraw();
        }
    }

    // ---- Ausgaben ----

    /// Eine Zeile oberhalb der Eingabezeile ausgeben (thread-sicher).
    pub fn print(&self, text: &str) {
        if let Ok(mut io) = self.io.lock() {
            io.print_above(text);
        }
    }

    fn tagged(&self, color: &str, symbol: &str, text: &str) {
        self.print(&format!("  {} {}", self.paint(color, symbol), text));
    }

    /// Beiläufige Meldung (grau).
    pub fn info(&self, text: &str) {
        self.tagged(GRAY, "·", &self.paint(GRAY, text));
    }

    /// Zustandsmeldung, die man sehen soll (verbinden, wechseln, Auto-Befehl …).
    pub fn note(&self, text: &str) {
        self.tagged(CYAN, "●", text);
    }

    /// Erfolg (verbunden, im Spiel).
    pub fn ok(&self, text: &str) {
        self.tagged(GREEN, "●", text);
    }

    pub fn warn(&self, text: &str) {
        self.tagged(YELLOW, "!", text);
    }

    pub fn error(&self, text: &str) {
        self.tagged(RED, "✗", text);
    }

    /// Chat-/Serverzeile mit Uhrzeit davor.
    pub fn chat(&self, text: &str) {
        self.print(&format!("{} {}", self.paint(GRAY, &clock::hhmm()), text));
    }

    pub fn clear_screen(&self) {
        if let Ok(mut io) = self.io.lock() {
            let _ = write!(io.out, "\x1b[2J\x1b[H");
            io.drawn = None; // der Schirm ist leer, es gibt nichts mehr zu löschen
            io.draw();
            let _ = io.out.flush();
        }
    }

    // ---- Eingabe ----

    /// Liest eine Zeile. `None` bei Strg+C / Strg+D (und immer im headless-Betrieb).
    pub fn read_line(&self, prompt: &str) -> Option<String> {
        if !self.interactive {
            return None;
        }
        {
            let mut io = self.io.lock().ok()?;
            io.prompt = prompt.to_string();
            io.width = terminal_width();
            io.line.clear();
            io.cursor = 0;
            io.scroll = 0;
            io.history_pos = None;
            io.active = true;
            io.draw();
        }

        let result = self.edit_loop();

        if let Ok(mut io) = self.io.lock() {
            io.erase();
            io.active = false;
            io.line.clear();
            io.cursor = 0;
            let _ = io.out.flush();
        }
        result
    }

    fn edit_loop(&self) -> Option<String> {
        loop {
            let key = match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => key,
                Ok(Event::Resize(width, _)) => {
                    if let Ok(mut io) = self.io.lock() {
                        io.erase(); // noch mit der alten Form/Zeilenzahl aufräumen
                        io.width = width.max(1);
                        io.follow_cursor();
                        io.draw();
                    }
                    continue;
                }
                Ok(_) => continue,
                Err(_) => return None,
            };
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let mut io = self.io.lock().ok()?;

            match key.code {
                KeyCode::Char('c') if ctrl => return None,
                KeyCode::Char('d') if ctrl && io.line.is_empty() => return None,
                KeyCode::Char('l') if ctrl => {
                    let _ = write!(io.out, "\x1b[2J\x1b[H");
                    io.drawn = None;
                }
                // Zeile bzw. Wort links vom Cursor löschen (wie in jeder Shell).
                KeyCode::Char('u') if ctrl => {
                    let to = io.cursor;
                    io.line.drain(..to);
                    io.cursor = 0;
                }
                KeyCode::Char('w') if ctrl => {
                    let mut end = io.cursor;
                    while end > 0 && io.line[end - 1].is_whitespace() {
                        end -= 1;
                    }
                    while end > 0 && !io.line[end - 1].is_whitespace() {
                        end -= 1;
                    }
                    let to = io.cursor;
                    io.line.drain(end..to);
                    io.cursor = end;
                }
                KeyCode::Char('a') if ctrl => io.cursor = 0,
                KeyCode::Char('e') if ctrl => io.cursor = io.line.len(),
                KeyCode::Char(c) => {
                    let at = io.cursor;
                    io.line.insert(at, c);
                    io.cursor += 1;
                }
                KeyCode::Backspace => {
                    if io.cursor > 0 {
                        io.cursor -= 1;
                        let at = io.cursor;
                        io.line.remove(at);
                    }
                }
                KeyCode::Delete => {
                    if io.cursor < io.line.len() {
                        let at = io.cursor;
                        io.line.remove(at);
                    }
                }
                KeyCode::Left => io.cursor = io.cursor.saturating_sub(1),
                KeyCode::Right => io.cursor = (io.cursor + 1).min(io.line.len()),
                KeyCode::Home => io.cursor = 0,
                KeyCode::End => io.cursor = io.line.len(),
                KeyCode::Up => history_step(&mut io, -1),
                KeyCode::Down => history_step(&mut io, 1),
                KeyCode::Enter => {
                    let line: String = io.line.iter().collect();
                    if !line.trim().is_empty() {
                        if io.history.last().map(String::as_str) != Some(line.as_str()) {
                            io.history.push(line.clone());
                            if io.history.len() > HISTORY_LIMIT {
                                io.history.remove(0);
                            }
                        }
                    }
                    // Block entfernen und die Eingabe als graue Zeile stehen lassen –
                    // so bleibt sichtbar, was man abgeschickt hat.
                    io.erase();
                    io.active = false;
                    if !line.trim().is_empty() {
                        let echo = format!(
                            "  {} {}",
                            io.paint(GRAY, "❯"),
                            io.paint(GRAY, line.trim_end())
                        );
                        let nl = io.newline;
                        let _ = io.out.write_all(echo.as_bytes());
                        let _ = io.out.write_all(nl.as_bytes());
                    }
                    let _ = io.out.flush();
                    return Some(line);
                }
                _ => {}
            }

            io.follow_cursor();
            io.redraw();
        }
    }

    pub fn close(&self) {
        if !self.interactive {
            return;
        }
        let _ = disable_raw_mode();
        if let Ok(mut io) = self.io.lock() {
            let _ = write!(io.out, "\r\n");
            let _ = io.out.flush();
        }
    }
}

/// Im Verlauf blättern: -1 = zurück, +1 = vorwärts.
fn history_step(io: &mut Io, direction: i32) {
    if io.history.is_empty() {
        return;
    }
    let last = io.history.len() - 1;
    let next = match (io.history_pos, direction) {
        (None, -1) => Some(last),
        (Some(0), -1) => Some(0),
        (Some(pos), -1) => Some(pos - 1),
        (Some(pos), 1) if pos < last => Some(pos + 1),
        (Some(_), 1) => None, // hinter dem letzten Eintrag: leere Zeile
        _ => return,
    };
    io.history_pos = next;
    let text = match next {
        Some(pos) => io.history[pos].clone(),
        None => String::new(),
    };
    io.set_line(&text);
}

fn terminal_width() -> u16 {
    crossterm::terminal::size().map(|(w, _)| w.max(1)).unwrap_or(80)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Ausgabekanal, der alles mitschreibt und von außen lesbar bleibt.
    #[derive(Clone)]
    struct Recorder(Arc<Mutex<Vec<u8>>>);

    impl Write for Recorder {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    fn io_for(width: u16, text: &str) -> Io {
        let mut io = Io {
            out: Box::new(io::stdout()),
            color: Arc::new(AtomicBool::new(false)),
            line: text.chars().collect(),
            cursor: text.chars().count(),
            scroll: 0,
            prompt: "❯ ".to_string(),
            link: Link::Online,
            // Absichtlich lang: die Statuszeile darf nie umbrechen.
            status: "verbunden  ·  ein.sehr.langer.servername.example  ·  Kontoname  ·  :help"
                .to_string(),
            history: Vec::new(),
            history_pos: None,
            width,
            active: true,
            drawn: None,
            newline: "\r\n",
            interactive: true,
        };
        io.follow_cursor();
        io
    }

    /// Rahmenlinien und Eingabezeile müssen exakt gleich breit sein, sonst zerfranst der
    /// Kasten im Terminal – und er darf nie über die Terminalbreite hinauslaufen (Umbruch!).
    #[test]
    fn rahmen_ist_buendig() {
        let long = "x".repeat(200);
        for width in [MIN_BOX_WIDTH, 60, 80, 200] {
            for text in ["", "hallo welt", long.as_str()] {
                let io = io_for(width, text);
                let block = io.render_box();
                let body = block.split("\x1b[2A").next().unwrap();
                let lines: Vec<&str> = body.trim_start_matches('\r').split("\r\n").collect();
                assert_eq!(lines.len(), 4, "Rahmen oben/Eingabe/Rahmen unten/Status");
                for line in &lines[..3] {
                    assert_eq!(
                        line.chars().count(),
                        width as usize - 1,
                        "Breite {} bei text-len {}",
                        width,
                        text.len()
                    );
                }
                // Keine Zeile darf umbrechen – auch die Statuszeile nicht.
                for line in &lines {
                    assert!(
                        line.chars().count() < width as usize,
                        "Zeile bricht bei Breite {} um: {:?}",
                        width,
                        line
                    );
                }
            }
        }
    }

    /// Der Cursor muss immer innerhalb des Rahmens stehen – auch wenn die Zeile viel länger
    /// ist als das Terminal.
    #[test]
    fn cursor_bleibt_im_rahmen() {
        let io = io_for(60, "hallo");
        assert!(io.render_box().ends_with("\x1b[2A\r\x1b[10C")); // 3 + "❯ " + 5

        let io = io_for(60, &"x".repeat(500));
        let escape = io.render_box();
        let column: usize = escape
            .rsplit_once("\r\x1b[")
            .and_then(|(_, tail)| tail.trim_end_matches('C').parse().ok())
            .expect("Spaltenangabe");
        assert!(column < 60, "Cursor bei Spalte {} außerhalb des Rahmens", column);
    }

    #[test]
    fn schmales_terminal_faellt_auf_einzeiler_zurueck() {
        let io = io_for(MIN_BOX_WIDTH - 1, "hi");
        assert!(!io.boxed());
        assert_eq!(io.render_plain(), "\r\x1b[2K❯ hi\r\x1b[4C");
    }

    /// Zeilenvorschübe minus Cursor-Bewegungen nach oben = Verschiebung des Blocks auf dem
    /// Schirm. Solange der Block steht, muss sie genau 1 sein (der Cursor parkt eine Zeile
    /// unter der oberen Rahmenlinie); jede ausgegebene Chatzeile kommt als 1 hinzu.
    fn verschiebung(recorder: &Recorder) -> i32 {
        let raw = String::from_utf8(recorder.0.lock().unwrap().clone()).unwrap();
        let down = raw.matches("\r\n").count() as i32;
        let up: i32 = raw
            .match_indices("\x1b[")
            .filter_map(|(at, _)| {
                let tail = &raw[at + 2..];
                let digits: String = tail.chars().take_while(char::is_ascii_digit).collect();
                tail[digits.len()..]
                    .starts_with('A')
                    .then(|| digits.parse::<i32>().unwrap_or(1))
            })
            .sum();
        down - up
    }

    /// Der Block darf beim Neuzeichnen nicht durchs Terminal wandern – genau das passiert, wenn
    /// vor dem Zeichnen nicht gelöscht wird, weil der Cursor in der Eingabezeile geparkt steht.
    #[test]
    fn neuzeichnen_bleibt_an_derselben_stelle() {
        let recorder = Recorder(Arc::new(Mutex::new(Vec::new())));
        let mut io = io_for(48, "");
        io.out = Box::new(recorder.clone());
        io.active = true;

        io.draw(); // Eingabefeld erscheint
        assert_eq!(verschiebung(&recorder), 1);

        for c in "hallo".chars() {
            io.line.push(c);
            io.cursor += 1;
            io.follow_cursor();
            io.redraw(); // je Tastendruck – darf nichts verschieben
            assert_eq!(verschiebung(&recorder), 1, "nach '{}'", c);
        }

        // Chat trudelt ein, während getippt wird: genau eine Zeile mehr, Block wieder darunter.
        io.print_above("12:00 <Peter> na?");
        assert_eq!(verschiebung(&recorder), 2);

        io.erase(); // Enter: Block weg, Cursor zurück auf der Startzeile
        assert_eq!(verschiebung(&recorder), 1);
    }

    /// Zeichnen und Löschen müssen zusammenpassen: der Block ist 4 Zeilen hoch, der Cursor
    /// parkt 2 Zeilen über dem Ende (= 1 unter der oberen Rahmenlinie), also darf das Löschen
    /// genau 1 Zeile nach oben gehen. Stimmt das nicht, wandert der Rahmen bei jedem
    /// Tastendruck durchs Terminal.
    #[test]
    fn loeschen_passt_zum_zeichnen() {
        let block = io_for(60, "hallo").render_box();
        assert_eq!(block.matches("\r\n").count(), 3, "4 Zeilen = 3 Umbrüche");
        assert!(block.contains("\x1b[2A"), "Cursor 2 Zeilen zurück");
        assert_eq!(ERASE_BOX, "\r\x1b[J\x1b[1A\r\x1b[2K");
    }
}
