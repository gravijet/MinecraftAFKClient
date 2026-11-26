//! Die Welt, wie der Server sie schickt: Chunks, Blockänderungen, Dimension.
//!
//! Beide Bauformen halten sie, aber verschieden dicht:
//! * **Standard** speichert je Block nur seine **Bewegungsklasse** (siehe
//!   [`crate::physics::blocks`]). Stein, Erde und Erze sind dieselbe Klasse, Luft, Gras und
//!   Fackeln ebenso – ein typischer Abschnitt braucht so 1 bis 2 Bit je Block.
//! * **POV** speichert den Blockzustand (für Texturen) und zusätzlich Licht und Biome.
//!
//! Gespeichert wird bit-gepackt wie im Spiel selbst (1, 2, 4, 8 oder 16 Bit je Block), Luft
//! bleibt Paletteneintrag 0, reine Luftabschnitte belegen gar nichts.

use crate::client::Shared;
use crate::physics::blocks::StateTable;
use crate::protocol::buf::Reader;
use crate::protocol::nbt::Nbt;
use crate::protocol::In;

use std::collections::HashMap;
use std::io;
use std::sync::Arc;

pub(crate) const SECTION_BLOCKS: usize = 16 * 16 * 16;
#[cfg(feature = "pov")]
pub(crate) const SECTION_BIOMES: usize = 4 * 4 * 4;
#[cfg(not(feature = "pov"))]
const SECTION_BIOMES: usize = 4 * 4 * 4;
pub(crate) const MAX_SECTIONS: usize = 256;

// ===================== Gepackte Indizes =====================

/// Ein Paletten-Index je Block, dicht gepackt. Nur Zweierpotenzen als Breite: Dann kostet ein
/// Zugriff eine Verschiebung und eine Maske statt einer Division – der Strahl der Live-Ansicht
/// liest pro Bild zehntausende Blöcke.
///
/// Ohne Wörter steht in jedem Block derselbe Index `fill` (reiner Stein, reines Wasser): Solche
/// Abschnitte sind in der Standard-Bauform häufig und kosten so nichts statt 512 Byte.
#[derive(Clone)]
struct Packed {
    /// log2 der Bitbreite (0 = 1 Bit … 4 = 16 Bit).
    shift: u8,
    fill: u16,
    words: Box<[u64]>,
}

impl Packed {
    fn for_palette(len: usize) -> Packed {
        let bits = (usize::BITS - len.saturating_sub(1).leading_zeros()).max(1);
        let shift = bits.next_power_of_two().trailing_zeros() as u8;
        let per_word = 64 >> shift;
        Packed {
            shift,
            fill: 0,
            words: vec![0u64; SECTION_BLOCKS / per_word].into_boxed_slice(),
        }
    }

    fn uniform(slot: usize) -> Packed {
        Packed {
            shift: 0,
            fill: slot as u16,
            words: Box::new([]),
        }
    }

    fn bits(&self) -> usize {
        1 << self.shift
    }

    /// Wie viele Paletteneinträge ohne Verbreitern Platz haben (einheitlich: keiner mehr).
    fn capacity(&self) -> usize {
        if self.words.is_empty() {
            0
        } else if self.bits() >= 16 {
            u16::MAX as usize + 1
        } else {
            1 << self.bits()
        }
    }

    #[inline]
    fn get(&self, index: usize) -> usize {
        if self.words.is_empty() {
            return self.fill as usize;
        }
        let bits = self.bits();
        let per_word_shift = 6 - self.shift;
        let word = self.words[index >> per_word_shift];
        let offset = (index & ((1 << per_word_shift) - 1)) * bits;
        ((word >> offset) & ((1u64 << bits) - 1)) as usize
    }

    #[inline]
    fn set(&mut self, index: usize, value: usize) {
        if self.words.is_empty() {
            if value == self.fill as usize {
                return;
            }
            *self = self.widened(value.max(self.fill as usize) + 1);
        }
        let bits = self.bits();
        let per_word_shift = 6 - self.shift;
        let slot = &mut self.words[index >> per_word_shift];
        let offset = (index & ((1 << per_word_shift) - 1)) * bits;
        let mask = ((1u64 << bits) - 1) << offset;
        *slot = (*slot & !mask) | (((value as u64) << offset) & mask);
    }

    fn widened(&self, palette_len: usize) -> Packed {
        let mut wider = Packed::for_palette(palette_len);
        for index in 0..SECTION_BLOCKS {
            wider.set(index, self.get(index));
        }
        wider
    }
}

// ===================== Werte je Bauform =====================

/// Was je Block gespeichert wird: Blockzustand (POV) oder Bewegungsklasse (Standard).
pub(crate) struct Values {
    #[cfg_attr(feature = "pov", allow(dead_code))]
    table: StateTable,
}

impl Values {
    pub(crate) fn for_version(name: &str) -> Values {
        Values {
            table: StateTable::for_version(name),
        }
    }

    #[inline]
    fn store(&self, state: u32) -> u32 {
        #[cfg(feature = "pov")]
        return state;
        #[cfg(not(feature = "pov"))]
        return self.table.class(state) as u32;
    }

    /// Gilt dieser gespeicherte Wert als Luft (Paletteneintrag 0)?
    #[inline]
    fn is_air_value(&self, value: u32) -> bool {
        #[cfg(feature = "pov")]
        return value == 0;
        #[cfg(not(feature = "pov"))]
        return value == crate::physics::blocks::air_class() as u32;
    }

