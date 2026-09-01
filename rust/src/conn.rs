//! TCP-Verbindung: Rahmen (Länge+Paket), zlib-Kompression und AES-128-CFB8-Verschlüsselung.
//!
//! CFB8 ist hier von Hand implementiert (ein Byte pro Block) – das sind zwanzig Zeilen und
//! spart eine weitere Abhängigkeit. Die Rechenlast ist bei Chat-Verkehr vernachlässigbar.

use crate::buf::{err, push_var_int, Reader, Writer};
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::{Aes128, Block};
use flate2::{
    Compress, Compression, Decompress, FlushCompress, FlushDecompress, Status,
};
use std::io::{self, BufReader, Read, Write};
use std::net::{SocketAddr, TcpStream, ToSocketAddrs};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

/// Antwortet der Server so lange nicht, gilt die Verbindung als tot (Server sendet KeepAlive
/// im 15-Sekunden-Takt – 120 s Stille heißt: da kommt nichts mehr).
const READ_TIMEOUT: Duration = Duration::from_secs(120);
/// Nimmt die Gegenstelle nichts mehr an, darf das Senden nicht ewig blockieren: der Sender hält
/// dabei die Schreibsperre, an der auch der Netz-Thread hängt (KeepAlive!).
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);
/// So lange darf der TCP-Aufbau höchstens dauern. `TcpStream::connect` kennt von sich aus **kein**
/// Zeitlimit: Ein Server, der das SYN verschluckt (Firewall, falscher Port), hing damit je nach
/// Betriebssystem zwei Minuten am Netz-Thread, ohne dass der Client etwas gemeldet hätte.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
/// Verzögerung bis zum parallelen Versuch der nächsten Adresse (Happy Eyeballs, RFC 8305).
/// Ein sofortiges `connection refused` startet den nächsten Kandidaten ohne diese Wartezeit.
const CONNECT_FALLBACK_DELAY: Duration = Duration::from_millis(250);
/// DNS kann für einen Namen sehr viele Adressen liefern. Mehr parallele SYNs verbessern den
/// Aufbau nicht mehr, würden bei einer kaputten Antwort aber unnötig Ressourcen binden.
const MAX_CONNECT_ADDRESSES: usize = 8;

/// Kompressionsstufe für ausgehende Pakete – dieselbe, die auch der Vanilla-Client benutzt.
///
/// Sein `CompressionEncoder` legt einen `new Deflater()` an, und der läuft auf der zlib-Vorgabe
/// (Stufe 6). Das ist keine Geschmacksfrage: Die Stufe steckt in den FLEVEL-Bits **jedes**
/// zlib-Kopfes, den wir senden. Mit der vorher eingestellten schnellsten Stufe trug damit jedes
/// komprimierte Paket sichtbar „von keinem Minecraft-Client" – und gespart hätte es ohnehin fast
/// nichts: Ein AFK-Client sendet Chat und Positionen, beides weit unter der üblichen Schwelle von
/// 256 Byte, also unkomprimiert.
const OUTGOING_COMPRESSION: Compression = Compression::new(6);

/// Harte Obergrenze für Rahmen und entpackte Pakete. Sie gilt vor jeder Reservierung, damit eine
/// kaputte oder bösartige Gegenstelle nicht über ein Längenfeld beliebig viel Speicher anfordert.
const MAX_PACKET_SIZE: usize = 32 * 1024 * 1024;

/// Ab dieser Größe gilt ein Puffer als Ausreißer und wird nach dem Gebrauch wieder eingezogen.
///
/// Die Puffer bleiben absichtlich zwischen den Paketen bestehen – sonst kostete jedes Paket eine
/// neue Allokation. Sie wachsen dabei aber auf das **größte je gesehene** Paket und geben diesen
/// Platz nie wieder her: Ein einzelner 8-MB-Chunk beim Beitritt hielt so für den Rest der Laufzeit
/// 8 MB belegt, obwohl danach nur noch Chat mit ein paar Dutzend Byte kommt.
const BUFFER_HIGH_WATER: usize = 1024 * 1024;
/// So viel bleibt beim Einziehen stehen – genug für jedes gewöhnliche Paket.
const BUFFER_KEEP: usize = 64 * 1024;

