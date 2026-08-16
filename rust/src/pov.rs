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

use crate::buf::{Reader, Writer};
use crate::client::Shared;
use crate::nbt::Nbt;
use crate::options::Options;
use crate::proto::In;

use std::collections::HashMap;
use std::f64::consts::PI;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

const SECTION_BLOCKS: usize = 16 * 16 * 16;
const SECTION_BIOMES: usize = 4 * 4 * 4;
const MAX_CHUNKS: usize = 1024;
const MAX_ENTITIES: usize = 4096;
const MAX_SECTIONS: usize = 256;
const RENDER_INTERVAL: Duration = Duration::from_millis(125);
const HORIZONTAL_FOV: f64 = 90.0;
const MAX_DISTANCE: f64 = 72.0;
const DEFAULT_WIDTH: usize = 64;
/// Pixelhöhe; ANSI stellt je Terminalzeile zwei Pixel mit `▀` dar.
const DEFAULT_HEIGHT: usize = 32;

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

#[derive(Clone)]
struct Section {
    /// Reihenfolge des Protokolls: `(y << 8) | (z << 4) | x`.
    blocks: Box<[u32]>,
}

#[derive(Clone)]
struct Chunk {
    min_section: i32,
    sections: Vec<Option<Section>>,
}

impl Chunk {
    fn decode(
        proto_modern: bool,
        fluid_count: bool,
        dimension: &Dimension,
        data: &[u8],
    ) -> io::Result<Chunk> {
        let mut r = Reader::new(data);
        let mut sections = Vec::with_capacity(dimension.section_count());
        for _ in 0..dimension.section_count() {
            let block_count = r.i16()?;
            if !(0..=SECTION_BLOCKS as i16).contains(&block_count) {
                return Err(crate::buf::err("POV: Blockzaehler unplausibel"));
            }
            if fluid_count {
                r.i16()?;
            }
            let mut blocks = read_palette(&mut r, SECTION_BLOCKS, 8, proto_modern)?;
            // Biome wird für die Geometrie nicht gebraucht, muss aber exakt übersprungen werden.
            let _ = read_palette(&mut r, SECTION_BIOMES, 3, proto_modern)?;

            if block_count == 0 {
                sections.push(None);
            } else {
                normalize_air(&mut blocks, block_count as usize);
                sections.push(Some(Section { blocks }));
            }
        }
        if r.remaining() != 0 {
            return Err(crate::buf::err("POV: Chunk hat unerwartete Restdaten"));
        }
        Ok(Chunk {
            min_section: dimension.min_y.div_euclid(16),
            sections,
        })
    }

    fn block(&self, x: i32, y: i32, z: i32) -> u32 {
        let section_y = y.div_euclid(16) - self.min_section;
        let Some(Some(section)) = usize::try_from(section_y)
            .ok()
            .and_then(|index| self.sections.get(index))
        else {
            return 0;
        };
        let index = ((y.rem_euclid(16) as usize) << 8)
            | ((z.rem_euclid(16) as usize) << 4)
            | x.rem_euclid(16) as usize;
        section.blocks[index]
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let section_y = y.div_euclid(16) - self.min_section;
        let Some(index) = usize::try_from(section_y)
            .ok()
            .filter(|i| *i < self.sections.len())
        else {
            return;
        };
        if self.sections[index].is_none() {
            if state == 0 {
                return;
            }
            self.sections[index] = Some(Section {
                blocks: vec![0; SECTION_BLOCKS].into_boxed_slice(),
            });
        }
        let local = ((y.rem_euclid(16) as usize) << 8)
            | ((z.rem_euclid(16) as usize) << 4)
            | x.rem_euclid(16) as usize;
        self.sections[index].as_mut().unwrap().blocks[local] = state;
    }
}

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
}

impl Default for World {
    fn default() -> Self {
        World {
            dimension: Dimension::for_world("minecraft:overworld"),
            chunks: HashMap::new(),
            entities: HashMap::new(),
            own_entity: -1,
        }
    }
}

impl World {
    fn clear_visible(&mut self) {
        self.chunks.clear();
        self.entities.clear();
    }

    fn set_block(&mut self, x: i32, y: i32, z: i32, state: u32) {
        let key = (x.div_euclid(16), z.div_euclid(16));
        if let Some(chunk) = self.chunks.get_mut(&key) {
            Arc::make_mut(chunk).set_block(x, y, z, state);
        }
    }
}

#[derive(Clone)]
struct Scene {
    chunks: HashMap<(i32, i32), Arc<Chunk>>,
    entities: Vec<Entity>,
}

impl Scene {
    fn block(&self, x: i32, y: i32, z: i32) -> Option<u32> {
        self.chunks
            .get(&(x.div_euclid(16), z.div_euclid(16)))
            .map(|chunk| chunk.block(x, y, z))
    }
}

