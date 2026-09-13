//! Echte Live-POV – nur mit `--features pov` (eigene POV-Bauform und Ultra).
//!
//! Der Server schickt einem Vanilla-Client keine fertigen Bilder, sondern Chunk-Abschnitte mit
//! kompakten Blockzustands-Paletten sowie einzelne Block- und Entitätsänderungen. Dieses Modul
//! hält genau diese sichtbare Umgebung im Speicher und zeichnet daraus eine First-Person-Ansicht.
//! Der neue Browser-Weg liest originale Blockmodelle, PNGs und GUI-Texturen aus der passenden
//! Client-JAR; die bisherige farbige Terminalansicht bleibt als kompatibler Fallback. Blickrichtung
//! und Kameraposition kommen unmittelbar aus dem eigenen Positionszustand; die Ansicht folgt daher
//! `:look`, Bewegung und Server-Teleports live.
//!
//! Paketformate sind nicht geschätzt. Die drei tatsächlich verschiedenen Codepfade entsprechen
//! den mitgelieferten MCProtocolLib-Codecs:
//! * 1.21.1: NBT-Höhenkarten, Long-Array-Länge in jeder Palette, ein Blockzähler je Abschnitt.
//! * 1.21.11: Höhenkarten-Liste, fest berechnete Palettenlänge, ein Blockzähler.
//! * 26.1/26.2: wie 1.21.11, zusätzlich ein Fluidzähler je Abschnitt.
//!
//! Bricht ein Chunk trotzdem, wird nicht geraten und auch nicht aufgegeben: [`Format::probe`]
//! probiert die wenigen möglichen Spielarten durch und merkt sich die, die den Puffer restlos
//! aufgeht. Ein Protokollupdate kostet damit höchstens den ersten Chunk.
//!
//! **Speicher.** Ein Abschnitt liegt als Palette plus ein Index je Block (ein Byte, solange die
//! Palette klein bleibt) im Speicher – nicht als 4096 volle Zustands-IDs. Reine Luftabschnitte
//! werden gar nicht erst behalten. Zusammen mit der kleinen angeforderten Sichtweite bleibt die
//! Live-Ansicht damit im einstelligen MB-Bereich statt im dreistelligen.
//!
//! **Terminal-Ausgabeformat.** Ein Bild besteht aus einer Kopfzeile `POV x=… (:pov stop)` und danach je
//! Terminalzeile einer Reihe Halbblöcke `▀` mit Vorder- und Hintergrundfarbe (ein Zeichen = zwei
//! Bildpunkte). Ohne Farbe kommt stattdessen eine Helligkeitsrampe, eine Zeichenzeile je
//! Bildzeile. Das Format ist Teil der Schnittstelle nach außen – ein Panel liest es mit –, es
//! wird deshalb nicht ohne Not geändert.

use crate::buf::Reader;
use crate::client::Shared;
use crate::nbt::Nbt;
use crate::options::Options;
use crate::pov_assets::Assets;
use crate::pov_assets::BiomeTint;
use crate::proto::In;

use std::collections::HashMap;
use std::f64::consts::PI;
use std::fmt::Write as _;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

const SECTION_BLOCKS: usize = 16 * 16 * 16;
const SECTION_BIOMES: usize = 4 * 4 * 4;
const MAX_ENTITIES: usize = 4096;
const MAX_SECTIONS: usize = 256;
/// Weiter entfernte Chunks kann die Ansicht nie sehen: [`MAX_DISTANCE`] sind 72 Blöcke, ein Strahl
/// erreicht damit höchstens den fünften Chunk in jede Richtung. Alles darüber wird verworfen,
/// sobald ein neuer Chunk ankommt – ohne das lassen Server, die nie ein `ForgetChunk` schicken,
/// den Speicher unbegrenzt wachsen.
const KEEP_CHUNK_RADIUS: i32 = 6;
/// Notbremse für die Zeit, in der noch nicht aufgeräumt werden kann (der Server schickt Chunks,
/// bevor er die erste Position schickt).
///
/// Abgeleitet aus [`KEEP_CHUNK_RADIUS`] statt frei gegriffen: Aufgeräumt wird auf 13x13 Chunks,
/// mehr als zwei Chunks Anlauf darüber hinaus kann keine Ansicht je brauchen. Vorher stand hier
/// eine glatte 1024 – rund das Sechsfache, und ein Chunk mit den üblichen acht gefüllten
/// Abschnitten wiegt gut 35 KB. Aus höchstens 40 MB werden damit höchstens 11.
const MAX_CHUNKS: usize = (2 * (KEEP_CHUNK_RADIUS as usize + 2) + 1).pow(2);
const HORIZONTAL_FOV: f64 = 90.0;
const MAX_DISTANCE: f64 = 72.0;
const DEFAULT_WIDTH: usize = 64;
/// Pixelhöhe; ANSI stellt je Terminalzeile zwei Pixel mit `▀` dar.
const DEFAULT_HEIGHT: usize = 32;
const MIN_WIDTH: usize = 24;
const MAX_WIDTH: usize = 160;
const MIN_HEIGHT: usize = 12;
const MAX_HEIGHT: usize = 80;
/// Bilder je Sekunde ohne `--pov-fps`.
const DEFAULT_FPS: usize = 8;
/// Auch ein unverändertes Bild wird spätestens so oft wiederholt – ein Programm davor soll an
/// der Stille nicht ablesen, die Ansicht sei tot.
const REPEAT_UNCHANGED: Duration = Duration::from_secs(2);
/// Nahezu gleichzeitige Browser-Anfragen teilen sich denselben teuren Raycast und PNG-Encode.
/// Der normale Viewer fragt nur alle 200 ms ab; 150 ms Cachezeit veraendern dessen Takt daher
/// nicht, verhindern aber, dass mehrere Tabs denselben Weltstand parallel neu zeichnen.
const WEB_FRAME_CACHE: Duration = Duration::from_millis(150);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dimension {
    name: String,
    min_y: i32,
    height: i32,
}

impl Dimension {
    fn legacy(id: i32) -> Dimension {
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

    fn for_world(name: &str) -> Dimension {
        // Vanilla-Fallback, falls ein ungewöhnlicher Proxy die Registry nicht weiterreicht.
        let overworld = name.ends_with("overworld") || name == "minecraft:dimension_type";
        Dimension {
            name: name.to_string(),
            min_y: if overworld { -64 } else { 0 },
            height: if overworld { 384 } else { 256 },
        }
    }

    fn section_count(&self) -> usize {
        (self.height / 16).clamp(1, MAX_SECTIONS as i32) as usize
    }
}

// ===================== Abschnitte =====================

/// Ein Index je Block. Fast jeder Abschnitt kommt mit höchstens 256 verschiedenen Zuständen aus –
/// dann kostet er 4 KB statt 8 KB.
#[derive(Clone)]
enum Indices {
    Small(Box<[u8]>),
    Large(Box<[u16]>),
}

impl Indices {
    /// Aus den rohen Paletten-Steckplätzen einen Abschnitt bauen und dabei gleich umnummerieren.
    ///
    /// Bewusst in einem Durchgang: vorher entstand erst ein voller Zwischenvektor mit 4096
    /// `u16`-Einträgen und daraus dann der endgültige. Das waren je Chunk-Abschnitt 8 KB, die
    /// unmittelbar wieder weggeworfen wurden – beim Beitritt mit gut 170 Chunks also mehrere
    /// Dutzend Megabyte reine Verwaltungsarbeit.
    fn remapped(slots: &[u16], remap: &[u16], palette_len: usize) -> Indices {
        if palette_len <= u8::MAX as usize + 1 {
            Indices::Small(slots.iter().map(|s| remap[*s as usize] as u8).collect())
        } else {
            Indices::Large(slots.iter().map(|s| remap[*s as usize]).collect())
        }
    }

    fn get(&self, index: usize) -> u16 {
        match self {
            Indices::Small(data) => data[index] as u16,
            Indices::Large(data) => data[index],
        }
    }

    fn set(&mut self, index: usize, value: u16) {
        match self {
            Indices::Small(data) => data[index] = value as u8,
            Indices::Large(data) => data[index] = value,
        }
    }

    /// Auf die breite Darstellung umstellen, sobald die Palette über 256 Einträge wächst.
    fn widen(&mut self) {
        if let Indices::Small(data) = self {
            *self = Indices::Large(data.iter().map(|v| *v as u16).collect());
        }
    }
}

/// Ein Chunk-Abschnitt: Palette plus ein Index je Block. Eintrag 0 ist immer Luft, damit die
/// Sichtprüfung ein einziger Vergleich ist.
#[derive(Clone)]
struct Section {
    palette: Vec<u32>,
    indices: Indices,
    /// Biom je 4x4x4-Zelle – daraus kommt die Einfärbung von Gras, Laub und Wasser.
    biomes: Biomes,
}

impl Section {
    fn state(&self, index: usize) -> u32 {
        let slot = self.indices.get(index) as usize;
        self.palette.get(slot).copied().unwrap_or(0)
    }

    fn is_air(&self, index: usize) -> bool {
        self.indices.get(index) == 0
    }

    fn set(&mut self, index: usize, state: u32) {
        if state == 0 {
            return self.indices.set(index, 0);
        }
        let slot = match self.palette.iter().position(|value| *value == state) {
            Some(slot) => slot,
            None => {
                if self.palette.len() >= u16::MAX as usize {
                    return; // absurd bunter Abschnitt: der letzte Zustand bleibt eben stehen
                }
                self.palette.push(state);
                if self.palette.len() > u8::MAX as usize + 1 {
                    self.indices.widen();
                }
                self.palette.len() - 1
            }
        };
        self.indices.set(index, slot as u16);
    }

    fn empty() -> Section {
        Section {
            palette: vec![0],
            indices: Indices::Small(vec![0u8; SECTION_BLOCKS].into_boxed_slice()),
            // Ein Abschnitt, der erst durch eine Blockänderung entsteht, war vorher reine Luft –
            // und für die hat der Server nie eine Biompalette mitgeschickt. Er bleibt deshalb
            // ohne Biom, statt eines zu erfinden.
            biomes: Biomes::Single(0),
        }
    }
}

// ===================== Chunks =====================

/// Welche Spielart des Abschnittsformats der Server spricht. Wird einmal ermittelt und dann
/// behalten – siehe [`Format::probe`].
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
struct Format {
    /// Palettendaten ohne Längenangabe (ab 1.21.11).
    modern_palette: bool,
    /// Zusätzlicher Flüssigkeitszähler je Abschnitt (ab 26.1).
    fluid_count: bool,
}

impl Format {
    fn code(self) -> u32 {
        (self.modern_palette as u32) | ((self.fluid_count as u32) << 1)
    }

    fn from_code(code: u32) -> Format {
        Format {
            modern_palette: code & 1 != 0,
            fluid_count: code & 2 != 0,
        }
    }

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
            },
        ]
    }

    /// Chunk lesen und dabei die passende Spielart bestimmen. Bevorzugt wird die, die den Puffer
    /// restlos aufgeht **und** die erwartete Zahl Abschnitte liefert.
    fn probe(preferred: Format, dimension: &Dimension, data: &[u8]) -> io::Result<(Chunk, Format)> {
        let mut first_error = None;
        let mut fallback: Option<(Chunk, Format)> = None;
        for format in Format::candidates(preferred) {
            match Chunk::decode(format, dimension, data) {
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
            first_error.unwrap_or_else(|| crate::buf::err("POV: Chunk nicht lesbar"))
        })
    }
}

/// Ein Chunk ist billig zu klonen: die Abschnitte hängen einzeln an einem `Arc`, eine
/// Blockänderung kopiert deshalb nur den einen betroffenen Abschnitt.
#[derive(Clone)]
struct Chunk {
    min_section: i32,
    /// `None` = reine Luft; solche Abschnitte belegen keinen Speicher.
    sections: Vec<Option<Arc<Section>>>,
    /// Himmels- und Blocklicht, sofern der Server sie mitgeschickt hat. `None` heißt: keine
    /// Angabe – dann bleibt es bei der geschätzten Flächenhelligkeit von früher.
    ///
    /// Hinter einem eigenen `Arc`, damit eine Blockänderung (die das Licht nicht anfasst) beim
    /// Kopieren des Chunks nur einen Zähler erhöht statt das ganze Lichtfeld mitzunehmen.
    light: Option<Arc<Light>>,
}

impl Chunk {
    /// Abschnitte lesen, bis der Puffer leer ist. Bewusst nicht „genau `section_count` Stück":
    /// meldet ein Server eine andere Welthöhe, als seine Registry sagt, folgt die Ansicht dem,
    /// was wirklich im Paket steht, statt jeden Chunk zu verwerfen.
    fn decode(format: Format, dimension: &Dimension, data: &[u8]) -> io::Result<Chunk> {
        let mut r = Reader::new(data);
        let mut sections = Vec::with_capacity(dimension.section_count());
        while r.remaining() > 0 {
            if sections.len() >= MAX_SECTIONS {
                return Err(crate::buf::err("POV: zu viele Chunk-Abschnitte"));
            }
            sections.push(read_section(&mut r, format)?);
        }
        if sections.is_empty() {
            return Err(crate::buf::err("POV: Chunk ohne Abschnitte"));
        }
        Ok(Chunk {
            min_section: dimension.min_y.div_euclid(16),
            sections,
            light: None,
        })
    }

    fn section_index(&self, y: i32) -> Option<usize> {
        usize::try_from(y.div_euclid(16) - self.min_section)
            .ok()
            .filter(|index| *index < self.sections.len())
    }

    /// Der Abschnitt zu dieser Höhe – `None` heißt „außerhalb der Welt oder reine Luft".
    fn section(&self, y: i32) -> Option<&Section> {
        self.sections[self.section_index(y)?].as_deref()
    }

    /// Direkter Blockzugriff ohne jeden Merker. Im laufenden Betrieb geht alles über [`Cursor`];
    /// diese beiden Wege bleiben als **Vergleichsmaßstab** für die Tests stehen – nur so lässt
    /// sich zeigen, dass der Merker wirklich nichts am Ergebnis ändert.
    #[cfg(test)]
    fn block(&self, x: i32, y: i32, z: i32) -> u32 {
        match self.section(y) {
            Some(section) => section.state(local_index(x, y, z)),
            None => 0,
        }
    }

    /// `true`, wenn an der Stelle etwas Sichtbares steht. Billiger als [`Chunk::block`], weil die
    /// Palette dafür gar nicht angefasst werden muss.
    #[cfg(test)]
    fn solid(&self, x: i32, y: i32, z: i32) -> bool {
        self.section(y)
            .is_some_and(|section| !section.is_air(local_index(x, y, z)))
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
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
fn local_index(x: i32, y: i32, z: i32) -> usize {
    ((y.rem_euclid(16) as usize) << 8)
        | ((z.rem_euclid(16) as usize) << 4)
        | x.rem_euclid(16) as usize
}

/// Einen Abschnitt lesen. `None` = reine Luft.
fn read_section(r: &mut Reader, format: Format) -> io::Result<Option<Arc<Section>>> {
    let block_count = r.i16()?;
    if !(0..=SECTION_BLOCKS as i16).contains(&block_count) {
        return Err(crate::buf::err("POV: Blockzaehler unplausibel"));
    }
    if format.fluid_count {
        let fluid = r.i16()?;
        if !(0..=SECTION_BLOCKS as i16).contains(&fluid) {
            return Err(crate::buf::err("POV: Fluidzaehler unplausibel"));
        }
    }

    // Ein Abschnitt ohne einen einzigen Nicht-Luft-Block wird ohnehin verworfen – dann braucht
    // seine Palette gar nicht erst ausgepackt zu werden, sie muss nur exakt übersprungen werden.
    // In einer gewachsenen Überwelt sind das zwei Drittel aller Abschnitte, und jeder von ihnen
    // hat bisher 4096 Indizes ausgepackt und sofort wieder weggeworfen.
    if block_count == 0 {
        skip_palette(r, SECTION_BLOCKS, 8, format.modern_palette)?;
        skip_palette(r, SECTION_BIOMES, 3, format.modern_palette)?;
        return Ok(None);
    }

    let palette = read_palette(r, SECTION_BLOCKS, 8, format.modern_palette)?;
    // Biome stehen im selben Abschnitt, nur gröber aufgelöst: eine Nummer je 4x4x4 Zelle.
    // Aus ihr kommt die Einfärbung von Gras, Laub und Wasser (siehe [`crate::pov_assets`]).
    let biomes = read_biomes(r, format.modern_palette)?;

    Ok(compact(palette, block_count as usize).map(|mut section| {
        section.biomes = biomes;
        Arc::new(section)
    }))
}

/// Was ein Biom zur Einfärbung beisteuert – genau die Felder, die der Server in seiner Registry
/// mitschickt.
///
/// Gras- und Laubfarbe stehen dort nur in Ausnahmefällen ausdrücklich drin. Im Regelfall ergeben
/// sie sich aus Temperatur und Niederschlag über die Farbkarten der Original-JAR – so, wie es
/// auch das Spiel selbst rechnet. Wasser dagegen ist je Biom ein fester Wert.
#[derive(Clone, Copy)]
pub(crate) struct BiomeParams {
    pub temperature: f32,
    pub downfall: f32,
    pub grass: Option<u32>,
    pub foliage: Option<u32>,
    pub water: u32,
    pub modifier: GrassModifier,
}

/// Vanillas `GrassColorModifier`: Zwei Biome färben ihr Gras abweichend von der Farbkarte.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum GrassModifier {
    None,
    Swamp,
    DarkForest,
}

impl BiomeParams {
    /// Der Stand ohne jede Registry-Angabe: die Werte, die Vanilla für die Ebene benutzt.
    pub(crate) const PLAINS: BiomeParams = BiomeParams {
        temperature: 0.8,
        downfall: 0.4,
        grass: None,
        foliage: None,
        // `BiomeSpecialEffects` setzt für fast alle Biome genau diesen Wasserton.
        water: 0x3F76E4,
        modifier: GrassModifier::None,
    };