    /// Bewegungsklasse eines gespeicherten Werts.
    #[inline]
    pub(crate) fn class(&self, value: u32) -> u16 {
        #[cfg(feature = "pov")]
        return self.table.class(value);
        #[cfg(not(feature = "pov"))]
        return value as u16;
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dimension {
    pub(crate) name: String,
    pub(crate) min_y: i32,
    pub(crate) height: i32,
}

impl Dimension {
    pub(crate) fn legacy(id: i32) -> Dimension {
        Dimension {
            name: match id {
                -1 => "minecraft:the_nether",
                1 => "minecraft:the_end",
                _ => "minecraft:overworld",
            }
            .to_string(),
            min_y: 0,
            height: 256,
        }
    }

    pub(crate) fn from_registry(name: String, data: Option<&Nbt>) -> Dimension {
        let fallback = Dimension::for_world(&name);
        let min_y = data
            .and_then(|tag| tag.get_i32("min_y"))
            .unwrap_or(fallback.min_y);
        let height = data
            .and_then(|tag| tag.get_i32("height"))
            .unwrap_or(fallback.height);
        Dimension {
            name,
            min_y: min_y.clamp(-4096, 4096).div_euclid(16) * 16,
            height: height.clamp(16, 4096).div_euclid(16) * 16,
        }
    }

    pub(crate) fn for_world(name: &str) -> Dimension {
        // Vanilla-Fallback, falls ein ungewöhnlicher Proxy die Registry nicht weiterreicht.
        let overworld = name.ends_with("overworld") || name == "minecraft:dimension_type";
        Dimension {
            name: name.to_string(),
            min_y: if overworld { -64 } else { 0 },
            height: if overworld { 384 } else { 256 },
        }
    }

    pub(crate) fn section_count(&self) -> usize {
        (self.height / 16).clamp(1, MAX_SECTIONS as i32) as usize
    }
}

// ===================== Abschnitte =====================

/// Ein Chunk-Abschnitt: Palette plus gepackter Index je Block. Eintrag 0 ist immer Luft, damit
/// die Sichtprüfung ein einziger Vergleich ist.
#[derive(Clone)]
pub(crate) struct Section {
    pub(crate) palette: Vec<u32>,
    data: Packed,
    /// Biom je 4x4x4-Zelle – daraus kommt die Einfärbung von Gras, Laub und Wasser.
    #[cfg(feature = "pov")]
    pub(crate) biomes: Biomes,
}

impl Section {
    #[inline]
    pub(crate) fn state(&self, index: usize) -> u32 {
        self.palette.get(self.data.get(index)).copied().unwrap_or(0)
    }

    #[cfg(feature = "pov")]
    #[inline]
    pub(crate) fn is_air(&self, index: usize) -> bool {
        self.data.get(index) == 0
    }

    fn from_slots(palette: Vec<u32>, slots: impl Iterator<Item = usize>) -> Section {
        let mut data = Packed::for_palette(palette.len());
        let (mut first, mut same, mut count) = (None, true, 0);
        for (index, slot) in slots.enumerate().take(SECTION_BLOCKS) {
            data.set(index, slot);
            same &= *first.get_or_insert(slot) == slot;
            count += 1;
        }
        // Fehlende Einträge bleiben Luft – einheitlich ist nur ein vollständiger Abschnitt.
        match first {
            None => data = Packed::uniform(0),
            Some(slot) if same && count == SECTION_BLOCKS => data = Packed::uniform(slot),
            _ => {}
        }
        Section {
            palette,
            data,
            #[cfg(feature = "pov")]
            biomes: Biomes::Single(0),
        }
    }

    /// Gespeicherten Wert setzen (Luft = 0).
    pub(crate) fn set(&mut self, index: usize, value: u32) {
        if value == 0 {
            return self.data.set(index, 0);
        }
        let slot = match self.palette.iter().position(|v| *v == value) {
            Some(slot) => slot,
            None => {
                if self.palette.len() > u16::MAX as usize {
                    return; // absurd bunter Abschnitt: der letzte Zustand bleibt eben stehen
                }
                self.palette.push(value);
                if self.palette.len() > self.data.capacity() {
                    self.data = self.data.widened(self.palette.len());
                }
                self.palette.len() - 1
            }
        };
        self.data.set(index, slot);
    }

    pub(crate) fn empty() -> Section {
        Section::from_slots(vec![0], std::iter::empty())
    }

    /// Belegter Speicher in Byte (für die Selbstauskunft).
    pub(crate) fn bytes(&self) -> usize {
        #[cfg(feature = "pov")]
        let biomes = match self.biomes {
            Biomes::Single(_) => 0,
            Biomes::Cells(_) => SECTION_BIOMES * 2,
        };
        #[cfg(not(feature = "pov"))]
        let biomes = 0;
        self.palette.capacity() * 4 + self.data.words.len() * 8 + biomes
    }
}

// ===================== Chunks =====================

/// Welche Spielart des Abschnittsformats der Server spricht. Wird einmal ermittelt und dann
/// behalten – siehe [`Format::probe`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub(crate) struct Format {
    /// Palettendaten ohne Längenangabe (ab 1.21.11).
    pub(crate) modern_palette: bool,
    /// Zusätzlicher Flüssigkeitszähler je Abschnitt (ab 26.1).
    pub(crate) fluid_count: bool,
    /// Biom je Block statt je 4x4x4-Zelle (ab 26.4): 4096 Einträge, Palette bis 8 Bit.
    pub(crate) block_biomes: bool,
}

impl Format {
    /// Einträge und größte indirekte Palettenbreite der Biome eines Abschnitts.
    fn biome_layout(self) -> (usize, u8) {
        if self.block_biomes {
            (SECTION_BLOCKS, 8)
        } else {
            (SECTION_BIOMES, 3)
        }
    }
}

impl Format {
    /// Alle vier Spielarten, die bevorzugte zuerst. Danach die, die sich in genau einem Merkmal
    /// unterscheiden – ein Protokollupdate dreht erfahrungsgemäß nur eines von beiden.
    fn candidates(preferred: Format) -> [Format; 4] {
        [
            preferred,
            Format {
                modern_palette: !preferred.modern_palette,
                ..preferred
            },
            Format {
                fluid_count: !preferred.fluid_count,
                ..preferred
            },
            Format {
                modern_palette: !preferred.modern_palette,
                fluid_count: !preferred.fluid_count,
                ..preferred
            },
        ]
    }

    /// Chunk lesen und dabei die passende Spielart bestimmen. Bevorzugt wird die, die den Puffer
    /// restlos aufgeht **und** die erwartete Zahl Abschnitte liefert.
    pub(crate) fn probe(
        preferred: Format,
        dimension: &Dimension,
        data: &[u8],
        values: &Values,
    ) -> io::Result<(Chunk, Format)> {
        let mut first_error = None;
        let mut fallback: Option<(Chunk, Format)> = None;
        for format in Format::candidates(preferred) {
            match Chunk::decode(format, dimension, data, values) {
                Ok(chunk) => {
                    if chunk.sections.len() == dimension.section_count() {
                        return Ok((chunk, format));
                    }
                    fallback.get_or_insert((chunk, format));
                }
                Err(error) => {
                    first_error.get_or_insert(error);
                }
            }
        }
        fallback.ok_or_else(|| {
            first_error.unwrap_or_else(|| crate::protocol::buf::err("POV: Chunk nicht lesbar"))
        })
    }
}

/// Ein Chunk ist billig zu klonen: die Abschnitte hängen einzeln an einem `Arc`, eine
/// Blockänderung kopiert deshalb nur den einen betroffenen Abschnitt.
#[derive(Clone)]
pub(crate) struct Chunk {
    pub(crate) min_section: i32,
    /// `None` = reine Luft; solche Abschnitte belegen keinen Speicher.
    pub(crate) sections: Vec<Option<Arc<Section>>>,
    /// Himmels- und Blocklicht, sofern der Server sie mitgeschickt hat. `None` heißt: keine
    /// Angabe – dann bleibt es bei der geschätzten Flächenhelligkeit von früher.
    ///
    /// Hinter einem eigenen `Arc`, damit eine Blockänderung (die das Licht nicht anfasst) beim
    /// Kopieren des Chunks nur einen Zähler erhöht statt das ganze Lichtfeld mitzunehmen.
    #[cfg(feature = "pov")]
    pub(crate) light: Option<Arc<Light>>,
}

impl Chunk {
    /// Abschnitte lesen, bis der Puffer leer ist. Bewusst nicht „genau `section_count` Stück":
    /// meldet ein Server eine andere Welthöhe, als seine Registry sagt, folgt die Ansicht dem,
    /// was wirklich im Paket steht, statt jeden Chunk zu verwerfen.
    pub(crate) fn decode(
        format: Format,
        dimension: &Dimension,
        data: &[u8],
        values: &Values,
    ) -> io::Result<Chunk> {
        let mut r = Reader::new(data);
        let mut sections = Vec::with_capacity(dimension.section_count());
        while r.remaining() > 0 {
            if sections.len() >= MAX_SECTIONS {
                return Err(crate::protocol::buf::err("POV: zu viele Chunk-Abschnitte"));
            }
            sections.push(read_section(&mut r, format, values)?);
        }
        if sections.is_empty() {
            return Err(crate::protocol::buf::err("POV: Chunk ohne Abschnitte"));
        }
        Ok(Chunk {
            min_section: dimension.min_y.div_euclid(16),
            sections,
            #[cfg(feature = "pov")]
            light: None,
        })
    }

    pub(crate) fn section_index(&self, y: i32) -> Option<usize> {
        usize::try_from(y.div_euclid(16) - self.min_section)
            .ok()
            .filter(|index| *index < self.sections.len())
    }

    /// Der Abschnitt zu dieser Höhe – `None` heißt „außerhalb der Welt oder reine Luft".
    pub(crate) fn section(&self, y: i32) -> Option<&Section> {
        self.sections[self.section_index(y)?].as_deref()
    }

    /// Direkter Blockzugriff ohne jeden Merker. Im laufenden Betrieb geht alles über [`Cursor`];
    /// diese beiden Wege bleiben als **Vergleichsmaßstab** für die Tests stehen – nur so lässt
    /// sich zeigen, dass der Merker wirklich nichts am Ergebnis ändert.
    #[cfg(all(test, feature = "pov"))]
    pub(crate) fn block(&self, x: i32, y: i32, z: i32) -> u32 {
        match self.section(y) {
            Some(section) => section.state(local_index(x, y, z)),
            None => 0,
        }
    }