/// Einen ausgeuferten Puffer wieder einziehen (siehe [`BUFFER_HIGH_WATER`]). Der Regelfall kostet
/// genau einen Vergleich.
#[inline]
fn trim(buffer: &mut Vec<u8>) {
    if buffer.capacity() > BUFFER_HIGH_WATER && buffer.len() <= BUFFER_KEEP {
        buffer.shrink_to(BUFFER_KEEP);
    }
}

/// Wie [`trim`], aber für einen Puffer, dessen Länge absichtlich auf dem größten je gesehenen
/// Paket stehen bleibt (siehe [`PacketReader::read_packet`]). Gemessen wird deshalb nicht die
/// Länge des Puffers, sondern die des zuletzt wirklich benutzten Stücks.
#[inline]
fn trim_used(buffer: &mut Vec<u8>, used: usize) {
    if buffer.len() > BUFFER_HIGH_WATER && used <= BUFFER_KEEP {
        buffer.truncate(BUFFER_KEEP);
        buffer.shrink_to(BUFFER_KEEP);
    }
}

// ===================== AES-128-CFB8 =====================

/// CFB8 mit einem Schieberegister doppelter Länge: statt je Byte 15 Bytes umzukopieren, wandert
/// nur ein Zeiger weiter. Bei Chunkverkehr geht jedes empfangene Byte hier durch.
pub struct Cfb8 {
    aes: Aes128,
    /// 32 Byte Ringspeicher; die gültigen 16 Byte beginnen bei `at`.
    register: [u8; 32],
    at: usize,
}

impl Cfb8 {
    pub fn new(key: &[u8; 16]) -> Self {
        let mut register = [0u8; 32];
        register[..16].copy_from_slice(key); // Minecraft nutzt den Schlüssel zugleich als IV
        Cfb8 {
            aes: Aes128::new(key.into()),
            register,
            at: 0,
        }
    }

    /// Ein Byte: AES über die aktuellen 16 Registerbytes, dann das Chiffrat hinten anhängen.
    /// Das Register wandert dabei nach rechts durch den Puffer und wird nur alle 16 Bytes
    /// einmal an den Anfang zurückgefaltet – statt bei jedem Byte 15 Bytes umzukopieren.
    ///
    /// Verschlüsselt wird von Quelle nach Ziel (`encrypt_block_b2b`): der Block muss dadurch
    /// nicht erst in eine eigene Variable kopiert werden.
    ///
    /// Schneller geht es hier nicht: CFB8 ist der Bauart nach seriell – das Schlüsselbyte für
    /// Byte n+1 steht erst fest, wenn Byte n verschlüsselt ist. Der Durchsatz ist damit genau
    /// die Latenz einer AES-Blockverschlüsselung je Byte (siehe den Messlauf `cfb8_durchsatz`);
    /// gespart werden kann nur an der Datenmenge, und dafür gibt es `--view-distance`.
    #[inline(always)]
    fn step(&mut self, cipher_byte: impl FnOnce(u8) -> (u8, u8)) -> u8 {
        if self.at == 16 {
            self.register.copy_within(16..32, 0);
            self.at = 0;
        }
        let iv: &[u8; 16] = self.register[self.at..self.at + 16].try_into().unwrap();
        let mut block = Block::default();
        self.aes.encrypt_block_b2b(iv.into(), &mut block);
        let (out, feedback) = cipher_byte(block[0]);
        self.register[self.at + 16] = feedback;
        self.at += 1;
        out
    }

    pub fn encrypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            let plain = *byte;
            *byte = self.step(|key| {
                let cipher = plain ^ key;
                (cipher, cipher)
            });
        }
    }

    pub fn decrypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            let cipher = *byte;
            *byte = self.step(|key| (cipher ^ key, cipher));
        }
    }
}

// ===================== Lesen =====================

pub struct PacketReader {
    stream: BufReader<TcpStream>,
    dec: Option<Cfb8>,
    threshold: i32,
    frame: Vec<u8>,
    /// Ein einziger zlib-Zustand für die ganze Verbindung: `ZlibDecoder::new` je Paket würde
    /// bei Chunkverkehr tausende Male einen 32-KB-Fensterpuffer anlegen und wegwerfen.
    inflate: Decompress,
}