    /// Ein Registry-Eintrag. Fehlt ein Feld, gilt der Vanilla-Vorgabewert – geraten wird nichts.
    pub(crate) fn from_registry(data: Option<&Nbt>) -> BiomeParams {
        let Some(data) = data else {
            return BiomeParams::PLAINS;
        };
        let effects = data.get("effects");
        let color = |key: &str| {
            effects
                .and_then(|tag| tag.get_i32(key))
                .map(|value| value as u32 & 0xFF_FFFF)
        };
        BiomeParams {
            temperature: data.get_f32("temperature").unwrap_or(0.5),
            downfall: data.get_f32("downfall").unwrap_or(0.5),
            grass: color("grass_color"),
            foliage: color("foliage_color"),
            water: color("water_color").unwrap_or(BiomeParams::PLAINS.water),
            modifier: match effects.and_then(|tag| tag.get_str("grass_color_modifier")) {
                Some("swamp") => GrassModifier::Swamp,
                Some("dark_forest") => GrassModifier::DarkForest,
                _ => GrassModifier::None,
            },
        }
    }
}

/// Die Biom-Nummern eines Abschnitts: 4x4x4 Zellen, Reihenfolge wie bei den Blöcken `y, z, x`.
#[derive(Clone)]
enum Biomes {
    /// Der ganze Abschnitt liegt in einem Biom. Das ist weit weg von jeder Biomgrenze der
    /// Normalfall – und dann kostet er zwei Byte statt 128.
    Single(u16),
    Cells(Box<[u16; SECTION_BIOMES]>),
}

impl Biomes {
    #[inline]
    fn get(&self, x: i32, y: i32, z: i32) -> u16 {
        match self {
            Biomes::Single(id) => *id,
            Biomes::Cells(cells) => cells[biome_index(x, y, z)],
        }
    }
}

/// Zellenindex innerhalb eines Abschnitts: je vier Blöcke eine Zelle, Reihenfolge `y, z, x`.
#[inline]
fn biome_index(x: i32, y: i32, z: i32) -> usize {
    ((y.rem_euclid(16) as usize / 4) << 4)
        | ((z.rem_euclid(16) as usize / 4) << 2)
        | (x.rem_euclid(16) as usize / 4)
}

fn read_biomes(r: &mut Reader, modern: bool) -> io::Result<Biomes> {
    let raw = read_palette(r, SECTION_BIOMES, 3, modern)?;
    // Eine einwertige Palette ist der Normalfall; sie hier zusammenzufassen spart den Kasten.
    if raw.values.len() == 1 {
        return Ok(Biomes::Single(clamp_biome(raw.values[0])));
    }
    let mut cells = [0u16; SECTION_BIOMES];
    for (cell, slot) in cells.iter_mut().zip(raw.indices) {
        *cell = clamp_biome(raw.values.get(slot as usize).copied().unwrap_or(0));
    }
    Ok(Biomes::Cells(Box::new(cells)))
}

/// Biom-Nummern sind Registry-Indizes; mehr als 65 535 Biome hat keine Registry. Ein größerer
/// Wert wäre ohnehin in keiner Tabelle zu finden und wird deshalb zu „unbekannt" (0).
#[inline]
fn clamp_biome(value: u32) -> u16 {
    u16::try_from(value).unwrap_or(0)
}

// ===================== Licht =====================

/// 4096 Blöcke zu je einem Nibble – zwei Blöcke teilen sich ein Byte.
const LIGHT_BYTES: usize = SECTION_BLOCKS / 2;
/// So viele Longs fasst ein Bitfeld über alle Lichtabschnitte (zwei mehr als Weltabschnitte).
const MASK_LONGS: usize = (MAX_SECTIONS + 2).div_ceil(64);

/// Das Licht **eines** Chunk-Abschnitts.
#[derive(Clone)]
enum LightSection {
    /// Überall derselbe Wert. Das ist nicht der Sonderfall, sondern der Normalfall: Über Tage
    /// steht der Himmel durchgehend auf 15, tief unten ist alles 0. Ein eingelesenes Feld wird
    /// deshalb zusammengefasst, sobald alle 4096 Nibbles gleich sind – das spart je Abschnitt
    /// 2 KiB und beim Nachschlagen den Speicherzugriff gleich mit.
    Uniform(u8),
    Nibbles(Arc<[u8; LIGHT_BYTES]>),
}

impl LightSection {
    #[inline]
    fn get(&self, index: usize) -> u8 {
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
#[derive(Clone)]
struct Light {
    /// Unterster Lichtabschnitt in Abschnittskoordinaten (ein Abschnitt unter der Welt).
    min_section: i32,
    sky: Box<[LightSection]>,
    block: Box<[LightSection]>,
}

impl Light {
    /// Himmels- und Blocklicht an dieser Stelle, jeweils 0..15.
    #[inline]
    fn get(&self, x: i32, y: i32, z: i32) -> (u8, u8) {
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
#[derive(Default, Clone, Copy)]
struct SectionMask([u64; MASK_LONGS]);

impl SectionMask {
    #[inline]
    fn has(&self, index: usize) -> bool {
        self.0
            .get(index >> 6)
            .is_some_and(|word| (word >> (index & 63)) & 1 == 1)
    }
}

fn read_mask(r: &mut Reader) -> io::Result<SectionMask> {
    let longs = r.var_int()?;
    if !(0..=MASK_LONGS as i32).contains(&longs) {
        return Err(crate::buf::err("POV: Lichtmaske unplausibel"));
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
fn compact_light(data: &[u8]) -> LightSection {
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
fn read_light_arrays(
    r: &mut Reader,
    count: usize,
    present: &SectionMask,
    empty: &SectionMask,
    previous: Option<&[LightSection]>,
) -> io::Result<Box<[LightSection]>> {
    let sent = r.var_int()?;
    if !(0..=count as i32).contains(&sent) {
        return Err(crate::buf::err("POV: Lichtfelder unplausibel"));
    }
    let mut arrays = Vec::with_capacity(sent as usize);
    for _ in 0..sent {
        let data = r.byte_slice()?;
        if data.len() != LIGHT_BYTES {
            return Err(crate::buf::err("POV: Lichtfeld hat die falsche Groesse"));
        }
        arrays.push(compact_light(data));
    }

    let mut out = Vec::with_capacity(count);
    let mut next = 0usize;
    for index in 0..count {
        if present.has(index) && empty.has(index) {
            return Err(crate::buf::err(
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
        return Err(crate::buf::err("POV: Lichtmaske und Felder passen nicht"));
    }
    Ok(out.into_boxed_slice())
}

/// Die Lichtdaten am Ende eines Chunk-Pakets und im `LightUpdate`-Paket.
///
/// Aufbau: vier Bitfelder (welche Abschnitte ein Feld mitbringen und welche bekanntermaßen ganz
/// dunkel sind), danach die Felder selbst.
fn read_light(
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
fn skip_block_entities(r: &mut Reader) -> io::Result<()> {
    let count = r.var_int()?;
    if !(0..=(SECTION_BLOCKS * MAX_SECTIONS) as i32).contains(&count) {
        return Err(crate::buf::err("POV: Blockentitaeten unplausibel"));
    }
    for _ in 0..count {
        r.u8()?; // x und z, je vier Bit
        r.i16()?; // y
        r.var_int()?; // Typ
        crate::nbt::read_network(r)?;
    }
    Ok(())
}

// ===================== Welt =====================

#[derive(Clone, Copy)]
struct Entity {
    x: f64,
    y: f64,
    z: f64,
    player: bool,
}

struct World {
    dimension: Dimension,
    chunks: HashMap<(i32, i32), Arc<Chunk>>,
    entities: HashMap<i32, Entity>,
    own_entity: i32,
    /// Zuletzt bekannter eigener Chunk – daran hängt das Verwerfen weit entfernter Chunks.
    center: (i32, i32),
    /// Zählt jede Änderung an [`World::chunks`]. Der Zeichner vergleicht sie mit seinem Stand und
    /// kopiert die Chunk-Tabelle nur, wenn sich wirklich etwas geändert hat – siehe
    /// [`Pov::fill_scene`].
    revision: u64,
}

impl Default for World {
    fn default() -> Self {
        World {
            dimension: Dimension::for_world("minecraft:overworld"),
            chunks: HashMap::new(),
            entities: HashMap::new(),
            own_entity: -1,
            center: (0, 0),
            // Nicht 0: der Zeichner beginnt bei 0 und muss beim ersten Bild kopieren.
            revision: 1,
        }
    }
}

impl World {
    /// Nach jeder Änderung an den Chunks aufrufen.
    fn touch(&mut self) {
        self.revision = self.revision.wrapping_add(1);
    }

    fn clear_visible(&mut self) {
        self.chunks.clear();
        self.chunks.shrink_to_fit();
        self.entities.clear();
        self.entities.shrink_to_fit();
        self.touch();
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let key = (x.div_euclid(16), z.div_euclid(16));
        if let Some(chunk) = self.chunks.get_mut(&key) {
            Arc::make_mut(chunk).set_block(x, y, z, state);
            self.touch();
        }
    }

    /// Alles wegwerfen, was die Ansicht ohnehin nie erreicht.
    fn prune(&mut self) {
        let (cx, cz) = self.center;
        let before = self.chunks.len();
        self.chunks.retain(|(x, z), _| {
            (x - cx).abs() <= KEEP_CHUNK_RADIUS && (z - cz).abs() <= KEEP_CHUNK_RADIUS
        });
        if self.chunks.len() != before {
            self.touch();
        }
    }
}

#[derive(Clone, Default)]
struct Scene {
    chunks: HashMap<(i32, i32), Arc<Chunk>>,
    entities: Vec<Entity>,
    /// Stand von [`World::revision`], zu dem `chunks` gehört.
    chunk_revision: u64,
    /// Farbton je Biom, fertig ausgerechnet. Leer heißt: keine Registry – dann färbt die
    /// Ansicht wie früher mit dem Ton der gemäßigten Ebene.
    biome_tints: Arc<[BiomeTint]>,
}

/// Wiederverwendete Puffer des Zeichners.
///
/// Ohne sie legt **jedes** Bild die Szene, den Bildpunktpuffer und die Ausgabezeichenkette neu
/// an: bei 160x80 und acht Bildern je Sekunde sind das mehrere Megabyte Allokation je Sekunde,
/// die sofort wieder weggeworfen werden. Die Sperre ist praktisch immer frei – hier arbeitet nur
/// der Zeichen-Thread, und `:pov frame` fällt einmal dazwischen.
#[derive(Default)]
struct Scratch {
    scene: Scene,
    pixels: Vec<Pixel>,
    frame: String,
    web_rgba: Vec<u8>,
    web_png: Option<Arc<[u8]>>,
    web_size: (usize, usize),
    web_rendered: Option<Instant>,
}

/// Zugriff auf die Szene, der sich den zuletzt benutzten Chunk **und Abschnitt** merkt.
///
/// Ein Strahl läuft fast immer etliche Blöcke am Stück durch denselben Chunk (der ist 16 Blöcke
/// breit), und der nächste Bildpunkt beginnt ohnehin im selben. Ohne dieses Merken kostet **jeder
/// durchquerte Block** einen Hashtabellen-Zugriff samt SipHash – bei 64x32 Bildpunkten, gut
/// hundert Schritten je Strahl und acht Bildern je Sekunde sind das rund zwei Millionen
/// Nachschlagevorgänge in der Sekunde, und damit der mit Abstand größte Einzelposten der
/// Live-Ansicht. Mit dem Merker bleibt davon der Bruchteil an echten Chunk-Wechseln übrig.
///
/// Gemerkt wird gleich der 16x16x16-Abschnitt, nicht nur der Chunk: Ein Strahl bleibt genauso
/// lange in derselben Höhenscheibe, und damit fallen bei jedem Schritt innerhalb des Abschnitts
/// auch noch die Höhenrechnung, die Bereichsprüfung und der Griff durch zwei Zeigerebenen weg.
/// Übrig bleiben drei Ganzzahlvergleiche und der eigentliche Blockzugriff.
struct Cursor<'a> {
    scene: &'a Scene,
    /// Zuletzt benutzter Chunk (Chunk-X, Chunk-Z).
    at: Option<(i32, i32)>,
    chunk: Option<&'a Chunk>,
    /// Zuletzt benutzter Abschnitt (Chunk-X, Abschnitt-Y, Chunk-Z).
    section_at: Option<(i32, i32, i32)>,
    /// `None` = reine Luft **oder** Chunk nicht geladen; durch beides läuft der Strahl weiter.
    section: Option<&'a Section>,
}

impl<'a> Cursor<'a> {
    fn new(scene: &'a Scene) -> Cursor<'a> {
        Cursor {
            scene,
            at: None,
            chunk: None,
            section_at: None,
            section: None,
        }
    }

    fn chunk(&mut self, x: i32, z: i32) -> Option<&'a Chunk> {
        let key = (x.div_euclid(16), z.div_euclid(16));
        if self.at != Some(key) {
            self.at = Some(key);
            self.chunk = self.scene.chunks.get(&key).map(|chunk| chunk.as_ref());
        }
        self.chunk
    }

    #[inline]
    fn section(&mut self, x: i32, y: i32, z: i32) -> Option<&'a Section> {
        let key = (x.div_euclid(16), y.div_euclid(16), z.div_euclid(16));
        if self.section_at != Some(key) {
            self.section_at = Some(key);
            self.section = self.chunk(x, z).and_then(|chunk| chunk.section(y));
        }
        self.section
    }

    /// Der Farbton des Bioms an dieser Stelle. Ohne Biom-Registry oder in einem Abschnitt, für
    /// den nie eine Palette kam, ist es der Ton der gemäßigten Ebene – also genau der, den diese
    /// Ansicht vorher für alles benutzt hat.
    #[inline]
    fn biome_tint(&mut self, x: i32, y: i32, z: i32) -> BiomeTint {
        let tints = &self.scene.biome_tints;
        if tints.is_empty() {
            return BiomeTint::PLAINS;
        }
        let id = self
            .section(x, y, z)
            .map_or(0, |section| section.biomes.get(x, y, z));
        tints.get(id as usize).copied().unwrap_or(BiomeTint::PLAINS)
    }

    /// Himmels- und Blocklicht an dieser Stelle, oder `None`, wenn der Server für den Chunk
    /// keins geschickt hat. Läuft über denselben Chunk-Merker wie die Blöcke – im Regelfall ist
    /// es derselbe Chunk, den der Strahl ohnehin gerade liest.
    #[inline]
    fn light(&mut self, x: i32, y: i32, z: i32) -> Option<(u8, u8)> {
        let chunk = self.chunk(x, z)?;
        Some(chunk.light.as_ref()?.get(x, y, z))
    }

    /// `false` heißt „Luft **oder** Chunk nicht geladen" – durch beides läuft der Strahl weiter.
    ///
    /// Der Strahl selbst benutzt diesen Einzelzugriff nicht mehr: er holt sich den Abschnitt
    /// einmal und läuft ihn dann Block für Block ab (siehe [`Ray::steps_in_section`]). Die beiden
    /// bleiben als **Prüfpfad** stehen – nur über sie lässt sich zeigen, dass der Merker dieselben
    /// Blöcke liefert wie ein direkter Griff in die Hashtabelle.
    #[cfg(test)]
    fn solid(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.section(x, y, z)
            .is_some_and(|section| !section.is_air(local_index(x, y, z)))
    }

    #[cfg(test)]
    fn block(&mut self, x: i32, y: i32, z: i32) -> u32 {
        self.section(x, y, z)
            .map_or(0, |section| section.state(local_index(x, y, z)))
    }
}

pub struct Pov {
    world: Mutex<World>,
    dimensions: Mutex<Vec<Dimension>>,
    /// Die Biome des Servers in Registry-Reihenfolge – die Chunk-Paletten nennen genau diese
    /// Nummern. Daraus und aus den Farbkarten der Original-JAR entsteht [`Pov::biome_tints`].
    biomes: Mutex<Vec<BiomeParams>>,
    /// Fertig ausgerechnete Farbtöne je Biom. Einmal gebaut, wenn Registry **und** Ressourcen
    /// da sind; der Zeichner reicht nur noch den `Arc` weiter, statt je Bild zu rechnen.
    biome_tints: Mutex<Arc<[BiomeTint]>>,
    live: AtomicBool,
    renderer_running: AtomicBool,
    width: AtomicUsize,
    height: AtomicUsize,
    /// Bilder je Sekunde.
    fps: AtomicUsize,
    parse_errors: AtomicU32,
    /// Zuletzt erkannte Spielart des Abschnittsformats (siehe [`Format`]).
    format: AtomicU32,
    /// Prüfsumme des zuletzt geschriebenen Bildes – unveränderte Bilder werden ausgelassen.
    last_frame: AtomicU64,
    /// Wann das letzte Bild geschrieben wurde (ms seit `started`).
    last_frame_ms: AtomicU64,
    started: Instant,
    /// Puffer, die von Bild zu Bild weiterverwendet werden.
    scratch: Mutex<Scratch>,
    /// Originale Modelle und Texturen aus einer echten Client-JAR.
    assets: Assets_,
    web_running: AtomicBool,
}

/// Die originalen Ressourcen samt ihrem Zustand.
///
/// Sie werden **nebenher** geladen: Das Auspacken von rund dreißigtausend Blockzuständen samt
/// ihren Modellen dauert je nach Rechner ein paar Sekunden, und vorher lief das mitten im Start –
/// der Client hing also erst am Einlesen von Texturen, ehe er überhaupt eine Verbindung aufbaute.
/// Beim allerersten Mal kommt sogar noch ein Download davor. Für den AFK-Betrieb ist das genau
/// die falsche Reihenfolge: Erst verbinden, dann hübsch werden.
#[derive(Default)]
struct Assets_ {
    ready: std::sync::OnceLock<Arc<Assets>>,
    /// Klartext für Terminal und Browser: was gerade läuft, oder warum es keine Texturen gibt.
    /// `None` heißt „alles in Ordnung".
    note: Mutex<Option<String>>,
}

impl Assets_ {
    fn get(&self) -> Option<&Assets> {
        self.ready.get().map(|assets| assets.as_ref())
    }

    fn note(&self) -> Option<String> {
        self.note.lock().ok().and_then(|note| note.clone())
    }

    fn set_note(&self, note: Option<String>) {
        if let Ok(mut slot) = self.note.lock() {
            *slot = note;
        }
    }
}

impl Pov {
    pub fn new(options: &Options) -> Pov {
        let (width, height) = options.pov_size.unwrap_or((DEFAULT_WIDTH, DEFAULT_HEIGHT));
        Pov {
            world: Mutex::new(World::default()),
            dimensions: Mutex::new(Vec::new()),
            biomes: Mutex::new(Vec::new()),
            biome_tints: Mutex::new(Arc::from(Vec::new())),
            live: AtomicBool::new(false),
            renderer_running: AtomicBool::new(false),
            width: AtomicUsize::new(width.clamp(MIN_WIDTH, MAX_WIDTH)),
            height: AtomicUsize::new(height.clamp(MIN_HEIGHT, MAX_HEIGHT)),
            fps: AtomicUsize::new(options.pov_fps.unwrap_or(DEFAULT_FPS).clamp(1, 20)),
            parse_errors: AtomicU32::new(0),
            format: AtomicU32::new(u32::MAX),
            last_frame: AtomicU64::new(0),
            last_frame_ms: AtomicU64::new(0),
            started: Instant::now(),
            scratch: Mutex::new(Scratch::default()),
            assets: Assets_::default(),
            web_running: AtomicBool::new(false),
        }
    }

    pub(crate) fn assets(&self) -> Option<&Assets> {
        self.assets.get()
    }

    /// Was gerade mit den Ressourcen ist – oder `None`, wenn sie einsatzbereit sind.
    pub(crate) fn asset_note(&self) -> Option<String> {
        self.assets.note()
    }

    pub(crate) fn set_dimensions(&self, dimensions: Vec<Dimension>) {
        *self.dimensions.lock().unwrap() = dimensions;
    }

    /// Die Biom-Registry des Servers übernehmen und die Farbtöne daraus neu ausrechnen.
    pub(crate) fn set_biomes(&self, biomes: Vec<BiomeParams>) {
        *self.biomes.lock().unwrap() = biomes;
        self.rebuild_biome_tints();
    }

    /// Farbtöne je Biom ausrechnen. Muss laufen, sobald sich Registry **oder** Ressourcen
    /// ändern – ohne Farbkarten fehlt der Gras-/Laubton, ohne Registry fehlen die Biome.
    ///
    /// Bewusst hier und nicht im Zeichner: Es sind ein paar Dutzend Einträge, aber sie stünden
    /// sonst in der Schleife, die achtmal je Sekunde läuft.
    pub(crate) fn rebuild_biome_tints(&self) {
        let biomes = self.biomes.lock().unwrap();
        let tints: Vec<BiomeTint> = match self.assets() {
            Some(assets) => biomes.iter().map(|p| assets.biome_tint(p)).collect(),
            // Ohne Ressourcen zeichnet die Browser-Ansicht ohnehin nicht; der Wasserton steht
            // aber schon in der Registry und braucht keine Farbkarte.
            None => biomes.iter().map(BiomeTint::without_colormaps).collect(),
        };
        *self.biome_tints.lock().unwrap() = Arc::from(tints);
    }

    pub fn clear(&self) {
        self.world.lock().unwrap().clear_visible();
        self.last_frame.store(0, Ordering::Relaxed);
    }

    /// Bevorzugte Formatspielart: die zuletzt erfolgreiche, sonst die der Protokollversion.
    fn preferred_format(&self, shared: &Shared) -> Format {
        match self.format.load(Ordering::Relaxed) {
            u32::MAX => Format {
                modern_palette: shared.proto.modern,
                fluid_count: shared.proto.extra.section_fluid_count,
            },
            code => Format::from_code(code),
        }
    }

    /// Den aktuellen Weltstand in eine bereits vorhandene Szene übertragen.
    ///
    /// Bewusst mit `clone_from` und `extend` statt mit frischen Behältern: die Tabelle und der
    /// Vektor behalten so ihren Speicher über alle Bilder hinweg. Gezeichnet wird anschließend
    /// **ohne** die Weltsperre – der Netz-Thread darf während der paar Millisekunden weiter
    /// Chunks einlesen.
    fn fill_scene(&self, scene: &mut Scene) {
        let world = self.world.lock().unwrap();
        // Die Chunk-Tabelle nur kopieren, wenn sich seit dem letzten Bild wirklich etwas geändert
        // hat. Ein stillstehender Bot bekommt minutenlang keinen neuen Chunk; die Tabelle mit
        // ihren gut 170 Einträgen trotzdem achtmal je Sekunde neu aufzubauen (mit einem
        // Arc-Zähler je Eintrag) war der teuerste Posten des Zeichners im Leerlauf.
        if scene.chunk_revision != world.revision {
            scene.chunks.clone_from(&world.chunks);
            scene.chunk_revision = world.revision;
        }
        scene.entities.clear();
        scene.entities.extend(world.entities.values().copied());
        drop(world);
        // Nur ein Zählerinkrement je Bild: Die Tabelle selbst entsteht beim Eintreffen der
        // Registry, nicht hier.
        scene.biome_tints = Arc::clone(&self.biome_tints.lock().unwrap());
    }

    fn select_dimension(&self, id: i32, world_name: Option<&str>) {
        let dimensions = self.dimensions.lock().unwrap();
        let dimension = usize::try_from(id)
            .ok()
            .and_then(|index| dimensions.get(index).cloned())
            .unwrap_or_else(|| Dimension::for_world(world_name.unwrap_or("minecraft:overworld")));
        drop(dimensions);
        let mut world = self.world.lock().unwrap();
        world.dimension = dimension;
        world.clear_visible();
    }
}

// ===================== Lebenszyklus und Registry-Auswahl =====================

/// Login-Paket ab direkt hinter der eigenen Entity-ID. Der erste Wert im eingebetteten
/// `PlayerSpawnInfo` ist die Dimension-Type-ID.
pub fn login(shared: &Arc<Shared>, own_entity: i32, r: &mut Reader) {
    if shared.proto.legacy {
        let dimension = r.u8().and_then(|_| r.i8()).map(|value| value as i32);
        let mut world = shared.extras.pov.world.lock().unwrap();
        world.own_entity = own_entity;
        if let Ok(dimension) = dimension {
            world.dimension = Dimension::legacy(dimension);
            world.clear_visible();
        }
        return;
    }
    let dimension = (|| -> io::Result<(i32, String)> {
        r.bool()?;
        let worlds = r.var_int()?;
        if !(0..=1024).contains(&worlds) {
            return Err(crate::buf::err("POV: Weltliste unplausibel"));
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
        let dimension = r.var_int()?;
        let world = r.string()?;
        Ok((dimension, world))
    })();

    let mut world = shared.extras.pov.world.lock().unwrap();
    world.own_entity = own_entity;
    drop(world);
    if let Ok((dimension, world_name)) = dimension {
        shared
            .extras
            .pov
            .select_dimension(dimension, Some(&world_name));
    }
}

/// Respawn beginnt direkt mit demselben `PlayerSpawnInfo`.
pub fn respawn(shared: &Arc<Shared>, r: &mut Reader) {
    if shared.proto.legacy {
        match r.i32() {
            Ok(dimension) => {
                let mut world = shared.extras.pov.world.lock().unwrap();
                world.dimension = Dimension::legacy(dimension);
                world.clear_visible();
            }
            Err(_) => shared.extras.pov.clear(),
        }
        return;
    }
    if let (Ok(dimension), Ok(world_name)) = (r.var_int(), r.string()) {
        shared
            .extras
            .pov
            .select_dimension(dimension, Some(&world_name));
    } else {
        shared.extras.pov.clear();
    }
}

pub fn on_join(shared: &Arc<Shared>) {
    shared.extras.pov.clear();
    // Die eigene POV-Bauform startet von selbst, Ultra erst auf `:pov live` – `--pov an|aus`
    // dreht beides um, ohne die Weltdaten abzuschalten.
    let autostart = shared
        .options()
        .pov_autostart
        // Mit Browser-Viewer bleibt der teure ANSI-Dauerstrom standardmaessig aus. Wer beides
        // will, kann ihn weiterhin ausdruecklich mit `--pov an` oder `:pov live` einschalten.
        .unwrap_or(cfg!(feature = "pov-client") && shared.options().pov_web.is_none());
    if autostart {
        shared.extras.pov.live.store(true, Ordering::SeqCst);
        start_renderer(shared);
    }
}

// ===================== Eingehende Weltpakete =====================

pub fn incoming(shared: &Arc<Shared>, kind: In, r: &mut Reader) {
    if let Err(error) = read_incoming(shared, kind, r) {
        let number = shared
            .extras
            .pov
            .parse_errors
            .fetch_add(1, Ordering::Relaxed);
        if number < 3 {
            shared
                .console
                .warn(&format!("POV-Paket verworfen: {}", error));
        }
    }
}

fn read_incoming(shared: &Arc<Shared>, kind: In, r: &mut Reader) -> io::Result<()> {
    match kind {
        In::LevelChunk => read_chunk(shared, r),
        In::LevelChunkBulk => read_chunk_bulk(shared, r),
        In::ForgetChunk => {
            // ChunkPos ist ein Long: x in den unteren, z in den oberen 32 Bit. Zwei
            // nacheinander gelesene i32 wären durch die Netzwerk-Bytefolge genau vertauscht.
            let key = unpack_chunk_pos(r.i64()?);
            let mut world = shared.extras.pov.world.lock().unwrap();
            if world.chunks.remove(&key).is_some() {
                world.touch();
            }
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
            shared
                .extras
                .pov
                .world
                .lock()
                .unwrap()
                .set_block(x, y, z, state);
            Ok(())
        }
        In::LightUpdate => read_light_update(shared, r),
        In::SectionBlocks => read_section_blocks(shared, r),
        In::AddEntity => read_add_entity(shared, r),
        In::RemoveEntities => {
            let count = r.var_int()?;
            if !(0..=MAX_ENTITIES as i32).contains(&count) {
                return Err(crate::buf::err("POV: Entity-Liste unplausibel"));
            }
            let mut world = shared.extras.pov.world.lock().unwrap();
            for _ in 0..count {
                world.entities.remove(&r.var_int()?);
            }
            Ok(())
        }
        In::MoveEntityPos | In::MoveEntityPosRot => read_move_entity(shared, r),
        In::TeleportEntity => read_teleport_entity(shared, r),
        In::EntityPositionSync => read_entity_sync(shared, r),
        _ => Ok(()),
    }
}

fn read_chunk(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    let x = r.i32()?;
    let z = r.i32()?;
    if shared.proto.legacy {
        let ground_up = r.bool()?;
        let mask = r.u16()?;
        let data = r.byte_slice()?;
        if ground_up && mask == 0 {
            let mut world = shared.extras.pov.world.lock().unwrap();
            if world.chunks.remove(&(x, z)).is_some() {
                world.touch();
            }
            return Ok(());
        }
        let chunk = decode_legacy_chunk(mask, data, ground_up, None)?;
        return store_chunk(shared, x, z, chunk);
    }
    if shared.proto.modern {
        let maps = r.var_int()?;
        if !(0..=64).contains(&maps) {
            return Err(crate::buf::err("POV: Hoehenkarten unplausibel"));
        }
        for _ in 0..maps {
            r.var_int()?;
            let longs = r.var_int()?;
            if !(0..=4096).contains(&longs) {
                return Err(crate::buf::err("POV: Hoehenkarte zu gross"));
            }
            r.skip(longs as usize * 8)?;
        }
    } else {
        crate::nbt::read_network(r)?;
    }
    // Ohne Kopie: die Chunkdaten werden unmittelbar aus dem Paketpuffer gelesen. Beim Beitritt
    // kommen gut 170 solche Pakete, und jede dieser Kopien lebte nur bis zum Ende dieser Funktion.
    let data = r.byte_slice()?;

    let pov = &shared.extras.pov;
    let dimension = pov.world.lock().unwrap().dimension.clone();
    let preferred = pov.preferred_format(shared);
    let (mut chunk, format) = Format::probe(preferred, &dimension, data)?;

    // Hinter den Blöcken stehen erst die Blockentitäten, dann das Licht. Beides ist eine Zugabe:
    // Misslingt es, behält der Chunk seine Blöcke und die Ansicht rechnet wie früher mit
    // geschätzter Flächenhelligkeit weiter. Ein Lesefehler darf hier weder den Chunk noch – über
    // die Fehlerkette – die Verbindung kosten.
    chunk.light = skip_block_entities(r)
        .and_then(|()| read_light(r, chunk.min_section, chunk.sections.len(), None))
        .ok()
        .map(Arc::new);
    if pov.format.swap(format.code(), Ordering::Relaxed) != format.code() {
        // Nur beim Wechsel melden – sonst stünde es bei jedem Chunk in der Ausgabe.
        if format != preferred {
            shared.console.info(&format!(
                "POV: Chunk-Format erkannt ({} Abschnitte, {}Fluidzaehler).",
                chunk.sections.len(),
                if format.fluid_count { "mit " } else { "ohne " }
            ));
        }
    }

    // Der eigene Chunk ist der Bezugspunkt fürs Verwerfen. Er wird hier gesetzt und nicht im
    // Renderer: sonst würde bei gestoppter Ansicht um den Nullpunkt herum aufgeräumt.
    let center = shared.position().map(|(x, _, z, _, _)| {
        (
            (x.floor() as i32).div_euclid(16),
            (z.floor() as i32).div_euclid(16),
        )
    });

    let mut world = pov.world.lock().unwrap();
    if let Some(center) = center {
        world.center = center;
    }
    if world.chunks.len() < MAX_CHUNKS || world.chunks.contains_key(&(x, z)) {
        world.chunks.insert((x, z), Arc::new(chunk));
        world.touch();
    }
    if center.is_some() {
        world.prune();
    }
    Ok(())
}

/// 1.8.9-Sammelpaket: Metadaten aller Spalten stehen vorn, ihre rohen Daten lückenlos dahinter.
fn read_chunk_bulk(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    if !shared.proto.legacy {
        return Ok(());
    }
    let sky_light = r.bool()?;
    let count = r.var_int()?;
    if !(0..=MAX_CHUNKS as i32).contains(&count) {
        return Err(crate::buf::err("POV: Chunk-Sammelpaket unplausibel"));
    }
    let mut columns = Vec::with_capacity(count as usize);
    for _ in 0..count {
        columns.push((r.i32()?, r.i32()?, r.u16()?));
    }
    for (x, z, mask) in columns {
        let sections = mask.count_ones() as usize;
        let size = sections
            * (SECTION_BLOCKS * 2 + LIGHT_BYTES + if sky_light { LIGHT_BYTES } else { 0 })
            + 256;
        let data = r.bytes(size)?;
        let chunk = decode_legacy_chunk(mask, data, true, Some(sky_light))?;
        store_chunk(shared, x, z, chunk)?;
    }
    Ok(())
}

/// Einen alten Chunk in dieselbe kompakte interne Darstellung wie moderne Paletten überführen.
fn decode_legacy_chunk(
    mask: u16,
    data: &[u8],
    ground_up: bool,
    sky_hint: Option<bool>,
) -> io::Result<Chunk> {
    let included = mask.count_ones() as usize;
    let block_bytes = included * SECTION_BLOCKS * 2;
    let light_bytes = included * LIGHT_BYTES;
    let biome_bytes = if ground_up { 256 } else { 0 };
    let without_sky = block_bytes + light_bytes + biome_bytes;
    let with_sky = without_sky + light_bytes;
    let has_sky = sky_hint.unwrap_or(data.len() == with_sky);
    let expected = if has_sky { with_sky } else { without_sky };
    if data.len() != expected {
        return Err(crate::buf::err("POV: alte Chunkdaten haben falsche Länge"));
    }

    let mut sections: Vec<Option<Arc<Section>>> = vec![None; 16];
    let mut raw_offset = 0usize;
    let mut included_indices = Vec::with_capacity(included);
    for (section_index, section) in sections.iter_mut().enumerate() {
        if mask & (1 << section_index) == 0 {
            continue;
        }
        included_indices.push(section_index);
        let raw = &data[raw_offset..raw_offset + SECTION_BLOCKS * 2];
        raw_offset += SECTION_BLOCKS * 2;
        *section = legacy_section(raw).map(Arc::new);
    }

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

fn legacy_section(raw: &[u8]) -> Option<Section> {
    let mut palette = vec![0u32];
    let mut by_state = HashMap::new();
    by_state.insert(0u32, 0u16);
    let mut indices = Vec::with_capacity(SECTION_BLOCKS);
    for bytes in raw.chunks_exact(2) {
        let state = u16::from_le_bytes([bytes[0], bytes[1]]) as u32;
        let slot = match by_state.get(&state) {
            Some(slot) => *slot,
            None => {
                let slot = palette.len() as u16;
                palette.push(state);
                by_state.insert(state, slot);
                slot
            }
        };
        indices.push(slot);
    }
    if palette.len() == 1 {
        return None;
    }
    let remap: Vec<u16> = (0..palette.len() as u16).collect();
    Some(Section {
        indices: Indices::remapped(&indices, &remap, palette.len()),
        palette,
        biomes: Biomes::Single(0),
    })
}

fn store_chunk(shared: &Arc<Shared>, x: i32, z: i32, chunk: Chunk) -> io::Result<()> {
    let center = shared.position().map(|(x, _, z, _, _)| {
        (
            (x.floor() as i32).div_euclid(16),
            (z.floor() as i32).div_euclid(16),
        )
    });
    let mut world = shared.extras.pov.world.lock().unwrap();
    if let Some(center) = center {
        world.center = center;
    }
    if world.chunks.len() < MAX_CHUNKS || world.chunks.contains_key(&(x, z)) {
        world.chunks.insert((x, z), Arc::new(chunk));
        world.touch();
    }
    if center.is_some() {
        world.prune();
    }
    Ok(())
}

/// Nachgereichtes Licht zu einem Chunk, den wir schon haben.
///
/// Der Server schickt das, sobald sich die Beleuchtung ändert – eine gesetzte Fackel, ein
/// abgebauter Block, oder schlicht, weil die Lichtberechnung beim Beitritt noch nicht fertig war.
/// Ohne dieses Paket bliebe die Ansicht auf dem Stand des Chunk-Pakets stehen.
fn read_light_update(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    let x = r.var_int()?;
    let z = r.var_int()?;
    let mut world = shared.extras.pov.world.lock().unwrap();
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
    // `make_mut` kopiert den Chunk nur, solange ihn ein Bild gerade liest; die Abschnitte selbst
    // hängen an eigenen `Arc`s und werden dabei nicht mitkopiert.
    Arc::make_mut(world.chunks.get_mut(&(x, z)).unwrap()).light = Some(light);
    world.touch();
    Ok(())
}

fn read_section_blocks(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    if shared.proto.legacy {
        let chunk_x = r.i32()?;
        let chunk_z = r.i32()?;
        let count = r.var_int()?;
        if !(0..=SECTION_BLOCKS as i32).contains(&count) {
            return Err(crate::buf::err("POV: alte Blockänderungen unplausibel"));
        }
        let mut world = shared.extras.pov.world.lock().unwrap();
        for _ in 0..count {
            let horizontal = r.u8()?;
            let y = r.u8()? as i32;
            let state = r.var_int()? as u32;
            let x = chunk_x * 16 + (horizontal >> 4) as i32;
            let z = chunk_z * 16 + (horizontal & 15) as i32;
            world.set_block(x, y, z, state);
        }
        return Ok(());
    }
    let (section_x, section_y, section_z) = unpack_section_pos(r.i64()?);
    let count = r.var_int()?;
    if !(0..=SECTION_BLOCKS as i32).contains(&count) {
        return Err(crate::buf::err("POV: Blockaenderungen unplausibel"));
    }
    let mut changes = Vec::with_capacity(count as usize);
    for _ in 0..count {
        let value = r.var_long()? as u64;
        let local = value & 0xFFF;
        let state = (value >> 12) as u32;
        let x = section_x * 16 + ((local >> 8) & 15) as i32;
        let z = section_z * 16 + ((local >> 4) & 15) as i32;
        let y = section_y * 16 + (local & 15) as i32;
        changes.push((x, y, z, state));
    }
    let mut world = shared.extras.pov.world.lock().unwrap();
    for (x, y, z, state) in changes {
        world.set_block(x, y, z, state);
    }
    Ok(())
}

fn read_add_entity(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    let id = r.var_int()?;
    let entity = if shared.proto.legacy {
        r.skip(16)?; // UUID
        Entity {
            x: r.i32()? as f64 / 32.0,
            y: r.i32()? as f64 / 32.0,
            z: r.i32()? as f64 / 32.0,
            player: true,
        }
    } else {
        r.skip(16)?;
        let kind = r.var_int()?;
        Entity {
            x: r.f64()?,
            y: r.f64()?,
            z: r.f64()?,
            player: kind == shared.proto.extra.player_entity_type,
        }
    };
    let mut world = shared.extras.pov.world.lock().unwrap();
    if id != world.own_entity
        && (world.entities.len() < MAX_ENTITIES || world.entities.contains_key(&id))
    {
        world.entities.insert(id, entity);
    }
    Ok(())
}

fn read_move_entity(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    let id = r.var_int()?;
    let (dx, dy, dz) = if shared.proto.legacy {
        (
            r.i8()? as f64 / 32.0,
            r.i8()? as f64 / 32.0,
            r.i8()? as f64 / 32.0,
        )
    } else {
        (
            r.i16()? as f64 / 4096.0,
            r.i16()? as f64 / 4096.0,
            r.i16()? as f64 / 4096.0,
        )
    };
    if let Some(entity) = shared
        .extras
        .pov
        .world
        .lock()
        .unwrap()
        .entities
        .get_mut(&id)
    {
        entity.x += dx;
        entity.y += dy;
        entity.z += dz;
    }
    Ok(())
}

fn read_teleport_entity(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    let id = r.var_int()?;
    let (x, y, z) = if shared.proto.legacy {
        (
            r.i32()? as f64 / 32.0,
            r.i32()? as f64 / 32.0,
            r.i32()? as f64 / 32.0,
        )
    } else {
        (r.f64()?, r.f64()?, r.f64()?)
    };
    let relatives = if shared.proto.modern {
        for _ in 0..3 {
            r.f64()?;
        }
        r.f32()?;
        r.f32()?;
        r.i32()?
    } else {
        0
    };
    if let Some(entity) = shared
        .extras
        .pov
        .world
        .lock()
        .unwrap()
        .entities
        .get_mut(&id)
    {
        entity.x = if relatives & 1 != 0 { entity.x + x } else { x };
        entity.y = if relatives & 2 != 0 { entity.y + y } else { y };
        entity.z = if relatives & 4 != 0 { entity.z + z } else { z };
    }
    Ok(())
}

fn read_entity_sync(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    let id = r.var_int()?;
    let (x, y, z) = (r.f64()?, r.f64()?, r.f64()?);
    if let Some(entity) = shared
        .extras
        .pov
        .world
        .lock()
        .unwrap()
        .entities
        .get_mut(&id)
    {
        entity.x = x;
        entity.y = y;
        entity.z = z;
    }
    Ok(())
}

// ===================== Palette und Koordinaten =====================

/// Eine gelesene Palette samt Häufigkeiten. Die Häufigkeiten fallen beim Auspacken ohnehin an –
/// sie noch einmal über eine Hashtabelle zu zählen, wäre je Abschnitt ein zweiter Durchlauf.
struct RawPalette {
    values: Vec<u32>,
    indices: Vec<u16>,
    counts: Vec<u32>,
}

/// Kopf einer Palette lesen: Bitbreite und (bei indirekten Paletten) die Einträge.
/// Rückgabe: (Bitbreite, Einträge oder `None` für die globale Palette).
fn palette_head(r: &mut Reader, max_indirect_bits: u8) -> io::Result<(u8, Option<Vec<u32>>)> {
    let bits = r.u8()?;
    if bits > 32 {
        return Err(crate::buf::err("POV: Palette breiter als 32 Bit"));
    }
    if bits == 0 || bits <= max_indirect_bits {
        let count = if bits == 0 { 1 } else { r.var_int()? };
        if !(1..=4096).contains(&count) {
            return Err(crate::buf::err("POV: lokale Palette unplausibel"));
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
        return Err(crate::buf::err("POV: ungueltige Palettenbreite"));
    }
    let expected = size.div_ceil(per_long);
    if modern {
        return Ok(expected);
    }
    let encoded = r.var_int()?;
    if encoded < 0 || encoded as usize != expected {
        return Err(crate::buf::err("POV: falsche Palettenlaenge"));
    }
    Ok(expected)
}

fn read_palette(
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
                return Err(crate::buf::err("POV: Singleton-Palette mit Daten"));
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
            return Err(crate::buf::err(message));
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
        return Err(crate::buf::err(message));
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
                return Err(crate::buf::err("POV: Singleton-Palette mit Daten"));
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
        return Err(crate::buf::err("POV: Palettendaten zu kurz"));
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
fn compact(raw: RawPalette, non_air: usize) -> Option<Section> {
    let RawPalette {
        values,
        indices,
        counts,
    } = raw;
    let air = air_entries(&values, &counts, SECTION_BLOCKS.saturating_sub(non_air));

    // Neue Palette: Eintrag 0 ist Luft.
    let mut palette = vec![0u32];
    let mut remap = vec![0u16; values.len()];
    for (slot, value) in values.iter().enumerate() {
        if air[slot] {
            continue;
        }
        remap[slot] = palette.len() as u16;
        palette.push(*value);
    }
    if palette.len() == 1 {
        return None; // doch nur Luft
    }

    let indices = Indices::remapped(&indices, &remap, palette.len());
    // Die Biome setzt der Aufrufer: Sie stehen im Paket erst hinter der Blockpalette.
    Some(Section {
        palette,
        indices,
        biomes: Biomes::Single(0),
    })
}

/// Welche Paletteneinträge sind Luft? `air_total` ist die Zahl der Luftblöcke im Abschnitt.
fn air_entries(values: &[u32], counts: &[u32], air_total: usize) -> Vec<bool> {
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

fn unpack_block_pos(value: i64) -> (i32, i32, i32) {
    let x = (value >> 38) as i32;
    let y = (value << 52 >> 52) as i32;
    let z = (value << 26 >> 38) as i32;
    (x, y, z)
}

/// Protokoll 47 legte die 12 Y-Bits noch zwischen X und Z; moderne Pakete legen sie ans Ende.
fn unpack_legacy_block_pos(value: i64) -> (i32, i32, i32) {
    let x = (value >> 38) as i32;
    let y = (value << 26 >> 52) as i32;
    let z = (value << 38 >> 38) as i32;
    (x, y, z)
}

fn unpack_chunk_pos(value: i64) -> (i32, i32) {
    let value = value as u64;
    (value as u32 as i32, (value >> 32) as u32 as i32)
}

fn unpack_section_pos(value: i64) -> (i32, i32, i32) {
    let x = (value >> 42) as i32;
    let y = (value << 44 >> 44) as i32;
    let z = (value << 22 >> 42) as i32;
    (x, y, z)
}

// ===================== Bedienung und Renderer =====================

pub fn command(shared: &Arc<Shared>, arg: &str) {
    let mut parts = arg.split_whitespace();
    match parts.next().unwrap_or("live").to_lowercase().as_str() {
        "live" | "start" | "an" | "on" => {
            shared.extras.pov.live.store(true, Ordering::SeqCst);
            shared.extras.pov.last_frame.store(0, Ordering::Relaxed);
            start_renderer(shared);
            shared
                .console
                .info("Live-POV gestartet. Beenden mit :pov stop.");
        }
        "stop" | "aus" | "off" => {
            shared.extras.pov.live.store(false, Ordering::SeqCst);
            if shared.console.is_color() {
                shared.console.pov_frame("", "\x1b[0m\x1b[2J\x1b[H");
            }
            shared.console.info("Live-POV beendet.");
        }
        "frame" | "bild" | "once" => draw_once(shared, false, true),
        "size" | "groesse" | "größe" => {
            let width = parts.next().and_then(|v| v.parse::<usize>().ok());
            let height = parts.next().and_then(|v| v.parse::<usize>().ok());
            match (width, height) {
                (Some(width), Some(height))
                    if (MIN_WIDTH..=MAX_WIDTH).contains(&width)
                        && (MIN_HEIGHT..=MAX_HEIGHT).contains(&height) =>
                {
                    shared.extras.pov.width.store(width, Ordering::Relaxed);
                    shared.extras.pov.height.store(height, Ordering::Relaxed);
                    shared.extras.pov.last_frame.store(0, Ordering::Relaxed);
                    shared
                        .console
                        .info(&format!("POV-Groesse: {}x{} Pixel.", width, height));
                }
                // Nicht stillschweigend zurechtbiegen: wer 400x300 tippt, meint etwas anderes
                // als 160x80 – genau wie bei `--pov-size` auf der Kommandozeile.
                _ => shared.console.error(&format!(
                    "Nutzung: :pov size <breite {}-{}> <hoehe {}-{}>   z. B. :pov size 160 80",
                    MIN_WIDTH, MAX_WIDTH, MIN_HEIGHT, MAX_HEIGHT
                )),
            }
        }
        "fps" | "takt" => match parts.next().and_then(|v| v.parse::<usize>().ok()) {
            Some(fps) => {
                let fps = fps.clamp(1, 20);
                shared.extras.pov.fps.store(fps, Ordering::Relaxed);
                shared
                    .console
                    .info(&format!("POV: {} Bilder je Sekunde.", fps));
            }
            None => shared.console.error("Nutzung: :pov fps <1-20>"),
        },
        "info" | "status" => {
            let pov = &shared.extras.pov;
            let world = pov.world.lock().unwrap();
            let sections: usize = world
                .chunks
                .values()
                .map(|chunk| chunk.sections.iter().flatten().count())
                .sum();
            let dimension = world.dimension.clone();
            let chunks = world.chunks.len();
            let lit = world
                .chunks
                .values()
                .filter(|chunk| chunk.light.is_some())
                .count();
            let entities = world.entities.len();
            drop(world);
            shared.console.info(&format!(
                "POV: {} · y={}..{} · {} Chunks · {} mit Licht · {} Abschnitte · {} Entities · {}x{} · {} fps · {}",
                dimension.name,
                dimension.min_y,
                dimension.min_y + dimension.height - 1,
                chunks,
                lit,
                sections,
                entities,
                pov.width.load(Ordering::Relaxed),
                pov.height.load(Ordering::Relaxed),
                pov.fps.load(Ordering::Relaxed),
                if pov.live.load(Ordering::Relaxed) {
                    "live"
                } else {
                    "gestoppt"
                }
            ));
        }
        _ => shared
            .console
            .error("Nutzung: :pov live|stop|frame|size <breite> <hoehe>|fps <1-20>|info"),
    }
}

/// Die originalen Ressourcen im Hintergrund beschaffen und einlesen.
///
/// Läuft nebenher, damit weder der Download noch das Auspacken der Modelle den Verbindungsaufbau
/// aufhält. Bis der Thread fertig ist, sagt [`Pov::asset_note`], was gerade passiert – die
/// Browser-Ansicht zeigt genau diesen Text an, statt einfach schwarz zu bleiben.
pub(crate) fn start_assets(shared: &Arc<Shared>) {
    if shared.options().pov_web.is_none() {
        return; // Ohne Browser-Ansicht liest niemand die Texturen.
    }
    let pov = &shared.extras.pov;
    if matches!(
        shared.options().pov_resources,
        crate::pov_resources::Source::Off
    ) {
        pov.assets
            .set_note(Some("ohne Texturen (--pov-resources aus)".to_string()));
        return;
    }
    pov.assets
        .set_note(Some("Ressourcen werden geladen ...".to_string()));

    let owned = Arc::clone(shared);
    let started = thread::Builder::new()
        .name("afk-pov-assets".into())
        .spawn(move || {
            let version = owned.proto.name;
            let source = &owned.options().pov_resources;
            let pov = &owned.extras.pov;
            let path = match crate::pov_resources::locate(&owned.console, version, source) {
                Ok(path) => path,
                Err(error) => {
                    owned
                        .console
                        .warn(&format!("Live-Ansicht ohne Texturen: {}", error));
                    return pov.assets.set_note(Some(error));
                }
            };
            match Assets::load(&path, version) {
                Ok(assets) => {
                    // `set` kann nur fehlschlagen, wenn schon jemand geladen hätte – den Thread
                    // gibt es aber genau einmal.
                    let _ = pov.assets.ready.set(Arc::new(assets));
                    pov.assets.set_note(None);
                    // Erst jetzt gibt es die Farbkarten. Die Biome sind längst da (sie kommen in
                    // der Konfigurationsphase, das Laden hier läuft nebenher) – ohne dieses
                    // Nachrechnen bliebe jedes Biom auf dem Ebene-Ton stehen.
                    pov.rebuild_biome_tints();
                    owned.console.ok("Live-Ansicht: Texturen sind geladen.");
                }
                Err(error) => {
                    owned
                        .console
                        .warn(&format!("Live-Ansicht ohne Texturen: {}", error));
                    pov.assets.set_note(Some(error));
                }
            }
        });
    if started.is_err() {
        pov.assets.set_note(Some(
            "Ressourcen-Thread liess sich nicht starten".to_string(),
        ));
    }
}

/// Den optionalen Browser-Viewer genau einmal starten. Er ist von `:pov live` unabhaengig: Das
/// Terminal kann gestoppt bleiben, waehrend der Browser Frames bei Bedarf abholt.
pub(crate) fn start_web(shared: &Arc<Shared>) {
    let Some(address) = shared.options().pov_web else {
        return;
    };
    if shared
        .extras
        .pov
        .web_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    if let Err(error) = crate::pov_web::start(shared, address) {
        shared.extras.pov.web_running.store(false, Ordering::SeqCst);
        shared.console.error(&format!("Browser-POV: {}", error));
    }
}

pub(crate) fn web_world(shared: &Arc<Shared>) -> serde_json::Value {
    let world = shared.extras.pov.world.lock().unwrap();
    let position = shared.position();
    serde_json::json!({
        "connected": shared.in_game.load(Ordering::Relaxed),
        "position": position.map(|value| serde_json::json!({
            "x": value.0, "y": value.1, "z": value.2,
            "yaw": value.3, "pitch": value.4
        })),
        "dimension": &world.dimension.name,
        "chunks": world.chunks.len(),
        "entities": world.entities.len(),
        "textures": shared.extras.pov.assets().is_some(),
        "texture_error": shared.extras.pov.asset_note(),
    })
}

pub(crate) fn web_asset(shared: &Arc<Shared>, path: &str) -> Option<Vec<u8>> {
    shared.extras.pov.assets()?.raw(path)
}

#[cfg(feature = "items")]
pub(crate) fn web_item(shared: &Arc<Shared>, id: i32) -> Option<Vec<u8>> {
    let name = crate::item_names::get(shared.proto.name, id)?;
    Some(shared.extras.pov.assets()?.item_png(name))
}

pub(crate) fn web_missing(shared: &Arc<Shared>) -> Option<Vec<u8>> {
    Some(shared.extras.pov.assets()?.missing_png().to_vec())
}

fn start_renderer(shared: &Arc<Shared>) {
    if shared
        .extras
        .pov
        .renderer_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    let owned = Arc::clone(shared);
    if thread::Builder::new()
        .name("afk-pov".into())
        .spawn(move || {
            if owned.console.is_color() {
                owned.console.pov_frame("", "\x1b[2J\x1b[H");
            }
            loop {
                while owned.running.load(Ordering::Relaxed)
                    && owned.extras.pov.live.load(Ordering::Relaxed)
                {
                    // Zwischen zwei Verbindungen gibt es nichts zu zeichnen; dann muss auch
                    // nicht im Bildtakt aufgewacht werden.
                    if !owned.in_game.load(Ordering::Relaxed) {
                        thread::sleep(Duration::from_millis(250));
                        continue;
                    }
                    let started = Instant::now();
                    draw_once(&owned, true, false);
                    let interval = Duration::from_millis(
                        1000 / owned.extras.pov.fps.load(Ordering::Relaxed).max(1) as u64,
                    );
                    thread::sleep(interval.saturating_sub(started.elapsed()));
                }
                owned
                    .extras
                    .pov
                    .renderer_running
                    .store(false, Ordering::SeqCst);
                // Zwischen dem Verlassen der Schleife und dem Freigeben kann `:pov live` (oder
                // ein Beitritt) die Ansicht längst wieder eingeschaltet haben. Ein neuer Start
                // wäre in diesem Fenster abgewiesen worden, weil der Merker noch stand – die
                // Ansicht galt dann als „live" und zeichnete trotzdem nie wieder. Deshalb hier
                // noch einmal nachsehen und den Posten gegebenenfalls selbst wieder übernehmen.
                if !owned.running.load(Ordering::Relaxed)
                    || !owned.extras.pov.live.load(Ordering::Relaxed)
                    || owned
                        .extras
                        .pov
                        .renderer_running
                        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
                        .is_err()
                {
                    return;
                }
            }
        })
        .is_err()
    {
        shared
            .extras
            .pov
            .renderer_running
            .store(false, Ordering::SeqCst);
    }
}

/// Ein Bild zeichnen. `home` setzt den Cursor davor zurück (Live-Betrieb im Terminal),
/// `force` schreibt auch ein unverändertes Bild.
fn draw_once(shared: &Arc<Shared>, home: bool, force: bool) {
    let Some(position) = shared.position() else {
        return;
    };
    let pov = &shared.extras.pov;
    let width = pov.width.load(Ordering::Relaxed);
    let height = pov.height.load(Ordering::Relaxed);
    let color = shared.console.is_color();

    let mut scratch = pov.scratch.lock().unwrap();
    let Scratch {
        scene,
        pixels,
        frame,
        ..
    } = &mut *scratch;
    pov.fill_scene(scene);
    render_into(scene, position, width, height, color, pixels, frame);

    // Auch bei `force` mitrechnen: Sonst behielte der Merker den Stand von **vor** dem
    // erzwungenen Bild, und das nächste Bild der Live-Ansicht galt fälschlich als unverändert –
    // nach einem `:pov frame` blieb die Ansicht dann bis zu zwei Sekunden stehen.
    let changed = frame_changed(pov, frame);
    if !force && !changed {
        return;
    }
    // Cursor-Steuerung und Bild in einem Zug – sonst rutscht eine Statuszeile dazwischen.
    shared
        .console
        .pov_frame(if home && color { "\x1b[H" } else { "" }, frame);
}

/// Unveränderte Bilder auslassen – das spart bei einem stillstehenden Bot fast die ganze
/// Ausgabe. Spätestens nach [`REPEAT_UNCHANGED`] wird trotzdem wieder geschrieben, damit ein
/// Programm davor aus der Stille nicht schließt, die Ansicht sei tot.
fn frame_changed(pov: &Pov, frame: &str) -> bool {
    // FNV-1a: ein Durchlauf, keine Allokation.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in frame.as_bytes() {
        hash ^= *byte as u64;
        hash = hash.wrapping_mul(0x1000_0000_01b3);
    }
    let now = pov.started.elapsed().as_millis() as u64;
    let previous = pov.last_frame.swap(hash, Ordering::Relaxed);
    let since = now.saturating_sub(pov.last_frame_ms.load(Ordering::Relaxed));
    if previous == hash && since < REPEAT_UNCHANGED.as_millis() as u64 {
        return false;
    }
    pov.last_frame_ms.store(now, Ordering::Relaxed);
    true
}

#[derive(Clone, Copy)]
struct Pixel {
    rgb: (u8, u8, u8),
    depth: f64,
}

/// Wie viele Kerne die Live-Ansicht höchstens benutzt.
///
/// Die Strahlen eines Bildes hängen nicht voneinander ab, das Bild lässt sich also in Bänder
/// zerlegen. Gedeckelt, weil dieser Client nebenher laufen soll und nicht die Maschine belegt:
/// Vier Bänder holen den Löwenanteil, alles darüber teilt vor allem den Speicherzugriff auf.
const MAX_RENDER_THREADS: usize = 4;
/// Ab dieser Bildgröße lohnt das Aufteilen. Ein Terminalbild ist höchstens 160x80 groß und in
/// wenigen Millisekunden gerechnet – dort kostet das Anlegen der Threads mehr, als es spart.
const PARALLEL_PIXELS: usize = 20_000;

/// Richtung des Strahls durch einen Bildpunkt. Echte Zentralprojektion statt gleichmäßig
/// verteilter Winkel: sonst „biegt" sich der Horizont bei 90° Blickfeld sichtbar nach außen.
#[inline]
fn pixel_direction(
    basis: Basis,
    half: (f64, f64),
    size: (usize, usize),
    px: usize,
    py: usize,
) -> (f64, f64, f64) {
    let sx = ((px as f64 + 0.5) / size.0 as f64 - 0.5) * 2.0 * half.0;
    let sy = (0.5 - (py as f64 + 0.5) / size.1 as f64) * 2.0 * half.1;
    normalize((
        basis.0 .0 + basis.1 .0 * sx + basis.2 .0 * sy,
        basis.0 .1 + basis.1 .1 * sx + basis.2 .1 * sy,
        basis.0 .2 + basis.1 .2 * sx + basis.2 .2 * sy,
    ))
}

/// Jeden Bildpunkt mit `shade` berechnen – auf großen Bildern über mehrere Kerne.
///
/// Verteilt wird **zeilenweise auf Zuruf**, nicht in feste Bänder. Das ist kein Selbstzweck: Der
/// Aufwand je Zeile geht weit auseinander. Ein Strahl in den Himmel läuft die vollen 72 Blöcke
/// ab, ein Strahl auf den Boden vor den Füßen ist nach drei Schritten fertig. Bei festen Bändern
/// bekäme der Thread mit dem Himmel ein Vielfaches der Arbeit, und die anderen drei warteten auf
/// ihn – die Wanduhr richtet sich nach dem langsamsten Band, nicht nach dem Mittel.
///
/// Jeder Thread bekommt seinen eigenen [`Cursor`]; geteilt wird nur die Szene, und die wird nur
/// gelesen.
fn render_pixels(
    scene: &Scene,
    width: usize,
    height: usize,
    pixels: &mut Vec<Pixel>,
    shade: impl Fn(&mut Cursor, usize, usize) -> Pixel + Sync,
) {
    let workers = render_workers(width * height, height);
    render_pixels_with(scene, width, height, workers, pixels, shade)
}

/// Wie [`render_pixels`], aber mit vorgegebener Thread-Zahl.
///
/// Getrennt, damit der Test beide Wege auf **derselben** Szene und Größe vergleichen kann: Ob ein
/// Bild geteilt gerechnet wird, darf an keinem einzigen Bildpunkt zu sehen sein.
fn render_pixels_with(
    scene: &Scene,
    width: usize,
    height: usize,
    workers: usize,
    pixels: &mut Vec<Pixel>,
    shade: impl Fn(&mut Cursor, usize, usize) -> Pixel + Sync,
) {
    pixels.clear();
    pixels.resize(
        width * height,
        Pixel {
            rgb: (0, 0, 0),
            depth: MAX_DISTANCE,
        },
    );

    if workers <= 1 {
        let mut cursor = Cursor::new(scene);
        for (py, row) in pixels.chunks_mut(width).enumerate() {
            for (px, pixel) in row.iter_mut().enumerate() {
                *pixel = shade(&mut cursor, px, py);
            }
        }
        return;
    }

    // Die Zeilen liegen als getrennte Ausschnitte vor; jeder gehört genau einem Thread, sobald er
    // ihn aus der Ausgabe genommen hat. Die Sperre wird je Zeile einmal kurz genommen – bei ein
    // paar hundert Zeilen je Bild ist das nicht messbar.
    let rows: Vec<(usize, &mut [Pixel])> = pixels.chunks_mut(width).enumerate().collect();
    let queue = Mutex::new(rows.into_iter());
    let shade = &shade;
    thread::scope(|scope| {
        for _ in 0..workers {
            let queue = &queue;
            scope.spawn(move || {
                let mut cursor = Cursor::new(scene);
                loop {
                    // Sperre nur fürs Herausnehmen halten, nicht fürs Rechnen.
                    let next = queue.lock().map(|mut queue| queue.next());
                    let Ok(Some((py, row))) = next else {
                        return;
                    };
                    for (px, pixel) in row.iter_mut().enumerate() {
                        *pixel = shade(&mut cursor, px, py);
                    }
                }
            });
        }
    });
}

/// Wie viele Threads ein Bild dieser Größe bekommt. `1` heißt „ohne Threads".
fn render_workers(pixels: usize, height: usize) -> usize {
    if pixels < PARALLEL_PIXELS {
        return 1;
    }
    std::thread::available_parallelism()
        .map(usize::from)
        .unwrap_or(1)
        .min(MAX_RENDER_THREADS)
        .min(height)
        .max(1)
}

/// Ein Bild in bereitgestellte Puffer zeichnen. `pixels` und `out` dürfen (und sollen) von einem
/// Bild zum nächsten weiterverwendet werden; ihr Inhalt wird hier vollständig ersetzt.
#[allow(clippy::too_many_arguments)]
fn render_into(
    scene: &Scene,
    position: (f64, f64, f64, f32, f32),
    width: usize,
    height: usize,
    color: bool,
    pixels: &mut Vec<Pixel>,
    out: &mut String,
) {
    let origin = (position.0, position.1 + 1.62, position.2);
    let half_width = (HORIZONTAL_FOV.to_radians() / 2.0).tan();
    let half_height = half_width * height as f64 / width as f64;
    let basis = camera_basis(position.3 as f64, position.4 as f64);

    render_pixels(scene, width, height, pixels, |cursor, px, py| {
        let sky = sky_color(py, height);
        let direction = pixel_direction(basis, (half_width, half_height), (width, height), px, py);
        match cast(cursor, origin, direction) {
            Some((state, distance, face)) => Pixel {
                rgb: fog(block_color(state, face, distance), sky, distance),
                depth: distance,
            },
            None => Pixel {
                rgb: sky,
                depth: MAX_DISTANCE,
            },
        }
    });
    overlay_entities(
        origin,
        basis,
        (half_width, half_height),
        width,
        height,
        &scene.entities,
        pixels,
    );

    out.clear();
    out.reserve(width * height * if color { 24 } else { 2 } + 128);
    // Direkt in den Puffer schreiben statt über eine Zwischenzeichenkette – die Kopfzeile
    // entsteht bei jedem Bild neu.
    let _ = write!(
        out,
        "POV  x={:.1} y={:.1} z={:.1}  Blick {:.0}/{:.0}  Chunks {}  (:pov stop)",
        position.0,
        position.1,
        position.2,
        position.3,
        position.4,
        scene.chunks.len()
    );
    out.push('\n');
    if color {
        // Zwei Bildzeilen je Terminalzeile: Vordergrundfarbe oben, Hintergrundfarbe unten.
        for y in (0..height).step_by(2) {
            for x in 0..width {
                let top = pixels[y * width + x].rgb;
                let bottom = pixels[(y + 1).min(height - 1) * width + x].rgb;
                push_color(out, "\x1b[38;2;", top);
                push_color(out, "\x1b[48;2;", bottom);
                out.push('▀');
            }
            out.push_str("\x1b[0m\n");
        }
    } else {
        const RAMP: &[u8] = b" .:-=+*#%@";
        for y in 0..height {
            for x in 0..width {
                let (r, g, b) = pixels[y * width + x].rgb;
                let light = (r as usize * 3 + g as usize * 6 + b as usize) / 10;
                out.push(RAMP[light * (RAMP.len() - 1) / 255] as char);
            }
            out.push('\n');
        }
    }
}

/// Hochaufloesender PNG-Frame fuer den Browser-Viewer. Anders als die zugesagte ANSI-Ausgabe
/// tastet dieser Weg die Originaltexturen ab. Die Szene wird genauso kurz kopiert und danach
/// ohne Weltsperre gerendert wie beim Terminalbild.
pub(crate) fn web_frame(
    shared: &Arc<Shared>,
    width: usize,
    height: usize,
) -> Result<Arc<[u8]>, String> {
    let position = shared
        .position()
        .ok_or_else(|| "Position noch unbekannt".to_string())?;
    let assets = shared.extras.pov.assets().ok_or_else(|| {
        shared
            .extras
            .pov
            .asset_note()
            .unwrap_or_else(|| "keine Ressourcen".to_string())
    })?;
    let width = width.clamp(160, 640);
    let height = height.clamp(90, 360);
    if width.saturating_mul(height) > 640 * 360 {
        return Err("Bild ist groesser als 640x360".to_string());
    }
    let mut scratch = shared.extras.pov.scratch.lock().unwrap();
    if scratch.web_size == (width, height)
        && scratch
            .web_rendered
            .is_some_and(|at| at.elapsed() < WEB_FRAME_CACHE)
    {
        if let Some(bytes) = &scratch.web_png {
            return Ok(Arc::clone(bytes));
        }
    }

    let bytes = {
        let Scratch {
            scene,
            pixels,
            web_rgba,
            ..
        } = &mut *scratch;
        shared.extras.pov.fill_scene(scene);
        render_textured(scene, position, width, height, assets, pixels);
        web_rgba.clear();
        web_rgba.reserve(width * height * 4);
        for pixel in pixels {
            web_rgba.extend_from_slice(&[pixel.rgb.0, pixel.rgb.1, pixel.rgb.2, 255]);
        }
        crate::pov_assets::encode_rgba_png(width, height, web_rgba)?
    };
    let bytes: Arc<[u8]> = bytes.into();
    scratch.web_size = (width, height);
    scratch.web_rendered = Some(Instant::now());
    scratch.web_png = Some(Arc::clone(&bytes));
    Ok(bytes)
}

fn render_textured(
    scene: &Scene,
    position: (f64, f64, f64, f32, f32),
    width: usize,
    height: usize,
    assets: &Assets,
    pixels: &mut Vec<Pixel>,
) {
    let origin = (position.0, position.1 + 1.62, position.2);
    let half_width = (HORIZONTAL_FOV.to_radians() / 2.0).tan();
    let half_height = half_width * height as f64 / width as f64;
    let basis = camera_basis(position.3 as f64, position.4 as f64);

    render_pixels(scene, width, height, pixels, |cursor, px, py| {
        let sky = sky_color(py, height);
        let direction = pixel_direction(basis, (half_width, half_height), (width, height), px, py);
        match cast_textured(cursor, origin, direction, assets) {
            Some(((rgba, distance, face), light)) => Pixel {
                rgb: fog(shade_face(rgba, face, light), sky, distance),
                depth: distance,
            },
            None => Pixel {
                rgb: sky,
                depth: MAX_DISTANCE,
            },
        }
    });
    // Der Browser legt absichtlich keine pinken/tuerkisen Ersatzrechtecke ueber die echten
    // Texturen. Fuer originalgetreue Entities braeuchte er Entity-Modelle, Metadaten, Ausruestung
    // und Skins, die dieser schlanke Weltzustand noch nicht fuehrt. Die alte Terminalausgabe
    // behaelt ihr kompaktes Overlay; die neue Ansicht erfindet an dieser Stelle nichts.
}

/// `\x1b[38;2;r;g;bm` ohne `format!` – bei 160x80 sind das sonst 12 800 Allokationen je Bild.
fn push_color(out: &mut String, prefix: &str, rgb: (u8, u8, u8)) {
    out.push_str(prefix);
    push_u8(out, rgb.0);
    out.push(';');
    push_u8(out, rgb.1);
    out.push(';');
    push_u8(out, rgb.2);
    out.push('m');
}

fn push_u8(out: &mut String, value: u8) {
    if value >= 100 {
        out.push((b'0' + value / 100) as char);
    }
    if value >= 10 {
        out.push((b'0' + (value / 10) % 10) as char);
    }
    out.push((b'0' + value % 10) as char);
}

/// Blickrichtung, Rechts- und Oben-Vektor der Kamera.
type Basis = ((f64, f64, f64), (f64, f64, f64), (f64, f64, f64));

fn camera_basis(yaw: f64, pitch: f64) -> Basis {
    let forward = view_direction(yaw, pitch);
    // Rechts liegt waagerecht (kein Rollen) – in Minecraft ist das (cos yaw, 0, sin yaw)·(-1).
    let (sin, cos) = yaw.to_radians().sin_cos();
    let right = (-cos, 0.0, -sin);
    let up = (
        right.1 * forward.2 - right.2 * forward.1,
        right.2 * forward.0 - right.0 * forward.2,
        right.0 * forward.1 - right.1 * forward.0,
    );
    (forward, right, up)
}

fn normalize(v: (f64, f64, f64)) -> (f64, f64, f64) {
    let length = (v.0 * v.0 + v.1 * v.1 + v.2 * v.2).sqrt();
    if length < 1e-12 {
        return (0.0, 0.0, 1.0);
    }
    (v.0 / length, v.1 / length, v.2 / length)
}

fn view_direction(yaw: f64, pitch: f64) -> (f64, f64, f64) {
    let yaw = yaw * PI / 180.0;
    let pitch = pitch * PI / 180.0;
    (
        -yaw.sin() * pitch.cos(),
        -pitch.sin(),
        yaw.cos() * pitch.cos(),
    )
}

/// Ein laufender Voxel-DDA: höchstens ein Schritt je durchquertem Block, nicht hunderte kleine
/// Ray-Schritte.
///
/// Ausgelagert, weil der farbige und der texturierte Strahl **denselben** Durchlauf brauchen und
/// sich nur darin unterscheiden, was sie bei einem Treffer tun. Vorher stand die Schrittlogik
/// zweimal da, und die beiden Fassungen waren bereits leicht auseinandergelaufen (die eine führte
/// die getroffene Seite mit, die andere nicht).
struct Ray {
    cell: (i32, i32, i32),
    /// Parameter, bei dem der Strahl die nächste Blockgrenze der jeweiligen Achse überschreitet.
    next: (f64, f64, f64),
    /// Parameterzuwachs je Blockschritt der jeweiligen Achse.
    delta: (f64, f64, f64),
    step: (i32, i32, i32),
    distance: f64,
    /// Achse, über deren Grenze der Strahl zuletzt eingetreten ist (0 = x, 1 = y, 2 = z).
    face: usize,
}

impl Ray {
    fn new(origin: (f64, f64, f64), dir: (f64, f64, f64)) -> Ray {
        let cell = (
            origin.0.floor() as i32,
            origin.1.floor() as i32,
            origin.2.floor() as i32,
        );
        Ray {
            cell,
            next: (
                first_boundary(origin.0, dir.0, cell.0),
                first_boundary(origin.1, dir.1, cell.1),
                first_boundary(origin.2, dir.2, cell.2),
            ),
            delta: (inv_abs(dir.0), inv_abs(dir.1), inv_abs(dir.2)),
            step: (sign(dir.0), sign(dir.1), sign(dir.2)),
            distance: 0.0,
            // Startet die Kamera in einem Block, gilt dessen Oberseite als getroffene Seite.
            face: 1,
        }
    }

    /// Ein Schritt: Es rückt die Achse weiter, deren nächste Blockgrenze am nächsten liegt.
    #[inline(always)]
    fn advance(&mut self) {
        if self.next.0 <= self.next.1 && self.next.0 <= self.next.2 {
            self.cell.0 += self.step.0;
            self.distance = self.next.0;
            self.next.0 += self.delta.0;
            self.face = 0;
        } else if self.next.1 <= self.next.2 {
            self.cell.1 += self.step.1;
            self.distance = self.next.1;
            self.next.1 += self.delta.1;
            self.face = 1;
        } else {
            self.cell.2 += self.step.2;
            self.distance = self.next.2;
            self.next.2 += self.delta.2;
            self.face = 2;
        }
    }

    /// Wie viele Blockschritte je Achse noch bleiben, bis der Strahl den 16er-Abschnitt verlässt,
    /// in dem er gerade steht.
    ///
    /// Damit lässt sich der Abschnitt genau einmal nachschlagen, statt bei jedem einzelnen Block
    /// erneut. Der Zähler ersetzt keinen Rechenschritt des Durchlaufs – die Schrittfolge bleibt
    /// Byte für Byte dieselbe wie ohne ihn; es fällt nur die Suche weg. Genau deshalb ist er
    /// gegenüber einer echten Abkürzung über die Abschnittsgrenze der sichere Weg: Dort müsste
    /// die Austrittsfläche geteilt, die Blockgrenzen dagegen aufaddiert werden, und bei einer
    /// Kamera genau auf einer Blockecke fallen beide Rechnungen um eine Zelle auseinander.
    #[inline(always)]
    fn steps_in_section(&self) -> (i32, i32, i32) {
        // `& 15` ist der Rest zur Sechzehn auch für negative Koordinaten (Zweierkomplement) –
        // dasselbe, was `local_index` mit `rem_euclid(16)` rechnet.
        let axis = |cell: i32, step: i32| {
            if step > 0 {
                16 - (cell & 15)
            } else {
                (cell & 15) + 1
            }
        };
        (
            axis(self.cell.0, self.step.0),
            axis(self.cell.1, self.step.1),
            axis(self.cell.2, self.step.2),
        )
    }
}

/// Farbiger Strahl für die Terminalansicht: liefert Blockzustand, Abstand und getroffene Achse.
fn cast(
    cursor: &mut Cursor,
    origin: (f64, f64, f64),
    dir: (f64, f64, f64),
) -> Option<(u32, f64, usize)> {
    let mut ray = Ray::new(origin, dir);
    while ray.distance <= MAX_DISTANCE {
        // Einmal je Abschnitt nachschlagen, dann darin Block für Block laufen. `None` heißt
        // „reine Luft oder gar nicht geladen" – durch beides läuft der Strahl ohne jede weitere
        // Prüfung durch, und das ist bei einer Ansicht in den Himmel der Normalfall.
        let section = cursor.section(ray.cell.0, ray.cell.1, ray.cell.2);
        let mut left = ray.steps_in_section();
        loop {
            if let Some(section) = section {
                let index = local_index(ray.cell.0, ray.cell.1, ray.cell.2);
                if !section.is_air(index) {
                    return Some((section.state(index), ray.distance, ray.face));
                }
            }
            if !step_within_section(&mut ray, &mut left) {
                break;
            }
        }
    }
    None
}

/// DDA fuer den Textur-Renderer. Der Treffer enthaelt die konkrete Seite und ihre UV-Koordinate;
/// ein voll transparenter Texel gilt nicht als Treffer. Dadurch werden Alpha-Ausschnitte echter
/// Vanilla-Texturen nicht wieder zu undurchsichtigen Farbklotzen.
/// Nachbarblock jenseits einer Fläche – dort steht das Licht, das sie beleuchtet, und nicht im
/// getroffenen Block selbst (der ist undurchsichtig und deshalb stockdunkel).
///
/// Reihenfolge wie in [`crate::pov_assets`]: west, ost, unten, oben, nord, süd.
const FACE_OFFSET: [(i32, i32, i32); 6] = [
    (-1, 0, 0),
    (1, 0, 0),
    (0, -1, 0),
    (0, 1, 0),
    (0, 0, -1),
    (0, 0, 1),
];

fn cast_textured(
    cursor: &mut Cursor,
    origin: (f64, f64, f64),
    dir: (f64, f64, f64),
    assets: &Assets,
) -> Option<(crate::pov_assets::TexturedHit, Option<u8>)> {
    let mut ray = Ray::new(origin, dir);
    while ray.distance <= MAX_DISTANCE {
        let section = cursor.section(ray.cell.0, ray.cell.1, ray.cell.2);
        let mut left = ray.steps_in_section();
        loop {
            if let Some(section) = section {
                let index = local_index(ray.cell.0, ray.cell.1, ray.cell.2);
                if !section.is_air(index) {
                    let leave = ray.next.0.min(ray.next.1).min(ray.next.2).min(MAX_DISTANCE);
                    let biome = cursor.biome_tint(ray.cell.0, ray.cell.1, ray.cell.2);
                    if let Some(hit) = assets.hit(
                        section.state(index),
                        ray.cell,
                        origin,
                        dir,
                        ray.distance,
                        leave,
                        &biome,
                    ) {
                        // Vanilla nimmt den helleren der beiden Werte: Eine Fackel erhellt eine
                        // Wand genauso wie die Sonne, und beides addiert sich nicht.
                        let (dx, dy, dz) = FACE_OFFSET[hit.2.min(5)];
                        let light = cursor
                            .light(ray.cell.0 + dx, ray.cell.1 + dy, ray.cell.2 + dz)
                            .map(|(sky, block)| sky.max(block));
                        return Some((hit, light));
                    }
                }
            }
            if !step_within_section(&mut ray, &mut left) {
                break;
            }
        }
    }
    None
}

/// Einen Block weiterrücken. `false` = der Strahl hat den Abschnitt verlassen oder seine
/// Reichweite erschöpft; dann muss der Aufrufer wieder nachschlagen.
#[inline(always)]
fn step_within_section(ray: &mut Ray, left: &mut (i32, i32, i32)) -> bool {
    ray.advance();
    if ray.distance > MAX_DISTANCE {
        return false;
    }
    // `advance` hat die Achse gesetzt, über deren Grenze wir gerade getreten sind – genau deren
    // Restzähler sinkt.
    let remaining = match ray.face {
        0 => &mut left.0,
        1 => &mut left.1,
        _ => &mut left.2,
    };
    *remaining -= 1;
    *remaining > 0
}

fn sign(value: f64) -> i32 {
    if value < 0.0 {
        -1
    } else {
        1
    }
}

fn inv_abs(value: f64) -> f64 {
    if value.abs() < 1e-12 {
        f64::INFINITY
    } else {
        1.0 / value.abs()
    }
}

fn first_boundary(origin: f64, direction: f64, cell: i32) -> f64 {
    if direction > 0.0 {
        (cell as f64 + 1.0 - origin) / direction
    } else if direction < 0.0 {
        (origin - cell as f64) / -direction
    } else {
        f64::INFINITY
    }
}

/// Seitenabhängige Helligkeit wie im Spiel: Oberseiten hell, Unterseiten dunkel, die vier
/// Seitenflächen dazwischen. Dieselben Faktoren benutzt auch das Gegenstands-Icon in
/// [`crate::pov_assets`].
///
/// Fullbright: Die Ansicht ignoriert bewusst die vom Server gemeldete Lichtstufe (`_light`) und
/// zeigt die Texturfarben so, wie sie in der JAR stehen – nur noch mit der Seitenabhängigkeit
/// verrechnet. Damit bleibt eine Höhle oder Nacht als Umriss erkennbar statt fast schwarz.
#[inline]
fn shade_face(rgba: (u8, u8, u8, u8), face: usize, _light: Option<u8>) -> (u8, u8, u8) {
    let side = match face {
        3 => 1.0,
        2 => 0.55,
        4 | 5 => 0.82,
        _ => 0.70,
    };
    let factor = side;
    (
        (rgba.0 as f64 * factor).min(255.0) as u8,
        (rgba.1 as f64 * factor).min(255.0) as u8,
        (rgba.2 as f64 * factor).min(255.0) as u8,
    )
}

fn sky_color(y: usize, height: usize) -> (u8, u8, u8) {
    let t = y as f64 / height.max(1) as f64;
    (
        (65.0 + 70.0 * t) as u8,
        (130.0 + 65.0 * t) as u8,
        (210.0 + 35.0 * t) as u8,
    )
}

fn block_color(state: u32, face: usize, distance: f64) -> (u8, u8, u8) {
    // Zustands-IDs sind pro Protokoll stabil, aber nicht namensgleich. Der multiplikative Hash
    // gibt jedem Blockzustand eine wiedererkennbare Farbe; benachbarte Flächen bleiben gleich.
    let hash = state.wrapping_mul(0x9E37_79B9).rotate_left(11);
    let mut rgb = (
        55 + (hash & 0x7F) as u8,
        55 + ((hash >> 8) & 0x7F) as u8,
        55 + ((hash >> 16) & 0x7F) as u8,
    );
    let face_light = match face {
        1 => 1.15,
        0 => 0.86,
        _ => 0.72,
    };
    let distance_light = (1.0 - distance / (MAX_DISTANCE * 1.8)).clamp(0.45, 1.0);
    let light = face_light * distance_light;
    rgb.0 = (rgb.0 as f64 * light).min(255.0) as u8;
    rgb.1 = (rgb.1 as f64 * light).min(255.0) as u8;
    rgb.2 = (rgb.2 as f64 * light).min(255.0) as u8;
    rgb
}

fn fog(color: (u8, u8, u8), sky: (u8, u8, u8), distance: f64) -> (u8, u8, u8) {
    let fog = (distance / MAX_DISTANCE).powi(2).clamp(0.0, 0.82);
    let mix = |a: u8, b: u8| (a as f64 * (1.0 - fog) + b as f64 * fog) as u8;
    (
        mix(color.0, sky.0),
        mix(color.1, sky.1),
        mix(color.2, sky.2),
    )
}

/// Entitäten über das gerasterte Bild legen.
///
/// Projiziert wird durch **dieselbe** Kamera wie die Strahlen: Richtung zur Entität in die
/// Kamerabasis zerlegen und durch die Tiefe teilen. Vorher lief das über eine lineare
/// Winkel-zu-Pixel-Rechnung – die stimmt nur genau in der Bildmitte und schob eine Entität bei
/// 90° Blickfeld am Bildrand um mehrere Zeichen neben den Block, auf dem sie steht.
fn overlay_entities(
    origin: (f64, f64, f64),
    basis: Basis,
    half: (f64, f64),
    width: usize,
    height: usize,
    entities: &[Entity],
    pixels: &mut [Pixel],
) {
    let (forward, right, up) = basis;
    let (half_width, half_height) = half;
    let dot = |a: (f64, f64, f64), b: (f64, f64, f64)| a.0 * b.0 + a.1 * b.1 + a.2 * b.2;

    for entity in entities {
        // Bezugspunkt ist die Körpermitte (Füße + 0,9); die Figur ist 1,8 Blöcke hoch.
        let to = (
            entity.x - origin.0,
            entity.y + 0.9 - origin.1,
            entity.z - origin.2,
        );
        let distance = dot(to, to).sqrt();
        if !(0.1..=MAX_DISTANCE).contains(&distance) {
            continue;
        }
        // Tiefe entlang der Blickachse. Alles dahinter oder daneben ist nicht im Bild.
        let depth = dot(to, forward);
        if depth <= 1e-6 {
            continue;
        }
        let sx = dot(to, right) / depth;
        let sy = dot(to, up) / depth;
        if sx.abs() > half_width * 1.5 || sy.abs() > half_height * 1.5 {
            continue;
        }

        let center_x = ((sx / (2.0 * half_width) + 0.5) * width as f64 - 0.5).round() as isize;
        let center_y = ((0.5 - sy / (2.0 * half_height)) * height as f64 - 0.5).round() as isize;
        // Höhe in Pixeln: 1,8 Blöcke auf Tiefe `depth` ergeben 1,8/depth in Bildkoordinaten.
        let entity_height = ((1.8 / depth) / (2.0 * half_height) * height as f64)
            .round()
            .clamp(1.0, height as f64) as isize;
        let entity_width = (entity_height / 3).max(1);
        let rgb = if entity.player {
            (70, 245, 255)
        } else {
            (255, 90, 145)
        };
        for py in center_y - entity_height / 2..=center_y + entity_height / 2 {
            for px in center_x - entity_width / 2..=center_x + entity_width / 2 {
                if px < 0 || py < 0 || px >= width as isize || py >= height as isize {
                    continue;
                }
                let pixel = &mut pixels[py as usize * width + px as usize];
                if distance < pixel.depth {
                    pixel.rgb = rgb;
                    pixel.depth = distance;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::Writer;

    /// Ein Lichtpaket für `sections` Weltabschnitte bauen: Himmelslicht nur in den in `lit`
    /// genannten **Licht**abschnitten, Blocklicht nirgends.
    fn light_bytes(sections: usize, lit: &[usize], level: u8) -> Vec<u8> {
        let mut w = Writer::default();
        let mut mask = 0u64;
        for index in lit {
            mask |= 1 << index;
        }
        w.var_int(1);
        w.i64(mask as i64);
        w.var_int(0); // Blockmaske: nichts
        w.var_int(0); // leer-Himmel
        w.var_int(0); // leer-Block
        w.var_int(lit.len() as i32);
        for _ in lit {
            w.var_int(LIGHT_BYTES as i32);
            for _ in 0..LIGHT_BYTES {
                w.u8(level << 4 | level);
            }
        }
        w.var_int(0); // keine Blocklichtfelder
        let _ = sections;
        w.data
    }

    fn empty_sky_light_bytes(index: usize) -> Vec<u8> {
        let mut w = Writer::default();
        w.var_int(0); // Himmelsmaske: kein neues Feld
        w.var_int(0); // Blockmaske
        w.var_int(1); // ein Long in der Leer-Himmelsmaske
        w.i64((1u64 << index) as i64);
        w.var_int(0); // Leer-Blockmaske
        w.var_int(0); // keine Himmelsfelder
        w.var_int(0); // keine Blockfelder
        w.data
    }

    /// Der Normalfall eines Chunks: Über Tage überall 15, darunter 0. Genau dafür gibt es die
    /// Zusammenfassung – ohne sie kostete jeder dieser Abschnitte 2 KiB.
    #[test]
    fn gleichmaessiges_licht_wird_zusammengefasst() {
        let bytes = light_bytes(24, &[10, 11], 15);
        let mut r = Reader::new(&bytes);
        let light = read_light(&mut r, -4, 24, None).expect("Lichtpaket");
        assert!(
            matches!(light.sky[10], LightSection::Uniform(15)),
            "ein durchgehend helles Feld muss zusammengefasst werden"
        );
        assert!(
            matches!(light.sky[0], LightSection::Uniform(0)),
            "ein Abschnitt ohne Feld ist dunkel – geraten wird nichts"
        );
        assert_eq!(r.remaining(), 0, "das Paket muss restlos gelesen sein");
    }

    /// Die Lichtabschnitte sind gegen die Weltabschnitte um einen verschoben: Lichtabschnitt 0
    /// liegt **unter** der Welt. Ein Fehler um eins wäre sonst genau ein Stockwerk daneben.
    #[test]
    fn lichtabschnitte_sind_um_einen_verschoben() {
        // min_section = -4 heißt: die Welt beginnt bei y = -64.
        let bytes = light_bytes(24, &[5], 12);
        let mut r = Reader::new(&bytes);
        let light = read_light(&mut r, -4, 24, None).expect("Lichtpaket");
        // Lichtabschnitt 5 = Weltabschnitt 4 = y 0..15.
        assert_eq!(light.get(0, 0, 0).0, 12);
        assert_eq!(light.get(0, 15, 0).0, 12);
        assert_eq!(light.get(0, 16, 0).0, 0, "der Abschnitt darüber ist dunkel");
        assert_eq!(light.get(0, -1, 0).0, 0, "der darunter auch");
    }

    /// Zwei Blöcke teilen sich ein Byte. Werden die Nibbles vertauscht, sieht jeder zweite Block
    /// falsch aus – und zwar so gleichmäßig, dass es wie ein Muster wirkt statt wie ein Fehler.
    #[test]
    fn nibbles_liegen_richtig_herum() {
        let mut data = [0u8; LIGHT_BYTES];
        // Block 0 (unteres Nibble) hell, Block 1 (oberes Nibble) dunkel.
        data[0] = 0x0F;
        let section = compact_light(&data);
        assert_eq!(section.get(0), 15);
        assert_eq!(section.get(1), 0);
        assert!(
            matches!(section, LightSection::Nibbles(_)),
            "ein ungleiches Feld darf nicht zusammengefasst werden"
        );
    }

    /// Über der Welt steht nichts mehr im Weg – dort ist voller Himmel, nicht Dunkelheit.
    /// Andernfalls bekäme die oberste Blockschicht der Welt eine schwarze Oberseite.
    #[test]
    fn ueber_der_welt_ist_voller_himmel() {
        let bytes = light_bytes(24, &[], 0);
        let mut r = Reader::new(&bytes);
        let light = read_light(&mut r, -4, 24, None).expect("Lichtpaket");
        assert_eq!(light.get(0, 5000, 0), (15, 0));
        assert_eq!(light.get(0, -5000, 0), (0, 0));
    }

    /// Ein LightUpdate enthaelt nur die geaenderten Abschnitte. Nicht gesetzte Bits bedeuten
    /// dort „alten Wert behalten", nicht „auf null setzen". Letzteres machte nach einer
    /// einzelnen Fackel- oder Tageslichtaenderung fast die gesamte Browser-POV schwarz.
    #[test]
    fn teilweises_lichtupdate_behaelt_unveraenderte_abschnitte() {
        let initial = light_bytes(24, &[5, 10], 15);
        let mut r = Reader::new(&initial);
        let old = read_light(&mut r, -4, 24, None).expect("Ausgangslicht");

        let update = light_bytes(24, &[5], 7);
        let mut r = Reader::new(&update);
        let patched = read_light(&mut r, -4, 24, Some(&old)).expect("Lichtpatch");

        assert_eq!(patched.sky[5].get(0), 7, "genannter Abschnitt wird ersetzt");
        assert_eq!(
            patched.sky[10].get(0),
            15,
            "nicht genannter Abschnitt muss erhalten bleiben"
        );

        let emptied = empty_sky_light_bytes(10);
        let mut r = Reader::new(&emptied);
        let patched = read_light(&mut r, -4, 24, Some(&patched)).expect("Leer-Patch");
        assert_eq!(
            patched.sky[10].get(0),
            0,
            "ausdruecklich geleerter Abschnitt wird dunkel"
        );
    }

    const LEGACY: Format = Format {
        modern_palette: false,
        fluid_count: false,
    };
    const MODERN: Format = Format {
        modern_palette: true,
        fluid_count: false,
    };
    const MODERN_FLUID: Format = Format {
        modern_palette: true,
        fluid_count: true,
    };

    fn dimension(sections: i32) -> Dimension {
        Dimension {
            name: "test".into(),
            min_y: 0,
            height: sections * 16,
        }
    }

    #[test]
    fn blickrichtung_folgt_minecraft_winkeln() {
        let south = view_direction(0.0, 0.0);
        assert!(south.0.abs() < 1e-9 && south.2 > 0.999);
        let west = view_direction(90.0, 0.0);
        assert!(west.0 < -0.999 && west.2.abs() < 1e-9);
        assert!(view_direction(0.0, -90.0).1 > 0.999);
    }

    /// Die Kamerabasis muss rechtwinklig sein, sonst kippt oder schert das Bild.
    #[test]
    fn kamerabasis_steht_senkrecht() {
        for (yaw, pitch) in [(0.0, 0.0), (37.0, -20.0), (-140.0, 45.0)] {
            let (forward, right, up) = camera_basis(yaw, pitch);
            let dot = |a: (f64, f64, f64), b: (f64, f64, f64)| a.0 * b.0 + a.1 * b.1 + a.2 * b.2;
            assert!(dot(forward, right).abs() < 1e-9, "{}/{}", yaw, pitch);
            assert!(dot(forward, up).abs() < 1e-9, "{}/{}", yaw, pitch);
            assert!(dot(right, up).abs() < 1e-9, "{}/{}", yaw, pitch);
            // Rechts zeigt bei Blick nach Süden nach Westen (−X) – wie `:go rechts`.
            if yaw == 0.0 {
                assert!(right.0 < -0.999);
            }
        }
    }

    /// Entitäten müssen durch **dieselbe** Kamera projiziert werden wie die Strahlen: Setzt man
    /// eine genau auf den Strahl eines Bildpunkts, muss sie auch auf diesem Bildpunkt landen.
    /// Die frühere lineare Winkelrechnung stimmte nur in der Bildmitte und schob eine Entität am
    /// Rand um mehrere Zeichen neben den Block, auf dem sie steht.
    #[test]
    fn entitaeten_landen_auf_dem_strahl_ihres_bildpunkts() {
        let (width, height) = (64usize, 32usize);
        let half_width = (HORIZONTAL_FOV.to_radians() / 2.0).tan();
        let half_height = half_width * height as f64 / width as f64;
        let origin = (100.5, 70.0, -40.25);

        for (yaw, pitch) in [(0.0, 0.0), (37.0, -12.0), (-140.0, 25.0)] {
            let basis = camera_basis(yaw, pitch);
            // Bewusst auch die vier Ecken: dort war der Fehler am größten.
            for (px, py) in [
                (0usize, 0usize),
                (63, 0),
                (0, 31),
                (63, 31),
                (32, 16),
                (10, 25),
            ] {
                let sx = ((px as f64 + 0.5) / width as f64 - 0.5) * 2.0 * half_width;
                let sy = (0.5 - (py as f64 + 0.5) / height as f64) * 2.0 * half_height;
                let dir = normalize((
                    basis.0 .0 + basis.1 .0 * sx + basis.2 .0 * sy,
                    basis.0 .1 + basis.1 .1 * sx + basis.2 .1 * sy,
                    basis.0 .2 + basis.1 .2 * sx + basis.2 .2 * sy,
                ));
                let away = 20.0;
                let entity = Entity {
                    x: origin.0 + dir.0 * away,
                    // `overlay_entities` rechnet ab Körpermitte (Füße + 0,9).
                    y: origin.1 + dir.1 * away - 0.9,
                    z: origin.2 + dir.2 * away,
                    player: true,
                };
                let mut pixels = vec![
                    Pixel {
                        rgb: (0, 0, 0),
                        depth: MAX_DISTANCE
                    };
                    width * height
                ];
                overlay_entities(
                    origin,
                    basis,
                    (half_width, half_height),
                    width,
                    height,
                    &[entity],
                    &mut pixels,
                );
                assert_eq!(
                    pixels[py * width + px].rgb,
                    (70, 245, 255),
                    "Blick {}/{}: Entität auf dem Strahl von {}/{} traf ihn nicht",
                    yaw,
                    pitch,
                    px,
                    py
                );
            }
        }
    }

    /// Was hinter der Kamera steht, darf nicht im Bild auftauchen – die alte Rechnung über den
    /// Neigungswinkel konnte einen Punkt im Rücken vorn einblenden.
    #[test]
    fn entitaeten_hinter_der_kamera_bleiben_draussen() {
        let (width, height) = (32usize, 16usize);
        let half_width = (HORIZONTAL_FOV.to_radians() / 2.0).tan();
        let half_height = half_width * height as f64 / width as f64;
        let basis = camera_basis(0.0, 0.0); // Blick nach Süden (+Z)
        let origin = (0.0, 64.0, 0.0);
        let entity = Entity {
            x: 0.0,
            y: 63.1,
            z: -8.0, // genau im Rücken
            player: false,
        };
        let mut pixels = vec![
            Pixel {
                rgb: (0, 0, 0),
                depth: MAX_DISTANCE
            };
            width * height
        ];
        overlay_entities(
            origin,
            basis,
            (half_width, half_height),
            width,
            height,
            &[entity],
            &mut pixels,
        );
        assert!(
            pixels.iter().all(|p| p.rgb == (0, 0, 0)),
            "eine Entität hinter der Kamera wurde gezeichnet"
        );
    }

    #[test]
    fn blockpositionen_mit_negativen_werten() {
        let pack = |x: i64, y: i64, z: i64| {
            ((x & 0x3ff_ffff) << 38) | ((z & 0x3ff_ffff) << 12) | (y & 0xfff)
        };
        assert_eq!(unpack_block_pos(pack(-12, -64, 99)), (-12, -64, 99));

        let legacy = |x: i64, y: i64, z: i64| {
            ((x & 0x3ff_ffff) << 38) | ((y & 0xfff) << 26) | (z & 0x3ff_ffff)
        };
        assert_eq!(
            unpack_legacy_block_pos(legacy(-12, 255, -99)),
            (-12, 255, -99)
        );
    }

    #[test]
    fn chunkposition_ist_nicht_netzwerkreihenfolge() {
        let packed = ((-99i32 as u32 as u64) << 32) | 42u32 as u64;
        assert_eq!(unpack_chunk_pos(packed as i64), (42, -99));
    }

    /// Ein Abschnitt: Zähler, Blockpalette, Biompalette. `states` sind 4096 Zustands-IDs.
    fn write_section(w: &mut Writer, format: Format, states: &[u32], non_air: usize) {
        w.u16(non_air as u16);
        if format.fluid_count {
            w.u16(0);
        }
        let mut palette: Vec<u32> = Vec::new();
        for state in states {
            if !palette.contains(state) {
                palette.push(*state);
            }
        }
        if palette.len() == 1 {
            w.u8(0);
            w.var_int(palette[0] as i32);
            if !format.modern_palette {
                w.var_int(0);
            }
        } else {
            let bits = (4u8..=8).find(|b| 1usize << b >= palette.len()).unwrap();
            w.u8(bits);
            w.var_int(palette.len() as i32);
            for value in &palette {
                w.var_int(*value as i32);
            }
            let per_long = 64 / bits as usize;
            let longs = SECTION_BLOCKS.div_ceil(per_long);
            if !format.modern_palette {
                w.var_int(longs as i32);
            }
            let mut data = vec![0u64; longs];
            for (index, state) in states.iter().enumerate() {
                let slot = palette.iter().position(|v| v == state).unwrap() as u64;
                data[index / per_long] |= slot << ((index % per_long) * bits as usize);
            }
            for long in data {
                w.i64(long as i64);
            }
        }
        write_biomes(w, format, &[1; SECTION_BIOMES]);
    }

    /// Die Biompalette eines Abschnitts schreiben – einwertig oder mit echten Zellen.
    fn write_biomes(w: &mut Writer, format: Format, biomes: &[u32; SECTION_BIOMES]) {
        let mut palette: Vec<u32> = Vec::new();
        for biome in biomes {
            if !palette.contains(biome) {
                palette.push(*biome);
            }
        }
        if palette.len() == 1 {
            w.u8(0);
            w.var_int(palette[0] as i32);
            if !format.modern_palette {
                w.var_int(0);
            }
            return;
        }
        // Biome benutzen 1..=3 Bit indirekt; darüber wäre es die globale Palette.
        let bits = (1u8..=3).find(|b| 1usize << b >= palette.len()).unwrap();
        w.u8(bits);
        w.var_int(palette.len() as i32);
        for value in &palette {
            w.var_int(*value as i32);
        }
        let per_long = 64 / bits as usize;
        let longs = SECTION_BIOMES.div_ceil(per_long);
        if !format.modern_palette {
            w.var_int(longs as i32);
        }
        let mut data = vec![0u64; longs];
        for (index, biome) in biomes.iter().enumerate() {
            let slot = palette.iter().position(|v| v == biome).unwrap() as u64;
            data[index / per_long] |= slot << ((index % per_long) * bits as usize);
        }
        for long in data {
            w.i64(long as i64);
        }
    }

    /// Die Biompalette steht **hinter** der Blockpalette im selben Abschnitt. Sie zu lesen heißt
    /// also auch, die Blockpalette exakt zu Ende gelesen zu haben – ein Byte daneben, und die
    /// Biome sind Zufallszahlen.
    #[test]
    fn biompalette_kommt_zellengenau_zurueck() {
        let mut biomes = [0u32; SECTION_BIOMES];
        for (index, biome) in biomes.iter_mut().enumerate() {
            *biome = (index % 3) as u32;
        }
        for format in [LEGACY, MODERN, MODERN_FLUID] {
            let mut w = Writer::default();
            w.u16(SECTION_BLOCKS as u16);
            if format.fluid_count {
                w.u16(0);
            }
            // Blockpalette: durchgehend Stein, damit der Abschnitt erhalten bleibt.
            w.u8(0);
            w.var_int(1);
            if !format.modern_palette {
                w.var_int(0);
            }
            write_biomes(&mut w, format, &biomes);

            let chunk = Chunk::decode(format, &dimension(1), &w.data).unwrap();
            let section = chunk.section(0).expect("Abschnitt");
            for (index, biome) in biomes.iter().enumerate() {
                // Zellenreihenfolge y, z, x – je vier Blöcke eine Zelle.
                let (x, y, z) = (
                    ((index & 3) * 4) as i32,
                    ((index >> 4) * 4) as i32,
                    (((index >> 2) & 3) * 4) as i32,
                );
                assert_eq!(
                    section.biomes.get(x, y, z) as u32,
                    *biome,
                    "Zelle {} in {:?}",
                    index,
                    format
                );
            }
        }
    }

    /// Der Normalfall: ein Abschnitt, ein Biom – dann steht dort keine Zellentabelle.
    #[test]
    fn einwertige_biompalette_bleibt_einwertig() {
        let mut w = Writer::default();
        write_section(&mut w, MODERN, &[1u32; SECTION_BLOCKS], SECTION_BLOCKS);
        let chunk = Chunk::decode(MODERN, &dimension(1), &w.data).unwrap();
        let section = chunk.section(0).expect("Abschnitt");
        assert!(matches!(section.biomes, Biomes::Single(1)));
        assert_eq!(section.biomes.get(7, 9, 11), 1);
    }

    #[test]
    fn singleton_abschnitt_legacy_und_modern() {
        for format in [LEGACY, MODERN, MODERN_FLUID] {
            let mut w = Writer::default();
            write_section(&mut w, format, &[0u32; SECTION_BLOCKS], 0);
            let chunk = Chunk::decode(format, &dimension(1), &w.data).unwrap();
            assert_eq!(chunk.block(0, 0, 0), 0);
            assert!(
                chunk.sections[0].is_none(),
                "reine Luft braucht keinen Platz"
            );
        }
    }

    /// Genau der Fall, der bisher den ganzen Prozess abgeschossen hat: ein Luftzustand, dessen
    /// Häufigkeit größer ist als die Zahl der gesuchten Luftblöcke.
    #[test]
    fn haeufigkeit_groesser_als_luftmenge_stuerzt_nicht_ab() {
        let values = [7u32, 42];
        let counts = [394u32, 12];
        let air = air_entries(&values, &counts, 90);
        assert!(!air[0] && !air[1], "nichts darf erfunden werden");
    }

    /// Der Regelfall unter Tage: Zustand 0 fehlt, `cave_air` trägt die ganze Luftmenge.
    #[test]
    fn cave_air_wird_ueber_den_blockzaehler_erkannt() {
        let values = [12u32, 5000, 1];
        let counts = [1000u32, 2000, 1096];
        let air = air_entries(&values, &counts, 2000);
        assert!(air[1] && !air[0] && !air[2]);
    }

    /// Drei Luftzustände (air + cave_air + void_air) müssen sich ebenfalls auflösen lassen.
    #[test]
    fn drei_luftzustaende_werden_gefunden() {
        let values = [0u32, 5000, 6000, 1];
        let counts = [100u32, 200, 300, 3496];
        let air = air_entries(&values, &counts, 600);
        assert!(air[0] && air[1] && air[2] && !air[3]);
    }

    #[test]
    fn luft_wird_ueber_blockzaehler_erkannt() {
        let mut states = vec![7u32; SECTION_BLOCKS];
        states[..3000].fill(42); // cave_air-artiger zweiter Luftzustand
        states[3000..3500].fill(0);
        let mut w = Writer::default();
        write_section(&mut w, MODERN, &states, SECTION_BLOCKS - 3500);
        let chunk = Chunk::decode(MODERN, &dimension(1), &w.data).unwrap();
        for index in 0..SECTION_BLOCKS {
            let (x, y, z) = (
                (index & 15) as i32,
                (index >> 8) as i32,
                ((index >> 4) & 15) as i32,
            );
            if index < 3500 {
                assert_eq!(chunk.block(x, y, z), 0, "Block {} sollte Luft sein", index);
            } else {
                assert_eq!(chunk.block(x, y, z), 7, "Block {}", index);
            }
        }
    }

    /// Ein voller Abschnitt mit mehreren Zuständen muss Block für Block wieder herauskommen.
    #[test]
    fn palette_kommt_blockgenau_zurueck() {
        let mut states = vec![0u32; SECTION_BLOCKS];
        for (index, state) in states.iter_mut().enumerate() {
            *state = (index % 5) as u32 + 1;
        }
        for format in [LEGACY, MODERN, MODERN_FLUID] {
            let mut w = Writer::default();
            write_section(&mut w, format, &states, SECTION_BLOCKS);
            let chunk = Chunk::decode(format, &dimension(1), &w.data).unwrap();
            for (index, state) in states.iter().enumerate() {
                let (x, y, z) = (
                    (index & 15) as i32,
                    (index >> 8) as i32,
                    ((index >> 4) & 15) as i32,
                );
                assert_eq!(chunk.block(x, y, z), *state, "Block {}", index);
            }
        }
    }

    /// Spricht der Server doch eine andere Spielart, findet `probe` sie – statt jeden Chunk zu
    /// verwerfen, bis jemand die Tabelle von Hand nachzieht.
    #[test]
    fn falsches_format_wird_erkannt() {
        let states = vec![1u32; SECTION_BLOCKS];
        let mut w = Writer::default();
        write_section(&mut w, MODERN_FLUID, &states, SECTION_BLOCKS);
        let (chunk, format) = Format::probe(LEGACY, &dimension(1), &w.data).expect("erkannt");
        assert_eq!(format, MODERN_FLUID);
        assert_eq!(chunk.block(0, 0, 0), 1);
    }

    /// Setzt der Server einen Block, darf das den Rest des Chunks nicht verändern.
    #[test]
    fn blockaenderung_trifft_nur_den_einen_block() {
        let states = vec![1u32; SECTION_BLOCKS];
        let mut w = Writer::default();
        write_section(&mut w, MODERN, &states, SECTION_BLOCKS);
        let mut chunk = Chunk::decode(MODERN, &dimension(1), &w.data).unwrap();
        chunk.set_block(3, 4, 5, 77);
        assert_eq!(chunk.block(3, 4, 5), 77);
        assert_eq!(chunk.block(3, 4, 6), 1);
        chunk.set_block(3, 4, 5, 0);
        assert_eq!(chunk.block(3, 4, 5), 0);
        assert!(!chunk.solid(3, 4, 5));
    }

    /// Ein Chunk mit Boden in Abschnitt 4 – nur für den Messlauf unten.
    fn ground_chunk() -> Arc<Chunk> {
        let mut section = Section::empty();
        // Die untersten beiden Schichten des Abschnitts füllen (y = 0 und 1).
        for index in 0..512 {
            section.set(index, 1);
        }
        let mut sections: Vec<Option<Arc<Section>>> = vec![None; 24];
        sections[4] = Some(Arc::new(section));
        Arc::new(Chunk {
            min_section: -4,
            sections,
            light: None,
        })
    }

    /// Ein Chunk mit unruhigem Gelände: so trifft ein Strahl auch mitten in einem Abschnitt
    /// auf einen Block, statt immer nur auf eine ebene Fläche.
    fn hilly_chunk(seed: i32) -> Arc<Chunk> {
        let mut sections: Vec<Option<Arc<Section>>> = vec![None; 24];
        for (offset, index) in [4usize, 5].into_iter().enumerate() {
            let mut section = Section::empty();
            for x in 0..16i32 {
                for z in 0..16i32 {
                    // Deterministische, aber zerklüftete Höhe je Säule.
                    let height = ((x * 7 + z * 13 + seed * 5).rem_euclid(9)) - offset as i32 * 4;
                    for y in 0..height.clamp(0, 16) {
                        section.set(local_index(x, y, z), (1 + (x + z + y) % 3) as u32);
                    }
                }
            }
            sections[index] = Some(Arc::new(section));
        }
        Arc::new(Chunk {
            min_section: -4,
            sections,
            light: None,
        })
    }

    /// Eine kleine Landschaft aus **echten** Blockzuständen von 1.21.1: Gras auf Stein, ein See,
    /// Sand am Ufer und ein Baum aus Stamm und Laub.
    ///
    /// Nur für die beiden Sichtprüfungen gedacht (`pov_als_png`): Erst mit richtigen Zuständen
    /// lässt sich sehen, ob Wasser blau, Laub grün und Sand sandfarben herauskommt.
    fn landscape_chunk(chunk_x: i32, chunk_z: i32) -> Arc<Chunk> {
        const STONE: u32 = 1;
        const GRASS: u32 = 9;
        const WATER: u32 = 80;
        const SAND: u32 = 112;
        const LOG: u32 = 131;
        const LEAVES: u32 = 264;

        let mut section = Section::empty();
        for x in 0..16i32 {
            for z in 0..16i32 {
                let world_x = chunk_x * 16 + x;
                let world_z = chunk_z * 16 + z;
                // Eine Senke um den Ursprung herum, gefüllt mit Wasser.
                let lake = world_x * world_x + world_z * world_z < 26 * 26;
                for y in 0..4i32 {
                    section.set(local_index(x, y, z), STONE);
                }
                if lake {
                    section.set(local_index(x, 4, z), SAND);
                    for y in 5..7i32 {
                        section.set(local_index(x, y, z), WATER);
                    }
                } else {
                    section.set(local_index(x, 4, z), STONE);
                    section.set(local_index(x, 5, z), GRASS);
                }
            }
        }
        // Ein Baum je Chunk, damit auch Stamm und Laub im Bild sind.
        if (chunk_x + chunk_z).rem_euclid(2) == 0 {
            let (tx, tz) = (4i32, 11i32);
            for y in 6..10i32 {
                section.set(local_index(tx, y, tz), LOG);
            }
            for dx in -2..=2i32 {
                for dz in -2..=2i32 {
                    for y in 9..12i32 {
                        if dx == 0 && dz == 0 && y < 11 {
                            continue;
                        }
                        let (lx, lz) = (tx + dx, tz + dz);
                        if (0..16).contains(&lx) && (0..16).contains(&lz) {
                            section.set(local_index(lx, y, lz), LEAVES);
                        }
                    }
                }
            }
        }
        let mut sections: Vec<Option<Arc<Section>>> = vec![None; 24];
        sections[4] = Some(Arc::new(section));
        Arc::new(Chunk {
            min_section: -4,
            sections,
            light: None,
        })
    }

    /// Der Voxel-Durchlauf **ohne** jede Abkürzung: Block für Block, genau nach Lehrbuch, und mit
    /// einem direkten Griff in die Hashtabelle statt über [`Cursor`] und [`Ray`].
    ///
    /// Maßstab für den optimierten [`cast`]. Bewusst mit eigener Schrittlogik statt mit
    /// [`Ray::advance`]: Ein Maßstab, der sich denselben Code teilt wie das Geprüfte, prüft nur
    /// noch, dass er sich selbst gleicht.
    fn cast_naive(
        scene: &Scene,
        origin: (f64, f64, f64),
        dir: (f64, f64, f64),
    ) -> Option<(u32, f64, usize)> {
        let mut cell = (
            origin.0.floor() as i32,
            origin.1.floor() as i32,
            origin.2.floor() as i32,
        );
        let step = (sign(dir.0), sign(dir.1), sign(dir.2));
        let delta = (inv_abs(dir.0), inv_abs(dir.1), inv_abs(dir.2));
        let mut next = (
            first_boundary(origin.0, dir.0, cell.0),
            first_boundary(origin.1, dir.1, cell.1),
            first_boundary(origin.2, dir.2, cell.2),
        );
        let mut distance = 0.0;
        let mut face = 1usize;
        while distance <= MAX_DISTANCE {
            let block = scene
                .chunks
                .get(&(cell.0.div_euclid(16), cell.2.div_euclid(16)))
                .filter(|chunk| chunk.solid(cell.0, cell.1, cell.2))
                .map(|chunk| chunk.block(cell.0, cell.1, cell.2));
            if let Some(state) = block {
                return Some((state, distance, face));
            }
            if next.0 <= next.1 && next.0 <= next.2 {
                cell.0 += step.0;
                distance = next.0;
                next.0 += delta.0;
                face = 0;
            } else if next.1 <= next.2 {
                cell.1 += step.1;
                distance = next.1;
                next.1 += delta.1;
                face = 1;
            } else {
                cell.2 += step.2;
                distance = next.2;
                next.2 += delta.2;
                face = 2;
            }
        }
        None
    }

    /// Das Überspringen leerer Abschnitte darf am Bild **nichts** ändern – es soll nur Arbeit
    /// sparen. Genau hier lauert der Fehler: Die Austrittsfläche eines Abschnitts wird einmal
    /// geteilt, die Blockgrenzen des Feindurchlaufs entstehen durch wiederholtes Addieren.
    /// Nimmt die Rundung die Grenze nicht mit, bleibt der Strahl im selben Abschnitt stehen und
    /// die Schleife läuft ewig – der ganze Client hängt dann still am Zeichnen.
    ///
    /// Deshalb hier zehntausend Strahlen in alle Richtungen gegen die Lehrbuchfassung, quer
    /// durch geladene und ungeladene Chunks und aus Kamerapositionen genau auf Blockgrenzen.
    #[test]
    fn abkuerzung_liefert_dasselbe_wie_der_einzelschritt() {
        let mut chunks = HashMap::new();
        for x in -3..=3i32 {
            for z in -3..=3i32 {
                // Ein paar Löcher: dort ist gar kein Chunk geladen.
                if (x + z).rem_euclid(5) == 0 {
                    continue;
                }
                chunks.insert((x, z), hilly_chunk(x * 31 + z));
            }
        }
        let scene = Scene {
            chunks,
            entities: Vec::new(),
            chunk_revision: 0,
            ..Scene::default()
        };

        // Auch genau auf Blockgrenzen und Abschnittsgrenzen: dort trifft die Rundung zu.
        let origins = [
            (8.0, 19.62, 8.0),
            (0.0, 16.0, 0.0),
            (16.0, 32.0, -16.0),
            (-0.5, 5.5, 0.5),
            (7.3, 70.0, -13.9),
            (0.0, 0.0, 0.0),
        ];
        let mut checked = 0;
        for origin in origins {
            for yaw_step in 0..90 {
                for pitch_step in 0..19 {
                    let yaw = yaw_step as f64 * 4.0 - 180.0;
                    let pitch = pitch_step as f64 * 10.0 - 90.0;
                    let dir = view_direction(yaw, pitch);
                    let mut cursor = Cursor::new(&scene);
                    let quick = cast(&mut cursor, origin, dir);
                    let naive = cast_naive(&scene, origin, dir);
                    match (quick, naive) {
                        (None, None) => {}
                        (Some((a, da, fa)), Some((b, db, fb))) => {
                            assert_eq!(a, b, "Block bei {:?} / {}/{}", origin, yaw, pitch);
                            assert_eq!(fa, fb, "Seite bei {:?} / {}/{}", origin, yaw, pitch);
                            assert!(
                                (da - db).abs() < 1e-6,
                                "Abstand bei {:?} / {}/{}: {} != {}",
                                origin,
                                yaw,
                                pitch,
                                da,
                                db
                            );
                        }
                        (a, b) => panic!(
                            "unterschiedliches Ergebnis bei {:?} / {}/{}: {:?} vs {:?}",
                            origin,
                            yaw,
                            pitch,
                            a.map(|v| v.0),
                            b.map(|v| v.0)
                        ),
                    }
                    checked += 1;
                }
            }
        }
        assert_eq!(checked, origins.len() * 90 * 19);
    }

    /// Messlauf statt Behauptung: wie lange braucht ein Bild wirklich?
    ///
    /// Gemessen wird genau das, was [`draw_once`] tut: Szene übernehmen **und** in die
    /// wiederverwendeten Puffer zeichnen. Vorher lief hier die bequeme Testfassung `render`, die
    /// ihre Puffer jedes Mal neu anlegt – das macht der Client seit 2.2.0 gerade nicht mehr, die
    /// Zahl beschrieb also einen Weg, den es so nicht mehr gibt.
    ///
    /// Läuft nicht im normalen Testlauf mit – die Zahl hängt vom Rechner ab. Aufruf:
    /// `cargo test --release --features ultra -- --ignored --nocapture bildrate`
    #[test]
    #[ignore]
    fn messlauf_bildrate() {
        let pov = Pov::new(&crate::options::Options::default());
        {
            let mut world = pov.world.lock().unwrap();
            // 13x13 Chunks – genau so viele behält die Ansicht nach dem Aufräumen.
            for x in -6..=6i32 {
                for z in -6..=6i32 {
                    world.chunks.insert((x, z), ground_chunk());
                }
            }
        }
        let position = (8.0, 18.0, 8.0, 30.0f32, -10.0f32);
        let mut scene = Scene::default();
        let mut pixels = Vec::new();
        let mut frame = String::new();

        // Die dritte Größe ist die des Browser-Viewers: dort liegt die eigentliche Rechenlast.
        for (width, height) in [(64usize, 32usize), (160, 80), (426, 240)] {
            // Einmal warmlaufen, damit die Messung nicht den ersten Zugriff mitzählt.
            pov.fill_scene(&mut scene);
            render_into(
                &scene,
                position,
                width,
                height,
                true,
                &mut pixels,
                &mut frame,
            );

            let runs = 200;
            let started = Instant::now();
            for _ in 0..runs {
                pov.fill_scene(&mut scene);
                render_into(
                    &scene,
                    position,
                    width,
                    height,
                    true,
                    &mut pixels,
                    &mut frame,
                );
                std::hint::black_box(&frame);
            }
            let each = started.elapsed() / runs;
            println!(
                "{}x{}: {:?} je Bild  ->  {:.0} Bilder/s möglich",
                width,
                height,
                each,
                1.0 / each.as_secs_f64()
            );
        }
    }

    /// Ob ein Bild auf einem oder auf vier Kernen gerechnet wird, darf an keinem einzigen
    /// Bildpunkt zu sehen sein.
    ///
    /// Der Browser-Viewer nimmt immer den geteilten Weg, die Terminalansicht nie – ohne diesen
    /// Vergleich liefe die geteilte Fassung also völlig ungeprüft. Gerechnet wird deshalb
    /// dieselbe Szene zweimal und Bildpunkt gegen Bildpunkt verglichen, samt Tiefenwert.
    #[test]
    fn geteiltes_rechnen_ergibt_dasselbe_bild() {
        let mut chunks = HashMap::new();
        for x in -3..=3i32 {
            for z in -3..=3i32 {
                // Ein paar Löcher: dort ist gar kein Chunk geladen.
                if (x + z).rem_euclid(7) == 0 {
                    continue;
                }
                chunks.insert((x, z), hilly_chunk(x * 17 + z));
            }
        }
        let scene = Scene {
            chunks,
            entities: Vec::new(),
            chunk_revision: 0,
            ..Scene::default()
        };

        // Auch eine Zeilenzahl, die nicht glatt aufgeht (77 Zeilen auf 3 Threads).
        for (width, height, workers) in [(40usize, 20usize, 4usize), (33, 77, 3), (16, 9, 8)] {
            let position = (8.0, 18.0, 8.0, 37.0f32, -12.0f32);
            let half_width = (HORIZONTAL_FOV.to_radians() / 2.0).tan();
            let half_height = half_width * height as f64 / width as f64;
            let basis = camera_basis(position.3 as f64, position.4 as f64);
            let shade = |cursor: &mut Cursor, px: usize, py: usize| {
                let sky = sky_color(py, height);
                let direction =
                    pixel_direction(basis, (half_width, half_height), (width, height), px, py);
                match cast(
                    cursor,
                    (position.0, position.1 + 1.62, position.2),
                    direction,
                ) {
                    Some((state, distance, face)) => Pixel {
                        rgb: fog(block_color(state, face, distance), sky, distance),
                        depth: distance,
                    },
                    None => Pixel {
                        rgb: sky,
                        depth: MAX_DISTANCE,
                    },
                }
            };

            let mut single = Vec::new();
            render_pixels_with(&scene, width, height, 1, &mut single, shade);
            let mut many = Vec::new();
            render_pixels_with(&scene, width, height, workers, &mut many, shade);

            assert_eq!(single.len(), width * height);
            for (index, (a, b)) in single.iter().zip(many.iter()).enumerate() {
                assert_eq!(
                    a.rgb,
                    b.rgb,
                    "Bildpunkt {}/{} weicht ab ({}x{}, {} Threads)",
                    index % width,
                    index / width,
                    width,
                    height,
                    workers
                );
                assert_eq!(a.depth.to_bits(), b.depth.to_bits(), "Tiefe bei {}", index);
            }
            // Und das Bild darf nicht einfach überall Himmel sein, sonst prüft der Test nichts.
            assert!(
                single.iter().any(|pixel| pixel.depth < MAX_DISTANCE),
                "{}x{}: kein einziger Treffer im Bild",
                width,
                height
            );
        }
    }

    /// Der Kern des Versprechens: Die Browser-Ansicht zeigt **echte** Minecraft-Texturen.
    ///
    /// Geprüft wird gegen eine wirklich geladene Original-JAR, nicht gegen eine Nachbildung: Boden
    /// aus Stein, Kamera darüber, Bild rendern – und dann nachsehen, ob unten wirklich der
    /// Steinton steht und nirgends die lila-schwarze Fehlertextur.
    ///
    /// Läuft nicht im normalen Testlauf mit (braucht Netz bzw. eine vorhandene Installation):
    /// `XDG_CONFIG_HOME=$(mktemp -d) cargo test --release --features ultra -- --ignored --nocapture echte_texturen`
    #[test]
    #[ignore]
    fn echte_texturen_landen_im_bild() {
        let console = crate::console::Console::new(false, false, false);
        let version = "1.21.1";
        let path =
            crate::pov_resources::locate(&console, version, &crate::pov_resources::Source::Auto)
                .expect("Ressourcen beschaffen");
        let assets = Assets::load(&path, version).expect("Assets lesen");

        // Stein grau, Gras grün, Wasser blau, Sand sandfarben, ein Baum – alles echte
        // Blockzustände von 1.21.1 (siehe data/block-states-1.21.1.txt.gz).
        let mut chunks = HashMap::new();
        for x in -3..=3i32 {
            for z in -3..=3i32 {
                chunks.insert((x, z), landscape_chunk(x, z));
            }
        }
        let scene = Scene {
            chunks,
            entities: Vec::new(),
            chunk_revision: 0,
            ..Scene::default()
        };

        let (width, height) = (256usize, 144usize);
        let mut pixels = Vec::new();
        // Positive Neigung heißt in Minecraft „nach unten": die untere Bildhälfte ist Landschaft.
        render_textured(
            &scene,
            (8.0, 20.0, 8.0, 35.0, 25.0),
            width,
            height,
            &assets,
            &mut pixels,
        );
        assert_eq!(pixels.len(), width * height);

        // --- Teil 1: blockgenau, ohne Kamera. Hier ist nichts zu verwechseln. ---
        //
        // Die Fehlertextur ist knallig lila (248, 0, 248); alles, was sie liefert, ist falsch
        // gelesen. Für jeden Block dazu die Erwartung an die Farbe selbst.
        let is_missing = |(r, g, b, _): (u8, u8, u8, u8)| r > 200 && g < 40 && b > 200;
        // (Zustand, Seite, Beschreibung, Prüfung)
        #[allow(clippy::type_complexity)]
        let cases: [(u32, usize, &str, fn(u8, u8, u8) -> bool); 5] = [
            // Wasser hat in Vanilla ein Modell **ohne Flächen**: `block/water.json` nennt nur
            // die Partikeltextur. Die Suche nach `block/water` ging deshalb ins Leere, und jeder
            // See wurde lila. Das ist der eigentliche Regressionstest hier.
            (80, 3, "Wasser", |r, g, b| b > r + 30 && b > g + 10),
            // `grass_block_top.png` ist grau – grün wird die Fläche erst durch die Einfärbung.
            (9, 3, "Grasoberseite", |r, g, b| g > r + 15 && g > b + 15),
            (264, 3, "Eichenlaub", |r, g, b| g > r + 15 && g > b + 15),
            // Stein ist grau: alle drei Kanäle dicht beieinander.
            (1, 3, "Stein", |r, g, b| {
                (r.max(g).max(b) as i32 - r.min(g).min(b) as i32) < 25
            }),
            // Sand ist warm getönt und wird **nicht** eingefärbt.
            (112, 3, "Sand", |r, g, b| r > b + 30 && g > b + 10),
        ];
        for (state, face, name, check) in cases {
            // Die ganze Fläche abtasten, nicht nur einen Punkt: Ein einzelner Texel beweist
            // nichts, und Blattwerk ist absichtlich löchrig – dort liefert die Textur
            // durchsichtige Stellen, die der Strahl im Bild ebenfalls überspringt.
            let mut opaque = 0;
            for row in 0..8 {
                for column in 0..8 {
                    let (u, v) = ((column as f64 + 0.5) / 8.0, (row as f64 + 0.5) / 8.0);
                    let sample = assets.sample(state, face, u, v);
                    assert!(
                        !is_missing(sample),
                        "{} (Zustand {}) liefert die Fehlertextur: {:?}",
                        name,
                        state,
                        sample
                    );
                    if sample.3 < 16 {
                        continue; // durchsichtig – im Bild ebenfalls kein Treffer
                    }
                    opaque += 1;
                    assert!(
                        check(sample.0, sample.1, sample.2),
                        "{} (Zustand {}) hat bei {:.2}/{:.2} die falsche Farbe: {:?}",
                        name,
                        state,
                        u,
                        v,
                        sample
                    );
                }
            }
            assert!(
                opaque >= 16,
                "{} (Zustand {}) ist fast vollstaendig durchsichtig – da wurde nichts gelesen",
                name,
                state
            );
        }

        // --- Teil 2: im fertigen Bild. Der Himmel ist selbst blau, deshalb zählen hier nur
        // Bildpunkte, die wirklich einen Block getroffen haben. ---
        let hits: Vec<(u8, u8, u8)> = pixels
            .iter()
            .filter(|pixel| pixel.depth < MAX_DISTANCE)
            .map(|pixel| pixel.rgb)
            .collect();
        assert!(
            hits.len() > width * height / 4,
            "nur {} von {} Bildpunkten treffen ueberhaupt einen Block",
            hits.len(),
            width * height
        );
        let magenta = hits
            .iter()
            .filter(|(r, g, b)| *r > 150 && *g < 70 && *b > 150)
            .count();
        assert_eq!(magenta, 0, "{} Bildpunkte zeigen die Fehlertextur", magenta);
        // Und es sind wirklich Texturen und kein Farbklotz: die Töne schwanken.
        let shades: std::collections::HashSet<(u8, u8, u8)> = hits.iter().copied().collect();
        assert!(
            shades.len() > 200,
            "das Bild hat nur {} Farbtoene – das sieht nach Farbklötzen aus, nicht nach Texturen",
            shades.len()
        );
    }

    /// Dasselbe Bild einmal zum Ansehen: legt ein PNG ab, damit sich die texturierte Ansicht
    /// wirklich mit dem Auge prüfen lässt statt nur über Zahlen.
    ///
    /// `XDG_CONFIG_HOME=$(mktemp -d) AFK_POV_PNG=/tmp/pov.png \
    ///   cargo test --release --features ultra -- --ignored --nocapture pov_als_png`
    #[test]
    #[ignore]
    fn pov_als_png() {
        let Ok(target) = std::env::var("AFK_POV_PNG") else {
            println!("AFK_POV_PNG nicht gesetzt – nichts zu tun");
            return;
        };
        let console = crate::console::Console::new(false, false, false);
        let version = std::env::var("AFK_POV_VERSION").unwrap_or_else(|_| "1.21.1".to_string());
        let path =
            crate::pov_resources::locate(&console, &version, &crate::pov_resources::Source::Auto)
                .expect("Ressourcen beschaffen");
        let assets = Assets::load(&path, &version).expect("Assets lesen");

        let mut chunks = HashMap::new();
        for x in -4..=4i32 {
            for z in -4..=4i32 {
                chunks.insert((x, z), landscape_chunk(x, z));
            }
        }
        let scene = Scene {
            chunks,
            entities: Vec::new(),
            chunk_revision: 0,
            ..Scene::default()
        };
        let (width, height) = (426usize, 240usize);
        let mut pixels = Vec::new();
        render_textured(
            &scene,
            (8.0, 20.0, 8.0, 35.0, 25.0),
            width,
            height,
            &assets,
            &mut pixels,
        );
        let mut rgba = Vec::with_capacity(width * height * 4);
        for pixel in &pixels {
            rgba.extend_from_slice(&[pixel.rgb.0, pixel.rgb.1, pixel.rgb.2, 255]);
        }
        let png = crate::pov_assets::encode_rgba_png(width, height, &rgba).expect("PNG");
        std::fs::write(&target, png).expect("schreiben");
        println!("Bild geschrieben: {}", target);
    }

    /// Der Chunk-Merker darf am Ergebnis nichts ändern – er spart nur Nachschlagevorgänge.
    /// Deshalb hier dieselbe Szene zweimal: einmal Block für Block direkt aus der Hashtabelle,
    /// einmal über den Merker, wie ihn der Strahl benutzt.
    #[test]
    fn merker_liefert_dieselben_bloecke_wie_die_hashtabelle() {
        let mut chunks = HashMap::new();
        for x in -2..=2 {
            for z in -2..=2 {
                chunks.insert((x, z), ground_chunk());
            }
        }
        let scene = Scene {
            chunks,
            entities: Vec::new(),
            chunk_revision: 0,
            ..Scene::default()
        };
        let mut cursor = Cursor::new(&scene);
        // Quer durch mehrere Chunks und auch daneben, wo keiner geladen ist.
        for x in -40i32..40 {
            for z in [-33i32, -16, -1, 0, 1, 15, 40] {
                for y in [-64i32, 0, 1, 2, 300] {
                    let direct = scene
                        .chunks
                        .get(&(x.div_euclid(16), z.div_euclid(16)))
                        .map(|chunk| (chunk.solid(x, y, z), chunk.block(x, y, z)));
                    assert_eq!(
                        cursor.solid(x, y, z),
                        direct.map(|(solid, _)| solid).unwrap_or(false),
                        "solid bei {}/{}/{}",
                        x,
                        y,
                        z
                    );
                    assert_eq!(
                        cursor.block(x, y, z),
                        direct.map(|(_, block)| block).unwrap_or(0),
                        "block bei {}/{}/{}",
                        x,
                        y,
                        z
                    );
                }
            }
        }
    }

    /// Der Decoder bekommt vom Server beliebige Bytes. Er darf daran nie in Panik geraten –
    /// `panic = "abort"` würde sonst den ganzen Client abschießen.
    /// Messlauf statt Behauptung: wie lange das Einlesen eines realistischen Chunks dauert.
    ///
    /// Läuft nicht im normalen Testlauf mit – die Zahl hängt vom Rechner ab. Aufruf:
    /// `cargo test --release --features pov -- --ignored --nocapture chunk_einlesen`
    #[test]
    #[ignore]
    fn chunk_einlesen_dauert() {
        // Eine gewachsene Überwelt: acht gefüllte Abschnitte mit gemischter Palette, der Rest Luft.
        let mut states = vec![0u32; SECTION_BLOCKS];
        for (index, state) in states.iter_mut().enumerate() {
            *state = (index % 37) as u32 + 1;
        }
        let mut w = Writer::default();
        for section in 0..24usize {
            if (8..16).contains(&section) {
                write_section(&mut w, MODERN_FLUID, &states, SECTION_BLOCKS);
            } else {
                write_section(&mut w, MODERN_FLUID, &[0u32; SECTION_BLOCKS], 0);
            }
        }
        let dim = Dimension {
            name: "test".into(),
            min_y: -64,
            height: 384,
        };
        // Einmal vorweg, damit der Zwischenspeicher warm ist.
        let chunk = Chunk::decode(MODERN_FLUID, &dim, &w.data).expect("lesbar");
        assert_eq!(chunk.sections.iter().flatten().count(), 8);

        const ROUNDS: usize = 400;
        let started = std::time::Instant::now();
        for _ in 0..ROUNDS {
            let chunk = Chunk::decode(MODERN_FLUID, &dim, &w.data).expect("lesbar");
            std::hint::black_box(&chunk);
        }
        let each = started.elapsed().as_secs_f64() * 1000.0 / ROUNDS as f64;
        println!(
            "\nChunk einlesen: {:.3} ms je Chunk ({} Runden)\n",
            each, ROUNDS
        );
    }

    #[test]
    fn beliebige_bytes_stuerzen_nicht_ab() {
        let mut seed = 0x243F_6A88_85A3_08D3u64;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for round in 0..400 {
            let len = (random() % 4096) as usize;
            let data: Vec<u8> = (0..len).map(|_| random() as u8).collect();
            for format in [LEGACY, MODERN, MODERN_FLUID] {
                let _ = Chunk::decode(format, &dimension(24), &data);
            }
            let _ = Format::probe(MODERN, &dimension(24), &data);
            assert!(round < 400);
        }
    }
}
