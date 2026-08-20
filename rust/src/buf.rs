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
        if !(0..=1024 * 1024).contains(&len) {
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
        if !(0..=1024 * 1024).contains(&len) {
            return Err(err("String-Laenge unplausibel"));
        }
        self.skip(len as usize)
    }
}

// ===================== Schreiben =====================

/// VarInt an einen beliebigen Bytepuffer anhängen.
///
/// Ausgelagert, weil der Rahmenbau in [`crate::conn`] dieselbe Kodierung braucht, dort aber
/// keinen ganzen [`Writer`] anlegen soll – der wäre je Paket eine weitere kurzlebige Allokation.
pub fn push_var_int(out: &mut Vec<u8>, value: i32) {
    let mut v = value as u32;
    loop {
        if v & !0x7F == 0 {
            out.push(v as u8);
            return;
        }
        out.push((v as u8 & 0x7F) | 0x80);
        v >>= 7;
    }
}

#[derive(Default)]
pub struct Writer {
    pub data: Vec<u8>,
}

impl Writer {
    /// Neues Paket mit gegebener ID (VarInt) beginnen.
    ///
    /// 64 Byte Startgröße, nicht 32: das Positionspaket der Bewegung ist mit ID allein schon
    /// 35 Byte lang und wuchs damit bisher bei **jedem** der zwanzig Pakete je Sekunde einmal
    /// nach. Die paar Byte mehr kosten nichts – der Puffer lebt nur bis zum Absenden.
    pub fn packet(id: i32) -> Self {
        let mut w = Writer {
            data: Vec::with_capacity(64),
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
        push_var_int(&mut self.data, value);
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
    // Über Bytes, nicht über Zeichen: `&text[i..i + 2]` schnitte sonst mitten durch ein
    // Mehrbyte-Zeichen und beendete den Prozess. 32 Bytes sind nicht zwingend 32 Zeichen,
    // und die UUID kommt aus einer Serverantwort bzw. aus einer Kontodatei.
    let clean: Vec<u8> = s.bytes().filter(|b| *b != b'-').collect();
    if clean.len() != 32 {
        return None;
    }
    let digit = |b: u8| (b as char).to_digit(16).map(|v| v as u8);
    let mut out = [0u8; 16];
    for i in 0..16 {
        out[i] = (digit(clean[i * 2])? << 4) | digit(clean[i * 2 + 1])?;
    }
    Some(out)
}

pub fn hex(bytes: &[u8]) -> String {
    const DIGITS: &[u8; 16] = b"0123456789abcdef";
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push(DIGITS[(*b >> 4) as usize] as char);
        s.push(DIGITS[(*b & 0x0f) as usize] as char);
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 32 Bytes sind nicht 32 Zeichen: Ein solcher Text darf `None` liefern, nicht abstürzen.
    #[test]
    fn uuid_aus_unsinn_stuerzt_nicht_ab() {
        let echt = "069a79f4-44e9-4726-a5be-fca90e38aaf5";
        assert!(uuid_from_str(echt).is_some());
        assert_eq!(uuid_to_dashed(&uuid_from_str(echt).unwrap()), echt);
        assert!(uuid_from_str(&"ä".repeat(16)).is_none());
        assert!(uuid_from_str("€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€€").is_none());
        assert!(uuid_from_str("zz9a79f444e94726a5befca90e38aaf5").is_none());
        assert!(uuid_from_str("").is_none());
        assert!(uuid_from_str("069a79f4").is_none());
    }

    #[test]
    fn hex_schreibt_wie_format() {
        assert_eq!(hex(&[0x00, 0x0f, 0xa5, 0xff]), "000fa5ff");
        assert_eq!(hex(&[]), "");
    }
}
