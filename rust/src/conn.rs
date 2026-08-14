//! TCP-Verbindung: Rahmen (Länge+Paket), zlib-Kompression und AES-128-CFB8-Verschlüsselung.
//!
//! CFB8 ist hier von Hand implementiert (ein Byte pro Block) – das sind zwanzig Zeilen und
//! spart eine weitere Abhängigkeit. Die Rechenlast ist bei Chat-Verkehr vernachlässigbar.

use crate::buf::{err, Reader, Writer};
use aes::cipher::{BlockEncrypt, KeyInit};
use aes::Aes128;
use flate2::read::ZlibDecoder;
use flate2::write::ZlibEncoder;
use flate2::Compression;
use std::io::{self, BufReader, Read, Write};
use std::net::TcpStream;
use std::time::Duration;

/// Antwortet der Server so lange nicht, gilt die Verbindung als tot (Server sendet KeepAlive
/// im 15-Sekunden-Takt – 120 s Stille heißt: da kommt nichts mehr).
const READ_TIMEOUT: Duration = Duration::from_secs(120);

// ===================== AES-128-CFB8 =====================

pub struct Cfb8 {
    aes: Aes128,
    iv: [u8; 16],
}

impl Cfb8 {
    pub fn new(key: &[u8; 16]) -> Self {
        Cfb8 {
            aes: Aes128::new(key.into()),
            iv: *key, // Minecraft nutzt den Schlüssel zugleich als IV
        }
    }

    pub fn encrypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            let mut block = self.iv.into();
            self.aes.encrypt_block(&mut block);
            let cipher = *byte ^ block[0];
            self.iv.copy_within(1.., 0);
            self.iv[15] = cipher;
            *byte = cipher;
        }
    }

    pub fn decrypt(&mut self, data: &mut [u8]) {
        for byte in data.iter_mut() {
            let mut block = self.iv.into();
            self.aes.encrypt_block(&mut block);
            let cipher = *byte;
            *byte = cipher ^ block[0];
            self.iv.copy_within(1.., 0);
            self.iv[15] = cipher;
        }
    }
}

// ===================== Lesen =====================

pub struct PacketReader {
    stream: BufReader<TcpStream>,
    dec: Option<Cfb8>,
    threshold: i32,
    frame: Vec<u8>,
}

impl PacketReader {
    pub fn new(stream: TcpStream) -> Self {
        PacketReader {
            stream: BufReader::with_capacity(4096, stream),
            dec: None,
            threshold: -1,
            frame: Vec::new(),
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
        let rest = r.rest();
        if uncompressed_len == 0 {
            out.extend_from_slice(rest);
        } else {
            if uncompressed_len < 0 || uncompressed_len > 32 * 1024 * 1024 {
                return Err(err("Unplausible entpackte Laenge"));
            }
            out.reserve(uncompressed_len as usize);
            ZlibDecoder::new(rest).read_to_end(out)?;
            if out.len() != uncompressed_len as usize {
                return Err(err("Entpackte Laenge weicht ab"));
            }
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
pub fn connect(addr: &str) -> io::Result<(PacketReader, PacketWriter)> {
    let stream = TcpStream::connect(addr)?;
    stream.set_nodelay(true)?;
    stream.set_read_timeout(Some(READ_TIMEOUT))?;
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
