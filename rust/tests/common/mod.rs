//! Winziger Minecraft-Server für Integrationstests.
//!
//! Er spricht genau so viel Protokoll, wie der Client zum Beitreten braucht: Handshake,
//! Login ohne Verschlüsselung und ohne Kompression, Konfigurationsphase mit Registerdaten
//! und danach die Spielphase. Damit lässt sich der komplette Ablauf des echten Binaries
//! prüfen – einschließlich der Live-POV, die sonst nur an einem echten Server sichtbar wäre.
#![allow(dead_code)]

use aes::cipher::{BlockEncrypt, KeyInit};
use aes::{Aes128, Block};
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use rsa::pkcs8::EncodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPrivateKey, RsaPublicKey};
use std::io::{BufRead, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

// ===================== Schreiben =====================

#[derive(Default, Clone)]
pub struct Buf {
    pub data: Vec<u8>,
}

impl Buf {
    pub fn packet(id: i32) -> Buf {
        let mut b = Buf::default();
        b.var_int(id);
        b
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.data.push(v);
        self
    }
    pub fn bool(&mut self, v: bool) -> &mut Self {
        self.data.push(v as u8);
        self
    }
    pub fn i16(&mut self, v: i16) -> &mut Self {
        self.data.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn i32(&mut self, v: i32) -> &mut Self {
        self.data.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn i64(&mut self, v: i64) -> &mut Self {
        self.data.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.data.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn f64(&mut self, v: f64) -> &mut Self {
        self.data.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn var_int(&mut self, value: i32) -> &mut Self {
        let mut v = value as u32;
        loop {
            if v & !0x7F == 0 {
                self.data.push(v as u8);
                return self;
            }
            self.data.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
    }
    pub fn string(&mut self, value: &str) -> &mut Self {
        self.var_int(value.len() as i32);
        self.data.extend_from_slice(value.as_bytes());
        self
    }
    pub fn byte_array(&mut self, value: &[u8]) -> &mut Self {
        self.var_int(value.len() as i32);
        self.data.extend_from_slice(value);
        self
    }
    pub fn raw(&mut self, value: &[u8]) -> &mut Self {
        self.data.extend_from_slice(value);
        self
    }
    pub fn uuid(&mut self, value: &[u8; 16]) -> &mut Self {
        self.data.extend_from_slice(value);
        self
    }
    /// Netzwerk-NBT eines Compounds mit lauter Int-Feldern (mehr braucht der Test nicht).
    pub fn nbt_ints(&mut self, fields: &[(&str, i32)]) -> &mut Self {
        self.u8(10); // TAG_Compound, kein Wurzelname
        for (name, value) in fields {
            self.u8(3); // TAG_Int
            self.i16(name.len() as i16);
            self.raw(name.as_bytes());
            self.i32(*value);
        }
        self.u8(0)
    }
    /// Netzwerk-NBT einer Textkomponente.
    pub fn nbt_text(&mut self, text: &str) -> &mut Self {
        self.u8(10);
        self.u8(8); // TAG_String
        self.i16(4);
        self.raw(b"text");
        self.nbt_string(text);
        self.u8(0)
    }

    /// Eine NBT-Zeichenkette: u16-Länge, danach Javas **modifiziertes** UTF-8.
    ///
    /// Für reinen ASCII-Text ist das dasselbe wie gewöhnliches UTF-8 – für alles darüber nicht,
    /// und genau daran hing ein Fehler: Zeichen über U+FFFF (jedes Emoji) schreibt Java als
    /// **zwei** Drei-Byte-Folgen, eine je Ersatzzeichen-Hälfte. Der Testserver muss das genauso
    /// machen, sonst prüft der Test etwas, das so nie vom Netz kommt.
    pub fn nbt_string(&mut self, text: &str) -> &mut Self {
        let mut bytes = Vec::with_capacity(text.len());
        let mut units = [0u16; 2];
        for c in text.chars() {
            let code = c as u32;
            if code != 0 && code < 0x80 {
                bytes.push(code as u8);
            } else if code < 0x800 {
                // Die 0 gehört bewusst hierher: sie wird als Überlänge C0 80 geschrieben.
                bytes.push(0xC0 | (code >> 6) as u8);
                bytes.push(0x80 | (code & 0x3F) as u8);
            } else {
                for unit in c.encode_utf16(&mut units) {
                    let unit = *unit as u32;
                    bytes.push(0xE0 | (unit >> 12) as u8);
                    bytes.push(0x80 | ((unit >> 6) & 0x3F) as u8);
                    bytes.push(0x80 | (unit & 0x3F) as u8);
                }
            }
        }
        self.i16(bytes.len() as i16);
        self.raw(&bytes)
    }
}

// ===================== Lesen =====================

pub struct Cursor<'a> {
    pub data: &'a [u8],
    pub pos: usize,
}

impl<'a> Cursor<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Cursor { data, pos: 0 }
    }
    pub fn u8(&mut self) -> u8 {
        let v = self.data[self.pos];
        self.pos += 1;
        v
    }
    pub fn var_int(&mut self) -> i32 {
        let mut value = 0i32;
        let mut shift = 0;
        loop {
            let b = self.u8();
            value |= ((b & 0x7F) as i32) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                return value;
            }
        }
    }
    pub fn string(&mut self) -> String {
        let len = self.var_int() as usize;
        let out = String::from_utf8_lossy(&self.data[self.pos..self.pos + len]).into_owned();
        self.pos += len;
        out
    }
    pub fn bool(&mut self) -> bool {
        self.u8() != 0
    }
    pub fn byte_array(&mut self) -> Vec<u8> {
        let len = self.var_int() as usize;
        let out = self.data[self.pos..self.pos + len].to_vec();
        self.pos += len;
        out
    }
}

// ===================== AES-128-CFB8 =====================

/// CFB8 nach Lehrbuch: ein Schieberegister, ein AES-Block je Byte.
///
/// Bewusst die einfache Fassung – der Client benutzt einen Ringspeicher, und genau dessen
/// Ergebnis soll hier geprüft werden, nicht dieselbe Abkürzung noch einmal.
pub struct Cfb8 {
    aes: Aes128,
    iv: [u8; 16],
}

impl Cfb8 {
    fn new(key: &[u8; 16]) -> Cfb8 {
        Cfb8 {
            aes: Aes128::new(key.into()),
            iv: *key, // Minecraft nutzt den Schlüssel zugleich als IV
        }
    }

    fn apply(&mut self, data: &mut [u8], encrypt: bool) {
        for byte in data.iter_mut() {
            let mut block: Block = self.iv.into();
            self.aes.encrypt_block(&mut block);
            let (out, feedback) = if encrypt {
                let cipher = *byte ^ block[0];
                (cipher, cipher)
            } else {
                (*byte ^ block[0], *byte)
            };
            self.iv.copy_within(1.., 0);
            self.iv[15] = feedback;
            *byte = out;
        }
    }
}

// ===================== Verbindung =====================

/// Kanal für die Rückmeldungen an den Testfall.
type Notes = std::sync::mpsc::Sender<Note>;

/// Die Schreibseite des Testservers – von der Hauptschleife **und** vom KeepAlive-Thread benutzt.
///
/// Zusammen und unter einer Sperre, nicht zwei getrennte Sockets: CFB8 ist ein Strom. Zwei
/// unabhängige Chiffre-Zustände auf derselben Verbindung ergäben ab dem ersten Nebeneinander
/// Müll – und der Fehler sähe aus wie ein Fehler im Client.
pub struct Wire {
    stream: TcpStream,
    threshold: i32,
    enc: Option<Cfb8>,
}

impl Wire {
    fn send(&mut self, packet: &Buf) {
        let mut framed = frame(self.threshold, packet);
        if let Some(enc) = &mut self.enc {
            enc.apply(&mut framed, true);
        }
        let _ = self.stream.write_all(&framed);
        let _ = self.stream.flush();
    }
}

pub struct Conn {
    stream: TcpStream,
    buffer: Vec<u8>,
    /// Kompressionsschwelle wie nach `login::CB_COMPRESSION`; negativ = aus.
    threshold: i32,
    /// Entschlüsselung der Leserichtung, sobald das Handshake durch ist.
    dec: Option<Cfb8>,
    out: Arc<Mutex<Wire>>,
}

/// Einen Paketrahmen bauen – mit oder ohne zlib, je nach Schwelle.
///
/// Fast jeder echte Server schaltet die Kompression ein (Vanilla ab 256 Byte). Ohne diesen Weg
/// im Testserver liefe im Ablauftest immer nur der unkomprimierte Zweig – also genau der, den
/// draußen kaum jemand benutzt.
pub fn frame(threshold: i32, packet: &Buf) -> Vec<u8> {
    let mut body = Buf::default();
    if threshold < 0 {
        body.raw(&packet.data);
    } else if packet.data.len() >= threshold as usize {
        body.var_int(packet.data.len() as i32);
        let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
        z.write_all(&packet.data).expect("zlib");
        body.raw(&z.finish().expect("zlib"));
    } else {
        body.var_int(0); // unter der Schwelle: unkomprimiert übertragen
        body.raw(&packet.data);
    }
    let mut framed = Buf::default();
    framed.var_int(body.data.len() as i32);
    framed.raw(&body.data);
    framed.data
}

impl Conn {
    pub fn send(&mut self, packet: &Buf) {
        self.out.lock().unwrap().send(packet);
    }

    /// Nächstes Paket (id, Nutzdaten). `None` = Verbindung zu.
    pub fn recv(&mut self) -> Option<(i32, Vec<u8>)> {
        let len = self.read_var_int()?;
        if len <= 0 || len > 8 * 1024 * 1024 {
            return None;
        }
        self.buffer.resize(len as usize, 0);
        self.stream.read_exact(&mut self.buffer).ok()?;
        if let Some(dec) = &mut self.dec {
            let mut buffer = std::mem::take(&mut self.buffer);
            dec.apply(&mut buffer, false);
            self.buffer = buffer;
        }

        let payload = if self.threshold < 0 {
            self.buffer.clone()
        } else {
            let mut c = Cursor::new(&self.buffer);
            let uncompressed = c.var_int();
            if uncompressed == 0 {
                self.buffer[c.pos..].to_vec()
            } else {
                let expected = usize::try_from(uncompressed).ok()?;
                let mut out = Vec::with_capacity(expected);
                ZlibDecoder::new(&self.buffer[c.pos..])
                    .read_to_end(&mut out)
                    .ok()?;
                assert_eq!(out.len(), expected, "entpackte Laenge");
                out
            }
        };
        let mut c = Cursor::new(&payload);
        let id = c.var_int();
        Some((id, payload[c.pos..].to_vec()))
    }

    /// Die Rahmenlänge steht als VarInt vor jedem Paket und muss byteweise entschlüsselt werden.
    fn read_var_int(&mut self) -> Option<i32> {
        let mut value = 0i32;
        let mut shift = 0;
        loop {
            let mut b = [0u8; 1];
            self.stream.read_exact(&mut b).ok()?;
            if let Some(dec) = &mut self.dec {
                dec.apply(&mut b, false);
            }
            value |= ((b[0] & 0x7F) as i32) << shift;
            shift += 7;
            if b[0] & 0x80 == 0 {
                return Some(value);
            }
            if shift > 35 {
                return None;
            }
        }
    }
}

// ===================== Protokolltabelle (Testkopie aus proto.rs) =====================

/// Feldreihenfolge im Team-Paket – die einzige Stelle, die sich zwischen den vier Versionen
/// dreimal ändert (siehe `proto::TeamLayout`).
#[derive(Clone, Copy, PartialEq, Eq)]
pub enum TeamLayout {
    /// 1.21.1: Flags, Sichtbarkeit und Kollision als **Zeichenketten**, dann Farbe, Präfix, Suffix.
    Legacy,
    /// 1.21.11 und 26.1: wie Legacy, aber Sichtbarkeit und Kollision als VarInt.
    VarIntRules,
    /// 26.2: Präfix und Suffix direkt hinter dem Anzeigenamen, Farbe optional, Flags zuletzt.
    Reordered,
}

pub struct Ids {
    pub name: &'static str,
    pub modern: bool,
    pub fluid_count: bool,
    pub team_layout: TeamLayout,
    pub cb_set_objective: i32,
    pub cb_set_display_objective: i32,
    pub cb_set_player_team: i32,
    pub cb_set_score: i32,
    pub cb_open_screen: i32,
    pub cb_container_set_content: i32,
    /// Netz-ID der Komponente `minecraft:custom_name`.
    pub component_custom_name: i32,
    /// Netz-ID der Komponente `minecraft:lore`.
    pub component_lore: i32,
    pub cb_login: i32,
    pub cb_position: i32,
    pub cb_system_chat: i32,
    pub cb_player_chat: i32,
    pub cb_keep_alive: i32,
    pub cb_level_chunk: i32,
    pub cb_chunk_batch_finished: i32,
    pub cb_block_update: i32,
    pub sb_chat_command: i32,
    pub sb_chat: i32,
    pub sb_chunk_batch_received: i32,
    pub sb_accept_teleportation: i32,
    pub sb_keep_alive: i32,
    pub sb_move: i32,
    pub sb_use_item: i32,
    pub cb_set_health: i32,
    pub cb_transfer: i32,
    pub cb_store_cookie: i32,
    pub sb_client_command: i32,
}

pub static MC_26_1: Ids = Ids {
    name: "26.1",
    modern: true,
    fluid_count: true,
    team_layout: TeamLayout::VarIntRules,
    cb_set_objective: 106,
    cb_set_display_objective: 98,
    cb_set_player_team: 109,
    cb_set_score: 110,
    cb_open_screen: 59,
    cb_container_set_content: 18,
    component_custom_name: 6,
    component_lore: 11,
    cb_login: 49,
    cb_position: 72,
    cb_system_chat: 121,
    cb_player_chat: 65,
    cb_keep_alive: 44,
    cb_level_chunk: 45,
    cb_chunk_batch_finished: 11,
    cb_block_update: 8,
    sb_chat_command: 7,
    sb_chat: 9,
    sb_chunk_batch_received: 11,
    sb_accept_teleportation: 0,
    sb_keep_alive: 28,
    sb_move: 31,
    sb_use_item: 67,
    cb_set_health: 104,
    cb_transfer: 129,
    cb_store_cookie: 120,
    sb_client_command: 12,
};

pub static MC_1_21_1: Ids = Ids {
    name: "1.21.1",
    modern: false,
    fluid_count: false,
    team_layout: TeamLayout::Legacy,
    cb_set_objective: 94,
    cb_set_display_objective: 87,
    cb_set_player_team: 96,
    cb_set_score: 97,
    cb_open_screen: 51,
    cb_container_set_content: 19,
    component_custom_name: 5,
    component_lore: 7,
    cb_login: 43,
    cb_position: 64,
    cb_system_chat: 108,
    cb_player_chat: 57,
    cb_keep_alive: 38,
    cb_level_chunk: 39,
    cb_chunk_batch_finished: 12,
    cb_block_update: 9,
    sb_chat_command: 4,
    sb_chat: 6,
    sb_chunk_batch_received: 8,
    sb_accept_teleportation: 0,
    sb_keep_alive: 24,
    sb_move: 27,
    sb_use_item: 57,
    cb_set_health: 93,
    cb_transfer: 115,
    cb_store_cookie: 107,
    sb_client_command: 9,
};

/// 26.2 verschiebt keine der hier benutzten IDs gegenüber 26.1 – nur das Team-Paket ist
/// umgestellt (siehe [`TeamLayout::Reordered`]).
pub static MC_26_2: Ids = Ids {
    name: "26.2",
    team_layout: TeamLayout::Reordered,
    ..MC_26_1
};

// ===================== Server =====================

/// Was der Testserver an den Testfall zurückmeldet.
#[derive(Debug, Clone)]
pub enum Note {
    /// Paket-ID und Länge der Nutzdaten. Die Länge verrät, ob ein Paket den Aufbau der jeweiligen
    /// Protokollversion hat – ein Feld zu viel oder zu wenig fällt genau dort auf.
    Packet(i32, usize),
    Command(String),
    Chat(String),
    /// Sichtweite aus `ClientInformation` – daran hängt, wie viele Chunkdaten der Server schickt.
    ViewDistance(u8),
    /// Antwort auf eine Cookie-Abfrage: Name und Inhalt (`None` = „habe ich nicht").
    Cookie(String, Option<Vec<u8>>),
    Joined,
    Closed,
}

pub struct Server {
    pub port: u16,
    pub notes: Receiver<Note>,
}

#[derive(Clone, Default)]
pub struct Plan {
    /// Chunks (x, z) mit einer soliden Schicht in Abschnitt `solid_section`.
    pub chunks: Vec<(i32, i32)>,
    pub solid_section: usize,
    /// Statt der Singleton-Palette eine gemischte Palette schicken, wie sie auf einer normal
    /// erzeugten Welt entsteht – mit einem Blockzähler, der weniger Luft meldet, als der
    /// häufigste Nicht-Luft-Zustand Blöcke hat. Genau daran ist der Client früher abgestürzt.
    pub mixed_palette: bool,
    /// So viele Abschnitte ab `solid_section` abwärts sind gefüllt (0 = nur `solid_section`).
    /// Eine echte Überwelt hat rund acht davon – daran hängt der Speicherbedarf der Ansicht.
    pub filled_sections: usize,
    /// Startposition des Spielers.
    pub position: (f64, f64, f64),
    /// Vor dem gewöhnlichen Chat eine Systemmeldung mit unlesbarer Komponente schicken.
    ///
    /// So etwas kommt von Plugins, die ihre Komponenten selbst zusammenbauen. Der Client darf
    /// die Zeile verwerfen – aber nicht die Verbindung.
    pub broken_chat: bool,
    /// Diese Chatzeilen werden nach dem Beitritt geschickt.
    pub chat: Vec<String>,
    /// Zeilen als **Spieler**-Chat (`ClientboundPlayerChatPacket`), Absender „Hugo".
    ///
    /// Ein ganz anderes Paket als der Systemchat: signierbar, mit Quittungsliste und
    /// Filterangabe. Genau in dieser Filterangabe steckte ein Feld, das der Client nicht gelesen
    /// hat – der Lesezeiger stand danach falsch und die Zeile fiel wortlos weg.
    pub player_chat: Vec<String>,
    /// Filterangabe des Servers: 0 = ungefiltert, 1 = ganz gefiltert, 2 = teilweise (dann folgt
    /// ein Bitfeld).
    pub player_chat_filter: i32,
    /// Sofort nach dem Chat auflegen – prüft, ob die letzten Zeilen es noch hinausschaffen.
    pub close_after_chat: bool,
    /// So viele zusätzliche Chatzeilen hinterher – genug, um eine ungelesene Pipe zu füllen.
    pub chat_flood: usize,
    /// Den Spieler nach dem Beitritt sterben lassen (Lebenspunkte 0).
    pub kill: bool,
    /// Eine Seitenleiste aufbauen: Ziel, Anzeigebereich, Team mit Präfix/Suffix und eine Punktzahl.
    pub scoreboard: bool,
    /// Ein Menü öffnen und seinen Inhalt schicken (ein benannter Gegenstand mit Lore).
    pub menu: bool,
    /// Vor dem Transfer dieses Cookie beim Client ablegen.
    pub store_cookie: Option<(String, Vec<u8>)>,
    /// Beim Login dieses Cookie abfragen und die Antwort als [`Note::Cookie`] melden.
    pub request_cookie: Option<String>,
    /// Nach dem Beitritt einen Server-Transfer auf diese Adresse anordnen.
    pub transfer_to: Option<(String, u16)>,
    /// Sekunden, die der Server danach noch offen bleibt.
    pub hold_secs: u64,
    /// Kompression mit dieser Schwelle einschalten (wie fast jeder echte Server).
    pub compression: Option<i32>,
    /// Verschlüsseltes Login wie ein Online-Mode-Server – nur ohne Sitzungsprüfung bei Mojang
    /// (`shouldAuthenticate = false`, dieselbe Angabe, mit der ein Proxy sie übernimmt).
    pub encrypt: bool,
}

/// Startet den Testserver auf einem freien Port.
pub fn start(ids: &'static Ids, plan: Plan) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx): (Notes, Receiver<Note>) = channel();
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let _ = stream.set_nodelay(true);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
            let out = Arc::new(Mutex::new(Wire {
                stream: stream.try_clone().unwrap(),
                threshold: -1,
                enc: None,
            }));
            let mut conn = Conn {
                stream,
                buffer: Vec::new(),
                threshold: -1,
                dec: None,
                out: Arc::clone(&out),
            };
            serve(&mut conn, out, ids, &plan, &tx);
            let _ = tx.send(Note::Closed);
        }
    });
    Server { port, notes: rx }
}