    /// `true`, wenn an der Stelle etwas Sichtbares steht. Billiger als [`Chunk::block`], weil die
    /// Palette dafür gar nicht angefasst werden muss.
    #[cfg(all(test, feature = "pov"))]
    pub(crate) fn solid(&self, x: i32, y: i32, z: i32) -> bool {
        self.section(y)
            .is_some_and(|section| !section.is_air(local_index(x, y, z)))
    }

    pub(crate) fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let Some(index) = self.section_index(y) else {
            return;
        };
        if self.sections[index].is_none() {
            if state == 0 {
                return;
            }
            self.sections[index] = Some(Arc::new(Section::empty()));
        }
        // `make_mut` kopiert höchstens diesen einen Abschnitt – nicht den ganzen Chunk.
        let section = Arc::make_mut(self.sections[index].as_mut().unwrap());
        section.set(local_index(x, y, z), state);
    }
}

/// Reihenfolge des Protokolls: `(y << 8) | (z << 4) | x`.
pub(crate) fn local_index(x: i32, y: i32, z: i32) -> usize {
    ((y.rem_euclid(16) as usize) << 8)
        | ((z.rem_euclid(16) as usize) << 4)
        | x.rem_euclid(16) as usize
}

/// Einen Abschnitt lesen. `None` = reine Luft.
pub(crate) fn read_section(
    r: &mut Reader,
    format: Format,
    values: &Values,
) -> io::Result<Option<Arc<Section>>> {
    let block_count = r.i16()?;
    if !(0..=SECTION_BLOCKS as i16).contains(&block_count) {
        return Err(crate::protocol::buf::err("POV: Blockzaehler unplausibel"));
    }
    if format.fluid_count {
        let fluid = r.i16()?;
        if !(0..=SECTION_BLOCKS as i16).contains(&fluid) {
            return Err(crate::protocol::buf::err("POV: Fluidzaehler unplausibel"));
        }
    }

    // Ein Abschnitt ohne einen einzigen Nicht-Luft-Block wird ohnehin verworfen – dann braucht
    // seine Palette gar nicht erst ausgepackt zu werden, sie muss nur exakt übersprungen werden.
    // In einer gewachsenen Überwelt sind das zwei Drittel aller Abschnitte, und jeder von ihnen
    // hat bisher 4096 Indizes ausgepackt und sofort wieder weggeworfen.
    if block_count == 0 {
        skip_palette(r, SECTION_BLOCKS, 8, format.modern_palette)?;
        let (cells, bits) = format.biome_layout();
        skip_palette(r, cells, bits, format.modern_palette)?;
        return Ok(None);
    }

    let palette = read_palette(r, SECTION_BLOCKS, 8, format.modern_palette)?;
    // Biome stehen im selben Abschnitt, nur gröber aufgelöst: eine Nummer je 4x4x4 Zelle.
    // Aus ihr kommt die Einfärbung von Gras, Laub und Wasser (siehe [`crate::web::assets`]).
    #[cfg(feature = "pov")]
    let biomes = read_biomes(r, format)?;
    #[cfg(not(feature = "pov"))]
    {
        let (cells, bits) = format.biome_layout();
        skip_palette(r, cells, bits, format.modern_palette)?;
    }

    #[cfg_attr(not(feature = "pov"), allow(unused_mut))]
    Ok(
        compact(palette, block_count as usize, values).map(|mut section| {
            #[cfg(feature = "pov")]
            {
                section.biomes = biomes;
            }
            Arc::new(section)
        }),
    )
}

/// Die Biom-Nummern eines Abschnitts: 4x4x4 Zellen, Reihenfolge wie bei den Blöcken `y, z, x`.
#[cfg(feature = "pov")]
#[derive(Clone)]
pub(crate) enum Biomes {
    /// Der ganze Abschnitt liegt in einem Biom. Das ist weit weg von jeder Biomgrenze der
    /// Normalfall – und dann kostet er zwei Byte statt 128.
    Single(u16),
    Cells(Box<[u16; SECTION_BIOMES]>),
}

#[cfg(feature = "pov")]
impl Biomes {
    #[inline]
    pub(crate) fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        match self {
            Biomes::Single(id) => *id,
            Biomes::Cells(cells) => cells[biome_index(x, y, z)],
        }
    }
}

/// Zellenindex innerhalb eines Abschnitts: je vier Blöcke eine Zelle, Reihenfolge `y, z, x`.
#[inline]
#[cfg(feature = "pov")]
fn biome_index(x: i32, y: i32, z: i32) -> usize {
    ((y.rem_euclid(16) as usize / 4) << 4)
        | ((z.rem_euclid(16) as usize / 4) << 2)
        | (x.rem_euclid(16) as usize / 4)
}

#[cfg(feature = "pov")]
fn read_biomes(r: &mut Reader, format: Format) -> io::Result<Biomes> {
    let (size, bits) = format.biome_layout();
    let raw = read_palette(r, size, bits, format.modern_palette)?;
    // Eine einwertige Palette ist der Normalfall; sie hier zusammenzufassen spart den Kasten.
    if raw.values.len() == 1 {
        return Ok(Biomes::Single(clamp_biome(raw.values[0])));
    }
    let mut cells = [0u16; SECTION_BIOMES];
    for (cell, value) in cells.iter_mut().enumerate() {
        // Ab 26.4 steht ein Biom je Block; für die Einfärbung reicht der Eckblock jeder Zelle.
        let index = if format.block_biomes {
            let (x, y, z) = (cell & 3, cell >> 4, (cell >> 2) & 3);
            ((y * 4) << 8) | ((z * 4) << 4) | (x * 4)
        } else {
            cell
        };
        let slot = raw.indices.get(index).copied().unwrap_or(0);
        *value = clamp_biome(raw.values.get(slot as usize).copied().unwrap_or(0));
    }
    Ok(Biomes::Cells(Box::new(cells)))
}

/// Biom-Nummern sind Registry-Indizes; mehr als 65 535 Biome hat keine Registry. Ein größerer
/// Wert wäre ohnehin in keiner Tabelle zu finden und wird deshalb zu „unbekannt" (0).
#[inline]
#[cfg(feature = "pov")]
fn clamp_biome(value: u32) -> u16 {
    u16::try_from(value).unwrap_or(0)
}

// ===================== Licht =====================

/// 4096 Blöcke zu je einem Nibble – zwei Blöcke teilen sich ein Byte.
#[cfg(feature = "pov")]
pub(crate) const LIGHT_BYTES: usize = SECTION_BLOCKS / 2;
/// So viele Longs fasst ein Bitfeld über alle Lichtabschnitte (zwei mehr als Weltabschnitte).
#[cfg(feature = "pov")]
const MASK_LONGS: usize = (MAX_SECTIONS + 2).div_ceil(64);

/// Das Licht **eines** Chunk-Abschnitts.
#[cfg(feature = "pov")]
#[derive(Clone)]
pub(crate) enum LightSection {
    /// Überall derselbe Wert. Das ist nicht der Sonderfall, sondern der Normalfall: Über Tage
    /// steht der Himmel durchgehend auf 15, tief unten ist alles 0. Ein eingelesenes Feld wird
    /// deshalb zusammengefasst, sobald alle 4096 Nibbles gleich sind – das spart je Abschnitt
    /// 2 KiB und beim Nachschlagen den Speicherzugriff gleich mit.
    Uniform(u8),
    Nibbles(Arc<[u8; LIGHT_BYTES]>),
}

#[cfg(feature = "pov")]
impl LightSection {
    #[inline]
    pub(crate) fn get(&self, index: usize) -> u8 {
        match self {
            LightSection::Uniform(value) => *value,
            // Zwei Blöcke je Byte: der gerade im unteren, der ungerade im oberen Nibble.
            LightSection::Nibbles(data) => (data[index >> 1] >> ((index & 1) * 4)) & 0xF,
        }
    }
}