pub struct Pov {
    world: Mutex<World>,
    dimensions: Mutex<Vec<Dimension>>,
    live: AtomicBool,
    renderer_running: AtomicBool,
    width: AtomicUsize,
    height: AtomicUsize,
    parse_errors: AtomicU32,
}

impl Pov {
    pub fn new(_options: &Options) -> Pov {
        Pov {
            world: Mutex::new(World::default()),
            dimensions: Mutex::new(Vec::new()),
            live: AtomicBool::new(false),
            renderer_running: AtomicBool::new(false),
            width: AtomicUsize::new(DEFAULT_WIDTH),
            height: AtomicUsize::new(DEFAULT_HEIGHT),
            parse_errors: AtomicU32::new(0),
        }
    }

    pub(crate) fn set_dimensions(&self, dimensions: Vec<Dimension>) {
        *self.dimensions.lock().unwrap() = dimensions;
    }

    pub fn clear(&self) {
        self.world.lock().unwrap().clear_visible();
    }

    fn scene(&self) -> Scene {
        let world = self.world.lock().unwrap();
        Scene {
            chunks: world.chunks.clone(),
            entities: world.entities.values().copied().collect(),
        }
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
    if cfg!(feature = "pov-client") {
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
    let dimension = shared.extras.pov.world.lock().unwrap().dimension.clone();
    let chunk = Chunk::decode(
        shared.proto.modern,
        shared.proto.extra.section_fluid_count,
        &dimension,
        &data,
    )?;
    let mut world = shared.extras.pov.world.lock().unwrap();
    if world.chunks.len() < MAX_CHUNKS || world.chunks.contains_key(&(x, z)) {
        world.chunks.insert((x, z), Arc::new(chunk));
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

fn read_palette(
    r: &mut Reader,
    size: usize,
    max_indirect_bits: u8,
    modern: bool,
) -> io::Result<Box<[u32]>> {
    let bits = r.u8()?;
    if bits > 32 {
        return Err(crate::buf::err("POV: Palette breiter als 32 Bit"));
    }
    if bits == 0 {
        let state = r.var_int()? as u32;
        if !modern {
            let longs = r.var_int()?;
            if longs != 0 {
                return Err(crate::buf::err("POV: Singleton-Palette mit Daten"));
            }
        }
        return Ok(vec![state; size].into_boxed_slice());
    }

    let palette = if bits <= max_indirect_bits {
        let count = r.var_int()?;
        if !(1..=4096).contains(&count) {
            return Err(crate::buf::err("POV: lokale Palette unplausibel"));
        }
        let mut palette = Vec::with_capacity(count as usize);
        for _ in 0..count {
            palette.push(r.var_int()? as u32);
        }
        Some(palette)
    } else {
        None
    };

    let per_long = 64usize / bits as usize;
    if per_long == 0 {
        return Err(crate::buf::err("POV: ungueltige Palettenbreite"));
    }
    let expected = size.div_ceil(per_long);
    let longs = if modern {
        expected
    } else {
        let encoded = r.var_int()?;
        if encoded < 0 || encoded as usize != expected {
            return Err(crate::buf::err("POV: falsche Palettenlaenge"));
        }
        encoded as usize
    };
    let mut data = Vec::with_capacity(longs);
    for _ in 0..longs {
        data.push(r.i64()? as u64);
    }

    let mask = if bits == 32 {
        u32::MAX as u64
    } else {
        (1u64 << bits) - 1
    };
    let mut out = Vec::with_capacity(size);
    for index in 0..size {
        let packed =
            ((data[index / per_long] >> ((index % per_long) * bits as usize)) & mask) as usize;
        let state = match &palette {
            Some(palette) => *palette
                .get(packed)
                .ok_or_else(|| crate::buf::err("POV: Palettenindex ausserhalb"))?,
            None => packed as u32,
        };
        out.push(state);
    }
    Ok(out.into_boxed_slice())
}

/// Der Abschnitt überträgt die exakte Zahl nicht-luftiger Blöcke. Damit lassen sich neben State
/// 0 auch `cave_air` und `void_air` erkennen, ohne versionsabhängige Block-State-Tabellen.
fn normalize_air(blocks: &mut [u32], non_air: usize) {
    let air_total = SECTION_BLOCKS.saturating_sub(non_air);
    if air_total == 0 {
        return;
    }
    let mut counts: HashMap<u32, usize> = HashMap::new();
    for state in blocks.iter().copied() {
        *counts.entry(state).or_default() += 1;
    }
    let mut air_states = Vec::new();
    let zero = counts.remove(&0).unwrap_or(0);
    if zero > 0 {
        air_states.push(0);
    }
    let remaining = air_total.saturating_sub(zero);
    if remaining > 0 {
        // Exakte Teilsumme der übrigen Häufigkeiten; Palette und Ziel sind klein (<=4096).
        let entries: Vec<(u32, usize)> = counts.into_iter().collect();
        let mut previous: Vec<Option<(usize, usize)>> = vec![None; remaining + 1];
        previous[0] = Some((usize::MAX, 0));
        for (index, (_, count)) in entries.iter().enumerate() {
            for sum in (0..=remaining.saturating_sub(*count)).rev() {
                if previous[sum].is_some() && previous[sum + count].is_none() {
                    previous[sum + count] = Some((index, sum));
                }
            }
        }
        if previous[remaining].is_some() {
            let mut sum = remaining;
            while sum > 0 {
                let (index, before) = previous[sum].unwrap();
                air_states.push(entries[index].0);
                sum = before;
            }
        }
    }
    for state in blocks {
        if air_states.contains(state) {
            *state = 0;
        }
    }
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
            start_renderer(shared);
            shared
                .console
                .info("Live-POV gestartet. Beenden mit :pov stop.");
        }
        "stop" | "aus" | "off" => {
            shared.extras.pov.live.store(false, Ordering::SeqCst);
            shared.console.pov_frame("\x1b[0m\x1b[2J\x1b[H");
            shared.console.info("Live-POV beendet.");
        }
        "frame" | "bild" | "once" => draw_once(shared, false),
        "size" | "groesse" | "größe" => {
            let width = parts.next().and_then(|v| v.parse::<usize>().ok());
            let height = parts.next().and_then(|v| v.parse::<usize>().ok());
            match (width, height) {
                (Some(width), Some(height)) => {
                    shared
                        .extras
                        .pov
                        .width
                        .store(width.clamp(24, 160), Ordering::Relaxed);
                    shared
                        .extras
                        .pov
                        .height
                        .store(height.clamp(12, 80), Ordering::Relaxed);
                    shared.console.info(&format!(
                        "POV-Groesse: {}x{} Pixel.",
                        width.clamp(24, 160),
                        height.clamp(12, 80)
                    ));
                }
                _ => shared
                    .console
                    .error("Nutzung: :pov size <breite> <hoehe>   z. B. :pov size 80 40"),
            }
        }
        "info" | "status" => {
            let world = shared.extras.pov.world.lock().unwrap();
            shared.console.info(&format!(
                "POV: {} · y={}..{} · {} Chunks · {} Entities · {}",
                world.dimension.name,
                world.dimension.min_y,
                world.dimension.min_y + world.dimension.height - 1,
                world.chunks.len(),
                world.entities.len(),
                if shared.extras.pov.live.load(Ordering::Relaxed) {
                    "live"
                } else {
                    "gestoppt"
                }
            ));
        }
        _ => shared
            .console
            .error("Nutzung: :pov live|stop|frame|size <breite> <hoehe>|info"),
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
            owned.console.pov_frame("\x1b[2J\x1b[H");
            while owned.running.load(Ordering::Relaxed)
                && owned.extras.pov.live.load(Ordering::Relaxed)
            {
                if owned.in_game.load(Ordering::Relaxed) {
                    draw_once(&owned, true);
                }
                thread::sleep(RENDER_INTERVAL);
            }
            owned
                .extras
                .pov
                .renderer_running
                .store(false, Ordering::SeqCst);
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

fn draw_once(shared: &Arc<Shared>, home: bool) {
    let Some(position) = shared.position() else {
        return;
    };
    let scene = shared.extras.pov.scene();
    let width = shared.extras.pov.width.load(Ordering::Relaxed);
    let height = shared.extras.pov.height.load(Ordering::Relaxed);
    let frame = render(&scene, position, width, height, shared.console.is_color());
    shared
        .console
        .pov_frame(&format!("{}{}", if home { "\x1b[H" } else { "" }, frame));
}

#[derive(Clone, Copy)]
struct Pixel {
    rgb: (u8, u8, u8),
    depth: f64,
}

fn render(
    scene: &Scene,
    position: (f64, f64, f64, f32, f32),
    width: usize,
    height: usize,
    color: bool,
) -> String {
    let origin = (position.0, position.1 + 1.62, position.2);
    let vertical_fov = HORIZONTAL_FOV * height as f64 / width as f64;
    let mut pixels = vec![
        Pixel {
            rgb: (0, 0, 0),
            depth: MAX_DISTANCE
        };
        width * height
    ];

    for py in 0..height {
        for px in 0..width {
            let yaw = position.3 as f64 + ((px as f64 + 0.5) / width as f64 - 0.5) * HORIZONTAL_FOV;
            let pitch =
                position.4 as f64 + ((py as f64 + 0.5) / height as f64 - 0.5) * vertical_fov;
            let direction = view_direction(yaw, pitch);
            let sky = sky_color(py, height);
            let pixel = match cast(scene, origin, direction) {
                Some((state, distance, face)) => Pixel {
                    rgb: fog(block_color(state, face, distance), sky, distance),
                    depth: distance,
                },
                None => Pixel {
                    rgb: sky,
                    depth: MAX_DISTANCE,
                },
            };
            pixels[py * width + px] = pixel;
        }
    }
    overlay_entities(
        scene,
        origin,
        position.3 as f64,
        position.4 as f64,
        vertical_fov,
        width,
        height,
        &mut pixels,
    );

    let mut out = String::with_capacity(width * height * if color { 20 } else { 2 });
    out.push_str(&format!(
        "POV  x={:.1} y={:.1} z={:.1}  Blick {:.0}/{:.0}  Chunks {}  (:pov stop)\n",
        position.0,
        position.1,
        position.2,
        position.3,
        position.4,
        scene.chunks.len()
    ));
    if color {
        for y in (0..height).step_by(2) {
            for x in 0..width {
                let top = pixels[y * width + x].rgb;
                let bottom = pixels[(y + 1).min(height - 1) * width + x].rgb;
                out.push_str(&format!(
                    "\x1b[38;2;{};{};{}m\x1b[48;2;{};{};{}m▀",
                    top.0, top.1, top.2, bottom.0, bottom.1, bottom.2
                ));
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
    out
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
fn cast(scene: &Scene, origin: (f64, f64, f64), dir: (f64, f64, f64)) -> Option<(u32, f64, usize)> {
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
        if let Some(state) = scene.block(cell.0, cell.1, cell.2) {
            if state != 0 {
                return Some((state, distance, face));
            }
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

fn overlay_entities(
    scene: &Scene,
    origin: (f64, f64, f64),
    yaw: f64,
    pitch: f64,
    vertical_fov: f64,
    width: usize,
    height: usize,
    pixels: &mut [Pixel],
) {
    for entity in &scene.entities {
        let dx = entity.x - origin.0;
        let dz = entity.z - origin.2;
        let horizontal = (dx * dx + dz * dz).sqrt();
        let distance = (horizontal * horizontal + (entity.y + 0.9 - origin.1).powi(2)).sqrt();
        if distance < 0.1 || distance > MAX_DISTANCE {
            continue;
        }
        let target_yaw = (-dx).atan2(dz) * 180.0 / PI;
        let target_pitch = -(entity.y + 0.9 - origin.1).atan2(horizontal) * 180.0 / PI;
        let rel_yaw = wrap_degrees(target_yaw - yaw);
        let rel_pitch = target_pitch - pitch;
        if rel_yaw.abs() > HORIZONTAL_FOV * 0.55 || rel_pitch.abs() > vertical_fov * 0.65 {
            continue;
        }
        let center_x = ((rel_yaw / HORIZONTAL_FOV + 0.5) * width as f64) as isize;
        let center_y = ((rel_pitch / vertical_fov + 0.5) * height as f64) as isize;
        let entity_height = ((1.8 / distance) / (2.0 * (vertical_fov * PI / 360.0).tan())
            * height as f64)
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

fn wrap_degrees(mut value: f64) -> f64 {
    value %= 360.0;
    if value > 180.0 {
        value -= 360.0;
    }
    if value <= -180.0 {
        value += 360.0;
    }
    value
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::Writer;

    #[test]
    fn blickrichtung_folgt_minecraft_winkeln() {
        let south = view_direction(0.0, 0.0);
        assert!(south.0.abs() < 1e-9 && south.2 > 0.999);
        let west = view_direction(90.0, 0.0);
        assert!(west.0 < -0.999 && west.2.abs() < 1e-9);
        assert!(view_direction(0.0, -90.0).1 > 0.999);
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

    #[test]
    fn singleton_abschnitt_legacy_und_modern() {
        for modern in [false, true] {
            let mut w = Writer::default();
            w.u16(0); // Blockzahl
            w.u8(0); // Singleton Blocks
            w.var_int(0); // Luft
            if !modern {
                w.var_int(0);
            }
            w.u8(0); // Singleton Biome
            w.var_int(1);
            if !modern {
                w.var_int(0);
            }
            let dimension = Dimension {
                name: "test".into(),
                min_y: 0,
                height: 16,
            };
            let chunk = Chunk::decode(modern, false, &dimension, &w.data).unwrap();
            assert_eq!(chunk.block(0, 0, 0), 0);
        }
    }

    #[test]
    fn luft_wird_ueber_blockzaehler_erkannt() {
        let mut blocks = vec![7u32; SECTION_BLOCKS];
        blocks[..3000].fill(42); // cave_air-artiger zweiter Luftzustand
        blocks[3000..3500].fill(0);
        normalize_air(&mut blocks, SECTION_BLOCKS - 3500);
        assert!(blocks[..3500].iter().all(|state| *state == 0));
        assert!(blocks[3500..].iter().all(|state| *state == 7));
    }
}