fn serve(conn: &mut Conn, out: Arc<Mutex<Wire>>, ids: &Ids, plan: &Plan, tx: &Notes) {
    // --- Handshake + Login (ohne Verschlüsselung, ohne Kompression) ---
    let Some((_, handshake)) = conn.recv() else {
        return;
    };
    let mut c = Cursor::new(&handshake);
    let _version = c.var_int();
    let _host = c.string();

    let Some((_, hello)) = conn.recv() else {
        return;
    };
    let mut c = Cursor::new(&hello);
    let name = c.string();

    // Verschlüsselung wie auf einem Online-Mode-Server. Reihenfolge wie in Vanilla:
    // EncryptionRequest, danach SetCompression, zuletzt LoginSuccess.
    if plan.encrypt && !encrypt(conn, &out) {
        return;
    }

    // Kompression einschalten, bevor irgendetwas anderes hinausgeht: Das Paket selbst geht noch
    // unkomprimiert, alles danach in beiden Richtungen im komprimierten Rahmen.
    if let Some(threshold) = plan.compression {
        let mut set = Buf::packet(3); // Login CB_COMPRESSION
        set.var_int(threshold);
        conn.send(&set);
        conn.threshold = threshold;
        out.lock().unwrap().threshold = threshold;
    }

    // Cookie-Abfrage noch in der Login-Phase – genau dort holt ein Transferziel das ab, was der
    // vorige Server hinterlegt hat.
    if let Some(key) = &plan.request_cookie {
        let mut ask = Buf::packet(5); // Login CB_COOKIE_REQUEST
        ask.string(key);
        conn.send(&ask);
        let Some((_, answer)) = conn.recv() else {
            return;
        };
        let mut c = Cursor::new(&answer);
        let key = c.string();
        let value = if c.bool() { Some(c.byte_array()) } else { None };
        let _ = tx.send(Note::Cookie(key, value));
    }

    let mut ok = Buf::packet(2); // CB_FINISHED
    ok.uuid(&[7u8; 16]).string(&name).var_int(0).bool(true);
    conn.send(&ok);
    if conn.recv().is_none() {
        return; // SB_ACKNOWLEDGED
    }

    // --- Konfigurationsphase ---
    let mut registry = Buf::packet(7); // CB_REGISTRY_DATA
    registry.string("minecraft:dimension_type").var_int(1);
    registry.string("minecraft:overworld").bool(true);
    registry.nbt_ints(&[("min_y", -64), ("height", 384)]);
    conn.send(&registry);
    conn.send(&Buf::packet(3)); // CB_FINISH

    loop {
        let Some((id, payload)) = conn.recv() else {
            return;
        };
        if id == 0 && payload.len() > 1 {
            // ClientInformation: Sprache (String), dann die Sichtweite als ein Byte.
            let mut c = Cursor::new(&payload);
            let _locale = c.string();
            let _ = tx.send(Note::ViewDistance(c.u8()));
        }
        if id == 3 {
            break; // SB_FINISH -> Spielphase
        }
    }

    // --- Spielphase ---
    let mut login = Buf::packet(ids.cb_login);
    login
        .i32(42) // eigene Entity-Nummer
        .bool(false) // hardcore
        .var_int(1)
        .string("minecraft:overworld") // Weltliste
        .var_int(20) // maxPlayers
        .var_int(8) // viewDistance
        .var_int(8) // simulationDistance
        .bool(false)
        .bool(true)
        .bool(false)
        .var_int(0) // Dimension-Typ-ID
        .string("minecraft:overworld")
        .i64(0) // hashedSeed
        .u8(0) // gameMode
        .u8(255) // previousGameMode
        .bool(false)
        .bool(false)
        .bool(false) // lastDeathPos: nein
        .var_int(0) // portalCooldown
        .var_int(63) // seaLevel
        .bool(false);
    conn.send(&login);
    let _ = tx.send(Note::Joined);

    // Position
    let (x, y, z) = plan.position;
    let mut pos = Buf::packet(ids.cb_position);
    if ids.modern {
        pos.var_int(1)
            .f64(x)
            .f64(y)
            .f64(z)
            .f64(0.0)
            .f64(0.0)
            .f64(0.0)
            .f32(0.0)
            .f32(0.0)
            .i32(0);
    } else {
        pos.f64(x).f64(y).f64(z).f32(0.0).f32(0.0).u8(0).var_int(1);
    }
    conn.send(&pos);

    // Chunks
    for (cx, cz) in &plan.chunks {
        conn.send(&chunk_packet(
            ids,
            *cx,
            *cz,
            plan.solid_section,
            plan.mixed_palette,
            plan.filled_sections.max(1),
        ));
    }
    if !plan.chunks.is_empty() {
        let mut done = Buf::packet(ids.cb_chunk_batch_finished);
        done.var_int(plan.chunks.len() as i32);
        conn.send(&done);
    }

    if plan.broken_chat {
        let mut chat = Buf::packet(ids.cb_system_chat);
        chat.u8(99).raw(b"kein NBT").bool(false); // 99 ist kein NBT-Typ
        conn.send(&chat);
    }

    for line in &plan.chat {
        let mut chat = Buf::packet(ids.cb_system_chat);
        chat.nbt_text(line).bool(false);
        conn.send(&chat);
    }
    for line in &plan.player_chat {
        conn.send(&player_chat_packet(
            ids,
            "Hugo",
            line,
            plan.player_chat_filter,
        ));
    }

    // Auflegen, sobald der Chat draußen ist: Der Client muss die Zeilen dann trotzdem noch
    // vollständig ausgeben, bevor er sich beendet.
    //
    // Nur die **Schreibrichtung** schließen und danach weiter mitlesen. Ein hartes Schließen mit
    // noch ungelesenen Daten im eigenen Empfangspuffer schickt ein RST – und ein RST wirft dem
    // Client seinen Empfangspuffer weg, mitsamt der Chatzeilen, die noch darin stehen. Der Test
    // prüfte dann nicht mehr den Client, sondern das Verhalten des Netzstapels.
    if plan.close_after_chat {
        let _ = out
            .lock()
            .unwrap()
            .stream
            .shutdown(std::net::Shutdown::Write);
        while conn.recv().is_some() {}
        return;
    }

    // Eine Flut, die keiner abholt: So läuft die Standardausgabe des Clients voll. Schreibt er
    // sie im Netz-Thread, bleibt er darin stecken und beantwortet kein KeepAlive mehr.
    for index in 0..plan.chat_flood {
        let mut chat = Buf::packet(ids.cb_system_chat);
        chat.nbt_text(&format!(
            "Flut {} – eine lange Zeile, damit die Pipe des Clients schnell voll ist ........",
            index
        ))
        .bool(false);
        conn.send(&chat);
    }

    if plan.scoreboard {
        send_scoreboard(conn, ids);
    }
    if plan.menu {
        send_menu(conn, ids);
    }

    // Tod: Lebenspunkte 0, Nahrung und Sättigung dahinter.
    if plan.kill {
        let mut health = Buf::packet(ids.cb_set_health);
        health.f32(0.0).var_int(20).f32(5.0);
        conn.send(&health);
    }

    if let Some((key, value)) = &plan.store_cookie {
        let mut cookie = Buf::packet(ids.cb_store_cookie);
        cookie.string(key).byte_array(value);
        conn.send(&cookie);
    }

    if let Some((host, port)) = &plan.transfer_to {
        let mut transfer = Buf::packet(ids.cb_transfer);
        transfer.string(host).var_int(*port as i32);
        conn.send(&transfer);
    }

    // Ab hier nur noch mitlesen und am Leben halten.
    let deadline = Instant::now() + Duration::from_secs(plan.hold_secs.max(1));
    let keep_alive_id = ids.cb_keep_alive;
    let wire = Arc::clone(&out);
    thread::spawn(move || {
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(500));
            let mut ka = Buf::packet(keep_alive_id);
            ka.i64(1234);
            let mut guard = wire.lock().unwrap();
            guard.send(&ka);
            if guard.stream.flush().is_err() {
                break;
            }
        }
        let _ = out
            .lock()
            .unwrap()
            .stream
            .shutdown(std::net::Shutdown::Both);
    });

    // Der Transfer beendet die Verbindung von unserer Seite – der Client baut dann eine neue auf.
    if plan.transfer_to.is_some() {
        thread::sleep(Duration::from_millis(200));
        return;
    }

    let chat_command = ids.sb_chat_command;
    let chat = ids.sb_chat;
    while let Some((id, payload)) = conn.recv() {
        if id == chat_command {
            let mut c = Cursor::new(&payload);
            let _ = tx.send(Note::Command(c.string()));
        } else if id == chat {
            let mut c = Cursor::new(&payload);
            let _ = tx.send(Note::Chat(c.string()));
        }
        // Zusätzlich immer die rohe Länge: Sie verrät, ob ein Paket den Aufbau der jeweiligen
        // Protokollversion hat – ein Feld zu viel oder zu wenig fällt genau daran auf.
        let _ = tx.send(Note::Packet(id, payload.len()));
    }
}

