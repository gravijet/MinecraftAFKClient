//! Winziger Minecraft-Server für Integrationstests.
//!
//! Er spricht genau so viel Protokoll, wie der Client zum Beitreten braucht: Handshake,
//! Login ohne Verschlüsselung und ohne Kompression, Konfigurationsphase mit Registerdaten
//! und danach die Spielphase. Damit lässt sich der komplette Ablauf des echten Binaries
//! prüfen – einschließlich der Live-POV, die sonst nur an einem echten Server sichtbar wäre.
#![allow(dead_code)]

use std::io::{Read, Write};
use std::net::{TcpListener, TcpStream};
use std::sync::mpsc::{channel, Receiver, Sender};
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
        self.i16(text.len() as i16);
        self.raw(text.as_bytes());
        self.u8(0)
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
}

// ===================== Verbindung =====================

pub struct Conn {
    stream: TcpStream,
    buffer: Vec<u8>,
}

impl Conn {
    pub fn send(&mut self, packet: &Buf) {
        let mut framed = Buf::default();
        framed.var_int(packet.data.len() as i32);
        framed.raw(&packet.data);
        let _ = self.stream.write_all(&framed.data);
        let _ = self.stream.flush();
    }

    /// Nächstes Paket (id, Nutzdaten). `None` = Verbindung zu.
    pub fn recv(&mut self) -> Option<(i32, Vec<u8>)> {
        let len = self.read_var_int()?;
        if len <= 0 || len > 8 * 1024 * 1024 {
            return None;
        }
        self.buffer.resize(len as usize, 0);
        self.stream.read_exact(&mut self.buffer).ok()?;
        let mut c = Cursor::new(&self.buffer);
        let id = c.var_int();
        Some((id, self.buffer[c.pos..].to_vec()))
    }

