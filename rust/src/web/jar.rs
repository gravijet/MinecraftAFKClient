//! Schlanker Leser für die Client-JAR.
//!
//! Die JAR hat gut 35 000 Einträge, davon braucht die Ansicht nur die Ressourcen unter
//! `assets/` (gut 11 000) und `version.json`. Ein allgemeiner ZIP-Leser hält das ganze
//! Inhaltsverzeichnis samt Metadaten im Speicher – hier bleiben je Eintrag 20 Byte: ein
//! Namens-Hash, Lage und Größe. Der Name selbst wird beim Lesen im lokalen Kopf gegengeprüft,
//! eine Hash-Kollision kann also nie die falsche Datei liefern.
//!
//! Unterstützt wird, was Mojang ausliefert: ZIP ohne ZIP64, Einträge unkomprimiert oder mit
//! Deflate.

use flate2::read::DeflateDecoder;
use std::fs::File;
use std::io::{self, Read, Seek, SeekFrom};
use std::path::Path;

#[derive(Clone, Copy)]
struct Entry {
    hash: u64,
    offset: u32,
    compressed: u32,
    size: u32,
    method: u16,
}

pub struct Jar {
    file: File,
    entries: Vec<Entry>,
}

const EOCD: u32 = 0x0605_4b50;
const CENTRAL: u32 = 0x0201_4b50;
const LOCAL: u32 = 0x0403_4b50;

fn le16(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes([b[at], b[at + 1]])
}

fn le32(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]])
}