impl PacketReader {
    pub fn new(stream: TcpStream) -> Self {
        PacketReader {
            // 32 KB statt 4 KB: ein Chunk-Paket kommt selten in einem Stück, und jeder
            // `read_exact` unter der Puffergröße ist ein Systemaufruf weniger.
            stream: BufReader::with_capacity(32 * 1024, stream),
            dec: None,
            threshold: -1,
            frame: Vec::new(),
            inflate: Decompress::new(true),
        }
    }

    pub fn enable_encryption(&mut self, key: &[u8; 16]) {
        self.dec = Some(Cfb8::new(key));
    }

    pub fn set_threshold(&mut self, threshold: i32) {
        self.threshold = threshold;
    }

    fn read_byte(&mut self) -> io::Result<u8> {
        let mut b = [0u8; 1];
        self.stream.read_exact(&mut b)?;
        if let Some(dec) = &mut self.dec {
            dec.decrypt(&mut b);
        }
        Ok(b[0])
    }

    /// Die Rahmenlänge steht als VarInt vor jedem Paket und muss byteweise entschlüsselt werden.
    fn read_frame_len(&mut self) -> io::Result<usize> {
        let mut value: u32 = 0;
        for i in 0..5 {
            let b = self.read_byte()?;
            value |= ((b & 0x7F) as u32) << (7 * i);
            if b & 0x80 == 0 {
                return Ok(value as usize);
            }
        }
        Err(err("Rahmenlaenge ist kein gueltiges VarInt"))
    }

    /// Nächstes Paket in `out` lesen: entschlüsseln, ggf. entpacken. Ergebnis ist `id + Nutzdaten`.
    ///
    /// `out` wird vom Aufrufer gestellt und wiederverwendet – so entsteht pro Paket keine neue
    /// Allokation, und der Reader bleibt danach frei ausleihbar.
    pub fn read_packet(&mut self, out: &mut Vec<u8>) -> io::Result<()> {
        let len = self.read_frame_len()?;
        if len == 0 || len > MAX_PACKET_SIZE {
            return Err(err("Unplausible Paketlaenge"));
        }

        // Der Rahmenpuffer wächst nur, er schrumpft nie von selbst. `resize(len, 0)` je Paket
        // hieß, den ganzen Puffer erst mit Nullen zu füllen, die `read_exact` unmittelbar danach
        // überschreibt – bei einem Megabyte Chunkdaten also ein Megabyte reines Nullenschreiben
        // pro Paket. Eingezogen wird er weiterhin, aber gezielt über [`trim_used`].
        if self.frame.len() < len {
            self.frame.resize(len, 0);
        }
        self.stream.read_exact(&mut self.frame[..len])?;
        if let Some(dec) = &mut self.dec {
            dec.decrypt(&mut self.frame[..len]);
        }

        // Vor dem Leeren: So entscheidet die Größe des **vorigen** Pakets, ob der Puffer
        // eingezogen wird. Nach dem Leeren wäre er immer leer – und ein Server, der lauter große
        // Pakete schickt, bekäme dann bei jedem einzelnen erst ein Schrumpfen und sofort danach
        // wieder ein Wachsen.
        trim(out);
        out.clear();
        if self.threshold < 0 {
            out.extend_from_slice(&self.frame[..len]);
            trim_used(&mut self.frame, len);
            return Ok(());
        }

        // Mit Kompression: VarInt „Länge im entpackten Zustand"; 0 = unkomprimiert übertragen.
        let mut r = Reader::new(&self.frame[..len]);
        let uncompressed_len = r.var_int()?;
        let start = len - r.remaining();
        if uncompressed_len == 0 {
            // Der Vanilla-Decoder akzeptiert die rohe Form nur *unterhalb* der ausgehandelten
            // Schwelle. Ohne diese Prüfung würden wir Rahmen hinnehmen, die ein echter Client als
            // fehlerhaft komprimiert trennt.
            if len - start >= self.threshold as usize {
                return Err(err("Unkomprimiertes Paket erreicht Kompressionsschwelle"));
            }
            out.extend_from_slice(&self.frame[start..len]);
            trim_used(&mut self.frame, len);
            return Ok(());
        }
        if !(0..=MAX_PACKET_SIZE as i32).contains(&uncompressed_len) {
            return Err(err("Unplausible entpackte Laenge"));
        }
        let expected = uncompressed_len as usize;
        if expected < self.threshold as usize {
            return Err(err("Komprimiertes Paket unterschreitet Kompressionsschwelle"));
        }
        out.reserve(expected);
        self.inflate.reset(true);
        let compressed = &self.frame[start..len];
        let compressed_len = compressed.len();
        let status = self
            .inflate
            .decompress_vec(compressed, out, FlushDecompress::Finish)
            .map_err(|e| err(&format!("zlib: {}", e)));
        trim_used(&mut self.frame, len);
        let status = status?;
        if out.len() != expected {
            return Err(err("Entpackte Laenge weicht ab"));
        }
        if status != Status::StreamEnd || self.inflate.total_in() != compressed_len as u64 {
            return Err(err("zlib-Strom ist nicht vollstaendig begrenzt"));
        }
        Ok(())
    }
}