/// Das Verschlüsselungs-Handshake eines Online-Mode-Servers.
///
/// Der Server schickt seinen öffentlichen Schlüssel und eine Prüffolge; der Client verschlüsselt
/// beides mit RSA und antwortet. Danach läuft die ganze Verbindung durch AES-128-CFB8. Ohne
/// `shouldAuthenticate` bleibt die Sitzungsprüfung bei Mojang außen vor – genau so, wie es auch
/// ein Proxy macht, der sie selbst übernommen hat.
///
/// `false` = das Handshake ist gescheitert.
fn encrypt(conn: &mut Conn, out: &Arc<Mutex<Wire>>) -> bool {
    // 1024 Bit wie Minecraft selbst.
    let private = RsaPrivateKey::new(&mut rand::rngs::OsRng, 1024).expect("RSA-Schlüssel");
    let der = RsaPublicKey::from(&private)
        .to_public_key_der()
        .expect("DER");
    let challenge = [0x11u8, 0x22, 0x33, 0x44];

    let mut request = Buf::packet(1); // Login CB_HELLO
    request
        .string("")
        .byte_array(der.as_bytes())
        .byte_array(&challenge)
        .bool(false); // shouldAuthenticate
    conn.send(&request);

    let Some((id, answer)) = conn.recv() else {
        return false;
    };
    assert_eq!(id, 1, "der Client antwortet nicht mit SB_KEY");
    let mut c = Cursor::new(&answer);
    let secret = private
        .decrypt(Pkcs1v15Encrypt, &c.byte_array())
        .expect("Geheimnis entschlüsselbar");
    let echo = private
        .decrypt(Pkcs1v15Encrypt, &c.byte_array())
        .expect("Prüffolge entschlüsselbar");
    assert_eq!(echo, challenge, "der Client hat die Prüffolge verändert");
    let secret: [u8; 16] = secret.try_into().expect("16 Byte Schlüssel");

    // Ab jetzt beide Richtungen verschlüsselt – der Client hat unmittelbar nach SB_KEY
    // umgeschaltet.
    conn.dec = Some(Cfb8::new(&secret));
    out.lock().unwrap().enc = Some(Cfb8::new(&secret));
    true
}

