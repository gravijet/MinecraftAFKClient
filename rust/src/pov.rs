//! Echte Live-POV – nur mit `--features pov` (eigene POV-Bauform und Ultra).
//!
//! Der Server schickt einem Vanilla-Client keine fertigen Bilder, sondern Chunk-Abschnitte mit
//! kompakten Blockzustands-Paletten sowie einzelne Block- und Entitätsänderungen. Dieses Modul
//! hält genau diese sichtbare Umgebung im Speicher und zeichnet daraus laufend eine farbige
//! First-Person-Ansicht im Terminal. Blickrichtung und Kameraposition kommen unmittelbar aus dem
//! eigenen Positionszustand; die Ansicht folgt daher `:look`, Bewegung und Server-Teleports live.
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
//! **Ausgabeformat.** Ein Bild besteht aus einer Kopfzeile `POV x=… (:pov stop)` und danach je
//! Terminalzeile einer Reihe Halbblöcke `▀` mit Vorder- und Hintergrundfarbe (ein Zeichen = zwei
//! Bildpunkte). Ohne Farbe kommt stattdessen eine Helligkeitsrampe, eine Zeichenzeile je
//! Bildzeile. Das Format ist Teil der Schnittstelle nach außen – ein Panel liest es mit –, es
//! wird deshalb nicht ohne Not geändert.

use crate::buf::{Reader, Writer};
use crate::client::Shared;
use crate::nbt::Nbt;
use crate::options::Options;
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
/// Notbremse. Normalerweise begrenzt die angeforderte Sichtweite die Zahl der Chunks vorher.
const MAX_CHUNKS: usize = 1024;
const MAX_ENTITIES: usize = 4096;
const MAX_SECTIONS: usize = 256;
/// Weiter entfernte Chunks kann die Ansicht nie sehen: [`MAX_DISTANCE`] sind 72 Blöcke, ein Strahl
/// erreicht damit höchstens den fünften Chunk in jede Richtung. Alles darüber wird verworfen,
/// sobald ein neuer Chunk ankommt – ohne das lassen Server, die nie ein `ForgetChunk` schicken,
/// den Speicher unbegrenzt wachsen.
const KEEP_CHUNK_RADIUS: i32 = 6;
const HORIZONTAL_FOV: f64 = 90.0;
const MAX_DISTANCE: f64 = 72.0;
const DEFAULT_WIDTH: usize = 64;
/// Pixelhöhe; ANSI stellt je Terminalzeile zwei Pixel mit `▀` dar.
const DEFAULT_HEIGHT: usize = 32;
const MIN_WIDTH: usize = 24;
const MAX_WIDTH: usize = 160;
const MIN_HEIGHT: usize = 12;
const MAX_HEIGHT: usize = 80;
/// Auch ein unverändertes Bild wird spätestens so oft wiederholt – ein Programm davor soll an
/// der Stille nicht ablesen, die Ansicht sei tot.
const REPEAT_UNCHANGED: Duration = Duration::from_secs(2);

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Dimension {
    name: String,
    min_y: i32,
    height: i32,
}