// ===================== Schreiben =====================

pub struct PacketWriter {
    stream: TcpStream,
    enc: Option<Cfb8>,
    threshold: i32,
    /// Wiederverwendeter Deflate-Zustand. `reset()` behält Fenster und Tabellen; ein neuer
    /// `ZlibEncoder` je Paket würde diese Arbeitsbereiche bei jedem großen Senden neu anlegen.
    deflate: Compress,
    /// Wiederverwendete Puffer für den Paketrahmen. Ohne sie legt **jedes** gesendete Paket zwei
    /// bis drei kurzlebige Vektoren an; bei zwanzig Positionspaketen je Sekunde (Bewegung,
    /// Anti-AFK) ist das reine Verwaltungsarbeit. Der Zugriff ist unkritisch: `send` läuft nur
    /// unter der Schreibsperre in [`crate::client::Shared::send`].
    body: Vec<u8>,
    frame: Vec<u8>,
}

impl PacketWriter {
    pub fn enable_encryption(&mut self, key: &[u8; 16]) {
        self.enc = Some(Cfb8::new(key));
    }

    pub fn set_threshold(&mut self, threshold: i32) {
        self.threshold = threshold;
    }

    /// Verbindung hart schließen. Nach einem gescheiterten Schreibvorgang ist der Strom
    /// unbrauchbar – ein Zeitablauf kann mitten im Rahmen zugeschlagen haben, dann steht die
    /// Gegenstelle auf einer halben Paketlänge. Der lesende Thread bekommt dadurch sofort ein
    /// Dateiende, statt bis zum Lese-Zeitablauf weiterzuschlafen.
    pub fn shutdown(&self) {
        let _ = self.stream.shutdown(std::net::Shutdown::Both);
    }

    pub fn send(&mut self, packet: Writer) -> io::Result<()> {
        self.send_payload(&packet.data)
    }

    /// Ein Paket ohne Nutzdaten ohne kurzlebigen `Vec` senden. Moderne Clients schicken davon
    /// zwanzig Tick-Enden pro Sekunde; eine Heap-Allokation je Tick wäre vermeidbare Dauerlast.
    pub fn send_empty(&mut self, packet_id: i32) -> io::Result<()> {
        let mut encoded = [0u8; 5];
        let len = encode_var_int(&mut encoded, packet_id);
        self.send_payload(&encoded[..len])
    }

