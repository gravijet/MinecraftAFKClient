//! Terminal im Rohmodus: Chat darf eintrudeln, **während** man tippt.
//!
//! Der Kniff ist derselbe wie bei JLine im Java-Client: Vor jeder Ausgabe wird die Eingabezeile
//! gelöscht, die Nachricht geschrieben und die Eingabezeile darunter neu gezeichnet. Es läuft
//! kein eigener Ausgabe-Thread – Ausgaben sind kurz und blockieren den Netz-Thread nicht spürbar,
//! da sie nur in den Terminalpuffer schreiben.

use crossterm::event::{self, Event, KeyCode, KeyEventKind, KeyModifiers};
use crossterm::terminal::{disable_raw_mode, enable_raw_mode};
use std::io::{self, Stdout, Write};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};

pub const RESET: &str = "\x1b[0m";
pub const GRAY: &str = "\x1b[90m";
pub const RED: &str = "\x1b[91m";
pub const CYAN: &str = "\x1b[96m";
pub const BOLD: &str = "\x1b[1m";

struct Io {
    out: Stdout,
    prompt: String,
    line: String,
    /// true, während eine Eingabezeile auf dem Schirm steht.
    active: bool,
    /// Im Rohmodus braucht jede Zeile ein CR; headless nicht.
    newline: &'static str,
}

impl Io {
    fn print_above(&mut self, text: &str) {
        if self.active {
            let _ = write!(self.out, "\r\x1b[2K");
        }
        let _ = write!(self.out, "{}{}", text, self.newline);
        if self.active {
            let _ = write!(self.out, "{}{}", self.prompt, self.line);
        }
        let _ = self.out.flush();
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

        Console {
            io: Arc::new(Mutex::new(Io {
                out: io::stdout(),
                prompt: String::new(),
                line: String::new(),
                active: false,
                newline: if interactive { "\r\n" } else { "\n" },
            })),
            color: Arc::new(AtomicBool::new(true)),
            interactive,
        }
    }

    pub fn set_color(&self, enabled: bool) {
        self.color.store(enabled, Ordering::Relaxed);
    }

    pub fn is_color(&self) -> bool {
        self.color.load(Ordering::Relaxed)
    }

    /// Eine Zeile oberhalb der Eingabezeile ausgeben (thread-sicher).
    pub fn print(&self, text: &str) {
        if let Ok(mut io) = self.io.lock() {
            io.print_above(text);
        }
    }

    pub fn info(&self, text: &str) {
        self.print(&self.paint(GRAY, text));
    }

    pub fn error(&self, text: &str) {
        self.print(&self.paint(RED, text));
    }

    pub fn paint(&self, code: &str, text: &str) -> String {
        if self.is_color() {
            format!("{}{}{}", code, text, RESET)
        } else {
            text.to_string()
        }
    }

    pub fn clear_screen(&self) {
        if let Ok(mut io) = self.io.lock() {
            let _ = write!(io.out, "\x1b[2J\x1b[H");
            let _ = io.out.flush();
        }
    }

    /// Liest eine Zeile. `None` bei Strg+C / Strg+D (und immer im headless-Betrieb).
    pub fn read_line(&self, prompt: &str) -> Option<String> {
        if !self.interactive {
            return None;
        }
        {
            let mut io = self.io.lock().ok()?;
            io.prompt = prompt.to_string();
            io.line.clear();
            io.active = true;
            let _ = write!(io.out, "\r\x1b[2K{}", prompt);
            let _ = io.out.flush();
        }

        let result = self.edit_loop();

        if let Ok(mut io) = self.io.lock() {
            io.active = false;
            io.line.clear();
        }
        result
    }

    fn edit_loop(&self) -> Option<String> {
        loop {
            let key = match event::read() {
                Ok(Event::Key(key)) if key.kind == KeyEventKind::Press => key,
                Ok(_) => continue,
                Err(_) => return None,
            };
            let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
            let mut io = self.io.lock().ok()?;

            match key.code {
                KeyCode::Char('c') if ctrl => return None,
                KeyCode::Char('d') if ctrl && io.line.is_empty() => return None,
                KeyCode::Char(c) => {
                    io.line.push(c);
                    let _ = write!(io.out, "{}", c);
                    let _ = io.out.flush();
                }
                KeyCode::Backspace => {
                    if io.line.pop().is_some() {
                        let _ = write!(io.out, "\x08 \x08");
                        let _ = io.out.flush();
                    }
                }
                KeyCode::Enter => {
                    let _ = write!(io.out, "\r\n");
                    let _ = io.out.flush();
                    return Some(std::mem::take(&mut io.line));
                }
                _ => {}
            }
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