/// Ein `ClientboundPlayerChatPacket` – der Weg, auf dem echter Spielerchat kommt.
///
/// `filter` ist die Filterangabe: 0 = ungefiltert, 1 = ganz gefiltert, 2 = teilweise. Nur bei 2
/// folgt ein Bitfeld, und genau das hat der Client übersehen.
fn player_chat_packet(ids: &Ids, sender: &str, content: &str, filter: i32) -> Buf {
    let mut p = Buf::packet(ids.cb_player_chat);
    if ids.modern {
        p.var_int(1); // globalIndex (erst ab 1.21.11)
    }
    p.uuid(&[3u8; 16]) // Absender
        .var_int(0) // index
        .bool(false) // keine Signatur
        .string(content)
        .i64(1) // timestamp
        .i64(2) // salt
        .var_int(0) // keine zuletzt gesehenen Nachrichten
        .bool(false) // kein abweichender unsignierter Inhalt
        .var_int(filter);
    if filter == 2 {
        p.var_int(1).i64(0); // Bitfeld: ein Long
    }
    p.var_int(1); // chatType: Registry-Nummer 0 (+1)
    p.nbt_text(sender); // Anzeigename
    p.bool(false); // kein targetName
    p
}

/// Eine Seitenleiste, wie sie ein gewöhnlicher Server aufbaut: Der Eintrag selbst ist ein
/// unsichtbarer Platzhalter, der sichtbare Text steckt in Präfix und Suffix eines Teams.
///
/// Genau das ist die Stelle, an der sich die vier Protokolle dreimal unterscheiden – deshalb wird
/// hier je nach [`TeamLayout`] eine andere Feldreihenfolge geschrieben.
fn send_scoreboard(conn: &mut Conn, ids: &Ids) {
    // Ziel anlegen: Name, Aktion 0 (anlegen), Anzeigename, Punktart, kein Zahlenformat.
    let mut objective = Buf::packet(ids.cb_set_objective);
    objective
        .string("sb")
        .u8(0)
        .nbt_text("Testserver")
        .var_int(0)
        .bool(false);
    conn.send(&objective);

    // In die Seitenleiste damit (Bereich 1).
    let mut display = Buf::packet(ids.cb_set_display_objective);
    display.var_int(1).string("sb");
    conn.send(&display);

    // Team anlegen – Präfix und Suffix tragen den sichtbaren Text.
    let mut team = Buf::packet(ids.cb_set_player_team);
    team.string("t1").u8(0).nbt_text("Team 1");
    match ids.team_layout {
        TeamLayout::Legacy => {
            team.u8(0) // Flags
                .string("always") // Sichtbarkeit der Namensschilder
                .string("always") // Kollisionsregel
                .var_int(10) // Farbe: green
                .nbt_text("Rang: ")
                .nbt_text(" *");
        }
        TeamLayout::VarIntRules => {
            team.u8(0)
                .var_int(0)
                .var_int(0)
                .var_int(10)
                .nbt_text("Rang: ")
                .nbt_text(" *");
        }
        TeamLayout::Reordered => {
            team.nbt_text("Rang: ")
                .nbt_text(" *")
                .var_int(0)
                .var_int(0)
                .bool(true)
                .var_int(10)
                .u8(0);
        }
    }
    team.var_int(1).string("hugo"); // ein Mitglied
    conn.send(&team);

    // Punktzahl: Eintrag, Ziel, Wert, kein eigener Anzeigetext, kein Zahlenformat.
    let mut score = Buf::packet(ids.cb_set_score);
    score
        .string("hugo")
        .string("sb")
        .var_int(5)
        .bool(false)
        .bool(false);
    conn.send(&score);
}

