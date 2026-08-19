//! TCP-Verbindung: Rahmen (Länge+Paket), zlib-Kompression und AES-128-CFB8-Verschlüsselung.
//!
//! CFB8 ist hier von Hand implementiert (ein Byte pro Block) – das sind zwanzig Zeilen und
//! spart eine weitere Abhängigkeit. Die Rechenlast ist bei Chat-Verkehr vernachlässigbar.

use crate::buf::{err, Reader, Writer};
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;
use flate2::write::ZlibEncoder;
use flate2::{Compression, Decompress, FlushDecompress};
use std::io::{self, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Antwortet der Server so lange nicht, gilt die Verbindung als tot (Server sendet KeepAlive
/// im 15-Sekunden-Takt – 120 s Stille heißt: da kommt nichts mehr).
const READ_TIMEOUT: Duration = Duration::from_secs(120);
/// Nimmt die Gegenstelle nichts mehr an, darf das Senden nicht ewig blockieren: der Sender hält
/// dabei die Schreibsperre, an der auch der Netz-Thread hängt (KeepAlive!).
const WRITE_TIMEOUT: Duration = Duration::from_secs(30);

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
    #[inline]
    fn step(&mut self, cipher_byte: impl FnOnce(u8) -> (u8, u8)) -> u8 {
        if self.at == 16 {
            self.register.copy_within(16..32, 0);
            self.at = 0;
        }
        let iv: [u8; 16] = self.register[self.at..self.at + 16].try_into().unwrap();
        let mut block = iv.into();
        self.aes.encrypt_block(&mut block);
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
        if len == 0 || len > 32 * 1024 * 1024 {
            return Err(err("Unplausible Paketlaenge"));
        }

        self.frame.resize(len, 0);
        self.stream.read_exact(&mut self.frame)?;
        if let Some(dec) = &mut self.dec {
            dec.decrypt(&mut self.frame);
        }

        out.clear();
        if self.threshold < 0 {
            out.extend_from_slice(&self.frame);
            return Ok(());
        }

        // Mit Kompression: VarInt „Länge im entpackten Zustand"; 0 = unkomprimiert übertragen.
        let mut r = Reader::new(&self.frame);
        let uncompressed_len = r.var_int()?;
        let start = self.frame.len() - r.remaining();
        if uncompressed_len == 0 {
            out.extend_from_slice(&self.frame[start..]);
            return Ok(());
        }
        if uncompressed_len < 0 || uncompressed_len > 32 * 1024 * 1024 {
            return Err(err("Unplausible entpackte Laenge"));
        }
        let expected = uncompressed_len as usize;
        out.reserve(expected);
        self.inflate.reset(true);
        self.inflate
            .decompress_vec(&self.frame[start..], out, FlushDecompress::Finish)
            .map_err(|e| err(&format!("zlib: {}", e)))?;
        if out.len() != expected {
            return Err(err("Entpackte Laenge weicht ab"));
        }
        Ok(())
    }
}

// ===================== Schreiben =====================

pub struct PacketWriter {
    stream: TcpStream,
    enc: Option<Cfb8>,
    threshold: i32,
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
        let payload = packet.data;

        let frame = if self.threshold < 0 {
            payload
        } else if payload.len() >= self.threshold as usize {
            let mut out = Writer::default();
            out.var_int(payload.len() as i32);
            let mut z = ZlibEncoder::new(Vec::new(), Compression::fast());
            z.write_all(&payload)?;
            out.raw(&z.finish()?);
            out.data
        } else {
            let mut out = Writer::default();
            out.var_int(0);
            out.raw(&payload);
            out.data
        };

        let mut framed = Writer::default();
        framed.var_int(frame.len() as i32);
        framed.raw(&frame);
        let mut bytes = framed.data;

        if let Some(enc) = &mut self.enc {
            enc.encrypt(&mut bytes);
        }
        self.stream.write_all(&bytes)
    }
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
        None => TcpStream::connect((host, port))?,
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
        },
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

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
        let key = [
            9u8, 1, 2, 3, 250, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 200,
        ];
        let plain: Vec<u8> = (0..500u32).map(|i| (i * 37 % 251) as u8).collect();

        let mut mine = plain.clone();
        Cfb8::new(&key).encrypt(&mut mine);
        assert_eq!(mine, reference(&key, &plain, true));

        let mut back = mine.clone();
        Cfb8::new(&key).decrypt(&mut back);
        assert_eq!(back, plain);
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