/// Himmels- und Blocklicht eines Chunks, wie der Server sie im selben Paket wie die Blöcke
/// mitschickt.
///
/// Die Lichtabschnitte reichen je einen Abschnitt **unter** und **über** die Welt hinaus – daher
/// zwei mehr als Weltabschnitte.
#[cfg(feature = "pov")]
#[derive(Clone)]
pub(crate) struct Light {
    /// Unterster Lichtabschnitt in Abschnittskoordinaten (ein Abschnitt unter der Welt).
    pub(crate) min_section: i32,
    pub(crate) sky: Box<[LightSection]>,
    pub(crate) block: Box<[LightSection]>,
}

#[cfg(feature = "pov")]
impl Light {
    /// Himmels- und Blocklicht an dieser Stelle, jeweils 0..15.
    #[inline]
    pub(crate) fn get(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
        let section = y.div_euclid(16) - self.min_section;
        if section < 0 {
            return (0, 0); // unter der Welt: kein Licht
        }
        let index = section as usize;
        if index >= self.sky.len() {
            // Über der Welt steht nichts mehr im Weg – dort ist voller Himmel.
            return (15, 0);
        }
        let local = local_index(x, y, z);
        (self.sky[index].get(local), self.block[index].get(local))
    }
}

/// Bitfeld über die Lichtabschnitte. Fest dimensioniert: 258 Bits passen in fünf Longs, und
/// damit kostet das Einlesen eines Chunks hier keine einzige Allokation.
#[cfg(feature = "pov")]
#[derive(Default, Clone, Copy)]
struct SectionMask([u64; MASK_LONGS]);

#[cfg(feature = "pov")]
impl SectionMask {
    #[inline]
    fn has(&self, index: usize) -> bool {
        self.0
            .get(index >> 6)
            .is_some_and(|word| (word >> (index & 63)) & 1 == 1)
    }
}

#[cfg(feature = "pov")]
fn read_mask(r: &mut Reader) -> io::Result<SectionMask> {
    let longs = r.var_int()?;
    if !(0..=MASK_LONGS as i32).contains(&longs) {
        return Err(crate::protocol::buf::err("POV: Lichtmaske unplausibel"));
    }
    let mut mask = SectionMask::default();
    for slot in mask.0.iter_mut().take(longs as usize) {
        *slot = r.i64()? as u64;
    }
    Ok(mask)
}

/// Ein Lichtfeld übernehmen – zusammengefasst, wenn alle 4096 Nibbles gleich sind.
///
/// Das Zusammenfassen ist hier keine Kosmetik, sondern der Grund, warum Licht überhaupt bezahlbar
/// ist. Nachgemessen mit `speicherbedarf_der_live_ansicht` (169 Chunks, 845 Abschnitte):
///
/// ```text
///   ohne Licht                       7984 kB
///   mit Licht, zusammengefasst       8256 kB   (+272 kB)
///   mit Licht, jedes Feld einzeln   15208 kB   (+7224 kB)
/// ```
///
/// Der Grund ist die Beschaffenheit der Daten und nicht ein Trick: Über Tage steht der Himmel
/// durchgehend auf 15, unter dem Boden durchgehend auf 0. Nur die paar Abschnitte an der
/// Geländekante sind wirklich uneinheitlich – und genau die behalten ihr volles Feld.
#[cfg(feature = "pov")]
pub(crate) fn compact_light(data: &[u8]) -> LightSection {
    let first = data[0];
    // „Alle gleich" heißt: jedes Byte gleich *und* die beiden Nibbles darin gleich.
    if first >> 4 == first & 0xF && data.iter().all(|byte| *byte == first) {
        return LightSection::Uniform(first & 0xF);
    }
    let mut array = [0u8; LIGHT_BYTES];
    array.copy_from_slice(data);
    LightSection::Nibbles(Arc::new(array))
}

/// Die Felder einer Lichtrichtung einlesen und den Abschnitten zuordnen.
///
/// Sie kommen in der Reihenfolge der gesetzten Bits der Maske. Ein Abschnitt ohne Feld ist
/// entweder ausdrücklich als dunkel gemeldet (`empty`) oder gar nicht erwähnt – für die Ansicht
/// ist beides dasselbe, und geraten wird in keinem der beiden Fälle.
#[cfg(feature = "pov")]
fn read_light_arrays(
    r: &mut Reader,
    count: usize,
    present: &SectionMask,
    empty: &SectionMask,
    previous: Option<&[LightSection]>,
) -> io::Result<Box<[LightSection]>> {
    let sent = r.var_int()?;
    if !(0..=count as i32).contains(&sent) {
        return Err(crate::protocol::buf::err("POV: Lichtfelder unplausibel"));
    }
    let mut arrays = Vec::with_capacity(sent as usize);
    for _ in 0..sent {
        let data = r.byte_slice()?;
        if data.len() != LIGHT_BYTES {
            return Err(crate::protocol::buf::err(
                "POV: Lichtfeld hat die falsche Groesse",
            ));
        }
        arrays.push(compact_light(data));
    }

    let mut out = Vec::with_capacity(count);
    let mut next = 0usize;
    for index in 0..count {
        if present.has(index) && empty.has(index) {
            return Err(crate::protocol::buf::err(
                "POV: Lichtabschnitt zugleich vorhanden und leer",
            ));
        }
        out.push(if present.has(index) {
            let section = arrays
                .get(next)
                .cloned()
                .unwrap_or(LightSection::Uniform(0));
            next += 1;
            section
        } else if empty.has(index) {
            LightSection::Uniform(0)
        } else {
            // Im Chunk-Paket gibt es noch keinen alten Stand; dort bedeutet ein nicht genanntes
            // Feld dunkel. Ein LightUpdate ist dagegen ein *Patch*: Nicht genannte Abschnitte
            // bleiben unveraendert. Die alte Fassung ersetzte sie alle durch Null und machte so
            // nach dem ersten kleinen Lichtupdate fast die ganze Live-POV schwarz.
            previous
                .and_then(|sections| sections.get(index))
                .cloned()
                .unwrap_or(LightSection::Uniform(0))
        });
    }
    if next != arrays.len() {
        return Err(crate::protocol::buf::err(
            "POV: Lichtmaske und Felder passen nicht",
        ));
    }
    Ok(out.into_boxed_slice())
}

/// Die Lichtdaten am Ende eines Chunk-Pakets und im `LightUpdate`-Paket.
///
/// Aufbau: vier Bitfelder (welche Abschnitte ein Feld mitbringen und welche bekanntermaßen ganz
/// dunkel sind), danach die Felder selbst.
#[cfg(feature = "pov")]
pub(crate) fn read_light(
    r: &mut Reader,
    min_section: i32,
    sections: usize,
    previous: Option<&Light>,
) -> io::Result<Light> {
    let count = sections + 2;
    let sky_mask = read_mask(r)?;
    let block_mask = read_mask(r)?;
    let empty_sky = read_mask(r)?;
    let empty_block = read_mask(r)?;
    let previous = previous.filter(|light| {
        light.min_section == min_section - 1
            && light.sky.len() == count
            && light.block.len() == count
    });
    let sky = read_light_arrays(
        r,
        count,
        &sky_mask,
        &empty_sky,
        previous.map(|light| light.sky.as_ref()),
    )?;
    let block = read_light_arrays(
        r,
        count,
        &block_mask,
        &empty_block,
        previous.map(|light| light.block.as_ref()),
    )?;
    Ok(Light {
        min_section: min_section - 1,
        sky,
        block,
    })
}

/// Die Blockentitäten zwischen Chunkdaten und Licht überspringen.
///
/// Gelesen wird davon nichts – die Ansicht zeichnet keine Truhenmodelle und keine Schilder. Der
/// Lesezeiger muss aber exakt hinter ihnen stehen, sonst beginnt das Licht an der falschen Stelle.
#[cfg(feature = "pov")]
fn skip_block_entities(r: &mut Reader) -> io::Result<()> {
    let count = r.var_int()?;
    if !(0..=(SECTION_BLOCKS * MAX_SECTIONS) as i32).contains(&count) {
        return Err(crate::protocol::buf::err(
            "POV: Blockentitaeten unplausibel",
        ));
    }
    for _ in 0..count {
        r.u8()?; // x und z, je vier Bit
        r.i16()?; // y
        r.var_int()?; // Typ
        crate::protocol::nbt::read_network(r)?;
    }
    Ok(())
}