    fn send_payload(&mut self, payload: &[u8]) -> io::Result<()> {
        if payload.is_empty() || payload.len() > MAX_PACKET_SIZE {
            return Err(err("Unplausible ausgehende Paketlaenge"));
        }

        self.body.clear();
        self.frame.clear();
        if self.threshold < 0 {
            // Ohne Kompression direkt in den Rahmen schreiben: der alte Weg kopierte jedes Paket
            // erst in `body` und unmittelbar danach ein zweites Mal in `frame`.
            push_var_int(&mut self.frame, payload.len() as i32);
            self.frame.extend_from_slice(payload);
        } else if payload.len() >= self.threshold as usize {
            // Mit Kompression: VarInt „Länge im entpackten Zustand", dahinter der zlib-Strom.
            push_var_int(&mut self.body, payload.len() as i32);
            self.body.reserve(zlib_bound(payload.len()));
            self.deflate.reset();
            let status = self
                .deflate
                .compress_vec(payload, &mut self.body, FlushCompress::Finish)
                .map_err(|e| err(&format!("zlib: {}", e)))?;
            if status != Status::StreamEnd || self.deflate.total_in() != payload.len() as u64 {
                return Err(err("Ausgehender zlib-Strom blieb unvollstaendig"));
            }
            push_var_int(&mut self.frame, self.body.len() as i32);
            self.frame.extend_from_slice(&self.body);
        } else {
            // Kompression ist aktiv, dieses Paket bleibt aber roh. Auch hier direkt in `frame`,
            // statt den Inhalt über den zweiten Puffer zu kopieren.
            push_var_int(&mut self.frame, (payload.len() + 1) as i32);
            self.frame.push(0); // Data Length = 0: unkomprimiert übertragen
            self.frame.extend_from_slice(payload);
        }

        if let Some(enc) = &mut self.enc {
            enc.encrypt(&mut self.frame);
        }
        let result = self.stream.write_all(&self.frame);
        trim(&mut self.body);
        trim(&mut self.frame);
        result
    }
}

/// VarInt in einen festen Fünf-Byte-Puffer schreiben; Rückgabe ist die tatsächlich belegte Länge.
fn encode_var_int(out: &mut [u8; 5], value: i32) -> usize {
    let mut value = value as u32;
    let mut at = 0;
    loop {
        if value & !0x7F == 0 {
            out[at] = value as u8;
            return at + 1;
        }
        out[at] = (value as u8 & 0x7F) | 0x80;
        value >>= 7;
        at += 1;
    }
}

/// Sichere Obergrenze von zlib/deflate (`deflateBound`-Formel plus kleiner Spielraum).
fn zlib_bound(len: usize) -> usize {
    len.saturating_add(len >> 12)
        .saturating_add(len >> 14)
        .saturating_add(len >> 25)
        .saturating_add(16)
}

// ===================== Aufbau =====================

/// Verbindet und liefert Lese- und Schreibseite getrennt (je eigener Chiffre-Zustand,
/// CFB8 läuft richtungsgetrennt).
///
/// Mit `proxy` läuft der Aufbau über SOCKS5 bzw. HTTP-CONNECT; danach ist der Socket ein
/// gewöhnlicher TCP-Strom und alles Weitere unverändert.
pub fn connect(
    host: &str,
    port: u16,
    proxy: Option<&crate::proxy::Proxy>,
) -> io::Result<(PacketReader, PacketWriter)> {
    let stream = match proxy {
        Some(proxy) => proxy.connect(host, port)?,
        None => dial(host, port)?,
    };
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
    stream.set_write_timeout(Some(WRITE_TIMEOUT))?;
    let write_half = stream.try_clone()?;
    Ok((
        PacketReader::new(stream),
        PacketWriter {
            stream: write_half,
            enc: None,
            threshold: -1,
            deflate: Compress::new(OUTGOING_COMPRESSION, true),
            body: Vec::with_capacity(256),
            frame: Vec::with_capacity(256),
        },
    ))
}

/// TCP-Verbindung mit Zeitlimit. `connect_timeout` verlangt eine bereits aufgelöste Adresse,
/// deshalb wird der Name hier selbst aufgelöst; scheitert jeder Kandidat, kommt der letzte
/// Fehler heraus (das ist der aussagekräftige – „connection refused" statt „unbekannter Name").
fn dial(host: &str, port: u16) -> io::Result<TcpStream> {
    dial_timeout(host, port, CONNECT_TIMEOUT)
}