/// Ein geöffnetes Menü samt Inhalt: neun Felder, davon eines mit Anzeigename und zwei
/// Lore-Zeilen. Genau so kommt ein Shop- oder Warp-Menü von einem gewöhnlichen Server.
fn send_menu(conn: &mut Conn, ids: &Ids) {
    let mut open = Buf::packet(ids.cb_open_screen);
    open.var_int(1).var_int(2).nbt_text("Warp-Menü");
    conn.send(&open);

    let mut content = Buf::packet(ids.cb_container_set_content);
    // Die Fenster-Nummer steht in 1.21.1 als vorzeichenloses Byte, ab 1.21.11 als VarInt.
    if ids.modern {
        content.var_int(1);
    } else {
        content.u8(1);
    }
    content.var_int(1).var_int(9); // Zustandszähler, Feldanzahl
    for slot in 0..9 {
        if slot == 4 {
            item_with_lore(
                &mut content,
                ids,
                848,
                "Zum Spawn",
                &["Klicken", "kostet nichts"],
            );
        } else {
            content.var_int(0); // leeres Feld
        }
    }
    content.var_int(0); // nichts in der Hand
    conn.send(&content);
}

/// Ein Gegenstand mit Anzeigename und Lore – die beiden Komponenten, die der Client wirklich liest.
fn item_with_lore(buf: &mut Buf, ids: &Ids, id: i32, name: &str, lore: &[&str]) {
    buf.var_int(1) // Anzahl
        .var_int(id)
        .var_int(2) // zwei hinzugefügte Komponenten
        .var_int(0); // keine entfernten
    buf.var_int(ids.component_custom_name);
    buf.nbt_text(name);
    buf.var_int(ids.component_lore);
    buf.var_int(lore.len() as i32);
    for line in lore {
        buf.nbt_text(line);
    }
}