// ===================== Welt =====================

pub(crate) struct World {
    pub(crate) dimension: Dimension,
    /// Dimensionstypen aus der Registry des Servers (Reihenfolge = Netz-ID).
    dimensions: Vec<Dimension>,
    pub(crate) chunks: HashMap<(i32, i32), Arc<Chunk>>,
    /// Zuletzt bekannter eigener Chunk – daran hängt das Verwerfen weit entfernter Chunks.
    center: (i32, i32),
    /// So viele Chunks um den eigenen herum bleiben (Sichtweite + 1).
    keep_radius: i32,
    /// Zählt jede Änderung an [`World::chunks`]. Die Live-Ansicht kopiert ihre Chunk-Tabelle nur,
    /// wenn sich die Zahl geändert hat.
    pub(crate) revision: u64,
    /// Zuletzt erkannte Spielart des Abschnittsformats (siehe [`Format`]).
    format: Option<Format>,
    pub(crate) values: Values,
    parse_errors: u32,
}

impl World {
    pub(crate) fn new(version: &str, view_distance: u8) -> World {
        World {
            dimension: Dimension::for_world("minecraft:overworld"),
            dimensions: Vec::new(),
            chunks: HashMap::new(),
            center: (0, 0),
            keep_radius: view_distance as i32 + 1,
            // Nicht 0: die Live-Ansicht beginnt bei 0 und muss beim ersten Bild kopieren.
            revision: 1,
            format: None,
            values: Values::for_version(version),
            parse_errors: 0,
        }
    }

    /// Nach jeder Änderung an den Chunks aufrufen.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    pub(crate) fn clear(&mut self) {
        self.chunks = HashMap::new();
        self.touch();
    }

    pub(crate) fn set_dimensions(&mut self, dimensions: Vec<Dimension>) {
        self.dimensions = dimensions;
    }

    fn select_dimension(&mut self, id: i32, world_name: Option<&str>) {
        self.dimension = usize::try_from(id)
            .ok()
            .and_then(|index| self.dimensions.get(index).cloned())
            .unwrap_or_else(|| Dimension::for_world(world_name.unwrap_or("minecraft:overworld")));
        self.clear();
    }

    fn max_chunks(&self) -> usize {
        // Zwei Chunks Anlauf über den Radius hinaus: So viel kann ankommen, bevor die erste
        // Position bekannt ist und aufgeräumt werden kann.
        (2 * (self.keep_radius as usize + 2) + 1).pow(2)
    }

    fn insert(&mut self, x: i32, z: i32, chunk: Chunk, center: Option<(i32, i32)>) {
        if let Some(center) = center {
            self.center = center;
        }
        if self.chunks.len() < self.max_chunks() || self.chunks.contains_key(&(x, z)) {
            self.chunks.insert((x, z), Arc::new(chunk));
            self.touch();
        }
        if center.is_some() {
            self.prune();
        }
    }

    fn forget(&mut self, key: (i32, i32)) {
        if self.chunks.remove(&key).is_some() {
            self.touch();
        }
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let value = self.values.store(state);
        let value = if self.values.is_air_value(value) {
            0
        } else {
            value
        };
        let key = (x.div_euclid(16), z.div_euclid(16));
        if let Some(chunk) = self.chunks.get_mut(&key) {
            Arc::make_mut(chunk).set_block(x, y, z, value);
            self.touch();
        }
    }

    /// Alles wegwerfen, was außerhalb der Sichtweite liegt. Ohne das lassen Server, die nie ein
    /// `ForgetChunk` schicken, den Speicher unbegrenzt wachsen.
    fn prune(&mut self) {
        let (cx, cz) = self.center;
        let radius = self.keep_radius;
        let before = self.chunks.len();
        self.chunks
            .retain(|(x, z), _| (x - cx).abs() <= radius && (z - cz).abs() <= radius);
        if self.chunks.len() != before {
            self.touch();
        }
    }

    /// Selbstauskunft: (Chunks, gefüllte Abschnitte, Byte).
    pub(crate) fn footprint(&self) -> (usize, usize, usize) {
        let mut sections = 0;
        let mut bytes = 0;
        for chunk in self.chunks.values() {
            for section in chunk.sections.iter().flatten() {
                sections += 1;
                bytes += section.bytes();
            }
            #[cfg(feature = "pov")]
            if let Some(light) = &chunk.light {
                bytes += light
                    .sky
                    .iter()
                    .chain(light.block.iter())
                    .map(|part| match part {
                        LightSection::Uniform(_) => 2,
                        LightSection::Nibbles(_) => 2 + LIGHT_BYTES,
                    })
                    .sum::<usize>();
            }
        }
        (self.chunks.len(), sections, bytes)
    }
}

impl crate::physics::Blocks for World {
    fn class_at(&self, x: i32, y: i32, z: i32) -> Option<u16> {
        let chunk = self.chunks.get(&(x.div_euclid(16), z.div_euclid(16)))?;
        // Unter und über der Welt ist Luft (dort fällt man ins Nichts bzw. springt frei).
        let value = match chunk.section(y) {
            Some(section) => section.state(local_index(x, y, z)),
            None => return Some(crate::physics::blocks::air_class()),
        };
        Some(if value == 0 {
            crate::physics::blocks::air_class()
        } else {
            self.values.class(value)
        })
    }
}

// ===================== Lebenszyklus =====================

/// Login-Paket ab direkt hinter der eigenen Entity-ID: Dimension wählen.
pub fn login(shared: &Arc<Shared>, r: &mut Reader) {
    let world = &shared.world;
    if shared.proto.legacy {
        // gameMode (u8), dann die Dimension als Byte
        if let Ok(dimension) = r.u8().and_then(|_| r.i8()) {
            let mut world = world.lock().unwrap();
            world.dimension = Dimension::legacy(dimension as i32);
            world.clear();
        }
        return;
    }
    let dimension = (|| -> io::Result<(i32, String)> {
        r.bool()?;
        let worlds = r.var_int()?;
        if !(0..=1024).contains(&worlds) {
            return Err(crate::protocol::buf::err("Weltliste unplausibel"));
        }
        for _ in 0..worlds {
            r.skip_string()?;
        }
        for _ in 0..3 {
            r.var_int()?;
        }
        for _ in 0..3 {
            r.bool()?;
        }
        Ok((r.var_int()?, r.string()?))
    })();
    let mut world = world.lock().unwrap();
    match dimension {
        Ok((id, name)) => world.select_dimension(id, Some(&name)),
        Err(_) => world.clear(),
    }
}

/// Respawn beginnt direkt mit demselben `PlayerSpawnInfo`.
pub fn respawn(shared: &Arc<Shared>, r: &mut Reader) {
    let mut world = shared.world.lock().unwrap();
    if shared.proto.legacy {
        match r.i32() {
            Ok(dimension) => {
                world.dimension = Dimension::legacy(dimension);
                world.clear();
            }
            Err(_) => world.clear(),
        }
        return;
    }
    match (r.var_int(), r.string()) {
        (Ok(id), Ok(name)) => world.select_dimension(id, Some(&name)),
        _ => world.clear(),
    }
}

// ===================== Eingehende Weltpakete =====================

pub fn incoming(shared: &Arc<Shared>, kind: In, r: &mut Reader) {
    if let Err(error) = read_incoming(shared, kind, r) {
        let mut world = shared.world.lock().unwrap();
        world.parse_errors += 1;
        if world.parse_errors <= 3 {
            shared
                .console
                .warn(&format!("Weltpaket verworfen: {}", error));
        }
    }
}