/// FNV-1a, 64 Bit – schnell, und Kollisionen fängt die Namensprüfung ab.
fn hash(name: &[u8]) -> u64 {
    name.iter().fold(0xcbf2_9ce4_8422_2325u64, |h, b| {
        (h ^ *b as u64).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

fn bad(message: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message.to_string())
}

impl Jar {
    pub fn open(path: &Path) -> io::Result<Jar> {
        let mut file = File::open(path)?;
        let length = file.seek(SeekFrom::End(0))?;
        // Das Ende-Kennzeichen steht in den letzten 22 Byte plus bis zu 64 KB Kommentar.
        let tail_len = length.min(22 + 65_535) as usize;
        let mut tail = vec![0u8; tail_len];
        file.seek(SeekFrom::Start(length - tail_len as u64))?;
        file.read_exact(&mut tail)?;
        let eocd = (0..=tail_len.saturating_sub(22))
            .rev()
            .find(|at| le32(&tail, *at) == EOCD)
            .ok_or_else(|| bad("kein ZIP-Verzeichnis gefunden"))?;
        let count = le16(&tail, eocd + 10) as usize;
        let dir_size = le32(&tail, eocd + 12) as usize;
        let dir_offset = le32(&tail, eocd + 16) as u64;
        if count == 0xFFFF || dir_offset == 0xFFFF_FFFF {
            return Err(bad("ZIP64 wird nicht unterstützt"));
        }
        let mut dir = vec![0u8; dir_size];
        file.seek(SeekFrom::Start(dir_offset))?;
        file.read_exact(&mut dir)?;

        let mut entries = Vec::new();
        let mut at = 0usize;
        for _ in 0..count {
            if at + 46 > dir.len() || le32(&dir, at) != CENTRAL {
                return Err(bad("ZIP-Verzeichnis beschädigt"));
            }
            let method = le16(&dir, at + 10);
            let compressed = le32(&dir, at + 20);
            let size = le32(&dir, at + 24);
            let name_len = le16(&dir, at + 28) as usize;
            let extra_len = le16(&dir, at + 30) as usize;
            let comment_len = le16(&dir, at + 32) as usize;
            let offset = le32(&dir, at + 42);
            let name_end = at + 46 + name_len;
            if name_end > dir.len() {
                return Err(bad("ZIP-Verzeichnis beschädigt"));
            }
            let name = &dir[at + 46..name_end];
            if name.starts_with(b"assets/") || name == b"version.json" {
                entries.push(Entry {
                    hash: hash(name),
                    offset,
                    compressed,
                    size,
                    method,
                });
            }
            at = name_end + extra_len + comment_len;
        }
        entries.sort_by_key(|entry| entry.hash);
        entries.shrink_to_fit();
        Ok(Jar { file, entries })
    }

    pub fn contains(&self, name: &str) -> bool {
        self.find(name).is_some()
    }

    fn find(&self, name: &str) -> Option<Entry> {
        let key = hash(name.as_bytes());
        let first = self.entries.partition_point(|entry| entry.hash < key);
        self.entries[first..]
            .iter()
            .take_while(|entry| entry.hash == key)
            .copied()
            .next()
    }

    /// Inhalt eines Eintrags, höchstens `max` Byte groß.
    pub fn read(&mut self, name: &str, max: u64) -> Option<Vec<u8>> {
        let key = hash(name.as_bytes());
        let first = self.entries.partition_point(|entry| entry.hash < key);
        let candidates: Vec<Entry> = self.entries[first..]
            .iter()
            .take_while(|entry| entry.hash == key)
            .copied()
            .collect();
        candidates
            .into_iter()
            .find_map(|entry| self.read_entry(entry, name, max).ok().flatten())
    }

    fn read_entry(&mut self, entry: Entry, name: &str, max: u64) -> io::Result<Option<Vec<u8>>> {
        if entry.size as u64 > max {
            return Ok(None);
        }
        let mut header = [0u8; 30];
        self.file.seek(SeekFrom::Start(entry.offset as u64))?;
        self.file.read_exact(&mut header)?;
        if le32(&header, 0) != LOCAL {
            return Err(bad("lokaler ZIP-Kopf fehlt"));
        }
        let name_len = le16(&header, 26) as usize;
        let extra_len = le16(&header, 28) as i64;
        let mut stored = vec![0u8; name_len];
        self.file.read_exact(&mut stored)?;
        if stored != name.as_bytes() {
            return Ok(None); // Hash-Kollision: anderer Eintrag
        }
        self.file.seek(SeekFrom::Current(extra_len))?;
        let mut raw = vec![0u8; entry.compressed as usize];
        self.file.read_exact(&mut raw)?;
        match entry.method {
            0 => Ok(Some(raw)),
            8 => {
                let mut out = Vec::with_capacity(entry.size as usize);
                DeflateDecoder::new(&raw[..])
                    .take(max + 1)
                    .read_to_end(&mut out)?;
                if out.len() as u64 > max {
                    return Ok(None);
                }
                Ok(Some(out))
            }
            _ => Err(bad("unbekannte ZIP-Kompression")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::Write;

    /// Eine winzige ZIP-Datei von Hand: ein gespeicherter und ein Deflate-Eintrag.
    fn sample_zip() -> Vec<u8> {
        use flate2::write::DeflateEncoder;
        let mut out = Vec::new();
        let mut central = Vec::new();
        let files: [(&str, &[u8], bool); 3] = [
            ("version.json", br#"{"id":"26.1"}"#, false),
            ("assets/minecraft/a.json", b"hallo hallo hallo hallo", true),
            ("data/ignored.txt", b"x", false),
        ];
        for (name, data, deflate) in files {
            let body = if deflate {
                let mut e = DeflateEncoder::new(Vec::new(), flate2::Compression::default());
                e.write_all(data).unwrap();
                e.finish().unwrap()
            } else {
                data.to_vec()
            };
            let offset = out.len() as u32;
            let method: u16 = if deflate { 8 } else { 0 };
            out.extend_from_slice(&LOCAL.to_le_bytes());
            out.extend_from_slice(&[20, 0, 0, 0]);
            out.extend_from_slice(&method.to_le_bytes());
            out.extend_from_slice(&[0; 8]); // Zeit, Datum, CRC (ungeprüft)
            out.extend_from_slice(&(body.len() as u32).to_le_bytes());
            out.extend_from_slice(&(data.len() as u32).to_le_bytes());
            out.extend_from_slice(&(name.len() as u16).to_le_bytes());
            out.extend_from_slice(&0u16.to_le_bytes());
            out.extend_from_slice(name.as_bytes());
            out.extend_from_slice(&body);

            central.extend_from_slice(&CENTRAL.to_le_bytes());
            central.extend_from_slice(&[20, 0, 20, 0, 0, 0]);
            central.extend_from_slice(&method.to_le_bytes());
            central.extend_from_slice(&[0; 8]);
            central.extend_from_slice(&(body.len() as u32).to_le_bytes());
            central.extend_from_slice(&(data.len() as u32).to_le_bytes());
            central.extend_from_slice(&(name.len() as u16).to_le_bytes());
            central.extend_from_slice(&[0; 12]);
            central.extend_from_slice(&offset.to_le_bytes());
            central.extend_from_slice(name.as_bytes());
        }
        let dir_offset = out.len() as u32;
        out.extend_from_slice(&central);
        out.extend_from_slice(&EOCD.to_le_bytes());
        out.extend_from_slice(&[0; 4]);
        out.extend_from_slice(&3u16.to_le_bytes());
        out.extend_from_slice(&3u16.to_le_bytes());
        out.extend_from_slice(&(central.len() as u32).to_le_bytes());
        out.extend_from_slice(&dir_offset.to_le_bytes());
        out.extend_from_slice(&0u16.to_le_bytes());
        out
    }

    #[test]
    fn liest_gespeicherte_und_komprimierte_eintraege() {
        let path = std::env::temp_dir().join(format!("afk-jar-{}.zip", std::process::id()));
        std::fs::write(&path, sample_zip()).unwrap();
        let mut jar = Jar::open(&path).unwrap();
        assert_eq!(jar.read("version.json", 1024).unwrap(), br#"{"id":"26.1"}"#);
        assert_eq!(
            jar.read("assets/minecraft/a.json", 1024).unwrap(),
            b"hallo hallo hallo hallo"
        );
        assert!(
            jar.read("assets/minecraft/a.json", 4).is_none(),
            "Obergrenze"
        );
        assert!(
            !jar.contains("data/ignored.txt"),
            "nur assets/ und version.json"
        );
        assert!(jar.read("assets/minecraft/fehlt.json", 1024).is_none());
        let _ = std::fs::remove_file(path);
    }
}