/// Ein Chunk mit 24 Abschnitten: `solid_section` ist gefüllt, der Rest Luft.
pub fn chunk_packet(
    ids: &Ids,
    x: i32,
    z: i32,
    solid_section: usize,
    mixed: bool,
    filled: usize,
) -> Buf {
    let mut data = Buf::default();
    for section in 0..24usize {
        let solid = section >= solid_section.saturating_sub(filled.saturating_sub(1))
            && section <= solid_section;
        if solid && mixed {
            write_mixed_section(&mut data, ids);
        } else {
            data.i16(if solid { 4096 } else { 0 });
            if ids.fluid_count {
                data.i16(0);
            }
            // Blockpalette: Singleton
            data.u8(0).var_int(if solid { 1 } else { 0 });
            if !ids.modern {
                data.var_int(0);
            }
            // Biompalette: Singleton
            data.u8(0).var_int(0);
            if !ids.modern {
                data.var_int(0);
            }
        }
    }

    let mut packet = Buf::packet(ids.cb_level_chunk);
    packet.i32(x).i32(z);
    if ids.modern {
        packet.var_int(0); // keine Höhenkarten
    } else {
        packet.nbt_ints(&[]); // leeres NBT-Compound
    }
    packet.byte_array(&data.data);
    packet.var_int(0); // keine Blockentitäten
    packet
}

/// Ein Abschnitt, wie ihn eine normal erzeugte Welt liefert: mehrere Zustände in einer
/// indirekten Palette, dazu ein Blockzähler, der genau 90 Luftblöcke übrig lässt, während der
/// erste Paletteneintrag 394 Blöcke belegt. Diese Kombination hat den Client abgeschossen
/// (`index out of bounds: the len is 91 but the index is 394`).
fn write_mixed_section(data: &mut Buf, ids: &Ids) {
    const AIR: usize = 90;
    // Bewusst OHNE Zustand 0: unter Tage ist die Luft `cave_air`, und genau dann muss der
    // Client die Luftzustände über den Blockzähler suchen.
    let palette = [7u32, 42, 1, 5000];
    let mut states = vec![2u16; 4096]; // Zustand 1 (Stein)
    states[..394].fill(0); // 394x Zustand 7 – mehr Blöcke, als der Abschnitt Luft hat
    states[394..406].fill(1); // 12x Zustand 42
    states[4096 - AIR..].fill(3); // 90x cave_air

    data.i16((4096 - AIR) as i16);
    if ids.fluid_count {
        data.i16(0);
    }
    let bits = 4u8;
    data.u8(bits).var_int(palette.len() as i32);
    for value in palette {
        data.var_int(value as i32);
    }
    let per_long = 64 / bits as usize;
    let longs = 4096usize.div_ceil(per_long);
    if !ids.modern {
        data.var_int(longs as i32);
    }
    let mut packed = vec![0u64; longs];
    for (index, slot) in states.iter().enumerate() {
        packed[index / per_long] |= (*slot as u64) << ((index % per_long) * bits as usize);
    }
    for long in packed {
        data.i64(long as i64);
    }
    // Biompalette: Singleton
    data.u8(0).var_int(0);
    if !ids.modern {
        data.var_int(0);
    }
}

// ===================== Proxys =====================
//
// Zwei winzige Proxys, damit der `--proxy`-Weg des Clients an einem echten Socket läuft und nicht
// nur im Kopf. Beide nehmen genau eine Verbindung an, führen ihr Handshake durch und reichen
// danach nur noch Bytes durch – ab dann ist es eine ganz gewöhnliche TCP-Verbindung.

