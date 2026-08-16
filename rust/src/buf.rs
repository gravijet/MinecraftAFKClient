//! Lese-/Schreibprimitive des Minecraft-Protokolls (VarInt, String, UUID, ...).
//!
//! Bewusst ohne Fremdbibliothek: ein Vec<u8> plus Lesezeiger genügt und allokiert nichts
//! außer dem Paketpuffer selbst.

use std::io;

pub fn err(msg: &str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, msg.to_string())
}

// ===================== Lesen =====================

pub struct Reader<'a> {
    data: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(data: &'a [u8]) -> Self {
        Reader { data, pos: 0 }
    }

    pub fn remaining(&self) -> usize {
        self.data.len() - self.pos
    }

    pub fn rest(&mut self) -> &'a [u8] {
        let out = &self.data[self.pos..];
        self.pos = self.data.len();
        out
    }

    pub fn bytes(&mut self, n: usize) -> io::Result<&'a [u8]> {
        if self.remaining() < n {
            return Err(err("Paket zu kurz"));
        }
        let out = &self.data[self.pos..self.pos + n];
        self.pos += n;
        Ok(out)
    }

    pub fn u8(&mut self) -> io::Result<u8> {
        Ok(self.bytes(1)?[0])
    }

    pub fn i8(&mut self) -> io::Result<i8> {
        Ok(self.u8()? as i8)
    }

    pub fn bool(&mut self) -> io::Result<bool> {
        Ok(self.u8()? != 0)
    }

    pub fn i16(&mut self) -> io::Result<i16> {
        Ok(i16::from_be_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub fn u16(&mut self) -> io::Result<u16> {
        Ok(u16::from_be_bytes(self.bytes(2)?.try_into().unwrap()))
    }

    pub fn i32(&mut self) -> io::Result<i32> {
        Ok(i32::from_be_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub fn i64(&mut self) -> io::Result<i64> {
        Ok(i64::from_be_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    pub fn f32(&mut self) -> io::Result<f32> {
        Ok(f32::from_be_bytes(self.bytes(4)?.try_into().unwrap()))
    }

    pub fn f64(&mut self) -> io::Result<f64> {
        Ok(f64::from_be_bytes(self.bytes(8)?.try_into().unwrap()))
    }

    /// VarLong – nur die Live-Ansicht braucht ihn (Sammel-Blockänderungen).
    #[cfg(feature = "pov")]
    pub fn var_long(&mut self) -> io::Result<i64> {
        let mut value: i64 = 0;
        let mut shift = 0;
        loop {
            if shift >= 70 {
                return Err(err("VarLong laenger als 10 Bytes"));
            }
            let b = self.u8()?;
            value |= ((b & 0x7F) as i64) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
    }

    pub fn var_int(&mut self) -> io::Result<i32> {
        let mut value: i32 = 0;
        let mut shift = 0;
        loop {
            if shift >= 35 {
                return Err(err("VarInt laenger als 5 Bytes"));
            }
            let b = self.u8()?;
            value |= ((b & 0x7F) as i32) << shift;
            shift += 7;
            if b & 0x80 == 0 {
                return Ok(value);
            }
        }
    }

    pub fn string(&mut self) -> io::Result<String> {
        let len = self.var_int()?;
        if len < 0 || len > 1024 * 1024 {
            return Err(err("String-Laenge unplausibel"));
        }
        let raw = self.bytes(len as usize)?;
        String::from_utf8(raw.to_vec()).map_err(|_| err("String ist kein UTF-8"))
    }

    pub fn byte_array(&mut self) -> io::Result<Vec<u8>> {
        let len = self.var_int()?;
        if len < 0 {
            return Err(err("Byte-Array-Laenge negativ"));
        }
        Ok(self.bytes(len as usize)?.to_vec())
    }

    pub fn uuid(&mut self) -> io::Result<[u8; 16]> {
        Ok(self.bytes(16)?.try_into().unwrap())
    }

    /// Feld überspringen, ohne es zu kopieren. Die Ausbaustufen laufen damit durch Felder, die
    /// sie nicht brauchen (etwa die Höhenkarten eines Chunks).
    #[cfg(feature = "extras")]
    pub fn skip(&mut self, n: usize) -> io::Result<()> {
        self.bytes(n).map(|_| ())
    }

    #[cfg(feature = "extras")]
    pub fn skip_string(&mut self) -> io::Result<()> {
        let len = self.var_int()?;
        if len < 0 || len > 1024 * 1024 {
            return Err(err("String-Laenge unplausibel"));
        }
        self.skip(len as usize)
    }
}

// ===================== Schreiben =====================

#[derive(Default)]
pub struct Writer {
    pub data: Vec<u8>,
}

impl Writer {
    /// Neues Paket mit gegebener ID (VarInt) beginnen.
    pub fn packet(id: i32) -> Self {
        let mut w = Writer {
            data: Vec::with_capacity(32),
        };
        w.var_int(id);
        w
    }

    pub fn u8(&mut self, v: u8) {
        self.data.push(v);
    }

    pub fn bool(&mut self, v: bool) {
        self.data.push(if v { 1 } else { 0 });
    }

    pub fn u16(&mut self, v: u16) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn i32(&mut self, v: i32) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn i64(&mut self, v: i64) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn f32(&mut self, v: f32) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn f64(&mut self, v: f64) {
        self.data.extend_from_slice(&v.to_be_bytes());
    }

    pub fn var_int(&mut self, value: i32) {
        let mut v = value as u32;
        loop {
            if v & !0x7F == 0 {
                self.data.push(v as u8);
                return;
            }
            self.data.push((v as u8 & 0x7F) | 0x80);
            v >>= 7;
        }
    }

    pub fn string(&mut self, value: &str) {
        self.var_int(value.len() as i32);
        self.data.extend_from_slice(value.as_bytes());
    }

    pub fn byte_array(&mut self, value: &[u8]) {
        self.var_int(value.len() as i32);
        self.data.extend_from_slice(value);
    }

    pub fn raw(&mut self, value: &[u8]) {
        self.data.extend_from_slice(value);
    }

    pub fn uuid(&mut self, value: &[u8; 16]) {
        self.data.extend_from_slice(value);
    }
}

// ===================== UUID-Hilfen =====================

/// 16 Bytes -> "xxxxxxxx-xxxx-xxxx-xxxx-xxxxxxxxxxxx"
pub fn uuid_to_dashed(u: &[u8; 16]) -> String {
    let h = hex(u);
    format!(
        "{}-{}-{}-{}-{}",
        &h[0..8],
        &h[8..12],
        &h[12..16],
        &h[16..20],
        &h[20..32]
    )
}

/// UUID-Text (mit oder ohne Bindestriche) -> 16 Bytes
pub fn uuid_from_str(s: &str) -> Option<[u8; 16]> {
    let clean: String = s.chars().filter(|c| *c != '-').collect();
    if clean.len() != 32 {
        return None;
    }
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = u8::from_str_radix(&clean[i * 2..i * 2 + 2], 16).ok()?;
    }
    Some(out)
}

pub fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{:02x}", b));
    }
    s
}