    fn read_var_int(&mut self) -> Option<i32> {
        let mut value = 0i32;
        let mut shift = 0;
        loop {
            let mut b = [0u8; 1];
            self.stream.read_exact(&mut b).ok()?;
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

pub struct Ids {
    pub name: &'static str,
    pub modern: bool,
    pub fluid_count: bool,
    pub cb_login: i32,
    pub cb_position: i32,
    pub cb_system_chat: i32,
    pub cb_keep_alive: i32,
    pub cb_level_chunk: i32,
    pub cb_chunk_batch_finished: i32,
    pub cb_block_update: i32,
    pub sb_chat_command: i32,
    pub sb_chat: i32,
    pub sb_chunk_batch_received: i32,
    pub sb_accept_teleportation: i32,
    pub sb_move: i32,
}

pub static MC_26_1: Ids = Ids {
    name: "26.1",
    modern: true,
    fluid_count: true,
    cb_login: 49,
    cb_position: 72,
    cb_system_chat: 121,
    cb_keep_alive: 44,
    cb_level_chunk: 45,
    cb_chunk_batch_finished: 11,
    cb_block_update: 8,
    sb_chat_command: 7,
    sb_chat: 9,
    sb_chunk_batch_received: 11,
    sb_accept_teleportation: 0,
    sb_move: 31,
};

pub static MC_1_21_1: Ids = Ids {
    name: "1.21.1",
    modern: false,
    fluid_count: false,
    cb_login: 43,
    cb_position: 64,
    cb_system_chat: 108,
    cb_keep_alive: 38,
    cb_level_chunk: 39,
    cb_chunk_batch_finished: 12,
    cb_block_update: 9,
    sb_chat_command: 4,
    sb_chat: 6,
    sb_chunk_batch_received: 8,
    sb_accept_teleportation: 0,
    sb_move: 27,
};

// ===================== Server =====================

/// Was der Testserver an den Testfall zurückmeldet.
#[derive(Debug, Clone)]
pub enum Note {
    Packet(i32),
    Command(String),
    Chat(String),
    /// Sichtweite aus `ClientInformation` – daran hängt, wie viele Chunkdaten der Server schickt.
    ViewDistance(u8),
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
    /// Diese Chatzeilen werden nach dem Beitritt geschickt.
    pub chat: Vec<String>,
    /// Sekunden, die der Server danach noch offen bleibt.
    pub hold_secs: u64,
}

/// Startet den Testserver auf einem freien Port.
pub fn start(ids: &'static Ids, plan: Plan) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").expect("bind");
    let port = listener.local_addr().unwrap().port();
    let (tx, rx) = channel();
    thread::spawn(move || {
        if let Ok((stream, _)) = listener.accept() {
            let _ = stream.set_nodelay(true);
            let _ = stream.set_read_timeout(Some(Duration::from_secs(60)));
            let write = stream.try_clone().unwrap();
            let mut conn = Conn {
                stream,
                buffer: Vec::new(),
            };
            serve(&mut conn, write, ids, &plan, &tx);
            let _ = tx.send(Note::Closed);
        }
    });
    Server { port, notes: rx }
}

fn serve(conn: &mut Conn, write: TcpStream, ids: &Ids, plan: &Plan, tx: &Sender<Note>) {
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

    for line in &plan.chat {
        let mut chat = Buf::packet(ids.cb_system_chat);
        chat.nbt_text(line).bool(false);
        conn.send(&chat);
    }

    // Ab hier nur noch mitlesen und am Leben halten.
    let deadline = Instant::now() + Duration::from_secs(plan.hold_secs.max(1));
    let keep_alive_id = ids.cb_keep_alive;
    thread::spawn(move || {
        let mut write = write;
        while Instant::now() < deadline {
            thread::sleep(Duration::from_millis(500));
            let mut ka = Buf::packet(keep_alive_id);
            ka.i64(1234);
            let mut framed = Buf::default();
            framed.var_int(ka.data.len() as i32);
            framed.raw(&ka.data);
            if write.write_all(&framed.data).is_err() {
                break;
            }
        }
        let _ = write.shutdown(std::net::Shutdown::Both);
    });

    let chat_command = ids.sb_chat_command;
    let chat = ids.sb_chat;
    while let Some((id, payload)) = conn.recv() {
        if id == chat_command {
            let mut c = Cursor::new(&payload);
            let _ = tx.send(Note::Command(c.string()));
        } else if id == chat {
            let mut c = Cursor::new(&payload);
            let _ = tx.send(Note::Chat(c.string()));
        } else {
            let _ = tx.send(Note::Packet(id));
        }
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

// ===================== Client starten =====================

/// Der gestartete Client. Bricht ein Test ab, muss der Prozess trotzdem weg – sonst hält er
/// die geerbten Ausgabekanäle offen und die ganze Testsitzung hängt.
pub struct Client(std::process::Child);

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
    }
}

/// Startet das gebaute Binary gegen den Testserver. Mit `AFK_TEST_BIN` lässt sich stattdessen
/// eine fertige Datei prüfen – praktisch, um eine ältere Fassung gegen die neue zu messen.
pub fn spawn_client(port: u16, mc: &str, extra: &[&str]) -> Client {
    let binary =
        std::env::var("AFK_TEST_BIN").unwrap_or_else(|_| env!("CARGO_BIN_EXE_afk").to_string());
    let mut command = std::process::Command::new(binary);
    command
        .arg(format!("127.0.0.1:{}", port))
        .arg("--mc")
        .arg(mc)
        .arg("--offline")
        .arg("Testkonto")
        .args(extra)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped());
    Client(command.spawn().expect("Client startet"))
}

/// Liest einen Ausgabestrom stückweise in einen Kanal – in einem eigenen Thread.
pub fn collect<R: Read + Send + 'static>(mut reader: R) -> Receiver<String> {
    let (tx, rx) = channel();
    thread::spawn(move || {
        let mut buffer = [0u8; 8192];
        loop {
            match reader.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(n) => {
                    if tx
                        .send(String::from_utf8_lossy(&buffer[..n]).into_owned())
                        .is_err()
                    {
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