/// SOCKS5 nach RFC 1928 (Anmeldung nach RFC 1929, falls `login` gesetzt ist).
pub fn start_socks5(target: u16, login: Option<(String, String)>) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let Ok((mut client, _)) = listener.accept() else {
            return;
        };
        // Begrüßung: Version, Zahl der angebotenen Verfahren, die Verfahren selbst.
        let mut head = [0u8; 2];
        if client.read_exact(&mut head).is_err() || head[0] != 0x05 {
            return;
        }
        let mut methods = vec![0u8; head[1] as usize];
        if client.read_exact(&mut methods).is_err() {
            return;
        }
        let want = if login.is_some() { 0x02 } else { 0x00 };
        if !methods.contains(&want) {
            let _ = client.write_all(&[0x05, 0xFF]);
            return;
        }
        if client.write_all(&[0x05, want]).is_err() {
            return;
        }

        if let Some((user, pass)) = &login {
            let mut version = [0u8; 2];
            if client.read_exact(&mut version).is_err() || version[0] != 0x01 {
                return;
            }
            let mut name = vec![0u8; version[1] as usize];
            if client.read_exact(&mut name).is_err() {
                return;
            }
            let mut length = [0u8; 1];
            if client.read_exact(&mut length).is_err() {
                return;
            }
            let mut password = vec![0u8; length[0] as usize];
            if client.read_exact(&mut password).is_err() {
                return;
            }
            let ok = name == user.as_bytes() && password == pass.as_bytes();
            let _ = client.write_all(&[0x01, u8::from(!ok)]);
            if !ok {
                return;
            }
        }

        // CONNECT: Version, Befehl, reserviert, Adresstyp – danach die Adresse und der Port.
        let mut request = [0u8; 4];
        if client.read_exact(&mut request).is_err() || request[1] != 0x01 {
            return;
        }
        let skip = match request[3] {
            0x01 => 4,
            0x04 => 16,
            0x03 => {
                let mut length = [0u8; 1];
                if client.read_exact(&mut length).is_err() {
                    return;
                }
                length[0] as usize
            }
            _ => return,
        };
        let mut address = vec![0u8; skip + 2]; // Adresse + Port
        if client.read_exact(&mut address).is_err() {
            return;
        }
        // Erfolg, gebundene Adresse 0.0.0.0:0 (der Client wirft sie ohnehin weg).
        if client
            .write_all(&[0x05, 0x00, 0x00, 0x01, 0, 0, 0, 0, 0, 0])
            .is_err()
        {
            return;
        }

        let Ok(server) = TcpStream::connect(("127.0.0.1", target)) else {
            return;
        };
        pump(client, server);
    });
    port
}

/// HTTP-CONNECT: Kopf lesen, `200` antworten, danach durchreichen.
pub fn start_http_proxy(target: u16) -> u16 {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    thread::spawn(move || {
        let Ok((client, _)) = listener.accept() else {
            return;
        };
        let mut reader = std::io::BufReader::new(client.try_clone().unwrap());
        let mut line = String::new();
        if reader.read_line(&mut line).is_err() || !line.starts_with("CONNECT ") {
            return;
        }
        // Kopfzeilen bis zur Leerzeile.
        loop {
            line.clear();
            match reader.read_line(&mut line) {
                Ok(0) => return,
                Ok(_) if line.trim_end_matches(['\r', '\n']).is_empty() => break,
                Ok(_) => {}
                Err(_) => return,
            }
        }
        let mut client = client;
        if client
            .write_all(b"HTTP/1.1 200 Connection established\r\nProxy-Agent: Test\r\n\r\n")
            .is_err()
        {
            return;
        }
        let Ok(server) = TcpStream::connect(("127.0.0.1", target)) else {
            return;
        };
        pump(client, server);
    });
    port
}

/// Beide Richtungen durchreichen, bis eine Seite auflegt.
fn pump(a: TcpStream, b: TcpStream) {
    let back = (a.try_clone().unwrap(), b.try_clone().unwrap());
    for (mut from, mut to) in [(a, back.1), (b, back.0)] {
        thread::spawn(move || {
            let mut buffer = [0u8; 8192];
            loop {
                match from.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => {
                        if to.write_all(&buffer[..n]).is_err() {
                            break;
                        }
                    }
                }
            }
            let _ = to.shutdown(std::net::Shutdown::Both);
        });
    }
}

// ===================== Client starten =====================

/// Der gestartete Client. Bricht ein Test ab, muss der Prozess trotzdem weg – sonst hält er
/// die geerbten Ausgabekanäle offen und die ganze Testsitzung hängt.
///
/// Dazu gehört auch sein Konfigurationsverzeichnis: Es liegt je Client im Temp-Verzeichnis und
/// wird hier wieder abgeräumt.
pub struct Client(std::process::Child, std::path::PathBuf);

impl std::ops::Deref for Client {
    type Target = std::process::Child;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl std::ops::DerefMut for Client {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.0
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
        let _ = std::fs::remove_dir_all(&self.1);
    }
}

/// Startet das gebaute Binary gegen den Testserver. Mit `AFK_TEST_BIN` lässt sich stattdessen
/// eine fertige Datei prüfen – praktisch, um eine ältere Fassung gegen die neue zu messen.
pub fn spawn_client(port: u16, mc: &str, extra: &[&str]) -> Client {
    spawn_client_at("127.0.0.1", port, mc, extra)
}

/// Wie [`spawn_client`], aber mit frei gewählter Zieladresse – für den Weg über einen Proxy,
/// wo der Client den Namen dem Proxy überlässt.
pub fn spawn_client_at(host: &str, port: u16, mc: &str, extra: &[&str]) -> Client {
    let binary =
        std::env::var("AFK_TEST_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_afk").to_string());

    // Eigenes Konfigurationsverzeichnis je Client.
    //
    // Ohne das schriebe ein Test mit `:home set` in die echte `movement.json` des Entwicklers
    // (und läse beim nächsten Lauf, was dort steht). Ein Test darf weder etwas hinterlassen noch
    // davon abhängen, was er vorfindet. `XDG_CONFIG_HOME` ist genau der Schalter, den auch der
    // Client zuerst befragt.
    static NEXT: std::sync::atomic::AtomicU32 = std::sync::atomic::AtomicU32::new(0);
    let config = std::env::temp_dir().join(format!(
        "afk-test-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed)
    ));
    let _ = std::fs::create_dir_all(&config);

    let mut command = std::process::Command::new(binary);
    command
        .arg(format!("{}:{}", host, port))
        .arg("--mc")
        .arg(mc)
        .arg("--offline")
        .arg("Testkonto")
        .args(extra)
        .env("XDG_CONFIG_HOME", &config)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    Client(command.spawn().expect("Client startet"), config)
}