impl Dimension {
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
    fn new(values: &[u16], palette_len: usize) -> Indices {
        if palette_len <= u8::MAX as usize + 1 {
            Indices::Small(values.iter().map(|v| *v as u8).collect())
        } else {
            Indices::Large(values.to_vec().into_boxed_slice())
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

    /// Alle vier Spielarten, die bevorzugte zuerst.
    fn candidates(preferred: Format) -> [Format; 4] {
        let mut all = [
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
        ];
        all.sort_by_key(|f| (f.code() != preferred.code()) as u8);
        all
    }

    /// Chunk lesen und dabei die passende Spielart bestimmen. Bevorzugt wird die, die den Puffer
    /// restlos aufgeht **und** die erwartete Zahl Abschnitte liefert.
    fn probe(
        preferred: Format,
        dimension: &Dimension,
        data: &[u8],
    ) -> io::Result<(Chunk, Format)> {
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
    ((y.rem_euclid(16) as usize) << 8) | ((z.rem_euclid(16) as usize) << 4) | x.rem_euclid(16) as usize
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
    let palette = read_palette(r, SECTION_BLOCKS, 8, format.modern_palette)?;
    // Biome braucht die Geometrie nicht – nur exakt überspringen.
    skip_palette(r, SECTION_BIOMES, 3, format.modern_palette)?;

    if block_count == 0 {
        return Ok(None);
    }
    Ok(compact(palette, block_count as usize).map(Arc::new))
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
}

impl Default for World {
    fn default() -> Self {
        World {
            dimension: Dimension::for_world("minecraft:overworld"),
            chunks: HashMap::new(),
            entities: HashMap::new(),
            own_entity: -1,
            center: (0, 0),
        }
    }
}

impl World {
    fn clear_visible(&mut self) {
        self.chunks.clear();
        self.chunks.shrink_to_fit();
        self.entities.clear();
        self.entities.shrink_to_fit();
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let key = (x.div_euclid(16), z.div_euclid(16));
        if let Some(chunk) = self.chunks.get_mut(&key) {
            Arc::make_mut(chunk).set_block(x, y, z, state);
        }
    }

    /// Alles wegwerfen, was die Ansicht ohnehin nie erreicht.
    fn prune(&mut self) {
        let (cx, cz) = self.center;
        self.chunks.retain(|(x, z), _| {
            (x - cx).abs() <= KEEP_CHUNK_RADIUS && (z - cz).abs() <= KEEP_CHUNK_RADIUS
        });
    }
}

#[derive(Clone, Default)]
struct Scene {
    chunks: HashMap<(i32, i32), Arc<Chunk>>,
    entities: Vec<Entity>,
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

    /// `false` heißt „Luft **oder** Chunk nicht geladen" – durch beides läuft der Strahl weiter.
    #[inline]
    fn solid(&mut self, x: i32, y: i32, z: i32) -> bool {
        self.section(x, y, z)
            .is_some_and(|section| !section.is_air(local_index(x, y, z)))
    }

    #[inline]
    fn block(&mut self, x: i32, y: i32, z: i32) -> u32 {
        self.section(x, y, z)
            .map_or(0, |section| section.state(local_index(x, y, z)))
    }
}

pub struct Pov {
    world: Mutex<World>,
    dimensions: Mutex<Vec<Dimension>>,
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
}

impl Pov {
    pub fn new(options: &Options) -> Pov {
        let (width, height) = options.pov_size.unwrap_or((DEFAULT_WIDTH, DEFAULT_HEIGHT));
        Pov {
            world: Mutex::new(World::default()),
            dimensions: Mutex::new(Vec::new()),
            live: AtomicBool::new(false),
            renderer_running: AtomicBool::new(false),
            width: AtomicUsize::new(width.clamp(MIN_WIDTH, MAX_WIDTH)),
            height: AtomicUsize::new(height.clamp(MIN_HEIGHT, MAX_HEIGHT)),
            fps: AtomicUsize::new(options.pov_fps.clamp(1, 20)),
            parse_errors: AtomicU32::new(0),
            format: AtomicU32::new(u32::MAX),
            last_frame: AtomicU64::new(0),
            last_frame_ms: AtomicU64::new(0),
            started: Instant::now(),
            scratch: Mutex::new(Scratch::default()),
        }
    }

    pub(crate) fn set_dimensions(&self, dimensions: Vec<Dimension>) {
        *self.dimensions.lock().unwrap() = dimensions;
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
        scene.chunks.clone_from(&world.chunks);
        scene.entities.clear();
        scene.entities.extend(world.entities.values().copied());
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
        .unwrap_or(cfg!(feature = "pov-client"));
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
        In::ForgetChunk => {
            // ChunkPos ist ein Long: x in den unteren, z in den oberen 32 Bit. Zwei
            // nacheinander gelesene i32 wären durch die Netzwerk-Bytefolge genau vertauscht.
            let key = unpack_chunk_pos(r.i64()?);
            shared.extras.pov.world.lock().unwrap().chunks.remove(&key);
            Ok(())
        }
        In::BlockUpdate => {
            let (x, y, z) = unpack_block_pos(r.i64()?);
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
        In::SectionBlocks => read_section_blocks(shared, r),
        In::ChunkBatchFinished => {
            r.var_int()?;
            let mut w = Writer::packet(shared.proto.extra.sb_chunk_batch_received);
            w.f32(8.0);
            shared.send(w);
            Ok(())
        }
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
    let data = r.byte_array()?;

    let pov = &shared.extras.pov;
    let dimension = pov.world.lock().unwrap().dimension.clone();
    let preferred = pov.preferred_format(shared);
    let (chunk, format) = Format::probe(preferred, &dimension, &data)?;
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
    }
    if center.is_some() {
        world.prune();
    }
    Ok(())
}

fn read_section_blocks(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
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
    r.skip(16)?;
    let kind = r.var_int()?;
    let entity = Entity {
        x: r.f64()?,
        y: r.f64()?,
        z: r.f64()?,
        player: kind == shared.proto.extra.player_entity_type,
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
    let dx = r.i16()? as f64 / 4096.0;
    let dy = r.i16()? as f64 / 4096.0;
    let dz = r.i16()? as f64 / 4096.0;
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
    let (x, y, z) = (r.f64()?, r.f64()?, r.f64()?);
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
fn palette_head(
    r: &mut Reader,
    max_indirect_bits: u8,
) -> io::Result<(u8, Option<Vec<u32>>)> {
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
        for index in 0..size {
            let packed = unpack(&data, index, per_long, bits, mask)?;
            if packed >= values.len() {
                return Err(crate::buf::err("POV: Palettenindex ausserhalb"));
            }
            counts[packed] += 1;
            indices.push(packed as u16);
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
    for index in 0..size {
        let state = unpack(&data, index, per_long, bits, mask)? as u32;
        let slot = match seen.get(&state) {
            Some(slot) => *slot,
            None => {
                if values.len() >= u16::MAX as usize {
                    return Err(crate::buf::err("POV: Abschnitt mit zu vielen Zustaenden"));
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

/// Einen Eintrag aus den gepackten Longs holen – mit Bereichsprüfung statt blindem Zugriff.
fn unpack(data: &[u64], index: usize, per_long: usize, bits: u8, mask: u64) -> io::Result<usize> {
    let long = *data
        .get(index / per_long)
        .ok_or_else(|| crate::buf::err("POV: Palettendaten zu kurz"))?;
    Ok(((long >> ((index % per_long) * bits as usize)) & mask) as usize)
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

    let mapped: Vec<u16> = indices.iter().map(|slot| remap[*slot as usize]).collect();
    let indices = Indices::new(&mapped, palette.len());
    Some(Section { palette, indices })
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
                (Some(width), Some(height)) => {
                    let width = width.clamp(MIN_WIDTH, MAX_WIDTH);
                    let height = height.clamp(MIN_HEIGHT, MAX_HEIGHT);
                    shared.extras.pov.width.store(width, Ordering::Relaxed);
                    shared.extras.pov.height.store(height, Ordering::Relaxed);
                    shared.extras.pov.last_frame.store(0, Ordering::Relaxed);
                    shared
                        .console
                        .info(&format!("POV-Groesse: {}x{} Pixel.", width, height));
                }
                _ => shared
                    .console
                    .error("Nutzung: :pov size <breite> <hoehe>   z. B. :pov size 160 80"),
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
            let entities = world.entities.len();
            drop(world);
            shared.console.info(&format!(
                "POV: {} · y={}..{} · {} Chunks · {} Abschnitte · {} Entities · {}x{} · {} fps · {}",
                dimension.name,
                dimension.min_y,
                dimension.min_y + dimension.height - 1,
                chunks,
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
                    let started = Instant::now();
                    if owned.in_game.load(Ordering::Relaxed) {
                        draw_once(&owned, true, false);
                    }
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
    } = &mut *scratch;
    pov.fill_scene(scene);
    render_into(scene, position, width, height, color, pixels, frame);

    if !force && !frame_changed(pov, frame) {
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
    pixels.clear();
    pixels.resize(
        width * height,
        Pixel {
            rgb: (0, 0, 0),
            depth: MAX_DISTANCE,
        },
    );

    // Echte Zentralprojektion statt gleichmäßig verteilter Winkel: sonst „biegt" sich der
    // Horizont bei 90° Blickfeld sichtbar nach außen.
    let half_width = (HORIZONTAL_FOV.to_radians() / 2.0).tan();
    let half_height = half_width * height as f64 / width as f64;
    let basis = camera_basis(position.3 as f64, position.4 as f64);
    // Ein Merker für das ganze Bild: benachbarte Strahlen beginnen im selben Chunk.
    let mut cursor = Cursor::new(scene);

    for py in 0..height {
        let sy = (0.5 - (py as f64 + 0.5) / height as f64) * 2.0 * half_height;
        let sky = sky_color(py, height);
        for px in 0..width {
            let sx = ((px as f64 + 0.5) / width as f64 - 0.5) * 2.0 * half_width;
            let direction = normalize((
                basis.0 .0 + basis.1 .0 * sx + basis.2 .0 * sy,
                basis.0 .1 + basis.1 .1 * sx + basis.2 .1 * sy,
                basis.0 .2 + basis.1 .2 * sx + basis.2 .2 * sy,
            ));
            pixels[py * width + px] = match cast(&mut cursor, origin, direction) {
                Some((state, distance, face)) => Pixel {
                    rgb: fog(block_color(state, face, distance), sky, distance),
                    depth: distance,
                },
                None => Pixel {
                    rgb: sky,
                    depth: MAX_DISTANCE,
                },
            };
        }
    }
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

/// Bequeme Fassung für die Tests und den Messlauf: legt die Puffer selbst an.
#[cfg(test)]
fn render(
    scene: &Scene,
    position: (f64, f64, f64, f32, f32),
    width: usize,
    height: usize,
    color: bool,
) -> String {
    let mut pixels = Vec::new();
    let mut out = String::new();
    render_into(
        scene, position, width, height, color, &mut pixels, &mut out,
    );
    out
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

/// Voxel-DDA: höchstens ein Zugriff je durchquertem Block, nicht hunderte kleine Ray-Schritte.
fn cast(
    cursor: &mut Cursor,
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
        // Erst die billige Sichtprüfung; die Zustands-ID kostet einen Palettenzugriff mehr.
        //
        // Einen ganzen leeren Abschnitt in einem Zug zu überspringen wäre verlockend, geht aber
        // nicht sauber: Die Austrittsfläche müsste geteilt, die Blockgrenzen dagegen aufaddiert
        // werden, und bei einer Kamera genau auf einer Blockecke fallen beide Rechnungen um eine
        // Zelle auseinander. Das wäre ein sichtbar falscher Bildpunkt für kaum gesparte Arbeit –
        // der Abschnittsmerker in [`Cursor`] holt den Löwenanteil ohnehin.
        if cursor.solid(cell.0, cell.1, cell.2) {
            return Some((cursor.block(cell.0, cell.1, cell.2), distance, face));
        }
        advance(&mut cell, &mut next, &mut distance, &mut face, step, delta);
    }
    None
}

/// Ein Schritt des Voxel-DDA: Es rückt die Achse weiter, deren nächste Blockgrenze am
/// nächsten liegt.
#[inline]
fn advance(
    cell: &mut (i32, i32, i32),
    next: &mut (f64, f64, f64),
    distance: &mut f64,
    face: &mut usize,
    step: (i32, i32, i32),
    delta: (f64, f64, f64),
) {
    if next.0 <= next.1 && next.0 <= next.2 {
        cell.0 += step.0;
        *distance = next.0;
        next.0 += delta.0;
        *face = 0;
    } else if next.1 <= next.2 {
        cell.1 += step.1;
        *distance = next.1;
        next.1 += delta.1;
        *face = 1;
    } else {
        cell.2 += step.2;
        *distance = next.2;
        next.2 += delta.2;
        *face = 2;
    }
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
            for (px, py) in [(0usize, 0usize), (63, 0), (0, 31), (63, 31), (32, 16), (10, 25)] {
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
        // Biome: Singleton
        w.u8(0);
        w.var_int(1);
        if !format.modern_palette {
            w.var_int(0);
        }
    }

    #[test]
    fn singleton_abschnitt_legacy_und_modern() {
        for format in [LEGACY, MODERN, MODERN_FLUID] {
            let mut w = Writer::default();
            write_section(&mut w, format, &[0u32; SECTION_BLOCKS], 0);
            let chunk = Chunk::decode(format, &dimension(1), &w.data).unwrap();
            assert_eq!(chunk.block(0, 0, 0), 0);
            assert!(chunk.sections[0].is_none(), "reine Luft braucht keinen Platz");
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
        })
    }

    /// Der Voxel-Durchlauf **ohne** jede Abkürzung: Block für Block, genau nach Lehrbuch.
    /// Maßstab für den optimierten [`cast`].
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
            advance(
                &mut cell,
                &mut next,
                &mut distance,
                &mut face,
                step,
                delta,
            );
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
    /// Läuft nicht im normalen Testlauf mit – die Zahl hängt vom Rechner ab. Aufruf:
    /// `cargo test --features ultra -- --ignored --nocapture bildrate`
    #[test]
    #[ignore]
    fn messlauf_bildrate() {
        // 13x13 Chunks – genau so viele behält die Ansicht nach dem Aufräumen.
        let mut chunks = HashMap::new();
        for x in -6..=6 {
            for z in -6..=6 {
                chunks.insert((x, z), ground_chunk());
            }
        }
        let scene = Scene {
            chunks,
            entities: Vec::new(),
        };
        let position = (8.0, 18.0, 8.0, 30.0f32, -10.0f32);

        for (width, height) in [(64usize, 32usize), (160, 80)] {
            // Einmal warmlaufen, damit die Messung nicht den ersten Zugriff mitzählt.
            let _ = render(&scene, position, width, height, true);
            let runs = 100;
            let started = Instant::now();
            for _ in 0..runs {
                let _ = render(&scene, position, width, height, true);
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

