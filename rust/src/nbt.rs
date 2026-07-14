//! Netzwerk-NBT lesen und Text-Komponenten als ANSI rendern.
//!
//! Seit 1.20.3 verschickt Minecraft Chat-Komponenten nicht mehr als JSON, sondern als NBT
//! ohne Wurzelnamen (Typ-Byte, dann direkt die Nutzdaten). Wir lesen genau so viel, wie zum
//! Anzeigen nötig ist, und geben eine fertig eingefärbte Zeile zurück.

use crate::buf::Reader;
use std::io;

/// Die Array-Varianten werden nie gerendert – gelesen werden müssen sie trotzdem, sonst stünde
/// der Lesezeiger falsch und das restliche Paket wäre Müll.
#[allow(dead_code)]
pub enum Nbt {
    End,
    Byte(i8),
    Short(i16),
    Int(i32),
    Long(i64),
    Float(f32),
    Double(f64),
    ByteArray(Vec<u8>),
    Str(String),
    List(Vec<Nbt>),
    Compound(Vec<(String, Nbt)>),
    IntArray(Vec<i32>),
    LongArray(Vec<i64>),
}

impl Nbt {
    fn get<'a>(&'a self, key: &str) -> Option<&'a Nbt> {
        match self {
            Nbt::Compound(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    fn as_str(&self) -> Option<&str> {
        match self {
            Nbt::Str(s) => Some(s),
            _ => None,
        }
    }

    fn as_bool(&self) -> Option<bool> {
        match self {
            Nbt::Byte(b) => Some(*b != 0),
            _ => None,
        }
    }

    /// Zahlen/Bools als Text – für „text"-Felder, die nicht immer Strings sind.
    fn scalar_text(&self) -> Option<String> {
        match self {
            Nbt::Str(s) => Some(s.clone()),
            Nbt::Byte(v) => Some(v.to_string()),
            Nbt::Short(v) => Some(v.to_string()),
            Nbt::Int(v) => Some(v.to_string()),
            Nbt::Long(v) => Some(v.to_string()),
            Nbt::Float(v) => Some(v.to_string()),
            Nbt::Double(v) => Some(v.to_string()),
            _ => None,
        }
    }
}

/// Netzwerk-NBT: Typ-Byte, dann Nutzdaten (kein Wurzelname).
pub fn read_network(r: &mut Reader) -> io::Result<Nbt> {
    let tag = r.u8()?;
    if tag == 0 {
        return Ok(Nbt::End);
    }
    read_payload(r, tag, 0)
}

fn read_payload(r: &mut Reader, tag: u8, depth: u32) -> io::Result<Nbt> {
    if depth > 64 {
        return Err(crate::buf::err("NBT zu tief verschachtelt"));
    }
    Ok(match tag {
        1 => Nbt::Byte(r.i8()?),
        2 => Nbt::Short(r.i16()?),
        3 => Nbt::Int(r.i32()?),
        4 => Nbt::Long(r.i64()?),
        5 => Nbt::Float(r.f32()?),
        6 => Nbt::Double(r.f64()?),
        7 => {
            let len = r.i32()?.max(0) as usize;
            Nbt::ByteArray(r.bytes(len)?.to_vec())
        }
        8 => Nbt::Str(read_nbt_string(r)?),
        9 => {
            let inner = r.u8()?;
            let len = r.i32()?;
            let mut items = Vec::new();
            if len > 0 && inner != 0 {
                items.reserve(len.min(4096) as usize);
                for _ in 0..len {
                    items.push(read_payload(r, inner, depth + 1)?);
                }
            }
            Nbt::List(items)
        }
        10 => {
            let mut fields = Vec::new();
            loop {
                let t = r.u8()?;
                if t == 0 {
                    break;
                }
                let name = read_nbt_string(r)?;
                fields.push((name, read_payload(r, t, depth + 1)?));
            }
            Nbt::Compound(fields)
        }
        11 => {
            let len = r.i32()?.max(0) as usize;
            let mut v = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                v.push(r.i32()?);
            }
            Nbt::IntArray(v)
        }
        12 => {
            let len = r.i32()?.max(0) as usize;
            let mut v = Vec::with_capacity(len.min(4096));
            for _ in 0..len {
                v.push(r.i64()?);
            }
            Nbt::LongArray(v)
        }
        _ => return Err(crate::buf::err("Unbekannter NBT-Typ")),
    })
}

/// NBT-Strings: u16-Länge, danach (modifiziertes) UTF-8. Ungültige Bytes ersetzen wir,
/// statt die Verbindung wegen einer kaputten Chat-Nachricht zu verlieren.
fn read_nbt_string(r: &mut Reader) -> io::Result<String> {
    let len = r.u16()? as usize;
    let raw = r.bytes(len)?;
    Ok(String::from_utf8_lossy(raw).into_owned())
}

// ===================== Rendern =====================

#[derive(Clone, Default, PartialEq)]
struct Style {
    color: Option<String>,
    bold: bool,
    italic: bool,
    underlined: bool,
    strikethrough: bool,
}

/// Komponente in eine Terminalzeile umwandeln. `color=false` liefert reinen Text.
pub fn render(tag: &Nbt, color: bool) -> String {
    let mut out = String::new();
    walk(tag, &Style::default(), color, &mut out);
    if color && !out.is_empty() {
        out.push_str("\x1b[0m");
    }
    out
}