/// Zu allen aufgelösten Adressen mit einem gemeinsamen Zeitbudget verbinden.
///
/// IPv6 und IPv4 werden versetzt versucht. Damit kostet ein schwarzes IPv6-Ziel nicht erst den
/// vollen Verbindungs-Timeout, bevor die funktionierende IPv4-Adresse drankommt. Antwortet ein
/// Kandidat sofort mit einem Fehler, startet der nächste sofort; nur bei Schweigen greift die
/// kleine Staffelung.
pub(crate) fn dial_timeout(host: &str, port: u16, timeout: Duration) -> io::Result<TcpStream> {
    let mut addresses: Vec<SocketAddr> = (host, port).to_socket_addrs()?.collect();
    // `dedup` allein hätte nur **unmittelbar** benachbarte Wiederholungen entfernt. Resolver
    // liefern dieselbe Adresse aber gern verstreut (Round-Robin über mehrere Einträge), und jede
    // Wiederholung kostete einen der wenigen parallelen Versuche und dazu ein SYN an ein Ziel,
    // das ohnehin schon läuft. Die Reihenfolge des Resolvers bleibt dabei erhalten.
    let mut seen = std::collections::HashSet::with_capacity(addresses.len());
    addresses.retain(|address| seen.insert(*address));
    addresses.truncate(MAX_CONNECT_ADDRESSES);
    let addresses = alternate_families(addresses);
    if addresses.is_empty() {
        return Err(err("Adresse liess sich nicht aufloesen"));
    }
    if addresses.len() == 1 {
        return TcpStream::connect_timeout(&addresses[0], timeout);
    }

    let deadline = Instant::now() + timeout;
    let (tx, rx) = mpsc::channel();
    let mut next = 0usize;
    let mut active = 0usize;
    let mut last_launch = Instant::now();
    let mut last_error = None;

    let launch = |address: SocketAddr, tx: mpsc::Sender<io::Result<TcpStream>>| {
        let left = deadline.saturating_duration_since(Instant::now());
        thread::Builder::new()
            .name("afk-connect".into())
            .spawn(move || {
                let _ = tx.send(TcpStream::connect_timeout(&address, left));
            })
            .map(drop)
    };

    loop {
        // Der erste Versuch sowie ein Ersatz nach einem sofortigen Fehler starten ohne Pause.
        while active == 0 && next < addresses.len() {
            match launch(addresses[next], tx.clone()) {
                Ok(()) => active += 1,
                Err(e) => last_error = Some(e),
            }
            next += 1;
            last_launch = Instant::now();
        }
        if active == 0 || Instant::now() >= deadline {
            break;
        }

        let until_fallback = if next < addresses.len() {
            CONNECT_FALLBACK_DELAY.saturating_sub(last_launch.elapsed())
        } else {
            deadline.saturating_duration_since(Instant::now())
        };
        let wait = until_fallback.min(deadline.saturating_duration_since(Instant::now()));
        match rx.recv_timeout(wait) {
            Ok(Ok(stream)) => return Ok(stream),
            Ok(Err(e)) => {
                active -= 1;
                last_error = Some(e);
            }
            Err(mpsc::RecvTimeoutError::Timeout) if next < addresses.len() => {
                match launch(addresses[next], tx.clone()) {
                    Ok(()) => active += 1,
                    Err(e) => last_error = Some(e),
                }
                next += 1;
                last_launch = Instant::now();
            }
            Err(mpsc::RecvTimeoutError::Timeout | mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }

    Err(last_error.unwrap_or_else(|| io::Error::new(io::ErrorKind::TimedOut, "Verbindungsaufbau dauerte zu lange")))
}

/// Resolver-Reihenfolge innerhalb jeder Adressfamilie erhalten, die Familien aber abwechseln.
fn alternate_families(addresses: Vec<SocketAddr>) -> Vec<SocketAddr> {
    let Some(first) = addresses.first() else {
        return addresses;
    };
    let starts_v6 = first.is_ipv6();
    let mut v4 = addresses.iter().copied().filter(SocketAddr::is_ipv4);
    let mut v6 = addresses.iter().copied().filter(SocketAddr::is_ipv6);
    let mut out = Vec::with_capacity(addresses.len());
    for index in 0..addresses.len() {
        let wants_v6 = (index % 2 == 0) == starts_v6;
        let address = if wants_v6 {
            v6.next().or_else(|| v4.next())
        } else {
            v4.next().or_else(|| v6.next())
        };
        if let Some(address) = address {
            out.push(address);
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn verbindungsadressen_wechseln_ipv6_und_ipv4_ab() {
        let addresses = vec![
            "[2001:db8::1]:25565".parse().unwrap(),
            "[2001:db8::2]:25565".parse().unwrap(),
            "192.0.2.1:25565".parse().unwrap(),
            "192.0.2.2:25565".parse().unwrap(),
        ];
        let ordered = alternate_families(addresses);
        assert!(ordered[0].is_ipv6());
        assert!(ordered[1].is_ipv4());
        assert!(ordered[2].is_ipv6());
        assert!(ordered[3].is_ipv4());
    }

    #[test]
    fn fester_varint_puffer_deckt_alle_ids_ab() {
        for value in [0, 127, 128, 16_384, i32::MAX, -1] {
            let mut fixed = [0u8; 5];
            let len = encode_var_int(&mut fixed, value);
            let mut vec = Vec::new();
            push_var_int(&mut vec, value);
            assert_eq!(&fixed[..len], vec);
        }
    }

    /// Die Kompressionsstufe steht in den obersten zwei Bits des zweiten zlib-Kopfbytes
    /// (FLEVEL). Vanilla komprimiert mit der zlib-Vorgabe, und die trägt dort die 2; die
    /// schnellste Stufe trüge eine 0 und wäre in jedem gesendeten Paket zu sehen.
    #[test]
    fn ausgehende_kompression_traegt_die_vanilla_stufe() {
        let payload = [7u8; 4096];
        // `compress_vec` schreibt in den freien Platz des Vektors – ohne Reservierung käme
        // nichts heraus. Genau so macht es auch [`PacketWriter::send_payload`].
        let mut out = Vec::with_capacity(zlib_bound(payload.len()));
        let mut deflate = Compress::new(OUTGOING_COMPRESSION, true);
        deflate
            .compress_vec(&payload, &mut out, FlushCompress::Finish)
            .unwrap();
        assert_eq!(out[0], 0x78, "zlib-Kopf fehlt");
        assert_eq!(out[1] >> 6, 2, "FLEVEL passt nicht zur Vanilla-Stufe");
    }

    #[test]
    fn zlib_obergrenze_liegt_ueber_dem_schlimmsten_fall() {
        for len in [0, 1, 255, 65_536, MAX_PACKET_SIZE] {
            assert!(zlib_bound(len) >= len + 6);
        }
    }

    /// Ein einzelnes großes Paket beim Beitritt hielt seinen Puffer für die ganze Laufzeit belegt.
    #[test]
    fn grosse_puffer_werden_wieder_eingezogen() {
        let mut buffer: Vec<u8> = Vec::with_capacity(8 * 1024 * 1024);
        buffer.resize(64, 0);
        trim(&mut buffer);
        assert!(buffer.capacity() <= BUFFER_KEEP, "{}", buffer.capacity());
        assert_eq!(buffer.len(), 64, "der Inhalt bleibt unberührt");

        // Gewöhnliche Puffer werden nicht angefasst – sonst kostete jedes Paket eine Allokation.
        let mut normal: Vec<u8> = Vec::with_capacity(4096);
        trim(&mut normal);
        assert_eq!(normal.capacity(), 4096);

        // Ein großer Puffer, der gerade wirklich gebraucht wird, bleibt ebenfalls stehen.
        let mut busy: Vec<u8> = vec![0; 4 * 1024 * 1024];
        let before = busy.capacity();
        trim(&mut busy);
        assert_eq!(busy.capacity(), before);
    }

    /// Der Rahmenpuffer behält seine Länge absichtlich (sonst müsste jedes Paket erst genullt
    /// werden). Eingezogen wird er trotzdem – nur nach dem zuletzt benutzten Stück, nicht nach
    /// seiner eigenen Länge.
    #[test]
    fn rahmenpuffer_wird_nach_gebrauch_eingezogen() {
        // Ein kleines Paket nach einem Ausreißer: der Platz darf wieder weg.
        let mut frame: Vec<u8> = vec![7; 8 * 1024 * 1024];
        trim_used(&mut frame, 64);
        assert!(frame.len() <= BUFFER_KEEP && frame.capacity() <= BUFFER_KEEP);

        // Solange große Pakete kommen, bleibt er stehen – sonst wüchse er bei jedem einzelnen neu.
        let mut busy: Vec<u8> = vec![0; 4 * 1024 * 1024];
        let before = busy.capacity();
        trim_used(&mut busy, 4 * 1024 * 1024);
        assert_eq!(busy.capacity(), before);

        // Und ein Puffer unterhalb der Ausreißergrenze wird gar nicht erst angefasst: Sonst
        // pendelte er bei abwechselnd großen und kleinen Paketen zwischen zwei Größen hin und
        // her – und jedes Pendeln ist eine Umlagerung.
        let mut mittel: Vec<u8> = vec![0; 128 * 1024];
        trim_used(&mut mittel, 100);
        assert_eq!(mittel.len(), 128 * 1024);
    }

    /// Vergleichsimplementierung: CFB8 direkt aus der Definition, ohne Ringspeicher.
    fn reference(key: &[u8; 16], data: &[u8], encrypt: bool) -> Vec<u8> {
        let aes = Aes128::new(key.into());
        let mut iv = *key;
        let mut out = Vec::with_capacity(data.len());
        for byte in data {
            let mut block = iv.into();
            aes.encrypt_block(&mut block);
            let (result, feedback) = if encrypt {
                let cipher = byte ^ block[0];
                (cipher, cipher)
            } else {
                (byte ^ block[0], *byte)
            };
            iv.copy_within(1.., 0);
            iv[15] = feedback;
            out.push(result);
        }
        out
    }

    /// Der Ringspeicher muss Byte für Byte dasselbe liefern wie die einfache Fassung – auch
    /// über die Registergrenze hinweg (deshalb deutlich mehr als 32 Bytes).
    #[test]
    fn cfb8_stimmt_mit_der_definition_ueberein() {
        let key = [9u8, 1, 2, 3, 250, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 200];
        let plain: Vec<u8> = (0..500u32).map(|i| (i * 37 % 251) as u8).collect();

        let mut mine = plain.clone();
        Cfb8::new(&key).encrypt(&mut mine);
        assert_eq!(mine, reference(&key, &plain, true));

        let mut back = mine.clone();
        Cfb8::new(&key).decrypt(&mut back);
        assert_eq!(back, plain);
    }

    /// Messlauf statt Behauptung: Wie schnell läuft der Datenstrom durch die Chiffre?
    ///
    /// Jedes einzelne empfangene Byte geht hier durch – bei einem Chunk-Stapel beim Beitritt
    /// sind das schnell ein paar Megabyte. Läuft nicht im normalen Testlauf mit:
    /// `cargo test --release -- --ignored --nocapture cfb8_durchsatz`
    #[test]
    #[ignore]
    fn cfb8_durchsatz() {
        let key = [42u8; 16];
        let mut data = vec![0u8; 4 * 1024 * 1024];
        for (index, byte) in data.iter_mut().enumerate() {
            *byte = index as u8;
        }
        let mut cipher = Cfb8::new(&key);
        let started = std::time::Instant::now();
        cipher.encrypt(&mut data);
        let elapsed = started.elapsed();
        std::hint::black_box(&data);
        println!(
            "\nCFB8: {:?} für {} MB  ->  {:.1} MB/s\n",
            elapsed,
            data.len() / (1024 * 1024),
            data.len() as f64 / (1024.0 * 1024.0) / elapsed.as_secs_f64()
        );
    }

    /// Der Zustand darf sich nicht daran stören, wie die Bytes auf Aufrufe verteilt sind –
    /// aus dem Netz kommen sie in beliebigen Stücken.
    #[test]
    fn cfb8_haengt_nicht_an_der_stueckelung() {
        let key = [42u8; 16];
        let plain: Vec<u8> = (0..300u32).map(|i| i as u8).collect();

        let mut whole = plain.clone();
        Cfb8::new(&key).encrypt(&mut whole);

        let mut piecewise = plain.clone();
        let mut cipher = Cfb8::new(&key);
        for chunk in piecewise.chunks_mut(7) {
            cipher.encrypt(chunk);
        }
        assert_eq!(whole, piecewise);
    }
}