/// Liest einen Ausgabestrom stückweise in einen Kanal – in einem eigenen Thread.
///
/// Ein angebrochenes Zeichen wird dabei aufgehoben und vorn an den nächsten Block gehängt.
/// Ohne das war der Leser selbst die Fehlerquelle: das Halbblockzeichen `▀` der Live-Ansicht ist
/// **drei Bytes** lang, und ein Lesevorgang liefert nur, was gerade in der Pipe steht. Endete er
/// mitten im Zeichen, machte `from_utf8_lossy` daraus Ersatzzeichen – im Bildformat-Test fehlten
/// dann Zellen (`left: 34, right: 40`), obwohl der Client ein völlig korrektes Bild geschickt
/// hatte. Genau daran ist der Build auf `main` gescheitert.
pub fn collect<R: Read + Send + 'static>(mut reader: R) -> Receiver<String> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        let mut rest: Vec<u8> = Vec::new();
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => {
                    if !rest.is_empty() {
                        let _ = tx.send(String::from_utf8_lossy(&rest).into_owned());
                    }
                    break;
                }
                Ok(n) => {
                    rest.extend_from_slice(&buffer[..n]);
                    let good = match std::str::from_utf8(&rest) {
                        Ok(_) => rest.len(),
                        Err(e) => match e.error_len() {
                            // Wirklich ungültige Bytes: mitnehmen, sonst käme der Puffer nie leer.
                            Some(bad) => e.valid_up_to() + bad,
                            // Nur abgeschnitten: den Anfang fürs nächste Mal aufheben.
                            None => e.valid_up_to(),
                        },
                    };
                    let text = String::from_utf8_lossy(&rest[..good]).into_owned();
                    rest.drain(..good);
                    if tx.send(text).is_err() {
                        break;
                    }
                }
            }
        }
    });
    rx
}

/// Wartet, bis `needle` in der gesammelten Ausgabe auftaucht (oder die Zeit abläuft).
pub fn wait_for(rx: &Receiver<String>, timeout: Duration, needle: &str) -> (bool, String) {
    let deadline = Instant::now() + timeout;
    let mut out = String::new();
    loop {
        if out.contains(needle) {
            return (true, out);
        }
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            return (out.contains(needle), out);
        };
        match rx.recv_timeout(left.min(Duration::from_millis(200))) {
            Ok(chunk) => out.push_str(&chunk),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return (out.contains(needle), out),
        }
    }
}

/// Wartet auf ein **vollständiges** Bild: eine Kopfzeile mit `header` und dahinter mindestens
/// `count` lückenlos folgende Zeilen, die `check` erfüllen.
///
/// Warum so umständlich und nicht einfach [`wait_for`]:
///
/// * `wait_for` kehrt zurück, sobald sein Suchtext auftaucht, und **verwirft dabei alles, was im
///   selben Block noch dahinter stand**. Die POV-Bauformen starten die Ansicht schon beim
///   Beitritt – wartet ein Test also erst auf „im Spiel", ist der Anfang des ersten Bildes
///   bereits weg, und die erste sichtbare Zeile ist in Wahrheit das *Ende* einer Bildzeile.
/// * Ein Lesevorgang liefert nur, was gerade in der Pipe steht. Die letzte Zeile im Puffer ist
///   deshalb oft erst halb da.
///
/// Beides ergab zu wenige Zellen (`left: 34, right: 40`) und hat den Build auf `main` zum
/// Scheitern gebracht – am Test, nicht am Client. Deshalb wird hier an der Kopfzeile verankert
/// und es zählen nur Zeilen, hinter denen bereits ein Zeilenumbruch gelesen wurde.
pub fn wait_for_frame(
    rx: &Receiver<String>,
    timeout: Duration,
    count: usize,
    header: &str,
    check: impl Fn(&str) -> bool,
) -> (Vec<String>, String) {
    let deadline = Instant::now() + timeout;
    let mut out = String::new();
    loop {
        let rows = frame_rows(&out, count, header, &check);
        if rows.len() >= count {
            return (rows, out);
        }
        let Some(left) = deadline.checked_duration_since(Instant::now()) else {
            return (rows, out);
        };
        match rx.recv_timeout(left.min(Duration::from_millis(200))) {
            Ok(chunk) => out.push_str(&chunk),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
            Err(_) => return (rows, out),
        }
    }
}

/// Die Zeilen des ersten Bildes, das hinter einer Kopfzeile vollständig vorliegt.
fn frame_rows(
    text: &str,
    count: usize,
    header: &str,
    check: &impl Fn(&str) -> bool,
) -> Vec<String> {
    let lines: Vec<&str> = complete_lines(text).collect();
    for (at, line) in lines.iter().enumerate() {
        if !line.contains(header) {
            continue;
        }
        let rows: Vec<String> = lines[at + 1..]
            .iter()
            .take_while(|line| check(line))
            .map(|line| line.to_string())
            .collect();
        if rows.len() >= count {
            return rows;
        }
    }
    Vec::new()
}

/// Nur die Zeilen, hinter denen schon ein Zeilenumbruch steht – der Rest ist noch unterwegs.
fn complete_lines(text: &str) -> std::str::Lines<'_> {
    let end = text.rfind('\n').map_or(0, |at| at + 1);
    text[..end].lines()
}

/// Einen örtlichen Befehl so lange wiederholen, bis `needle` in der Ausgabe auftaucht.
///
/// `:pov info` beantwortet den **Stand von jetzt**. Wird der Befehl nur einmal geschickt und ist
/// der Client gerade noch beim Einlesen der Chunks, steht in der einen Antwort eine kleinere Zahl
/// – und der Test wartet danach vergeblich auf eine Zahl, die nie wieder kommt. Die Auskunft ist
/// billig und ohne Nebenwirkung, deshalb wird hier einfach nachgefragt, bis der Stand da ist.
pub fn poll_command(
    stdin: &mut impl Write,
    rx: &Receiver<String>,
    timeout: Duration,
    command: &str,
    needle: &str,
) -> (bool, String) {
    let deadline = Instant::now() + timeout;
    let mut out = String::new();
    loop {
        if out.contains(needle) {
            return (true, out);
        }
        if writeln!(stdin, "{}", command).is_err() || stdin.flush().is_err() {
            return (out.contains(needle), out);
        }
        // Eine Runde lang mitlesen, dann noch einmal fragen.
        let until = Instant::now() + Duration::from_millis(500);
        while let Some(left) = until.checked_duration_since(Instant::now()) {
            if deadline < Instant::now() {
                return (out.contains(needle), out);
            }
            match rx.recv_timeout(left) {
                Ok(chunk) => {
                    out.push_str(&chunk);
                    if out.contains(needle) {
                        return (true, out);
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => break,
                Err(_) => return (out.contains(needle), out),
            }
        }
        if deadline < Instant::now() {
            return (out.contains(needle), out);
        }
    }
}

/// Wartet auf eine Rückmeldung des Testservers, die `check` erfüllt.
pub fn wait_note(notes: &Receiver<Note>, timeout: Duration, check: impl Fn(&Note) -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while let Some(left) = deadline.checked_duration_since(Instant::now()) {
        match notes.recv_timeout(left) {
            Ok(note) => {
                if check(&note) {
                    return true;
                }
            }
            Err(_) => return false,
        }
    }
    false
}
