//! Netzwerk-NBT lesen und Text-Komponenten ausgeben.
//!
//! Seit 1.20.3 verschickt Minecraft Chat-Komponenten nicht mehr als JSON, sondern als NBT
//! ohne Wurzelnamen (Typ-Byte, dann direkt die Nutzdaten). Wir lesen genau so viel, wie zum
//! Anzeigen nötig ist, und geben eine fertige Zeile zurück – in einem von drei Formaten
//! ([`Fmt`]): ANSI fürs Terminal, roher Text ohne Farben, oder die alten `§`-Codes für ein
//! Programm davor (Panel/Webseite). Letzteres ist der Weg, auf dem Anzeigetafel und
//! Gegenstandsnamen ihre Farben behalten.

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
    pub(crate) fn get<'a>(&'a self, key: &str) -> Option<&'a Nbt> {
        match self {
            Nbt::Compound(fields) => fields.iter().find(|(k, _)| k == key).map(|(_, v)| v),
            _ => None,
        }
    }

    /// Ganzzahliges Feld eines Compounds. Registry-Daten verwenden je nach Bereich Byte,
    /// Short, Int oder Long; für Welthöhen sind alle vier verlustfrei als `i32` darstellbar.
    #[cfg(feature = "pov")]
    pub(crate) fn get_i32(&self, key: &str) -> Option<i32> {
        match self.get(key)? {
            Nbt::Byte(value) => Some(*value as i32),
            Nbt::Short(value) => Some(*value as i32),
            Nbt::Int(value) => Some(*value),
            Nbt::Long(value) => i32::try_from(*value).ok(),
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

/// Obergrenze für die Zahl der Knoten einer Netzwerk-NBT-Struktur.
///
/// Ein Paket ist längenbegrenzt, beim Einlesen bläht es sich aber auf: aus einer Liste von
/// Einzelbytes wird je Eintrag ein vollwertiger [`Nbt`]-Wert von gut dreißig Byte. Ein Server
/// könnte damit aus 32 MB Paketdaten fast ein Gigabyte Speicher im Client machen, ohne dass
/// irgendeine Längenprüfung anschlüge. Echte Chat-Komponenten und Registerdaten kommen mit ein
/// paar tausend Knoten aus; die halbe Million hier ist bewusst großzügig und trotzdem eine
/// harte Schranke (rund 16 MB).
const MAX_NODES: u32 = 512 * 1024;

/// Netzwerk-NBT: Typ-Byte, dann Nutzdaten (kein Wurzelname).
pub fn read_network(r: &mut Reader) -> io::Result<Nbt> {
    let tag = r.u8()?;
    if tag == 0 {
        return Ok(Nbt::End);
    }
    let mut budget = MAX_NODES;
    read_payload(r, tag, 0, &mut budget)
}

fn read_payload(r: &mut Reader, tag: u8, depth: u32, budget: &mut u32) -> io::Result<Nbt> {
    if depth > 64 {
        return Err(crate::buf::err("NBT zu tief verschachtelt"));
    }
    // Jeder gelesene Wert kostet ein Guthaben; Listen und Compounds können damit nicht mehr
    // beliebig viele kleine Knoten erzeugen.
    match budget.checked_sub(1) {
        Some(left) => *budget = left,
        None => return Err(crate::buf::err("NBT zu gross")),
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
                    items.push(read_payload(r, inner, depth + 1, budget)?);
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
                fields.push((name, read_payload(r, t, depth + 1, budget)?));
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

/// Wohin die Zeile geht.
#[derive(Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)] // `Legacy` wird nur von Scoreboard-/Items-Bauformen konstruiert.
pub enum Fmt {
    /// Reiner Text ohne jede Formatierung.
    Plain,
    /// Für das Terminal: ANSI-Folgen.
    Ansi,
    /// Für ein Programm davor: die alten `§`-Codes von Minecraft. Echte Farben (`#rrggbb`)
    /// werden als `§x§r§r§g§g§b§b` geschrieben – so macht es auch BungeeCord, und genau das
    /// erwarten Panels und Weboberflächen.
    Legacy,
}

#[derive(Clone, Default, PartialEq)]
struct Style {
    color: Option<String>,
    bold: bool,
    italic: bool,
    underlined: bool,
    strikethrough: bool,
}

/// Komponente in eine Zeile umwandeln.
pub fn render(tag: &Nbt, fmt: Fmt) -> String {
    let mut out = String::new();
    walk(tag, &Style::default(), fmt, &mut out);
    if fmt == Fmt::Ansi && !out.is_empty() {
        out.push_str("\x1b[0m");
    }
    out
}

fn walk(tag: &Nbt, inherited: &Style, fmt: Fmt, out: &mut String) {
    match tag {
        // Ein reiner String ist eine gültige Komponente ("Hallo").
        Nbt::Str(s) => emit(s, inherited, fmt, out),
        Nbt::List(items) => {
            for item in items {
                walk(item, inherited, fmt, out);
            }
        }
        Nbt::Compound(_) => {
            let style = merge(inherited, tag);

            if let Some(text) = tag.get("text").and_then(|t| t.scalar_text()) {
                emit(&text, &style, fmt, out);
            } else if let Some(key) = tag.get("translate").and_then(|t| t.as_str()) {
                // Ohne Sprachdatei können wir nicht übersetzen: Schlüssel zeigen, Argumente anhängen
                // (so macht es auch der Java-Client mit Adventure ohne Translator).
                emit(key, &style, fmt, out);
                if let Some(Nbt::List(args)) = tag.get("with") {
                    for arg in args {
                        emit(" ", &style, fmt, out);
                        walk(arg, &style, fmt, out);
                    }
                }
            }

            if let Some(Nbt::List(extra)) = tag.get("extra") {
                for child in extra {
                    walk(child, &style, fmt, out);
                }
            }
        }
        other => {
            if let Some(text) = other.scalar_text() {
                emit(&text, inherited, fmt, out);
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
///
/// Alles wird direkt in `out` geschrieben. Das ist kein Selbstzweck: Chat ist der Dauerbetrieb
/// auch des schlanken Clients, und je Textbaustein wurden hier bisher zwei bis drei kurzlebige
/// Zeichenketten für den Stil angelegt, die sofort wieder weggeworfen wurden.
fn emit(text: &str, style: &Style, fmt: Fmt, out: &mut String) {
    if text.is_empty() {
        return;
    }
    match fmt {
        Fmt::Plain => push_stripped(out, text),
        // Der Text bringt seine eigenen §-Codes schon mit; davor kommt nur der geerbte Stil.
        Fmt::Legacy => {
            push_legacy(out, style);
            out.push_str(text);
        }
        Fmt::Ansi => {
            push_ansi(out, style);
            let mut chars = text.chars();
            while let Some(c) = chars.next() {
                if c != '\u{00a7}' {
                    out.push(c);
                    continue;
                }
                let Some(code) = chars.next() else { break };
                // §x leitet eine echte Farbe ein: §x§r§r§g§g§b§b.
                if code.eq_ignore_ascii_case(&'x') {
                    if !push_legacy_hex(out, &mut chars) {
                        break;
                    }
                    continue;
                }
                match legacy_ansi(code) {
                    Some(seq) => out.push_str(seq),
                    // Unbekannter Code: Grundstil wiederherstellen, damit nichts „ausblutet".
                    None => {
                        out.push_str("\x1b[0m");
                        push_ansi(out, style);
                    }
                }
            }
        }
    }
}

/// `§`-Text in ANSI übersetzen. Genutzt für alles, was im Speicher als §-Text liegt
/// (Anzeigetafel, Gegenstandsnamen) und erst beim Anzeigen eingefärbt wird.
#[allow(dead_code)] // nur Bauformen, die §-Text im Zustand halten
pub fn legacy_to_ansi(text: &str) -> String {
    let mut out = String::with_capacity(text.len() + 32);
    emit(text, &Style::default(), Fmt::Ansi, &mut out);
    if !out.is_empty() {
        out.push_str("\x1b[0m");
    }
    out
}

/// Sechs `§`-Ziffern nach `§x` einlesen und als ANSI-Echtfarbe anhängen.
/// `false` = die Folge war unvollständig, der Rest des Textes ist damit unbrauchbar.
fn push_legacy_hex(out: &mut String, chars: &mut std::str::Chars) -> bool {
    let mut value: u32 = 0;
    for _ in 0..6 {
        if chars.next() != Some('\u{00a7}') {
            return false;
        }
        let Some(digit) = chars.next().and_then(|c| c.to_digit(16)) else {
            return false;
        };
        value = (value << 4) | digit;
    }
    push_true_color(out, ((value >> 16) & 0xFF) as u8, ((value >> 8) & 0xFF) as u8, (value & 0xFF) as u8);
    true
}

fn push_true_color(out: &mut String, r: u8, g: u8, b: u8) {
    out.push_str("\x1b[38;2;");
    push_number(out, r);
    out.push(';');
    push_number(out, g);
    out.push(';');
    push_number(out, b);
    out.push('m');
}

fn push_number(out: &mut String, value: u8) {
    if value >= 100 {
        out.push((b'0' + value / 100) as char);
    }
    if value >= 10 {
        out.push((b'0' + (value / 10) % 10) as char);
    }
    out.push((b'0' + value % 10) as char);
}

/// `§`-Codes entfernen. Gegenstück zu [`legacy_to_ansi`] und aus demselben Grund nicht in jeder
/// Bauform benutzt: nur wer §-Text im Zustand hält (Anzeigetafel, Gegenstände), entfärbt ihn auch.
#[allow(dead_code)]
pub fn strip_legacy(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    push_stripped(&mut out, text);
    out
}

fn push_stripped(out: &mut String, text: &str) {
    let mut chars = text.chars();
    while let Some(c) = chars.next() {
        if c != '\u{00a7}' {
            out.push(c);
            continue;
        }
        // §x frisst die sechs folgenden §-Paare gleich mit.
        if chars
            .next()
            .is_some_and(|code| code.eq_ignore_ascii_case(&'x'))
        {
            for _ in 0..12 {
                chars.next();
            }
        }
    }
}

fn push_ansi(out: &mut String, style: &Style) {
    out.push_str("\x1b[0m");
    if let Some(c) = &style.color {
        if let Some(hex) = c.strip_prefix('#') {
            // Erst auf ASCII-Hexziffern prüfen: `&hex[0..2]` schnitte sonst mitten durch ein
            // Mehrbyte-Zeichen und risse den Client mit einem Panik-Abbruch weg. Sechs Bytes
            // Text sind nicht zwingend sechs Zeichen – `"#a€bc"` reichte dafür schon.
            if hex.len() == 6 && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
                if let (Ok(r), Ok(g), Ok(b)) = (
                    u8::from_str_radix(&hex[0..2], 16),
                    u8::from_str_radix(&hex[2..4], 16),
                    u8::from_str_radix(&hex[4..6], 16),
                ) {
                    push_true_color(out, r, g, b);
                }
            }
        } else if let Some(code) = named_ansi(c) {
            out.push_str(code);
        }
    }
    for (on, code) in [
        (style.bold, "\x1b[1m"),
        (style.italic, "\x1b[3m"),
        (style.underlined, "\x1b[4m"),
        (style.strikethrough, "\x1b[9m"),
    ] {
        if on {
            out.push_str(code);
        }
    }
}

/// Stil als `§`-Codes – das Gegenstück zu [`push_ansi`] für die Weitergabe an ein Programm davor.
fn push_legacy(out: &mut String, style: &Style) {
    out.push_str("\u{00a7}r");
    if let Some(c) = &style.color {
        match c.strip_prefix('#') {
            // Echte Farbe: §x§r§r§g§g§b§b (Schreibweise von BungeeCord).
            Some(hex) if hex.len() == 6 && hex.chars().all(|c| c.is_ascii_hexdigit()) => {
                out.push_str("\u{00a7}x");
                for digit in hex.chars() {
                    out.push('\u{00a7}');
                    out.push(digit.to_ascii_lowercase());
                }
            }
            Some(_) => {}
            None => {
                if let Some(code) = named_legacy(c) {
                    out.push('\u{00a7}');
                    out.push(code);
                }
            }
        }
    }
    for (on, code) in [
        (style.bold, 'l'),
        (style.italic, 'o'),
        (style.underlined, 'n'),
        (style.strikethrough, 'm'),
    ] {
        if on {
            out.push('\u{00a7}');
            out.push(code);
        }
    }
}

/// Den NBT-Stil eines Minecraft-`StyledFormat` als `§`-Präfix ausgeben. Scoreboards verwenden
/// dafür ein reines Style-Compound statt einer Text-Komponente.
#[cfg(feature = "board")]
pub(crate) fn style_legacy(tag: &Nbt) -> String {
    let mut out = String::new();
    push_legacy(&mut out, &merge(&Style::default(), tag));
    out
}

fn named_legacy(name: &str) -> Option<char> {
    Some(match name {
        "black" => '0',
        "dark_blue" => '1',
        "dark_green" => '2',
        "dark_aqua" => '3',
        "dark_red" => '4',
        "dark_purple" => '5',
        "gold" => '6',
        "gray" => '7',
        "dark_gray" => '8',
        "blue" => '9',
        "green" => 'a',
        "aqua" => 'b',
        "red" => 'c',
        "light_purple" => 'd',
        "yellow" => 'e',
        "white" => 'f',
        _ => return None,
    })
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

#[cfg(test)]
mod tests {
    use super::*;

    fn text(value: &str) -> Nbt {
        Nbt::Str(value.to_string())
    }

    fn compound(fields: Vec<(&str, Nbt)>) -> Nbt {
        Nbt::Compound(
            fields
                .into_iter()
                .map(|(k, v)| (k.to_string(), v))
                .collect(),
        )
    }

    /// Farben und Fettschrift müssen als §-Codes herauskommen – nur so kann ein Programm davor
    /// (Panel, Webseite) sie weiterverwenden.
    #[test]
    fn stil_wird_zu_farbcodes() {
        let tag = compound(vec![
            ("text", text("Punkte")),
            ("color", text("gold")),
            ("bold", Nbt::Byte(1)),
        ]);
        assert_eq!(render(&tag, Fmt::Legacy), "§r§6§lPunkte");
        assert_eq!(render(&tag, Fmt::Plain), "Punkte");
    }

    /// Echte Farben überstehen den Weg als §x-Folge und kommen als ANSI-Echtfarbe zurück.
    #[test]
    fn echte_farben_ueberleben_beide_richtungen() {
        let tag = compound(vec![("text", text("Hi")), ("color", text("#ff8800"))]);
        let legacy = render(&tag, Fmt::Legacy);
        assert_eq!(legacy, "§r§x§f§f§8§8§0§0Hi");
        assert!(legacy_to_ansi(&legacy).contains("\x1b[38;2;255;136;0m"));
        assert_eq!(strip_legacy(&legacy), "Hi");
    }

    /// §-Codes, die schon im Text stehen, dürfen nicht verloren gehen.
    #[test]
    fn vorhandene_farbcodes_bleiben_stehen() {
        let tag = text("§aGrün §cRot");
        assert_eq!(render(&tag, Fmt::Legacy), "§r§aGrün §cRot");
        assert_eq!(render(&tag, Fmt::Plain), "Grün Rot");
        let ansi = render(&tag, Fmt::Ansi);
        assert!(ansi.contains("\x1b[92m") && ansi.contains("\x1b[91m"));
    }

    /// Eine Farbangabe aus sechs *Bytes*, aber nicht sechs ASCII-Zeichen, hat den Client
    /// beim Zerlegen mitten durch ein Mehrbyte-Zeichen schneiden lassen – das beendete den
    /// Prozess. Sie muss stattdessen einfach ungefärbt durchgehen.
    #[test]
    fn kaputte_farbangabe_stuerzt_nicht_ab() {
        for farbe in ["#a\u{20ac}bc", "#\u{e4}\u{f6}\u{fc}", "#zzzzzz", "#12345", "#", "unsinn"] {
            let tag = compound(vec![("text", text("Hi")), ("color", text(farbe))]);
            assert_eq!(render(&tag, Fmt::Plain), "Hi");
            assert!(render(&tag, Fmt::Ansi).contains("Hi"));
            assert!(render(&tag, Fmt::Legacy).contains("Hi"));
        }
    }

    /// Ein Paket ist zwar längenbegrenzt, eine Liste aus Einzelbytes bläht sich beim Einlesen
    /// aber um das Dreißigfache auf. Aus wenigen Megabyte Paketdaten wurde so fast ein Gigabyte
    /// Speicher – das muss der Decoder abweisen, statt ihn anzulegen.
    #[test]
    fn riesige_listen_werden_abgewiesen() {
        // TAG_List mit MAX_NODES+1 Einträgen vom Typ Byte: die Nutzdaten sind ein Byte je
        // Eintrag, der gelesene Baum wäre ein Vielfaches davon.
        let count = MAX_NODES as usize + 1;
        let mut data = Vec::with_capacity(count + 8);
        data.push(9); // TAG_List
        data.push(1); // Inhalt: TAG_Byte
        data.extend_from_slice(&(count as i32).to_be_bytes());
        data.resize(data.len() + count, 0);

        let mut r = crate::buf::Reader::new(&data);
        assert!(read_network(&mut r).is_err());

        // Was ein Server wirklich schickt, bleibt selbstverständlich lesbar.
        let mut small = vec![9u8, 1];
        small.extend_from_slice(&1000i32.to_be_bytes());
        small.resize(small.len() + 1000, 0);
        let mut r = crate::buf::Reader::new(&small);
        assert!(matches!(read_network(&mut r), Ok(Nbt::List(items)) if items.len() == 1000));
    }

    /// Verschachtelte Komponenten erben den Stil des Elternteils.
    #[test]
    fn extra_erbt_den_stil() {
        let tag = compound(vec![
            ("text", text("a")),
            ("color", text("red")),
            ("extra", Nbt::List(vec![text("b")])),
        ]);
        assert_eq!(render(&tag, Fmt::Legacy), "§r§ca§r§cb");
        assert_eq!(render(&tag, Fmt::Plain), "ab");
    }
}