fn read_incoming(shared: &Arc<Shared>, kind: In, r: &mut Reader) -> io::Result<()> {
    match kind {
        In::LevelChunk => read_chunk(shared, r),
        In::LevelChunkBulk => read_chunk_bulk(shared, r),
        In::ForgetChunk => {
            // ChunkPos ist ein Long: x in den unteren, z in den oberen 32 Bit.
            let key = unpack_chunk_pos(r.i64()?);
            shared.world.lock().unwrap().forget(key);
            Ok(())
        }
        In::BlockUpdate => {
            let packed = r.i64()?;
            let (x, y, z) = if shared.proto.legacy {
                unpack_legacy_block_pos(packed)
            } else {
                unpack_block_pos(packed)
            };
            let state = r.var_int()? as u32;
            shared.world.lock().unwrap().set_block(x, y, z, state);
            Ok(())
        }
        In::SectionBlocks => read_section_blocks(shared, r),
        #[cfg(feature = "pov")]
        In::LightUpdate => read_light_update(shared, r),
        _ => Ok(()),
    }
}

/// Eigener Chunk – der Bezugspunkt fürs Verwerfen.
fn own_chunk(shared: &Shared) -> Option<(i32, i32)> {
    shared.position().map(|(x, _, z, _, _)| {
        (
            (x.floor() as i32).div_euclid(16),
            (z.floor() as i32).div_euclid(16),
        )
    })
}

fn read_chunk(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    let x = r.i32()?;
    let z = r.i32()?;
    if shared.proto.legacy {
        let ground_up = r.bool()?;
        let mask = r.u16()?;
        let data = r.byte_slice()?;
        if ground_up && mask == 0 {
            shared.world.lock().unwrap().forget((x, z));
            return Ok(());
        }
        let chunk = {
            let world = shared.world.lock().unwrap();
            decode_legacy_chunk(mask, data, ground_up, None, &world.values)?
        };
        let center = own_chunk(shared);
        shared.world.lock().unwrap().insert(x, z, chunk, center);
        return Ok(());
    }
    if shared.proto.modern {
        let maps = r.var_int()?;
        if !(0..=64).contains(&maps) {
            return Err(crate::protocol::buf::err("Hoehenkarten unplausibel"));
        }
        for _ in 0..maps {
            r.var_int()?;
            let longs = r.var_int()?;
            if !(0..=4096).contains(&longs) {
                return Err(crate::protocol::buf::err("Hoehenkarte zu gross"));
            }
            r.skip(longs as usize * 8)?;
        }
    } else {
        crate::protocol::nbt::read_network(r)?;
    }
    // Ohne Kopie: die Chunkdaten werden unmittelbar aus dem Paketpuffer gelesen.
    let data = r.byte_slice()?;

    // Dekodiert wird ohne Weltsperre: Die Live-Ansicht soll in der Zeit weiterzeichnen.
    let (dimension, preferred, values) = {
        let world = shared.world.lock().unwrap();
        let preferred = world.format.unwrap_or(Format {
            modern_palette: shared.proto.modern,
            fluid_count: shared.proto.section_fluid_count,
            block_biomes: shared.proto.block_biomes,
        });
        (
            world.dimension.clone(),
            preferred,
            Values::for_version(shared.proto.name),
        )
    };
    #[cfg_attr(not(feature = "pov"), allow(unused_mut))]
    let (mut chunk, format) = Format::probe(preferred, &dimension, data, &values)?;

    // Hinter den Blöcken stehen erst die Blockentitäten, dann das Licht. Das Licht ist eine
    // Zugabe: Misslingt es, behält der Chunk seine Blöcke.
    #[cfg(feature = "pov")]
    {
        chunk.light = skip_block_entities(r)
            .and_then(|()| read_light(r, chunk.min_section, chunk.sections.len(), None))
            .ok()
            .map(Arc::new);
    }
    let center = own_chunk(shared);
    let mut world = shared.world.lock().unwrap();
    if world.format != Some(format) {
        if world.format.is_some() || format != preferred {
            shared.console.info(&format!(
                "Chunk-Format erkannt ({} Abschnitte, {}Fluidzaehler).",
                chunk.sections.len(),
                if format.fluid_count { "mit " } else { "ohne " }
            ));
        }
        world.format = Some(format);
    }
    world.insert(x, z, chunk, center);
    Ok(())
}

/// 1.8.9-Sammelpaket: Metadaten aller Spalten stehen vorn, ihre rohen Daten lückenlos dahinter.
fn read_chunk_bulk(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    if !shared.proto.legacy {
        return Ok(());
    }
    let sky_light = r.bool()?;
    let count = r.var_int()?;
    if !(0..=1024).contains(&count) {
        return Err(crate::protocol::buf::err("Chunk-Sammelpaket unplausibel"));
    }
    let mut columns = Vec::with_capacity(count as usize);
    for _ in 0..count {
        columns.push((r.i32()?, r.i32()?, r.u16()?));
    }
    let values = Values::for_version(shared.proto.name);
    let center = own_chunk(shared);
    for (x, z, mask) in columns {
        let sections = mask.count_ones() as usize;
        let size = sections
            * (SECTION_BLOCKS * 2
                + LEGACY_LIGHT_BYTES
                + if sky_light { LEGACY_LIGHT_BYTES } else { 0 })
            + 256;
        let data = r.bytes(size)?;
        let chunk = decode_legacy_chunk(mask, data, true, Some(sky_light), &values)?;
        shared.world.lock().unwrap().insert(x, z, chunk, center);
    }
    Ok(())
}

/// Licht je Abschnitt in 1.8.9: ein Nibble je Block.
const LEGACY_LIGHT_BYTES: usize = SECTION_BLOCKS / 2;

/// Einen alten Chunk in dieselbe kompakte interne Darstellung wie moderne Paletten überführen.
fn decode_legacy_chunk(
    mask: u16,
    data: &[u8],
    ground_up: bool,
    sky_hint: Option<bool>,
    values: &Values,
) -> io::Result<Chunk> {
    let included = mask.count_ones() as usize;
    let block_bytes = included * SECTION_BLOCKS * 2;
    let light_bytes = included * LEGACY_LIGHT_BYTES;
    let biome_bytes = if ground_up { 256 } else { 0 };
    let without_sky = block_bytes + light_bytes + biome_bytes;
    let with_sky = without_sky + light_bytes;
    let has_sky = sky_hint.unwrap_or(data.len() == with_sky);
    let expected = if has_sky { with_sky } else { without_sky };
    if data.len() != expected {
        return Err(crate::protocol::buf::err(
            "alte Chunkdaten haben falsche Länge",
        ));
    }

    let mut sections: Vec<Option<Arc<Section>>> = vec![None; 16];
    let mut raw_offset = 0usize;
    #[cfg(feature = "pov")]
    let mut included_indices = Vec::with_capacity(included);
    for (section_index, section) in sections.iter_mut().enumerate() {
        if mask & (1 << section_index) == 0 {
            continue;
        }
        #[cfg(feature = "pov")]
        included_indices.push(section_index);
        let raw = &data[raw_offset..raw_offset + SECTION_BLOCKS * 2];
        raw_offset += SECTION_BLOCKS * 2;
        *section = legacy_section(raw, values).map(Arc::new);
    }

    #[cfg(feature = "pov")]
    {
        let block_start = block_bytes;
        let sky_start = block_start + light_bytes;
        let biome_start = sky_start + if has_sky { light_bytes } else { 0 };
        if ground_up {
            let biomes = &data[biome_start..biome_start + 256];
            for section in sections.iter_mut().flatten() {
                let section = Arc::make_mut(section);
                let mut cells = [0u16; SECTION_BIOMES];
                for cell_y in 0..4usize {
                    for cell_z in 0..4usize {
                        for cell_x in 0..4usize {
                            let column = (cell_z * 4 * 16) + cell_x * 4;
                            cells[(cell_y << 4) | (cell_z << 2) | cell_x] = biomes[column] as u16;
                        }
                    }
                }
                section.biomes = Biomes::Cells(Box::new(cells));
            }
        }
        let mut block_light = vec![LightSection::Uniform(0); 16];
        let mut sky_light = vec![LightSection::Uniform(if has_sky { 15 } else { 0 }); 16];
        for (slot, section_index) in included_indices.into_iter().enumerate() {
            let at = block_start + slot * LIGHT_BYTES;
            block_light[section_index] = compact_light(&data[at..at + LIGHT_BYTES]);
            if has_sky {
                let at = sky_start + slot * LIGHT_BYTES;
                sky_light[section_index] = compact_light(&data[at..at + LIGHT_BYTES]);
            }
        }
        Ok(Chunk {
            min_section: 0,
            sections,
            light: Some(Arc::new(Light {
                min_section: 0,
                sky: sky_light.into_boxed_slice(),
                block: block_light.into_boxed_slice(),
            })),
        })
    }
    #[cfg(not(feature = "pov"))]
    {
        let _ = has_sky;
        Ok(Chunk {
            min_section: 0,
            sections,
        })
    }
}