fn walk(tag: &Nbt, inherited: &Style, color: bool, out: &mut String) {
    match tag {
        // Ein reiner String ist eine gültige Komponente ("Hallo").
        Nbt::Str(s) => emit(s, inherited, color, out),
        Nbt::List(items) => {
            for item in items {
                walk(item, inherited, color, out);
            }
        }
        Nbt::Compound(_) => {
            let style = merge(inherited, tag);

            if let Some(text) = tag.get("text").and_then(|t| t.scalar_text()) {
                emit(&text, &style, color, out);
            } else if let Some(key) = tag.get("translate").and_then(|t| t.as_str()) {
                // Ohne Sprachdatei können wir nicht übersetzen: Schlüssel zeigen, Argumente anhängen
                // (so macht es auch der Java-Client mit Adventure ohne Translator).
                emit(key, &style, color, out);
                if let Some(Nbt::List(args)) = tag.get("with") {
                    for arg in args {
                        emit(" ", &style, color, out);
                        walk(arg, &style, color, out);
                    }
                }
            }

            if let Some(Nbt::List(extra)) = tag.get("extra") {
                for child in extra {
                    walk(child, &style, color, out);
                }
            }
        }
        other => {
            if let Some(text) = other.scalar_text() {
                emit(&text, inherited, color, out);
            }
        }
    }
}

fn merge(inherited: &Style, tag: &Nbt) -> Style {
    let mut s = inherited.clone();
    if let Some(c) = tag.get("color").and_then(|c| c.as_str()) {
        s.color = Some(c.to_string());
    }
    if let Some(v) = tag.get("bold").and_then(|v| v.as_bool()) {
        s.bold = v;
    }
    if let Some(v) = tag.get("italic").and_then(|v| v.as_bool()) {
        s.italic = v;
    }
    if let Some(v) = tag.get("underlined").and_then(|v| v.as_bool()) {
        s.underlined = v;
    }
    if let Some(v) = tag.get("strikethrough").and_then(|v| v.as_bool()) {
        s.strikethrough = v;
    }
    s
}

/// Text ausgeben – inklusive Übersetzung alter §-Farbcodes, die viele Server noch senden.
fn emit(text: &str, style: &Style, color: bool, out: &mut String) {
    if text.is_empty() {
        return;
    }
    if !color {
        out.push_str(&strip_legacy(text));
        return;
    }
    out.push_str(&ansi_of(style));
    let mut chars = text.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '\u{00a7}' {
            if let Some(code) = chars.next() {
                match legacy_ansi(code) {
                    Some(seq) => out.push_str(seq),
                    // Unbekannter Code: Grundstil wiederherstellen, damit nichts „ausblutet".
                    None => {
                        out.push_str("\x1b[0m");
                        out.push_str(&ansi_of(style));
                    }
                }
            }
        } else {
            out.push(c);
        }
    }
}

fn strip_legacy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c == '\u{00a7}' {
            chars.next();
        } else {
            out.push(c);
        }
    }
    out
}

fn ansi_of(style: &Style) -> String {
    let mut s = String::from("\x1b[0m");
    if let Some(c) = &style.color {
        if let Some(hex) = c.strip_prefix('#') {
            if hex.len() == 6 {
                if let (Ok(r), Ok(g), Ok(b)) = (
                    u8::from_str_radix(&hex[0..2], 16),
                    u8::from_str_radix(&hex[2..4], 16),
                    u8::from_str_radix(&hex[4..6], 16),
                ) {
                    s.push_str(&format!("\x1b[38;2;{};{};{}m", r, g, b));
                }
            }
        } else if let Some(code) = named_ansi(c) {
            s.push_str(code);
        }
    }
    if style.bold {
        s.push_str("\x1b[1m");
    }
    if style.italic {
        s.push_str("\x1b[3m");
    }
    if style.underlined {
        s.push_str("\x1b[4m");
    }
    if style.strikethrough {
        s.push_str("\x1b[9m");
    }
    s
}

fn named_ansi(name: &str) -> Option<&'static str> {
    Some(match name {
        "black" => "\x1b[30m",
        "dark_blue" => "\x1b[34m",
        "dark_green" => "\x1b[32m",
        "dark_aqua" => "\x1b[36m",
        "dark_red" => "\x1b[31m",
        "dark_purple" => "\x1b[35m",
        "gold" => "\x1b[33m",
        "gray" => "\x1b[37m",
        "dark_gray" => "\x1b[90m",
        "blue" => "\x1b[94m",
        "green" => "\x1b[92m",
        "aqua" => "\x1b[96m",
        "red" => "\x1b[91m",
        "light_purple" => "\x1b[95m",
        "yellow" => "\x1b[93m",
        "white" => "\x1b[97m",
        _ => return None,
    })
}

fn legacy_ansi(code: char) -> Option<&'static str> {
    Some(match code.to_ascii_lowercase() {
        '0' => "\x1b[30m",
        '1' => "\x1b[34m",
        '2' => "\x1b[32m",
        '3' => "\x1b[36m",
        '4' => "\x1b[31m",
        '5' => "\x1b[35m",
        '6' => "\x1b[33m",
        '7' => "\x1b[37m",
        '8' => "\x1b[90m",
        '9' => "\x1b[94m",
        'a' => "\x1b[92m",
        'b' => "\x1b[96m",
        'c' => "\x1b[91m",
        'd' => "\x1b[95m",
        'e' => "\x1b[93m",
        'f' => "\x1b[97m",
        'l' => "\x1b[1m",
        'o' => "\x1b[3m",
        'n' => "\x1b[4m",
        'm' => "\x1b[9m",
        'r' => "\x1b[0m",
        'k' => "", // obfuscated: im Terminal einfach ignorieren
        _ => return None,
    })
}