fn legacy_section(raw: &[u8], values: &Values) -> Option<Section> {
    let mut palette = vec![0u32];
    let mut by_value: HashMap<u32, usize> = HashMap::new();
    let mut slots = Vec::with_capacity(SECTION_BLOCKS);
    for bytes in raw.as_chunks::<2>().0 {
        let state = u16::from_le_bytes([bytes[0], bytes[1]]) as u32;
        let value = values.store(state);
        if state == 0 || values.is_air_value(value) {
            slots.push(0);
            continue;
        }
        let slot = *by_value.entry(value).or_insert_with(|| {
            palette.push(value);
            palette.len() - 1
        });
        slots.push(slot);
    }
    if palette.len() == 1 {
        return None;
    }
    Some(Section::from_slots(palette, slots.into_iter()))
}

/// Nachgereichtes Licht zu einem Chunk, den wir schon haben.
#[cfg(feature = "pov")]
fn read_light_update(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    let x = r.var_int()?;
    let z = r.var_int()?;
    let mut world = shared.world.lock().unwrap();
    // Nur für bereits bekannte Chunks: Die Abschnittszahl steht im Chunk, nicht im Lichtpaket.
    let Some(chunk) = world.chunks.get(&(x, z)) else {
        return Ok(());
    };
    let (min_section, sections, previous) = (
        chunk.min_section,
        chunk.sections.len(),
        chunk.light.as_ref().map(Arc::clone),
    );
    let light = Arc::new(read_light(r, min_section, sections, previous.as_deref())?);
    Arc::make_mut(world.chunks.get_mut(&(x, z)).unwrap()).light = Some(light);
    world.touch();
    Ok(())
}

fn read_section_blocks(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    let mut changes = Vec::new();
    if shared.proto.legacy {
        let chunk_x = r.i32()?;
        let chunk_z = r.i32()?;
        let count = r.var_int()?;
        if !(0..=SECTION_BLOCKS as i32).contains(&count) {
            return Err(crate::protocol::buf::err(
                "alte Blockänderungen unplausibel",
            ));
        }
        for _ in 0..count {
            let horizontal = r.u8()?;
            let y = r.u8()? as i32;
            let state = r.var_int()? as u32;
            let x = chunk_x * 16 + (horizontal >> 4) as i32;
            let z = chunk_z * 16 + (horizontal & 15) as i32;
            changes.push((x, y, z, state));
        }
    } else {
        let (section_x, section_y, section_z) = unpack_section_pos(r.i64()?);
        let count = r.var_int()?;
        if !(0..=SECTION_BLOCKS as i32).contains(&count) {
            return Err(crate::protocol::buf::err("Blockaenderungen unplausibel"));
        }
        for _ in 0..count {
            let value = r.var_long()? as u64;
            let local = value & 0xFFF;
            let state = (value >> 12) as u32;
            let x = section_x * 16 + ((local >> 8) & 15) as i32;
            let z = section_z * 16 + ((local >> 4) & 15) as i32;
            let y = section_y * 16 + (local & 15) as i32;
            changes.push((x, y, z, state));
        }
    }
    let mut world = shared.world.lock().unwrap();
    for (x, y, z, state) in changes {
        world.set_block(x, y, z, state);
    }
    Ok(())
}

// ===================== Palette und Koordinaten =====================

/// Eine gelesene Palette samt Häufigkeiten. Die Häufigkeiten fallen beim Auspacken ohnehin an –
/// sie noch einmal über eine Hashtabelle zu zählen, wäre je Abschnitt ein zweiter Durchlauf.
pub(crate) struct RawPalette {
    values: Vec<u32>,
    indices: Vec<u16>,
    counts: Vec<u32>,
}

/// Kopf einer Palette lesen: Bitbreite und (bei indirekten Paletten) die Einträge.
/// Rückgabe: (Bitbreite, Einträge oder `None` für die globale Palette).
fn palette_head(r: &mut Reader, max_indirect_bits: u8) -> io::Result<(u8, Option<Vec<u32>>)> {
    let bits = r.u8()?;
    if bits > 32 {
        return Err(crate::protocol::buf::err("POV: Palette breiter als 32 Bit"));
    }
    if bits == 0 || bits <= max_indirect_bits {
        let count = if bits == 0 { 1 } else { r.var_int()? };
        if !(1..=4096).contains(&count) {
            return Err(crate::protocol::buf::err("POV: lokale Palette unplausibel"));
        }
        let mut values = Vec::with_capacity(count as usize);
        for _ in 0..count {
            values.push(r.var_int()? as u32);
        }
        return Ok((bits, Some(values)));
    }
    Ok((bits, None))
}

/// Zahl der Longs hinter der Palette – und in den alten Protokollen die Prüfung der Längenangabe.
fn palette_longs(r: &mut Reader, size: usize, bits: u8, modern: bool) -> io::Result<usize> {
    let per_long = 64usize / bits as usize;
    if per_long == 0 {
        return Err(crate::protocol::buf::err("POV: ungueltige Palettenbreite"));
    }
    let expected = size.div_ceil(per_long);
    if modern {
        return Ok(expected);
    }
    let encoded = r.var_int()?;
    if encoded < 0 || encoded as usize != expected {
        return Err(crate::protocol::buf::err("POV: falsche Palettenlaenge"));
    }
    Ok(expected)
}

pub(crate) fn read_palette(
    r: &mut Reader,
    size: usize,
    max_indirect_bits: u8,
    modern: bool,
) -> io::Result<RawPalette> {
    let (bits, local) = palette_head(r, max_indirect_bits)?;

    if bits == 0 {
        let values = local.unwrap_or_else(|| vec![0]);
        if !modern {
            let longs = r.var_int()?;
            if longs != 0 {
                return Err(crate::protocol::buf::err(
                    "POV: Singleton-Palette mit Daten",
                ));
            }
        }
        return Ok(RawPalette {
            counts: vec![size as u32],
            values,
            indices: vec![0; size],
        });
    }

    let per_long = 64usize / bits as usize;
    let longs = palette_longs(r, size, bits, modern)?;
    let mut data = Vec::with_capacity(longs);
    for _ in 0..longs {
        data.push(r.i64()? as u64);
    }

    let mask = if bits == 32 {
        u32::MAX as u64
    } else {
        (1u64 << bits) - 1
    };

    // Indirekte Palette: der gepackte Wert ist direkt der Paletteneintrag.
    if let Some(values) = local {
        let mut counts = vec![0u32; values.len()];
        let mut indices = Vec::with_capacity(size);
        let mut fault = None;
        unpack_all(&data, size, per_long, bits, mask, |packed| {
            match counts.get_mut(packed) {
                Some(count) => *count += 1,
                None => fault = Some("POV: Palettenindex ausserhalb"),
            }
            indices.push(packed as u16);
        })?;
        if let Some(message) = fault {
            return Err(crate::protocol::buf::err(message));
        }
        return Ok(RawPalette {
            values,
            indices,
            counts,
        });
    }

    // Globale Palette: eine örtliche Palette daraus bauen, damit der Abschnitt klein bleibt.
    let mut values: Vec<u32> = Vec::new();
    let mut counts: Vec<u32> = Vec::new();
    let mut seen: HashMap<u32, u16> = HashMap::new();
    let mut indices = Vec::with_capacity(size);
    let mut fault = None;
    unpack_all(&data, size, per_long, bits, mask, |packed| {
        let state = packed as u32;
        let slot = match seen.get(&state) {
            Some(slot) => *slot,
            None => {
                if values.len() >= u16::MAX as usize {
                    fault = Some("POV: Abschnitt mit zu vielen Zustaenden");
                    return;
                }
                let slot = values.len() as u16;
                values.push(state);
                counts.push(0);
                seen.insert(state, slot);
                slot
            }
        };
        counts[slot as usize] += 1;
        indices.push(slot);
    })?;
    if let Some(message) = fault {
        return Err(crate::protocol::buf::err(message));
    }
    Ok(RawPalette {
        values,
        indices,
        counts,
    })
}

/// Palette nur überspringen – für die Biome, die die Ansicht nicht braucht.
fn skip_palette(
    r: &mut Reader,
    size: usize,
    max_indirect_bits: u8,
    modern: bool,
) -> io::Result<()> {
    let (bits, _) = palette_head(r, max_indirect_bits)?;
    if bits == 0 {
        if !modern {
            let longs = r.var_int()?;
            if longs != 0 {
                return Err(crate::protocol::buf::err(
                    "POV: Singleton-Palette mit Daten",
                ));
            }
        }
        return Ok(());
    }
    let longs = palette_longs(r, size, bits, modern)?;
    r.skip(longs * 8)
}

/// Alle gepackten Einträge der Reihe nach an `f` geben.
///
/// Bewusst über die Longs statt über die Blocknummern: Vorher kostete **jeder einzelne der 4096
/// Blöcke** eine Division, eine Modulo-Rechnung, einen Bereichstest und eine variable
/// Schiebeoperation. Jetzt wird ein Long einmal geholt und dann Eintrag für Eintrag
/// herausgeschoben – bei rund 170 Chunks mit je einem guten Dutzend Abschnitten sind das ein paar
/// Millionen eingesparte Divisionen pro Beitritt.
#[inline]
fn unpack_all(
    data: &[u64],
    size: usize,
    per_long: usize,
    bits: u8,
    mask: u64,
    mut f: impl FnMut(usize),
) -> io::Result<()> {
    let mut left = size;
    for long in data {
        if left == 0 {
            break;
        }
        let mut value = *long;
        let take = per_long.min(left);
        for _ in 0..take {
            f((value & mask) as usize);
            value >>= bits;
        }
        left -= take;
    }
    if left > 0 {
        return Err(crate::protocol::buf::err("POV: Palettendaten zu kurz"));
    }
    Ok(())
}

/// Aus der gelesenen Palette einen Abschnitt bauen: Luftzustände auf Eintrag 0 zusammenlegen,
/// alles Übrige dahinter. `None` = der Abschnitt besteht doch nur aus Luft.
///
/// Welche Zustände Luft sind, verrät der Blockzähler des Abschnitts: er zählt alle Blöcke, die
/// **nicht** Luft sind, und Luft gibt es in Vanilla in genau drei Ausprägungen (`air`,
/// `cave_air`, `void_air`). Gesucht wird deshalb eine Teilmenge von höchstens drei Einträgen,
/// deren Häufigkeiten die fehlende Zahl genau ergeben – das geht in einem Durchlauf.
pub(crate) fn compact(raw: RawPalette, non_air: usize, values: &Values) -> Option<Section> {
    let RawPalette {
        values: raw_values,
        indices,
        counts,
    } = raw;
    let air = air_entries(&raw_values, &counts, SECTION_BLOCKS.saturating_sub(non_air));

    // Neue Palette: Eintrag 0 ist Luft. Gleiche gespeicherte Werte (in der Standard-Bauform:
    // gleiche Bewegungsklassen) werden zusammengelegt.
    let mut palette = vec![0u32];
    let mut remap = vec![0usize; raw_values.len()];
    for (slot, state) in raw_values.iter().enumerate() {
        if air[slot] {
            continue;
        }
        let value = values.store(*state);
        if values.is_air_value(value) {
            continue;
        }
        remap[slot] = match palette.iter().position(|known| *known == value) {
            Some(existing) => existing,
            None => {
                palette.push(value);
                palette.len() - 1
            }
        };
    }
    if palette.len() == 1 {
        return None; // doch nur Luft
    }
    Some(Section::from_slots(
        palette,
        indices.iter().map(|slot| remap[*slot as usize]),
    ))
}

/// Welche Paletteneinträge sind Luft? `air_total` ist die Zahl der Luftblöcke im Abschnitt.
pub(crate) fn air_entries(values: &[u32], counts: &[u32], air_total: usize) -> Vec<bool> {
    let mut air = vec![false; values.len()];
    if air_total == 0 {
        return air;
    }
    // Zustand 0 ist in jeder Protokollversion `minecraft:air`.
    let mut rest = air_total as i64;
    for (slot, value) in values.iter().enumerate() {
        if *value == 0 {
            air[slot] = true;
            rest -= counts[slot] as i64;
        }
    }
    if rest <= 0 {
        return air;
    }
    let rest = rest as u32;

    // Ein einzelner weiterer Zustand (der Normalfall: cave_air).
    let open: Vec<usize> = (0..values.len()).filter(|slot| !air[*slot]).collect();
    if let Some(slot) = open.iter().find(|slot| counts[**slot] == rest) {
        air[*slot] = true;
        return air;
    }
    // Sonst zwei – mehr als drei Luftzustände gibt es in Minecraft nicht.
    let mut by_count: HashMap<u32, usize> = HashMap::with_capacity(open.len());
    for slot in &open {
        by_count.entry(counts[*slot]).or_insert(*slot);
    }
    for slot in &open {
        let Some(missing) = rest.checked_sub(counts[*slot]) else {
            continue;
        };
        if let Some(other) = by_count.get(&missing) {
            if other != slot {
                air[*slot] = true;
                air[*other] = true;
                return air;
            }
        }
    }
    // Nichts Eindeutiges gefunden: lieber alles stehen lassen, als Blöcke zu erfinden.
    air
}

pub(crate) fn unpack_block_pos(value: i64) -> (i32, i32, i32) {
    let x = (value >> 38) as i32;
    let y = (value << 52 >> 52) as i32;
    let z = (value << 26 >> 38) as i32;
    (x, y, z)
}

/// Protokoll 47 legte die 12 Y-Bits noch zwischen X und Z; moderne Pakete legen sie ans Ende.
pub(crate) fn unpack_legacy_block_pos(value: i64) -> (i32, i32, i32) {
    let x = (value >> 38) as i32;
    let y = (value << 26 >> 52) as i32;
    let z = (value << 38 >> 38) as i32;
    (x, y, z)
}

pub(crate) fn unpack_chunk_pos(value: i64) -> (i32, i32) {
    let value = value as u64;
    (value as u32 as i32, (value >> 32) as u32 as i32)
}

fn unpack_section_pos(value: i64) -> (i32, i32, i32) {
    let x = (value >> 42) as i32;
    let y = (value << 44 >> 44) as i32;
    let z = (value << 22 >> 42) as i32;
    (x, y, z)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn einheitlicher_abschnitt_kostet_keine_daten_und_bleibt_aenderbar() {
        let mut section = Section::from_slots(vec![0, 7], std::iter::repeat_n(1, SECTION_BLOCKS));
        assert!(section.data.words.is_empty());
        assert_eq!(section.state(4095), 7);

        // Ein einzelner Luftblock macht ihn wieder gepackt, der Rest bleibt erhalten.
        section.set(5, 0);
        assert_eq!(section.state(5), 0);
        assert_eq!(section.state(6), 7);
        section.set(9, 3);
        assert_eq!((section.state(9), section.state(10)), (3, 7));

        // Unvollständige Eingabe: Fehlendes ist Luft, nicht der erste Wert.
        let short = Section::from_slots(vec![0, 7], std::iter::repeat_n(1, 10));
        assert_eq!((short.state(9), short.state(10)), (7, 0));
        assert_eq!(Section::empty().state(0), 0);
    }
}
