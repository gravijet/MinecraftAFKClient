//! Originale Minecraft-Ressourcen fuer die Browser-POV.
//!
//! Die Binary enthaelt bewusst keine Mojang-Texturen. Sie liest Blockstates, Modelle, Texturen
//! und GUI-Sprites aus der mit `--pov-resources` angegebenen Client-JAR bzw. Resourcepack-ZIP.
//! Nur die Zuordnung der Netzwerk-State-ID zum Blockzustand liegt komprimiert bei; sie wurde mit
//! `rust/data/generate-block-states.sh` aus den offiziellen Server-Reports erzeugt.

use flate2::read::GzDecoder;
use serde_json::Value;
use std::collections::{HashMap, HashSet};
use std::fs::File;
use std::io::{Cursor, Read};
use std::path::Path;
use std::sync::Mutex;
use zip::ZipArchive;

const STATES_1_21_1: &[u8] = include_bytes!("../data/block-states-1.21.1.txt.gz");
const STATES_1_21_11: &[u8] = include_bytes!("../data/block-states-1.21.11.txt.gz");
const STATES_26_1: &[u8] = include_bytes!("../data/block-states-26.1.txt.gz");
const STATES_26_2: &[u8] = include_bytes!("../data/block-states-26.2.txt.gz");

const MAX_ARCHIVE_ASSET_BYTES: u64 = 32 * 1024 * 1024;
const MAX_JSON_BYTES: u64 = 4 * 1024 * 1024;
const MAX_TEXTURE_DIMENSION: usize = 4096;
const MAX_TEXTURE_PIXELS: usize = 16 * 1024 * 1024;

/// Reihenfolge der sechs Blockseiten in Material und Treffer: West, Ost, Unten, Oben, Nord,
/// Sued. Die Namen entsprechen denen in den Vanilla-Modelldateien.
pub(crate) const FACE_NAMES: [&str; 6] = ["west", "east", "down", "up", "north", "south"];
pub(crate) type TexturedHit = ((u8, u8, u8, u8), f64, usize);

#[derive(Clone, Copy, Debug)]
pub(crate) struct Face {
    texture: u16,
    /// Woher diese Fläche ihren Farbton nimmt – `None` heißt „Textur unverändert".
    ///
    /// Beim Laden aufgelöst: Ob eine Fläche eingefärbt wird, sagt das Modell (`tintindex`);
    /// **woraus**, hängt am Block und steht in Vanilla im Code, nicht in den Daten
    /// (`BlockColors`). Siehe [`tint_for`].
    tint: Option<TintKind>,
    uv: Option<[f64; 4]>,
    rotation: i32,
}

/// **Woraus** eine Fläche mit `tintindex` ihre Farbe bezieht.
///
/// Das Modell sagt nur, *dass* eingefärbt wird; *womit*, hängt am Block (`BlockColors`) und bei
/// Gras, Laub und Wasser zusätzlich am Biom. Deshalb steht hier die Herkunft und nicht der
/// fertige Ton – der entsteht erst beim Zeichnen, wenn das Biom an der getroffenen Stelle
/// bekannt ist.
///
/// Blöcke, für die Vanilla gar keinen Einfärber kennt, bleiben **ungefärbt**. Vorher bekam jede
/// Fläche mit `tintindex` denselben Grünton – auch Redstone, Kürbisstiele und Karten.
#[derive(Clone, Copy, Debug)]
enum TintKind {
    Grass,
    Foliage,
    Water,
    /// Diese Töne setzt Vanilla unabhängig vom Biom.
    Fixed([u8; 3]),
}

fn tint_for(block: &str) -> Option<TintKind> {
    let (_, name) = resource_id(block);
    Some(match name {
        "grass_block" | "grass" | "short_grass" | "tall_grass" | "fern" | "large_fern"
        | "potted_fern" | "sugar_cane" | "pink_petals" | "attached_melon_stem"
        | "attached_pumpkin_stem" => TintKind::Grass,
        // Diese drei färbt Vanilla fest ein, unabhängig vom Biom.
        "spruce_leaves" => TintKind::Fixed([0x61, 0x99, 0x61]),
        "birch_leaves" => TintKind::Fixed([0x80, 0xA7, 0x55]),
        "lily_pad" => TintKind::Fixed([0x71, 0xC3, 0x5C]),
        "water" | "bubble_column" | "water_cauldron" => TintKind::Water,
        other if other.ends_with("_leaves") || other == "vine" => TintKind::Foliage,
        _ => return None,
    })
}

/// Die drei Farbtöne, mit denen ein Biom Flächen einfärbt – für ein Biom einmal ausgerechnet.
#[derive(Clone, Copy)]
pub(crate) struct BiomeTint {
    grass: [u8; 3],
    foliage: [u8; 3],
    water: [u8; 3],
}

impl BiomeTint {
    /// Der Stand ohne Biomdaten: genau die Töne, die diese Ansicht schon vorher für alles
    /// benutzt hat (gemäßigte Ebene/Wald). Damit sieht ein Server ohne Biom-Registry aus wie
    /// bisher, statt plötzlich grau zu werden.
    pub(crate) const PLAINS: BiomeTint = BiomeTint {
        grass: [0x91, 0xBD, 0x59],
        foliage: [0x77, 0xAB, 0x2F],
        water: [0x3F, 0x76, 0xE4],
    };

    /// Ohne geladene Farbkarten: Wasser steht schon in der Registry, Gras und Laub nicht.
    pub(crate) fn without_colormaps(params: &crate::pov::BiomeParams) -> BiomeTint {
        BiomeTint {
            water: rgb(params.water),
            ..BiomeTint::PLAINS
        }
    }

    #[inline]
    fn of(&self, kind: TintKind) -> [u8; 3] {
        match kind {
            TintKind::Grass => self.grass,
            TintKind::Foliage => self.foliage,
            TintKind::Water => self.water,
            TintKind::Fixed(color) => color,
        }
    }
}

fn rgb(value: u32) -> [u8; 3] {
    [(value >> 16) as u8, (value >> 8) as u8, value as u8]
}

/// Eine Farbkarte der JAR als 256x256-Tabelle. Fehlt oder passt sie nicht, bleibt es beim
/// Ebene-Ton – erfunden wird keine.
fn load_colormap(archive: &mut ZipArchive<File>, name: &str) -> Option<Box<[[u8; 3]]>> {
    let path = format!("assets/minecraft/textures/colormap/{}.png", name);
    let bytes = read_zip(archive, &path, MAX_ARCHIVE_ASSET_BYTES)?;
    let texture = decode_png(&bytes).ok()?;
    if texture.width != 256 || texture.rgba.len() < 256 * 256 * 4 {
        return None;
    }
    Some(
        texture
            .rgba
            .chunks_exact(4)
            .take(256 * 256)
            .map(|pixel| [pixel[0], pixel[1], pixel[2]])
            .collect(),
    )
}

/// Vanillas Griff in eine Farbkarte (`ColorMapColorUtil.get`).
///
/// Die Karte ist 256x256 groß und wird **nicht** linear abgetastet: Der Niederschlag geht mit der
/// Temperatur multipliziert ein, und beide Achsen laufen rückwärts. Genau diese Rechnung steht
/// hier – eine eigene, „vernünftigere" wäre eine andere Welt.
fn colormap_sample(map: &[[u8; 3]], temperature: f32, downfall: f32) -> [u8; 3] {
    let temperature = temperature.clamp(0.0, 1.0) as f64;
    let downfall = downfall.clamp(0.0, 1.0) as f64 * temperature;
    let x = ((1.0 - temperature) * 255.0) as usize;
    let y = ((1.0 - downfall) * 255.0) as usize;
    // Fehlt der Punkt, ist es dieselbe auffällige Fehlfarbe wie bei einer fehlenden Textur.
    map.get((y << 8) | x).copied().unwrap_or([0xFF, 0x00, 0xFF])
}

#[derive(Clone, Debug)]
pub(crate) struct Material {
    elements: Vec<Element>,
}

#[derive(Clone, Debug)]
struct Element {
    from: [f64; 3],
    to: [f64; 3],
    faces: [Option<Face>; 6],
    /// `world = transform * model + offset`; reine Rotationen, inverse = Transponierte.
    transform: [[f64; 3]; 3],
    offset: [f64; 3],
}

#[derive(Clone, Debug)]
struct Texture {
    width: usize,
    /// Nur das erste Animationsbild. Animierte Vanilla-Texturen liegen senkrecht untereinander.
    frame_height: usize,
    rgba: Vec<u8>,
}

pub(crate) struct Assets {
    states: Vec<u16>,
    materials: Vec<Material>,
    textures: Vec<Texture>,
    #[cfg(feature = "items")]
    block_defaults: HashMap<String, u32>,
    #[cfg(feature = "items")]
    item_icons: Mutex<HashMap<String, Vec<u8>>>,
    archive: Mutex<ZipArchive<File>>,
    missing_png: Vec<u8>,
    /// Die beiden Farbkarten der JAR, je 256x256 RGB. Aus ihnen kommt der Gras- und Laubton
    /// eines Bioms – dieselbe Tabelle, die auch das Spiel selbst abtastet.
    grass_map: Option<Box<[[u8; 3]]>>,
    foliage_map: Option<Box<[[u8; 3]]>>,
}

impl Assets {
    pub(crate) fn load(path: &Path, protocol: &str) -> Result<Assets, String> {
        let file = File::open(path)
            .map_err(|error| format!("{} liess sich nicht oeffnen: {}", path.display(), error))?;
        let mut archive = ZipArchive::new(file)
            .map_err(|error| format!("{} ist keine lesbare JAR/ZIP: {}", path.display(), error))?;
        verify_archive_version(&mut archive, protocol)?;
        let mut loader = Loader::new(archive);
        let state_text = state_table(protocol)?;
        let mut states = Vec::new();
        let mut block_defaults = HashMap::new();
        let mut blocks = HashSet::new();
        for (expected, line) in state_text.lines().enumerate() {
            let mut fields = line.splitn(4, '\t');
            let id = fields
                .next()
                .and_then(|value| value.parse::<usize>().ok())
                .ok_or_else(|| format!("Block-State-Tabelle fuer {} ist beschaedigt", protocol))?;
            let block = fields
                .next()
                .ok_or_else(|| format!("Block-State {} ohne Namen", id))?;
            let properties = fields.next().unwrap_or("");
            let is_default = match fields.next() {
                Some("0") => false,
                Some("1") => true,
                _ => {
                    return Err(format!(
                        "Block-State {} hat keine gueltige Default-Markierung",
                        id
                    ))
                }
            };
            if id != expected {
                return Err(format!(
                    "Block-State-Tabelle fuer {} hat eine Luecke bei {} (gefunden {})",
                    protocol, expected, id
                ));
            }
            blocks.insert(block.to_string());
            if is_default
                && block_defaults
                    .insert(block.to_string(), id as u32)
                    .is_some()
            {
                return Err(format!("Block {} hat mehrere Default-States", block));
            }
            states.push(loader.material_id(block, properties));
        }
        if states.is_empty() || loader.materials.is_empty() {
            return Err(format!("Block-State-Tabelle fuer {} ist leer", protocol));
        }
        if block_defaults.len() != blocks.len() {
            return Err(format!(
                "Block-State-Tabelle fuer {} hat nicht fuer jeden Block genau einen Default-State",
                protocol
            ));
        }

        let missing_png = encode_rgba_png(16, 16, &loader.textures[0].rgba)
            .map_err(|error| format!("Fehlertextur konnte nicht codiert werden: {}", error))?;
        let grass_map = load_colormap(&mut loader.archive, "grass");
        let foliage_map = load_colormap(&mut loader.archive, "foliage");
        Ok(Assets {
            states,
            materials: loader.materials,
            textures: loader.textures,
            #[cfg(feature = "items")]
            block_defaults,
            #[cfg(feature = "items")]
            item_icons: Mutex::new(HashMap::new()),
            archive: Mutex::new(loader.archive),
            missing_png,
            grass_map,
            foliage_map,
        })
    }

    /// Die Farbtöne eines Bioms, so wie das Spiel sie rechnet.
    ///
    /// Ausdrücklich gesetzte Farben aus der Registry gewinnen; sonst kommt der Ton aus der
    /// Farbkarte der JAR, abgetastet mit Temperatur und Niederschlag genau dieses Bioms.
    pub(crate) fn biome_tint(&self, params: &crate::pov::BiomeParams) -> BiomeTint {
        use crate::pov::GrassModifier;

        let from_map = |map: &Option<Box<[[u8; 3]]>>, fallback: [u8; 3]| match map {
            Some(map) => colormap_sample(map, params.temperature, params.downfall),
            None => fallback,
        };
        let grass = match params.grass {
            Some(color) => rgb(color),
            None => from_map(&self.grass_map, BiomeTint::PLAINS.grass),
        };
        let grass = match params.modifier {
            GrassModifier::None => grass,
            // Vanilla würfelt im Sumpf über ein Rauschfeld zwischen zwei Tönen. Das Feld gehört
            // zur Weltgenerierung und wird dem Client nie geschickt; genommen wird deshalb der
            // Ton, den der Sumpf auf dem allergrößten Teil seiner Fläche hat.
            GrassModifier::Swamp => rgb(6_975_545),
            // `(farbe & 0xFEFEFE) + 0x28340A >> 1` – Vanillas Mischung zum Dunkelwald-Ton hin.
            GrassModifier::DarkForest => {
                let packed =
                    (grass[0] as u32) << 16 | (grass[1] as u32) << 8 | grass[2] as u32;
                rgb(((packed & 0xFE_FEFE) + 0x28_340A) >> 1)
            }
        };
        BiomeTint {
            grass,
            foliage: match params.foliage {
                Some(color) => rgb(color),
                None => from_map(&self.foliage_map, BiomeTint::PLAINS.foliage),
            },
            water: rgb(params.water),
        }
    }

    /// Naechster sichtbarer Modellelement-Treffer innerhalb eines Blocks. Das ist mehr als ein
    /// Texturlookup: Slabs, Treppen, Zaunpfosten und gedrehte Cross-Modelle benutzen ihre echten
    /// JSON-Elemente statt zwangsweise einen vollen Einheitswuerfel zu belegen.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn hit(
        &self,
        state: u32,
        cell: (i32, i32, i32),
        origin: (f64, f64, f64),
        direction: (f64, f64, f64),
        enter: f64,
        leave: f64,
        biome: &BiomeTint,
    ) -> Option<TexturedHit> {
        let material_id = self.states.get(state as usize).copied().unwrap_or(0);
        let material = self
            .materials
            .get(material_id as usize)
            .unwrap_or(&self.materials[0]);
        let origin16 = [
            (origin.0 - cell.0 as f64) * 16.0,
            (origin.1 - cell.1 as f64) * 16.0,
            (origin.2 - cell.2 as f64) * 16.0,
        ];
        let direction16 = [direction.0 * 16.0, direction.1 * 16.0, direction.2 * 16.0];
        let mut best = None;
        for element in &material.elements {
            let inverse = transpose(element.transform);
            let local_origin = matrix_vector(inverse, subtract(origin16, element.offset));
            let local_direction = matrix_vector(inverse, direction16);
            let Some((near, near_face, far, far_face)) =
                ray_box(local_origin, local_direction, element.from, element.to)
            else {
                continue;
            };
            for (distance, face_index) in [(near, near_face), (far, far_face)] {
                if distance + 1e-7 < enter || distance > leave + 1e-7 {
                    continue;
                }
                if best.as_ref().is_some_and(|(_, old, _)| distance >= *old) {
                    continue;
                }
                let Some(face) = element.faces[face_index] else {
                    continue;
                };
                let point = add_scaled(local_origin, local_direction, distance);
                let (u, v) = face_uv(point, element.from, element.to, face_index, face.uv);
                let rgba = self.sample_face(face, u, v, biome);
                if rgba.3 < 16 {
                    continue;
                }
                let normal = matrix_vector(element.transform, face_normal(face_index));
                best = Some((rgba, distance.max(0.0), dominant_face(normal)));
            }
        }
        best
    }

    fn sample_face(&self, face: Face, u: f64, v: f64, biome: &BiomeTint) -> (u8, u8, u8, u8) {
        let (u, v) = match face.rotation.rem_euclid(360) {
            90 => (1.0 - v, u),
            180 => (1.0 - u, 1.0 - v),
            270 => (v, 1.0 - u),
            _ => (u, v),
        };
        let texture = self
            .textures
            .get(face.texture as usize)
            .unwrap_or(&self.textures[0]);
        let x = ((u.rem_euclid(1.0) * texture.width as f64) as usize).min(texture.width - 1);
        let y = ((v.rem_euclid(1.0) * texture.frame_height as f64) as usize)
            .min(texture.frame_height - 1);
        let at = (y * texture.width + x) * 4;
        let mut rgba = (
            texture.rgba[at],
            texture.rgba[at + 1],
            texture.rgba[at + 2],
            texture.rgba[at + 3],
        );
        if let Some(kind) = face.tint {
            // Genau wie im Spiel: Der Farbton multipliziert die Textur, er ersetzt sie nicht.
            // Die Struktur der echten Pixel bleibt darunter erhalten.
            let tint = biome.of(kind);
            rgba.0 = (rgba.0 as u16 * tint[0] as u16 / 255) as u8;
            rgba.1 = (rgba.1 as u16 * tint[1] as u16 / 255) as u8;
            rgba.2 = (rgba.2 as u16 * tint[2] as u16 / 255) as u8;
        }
        rgba
    }

    /// Die Farbe, die ein Blockzustand auf einer bestimmten Seite an dieser Stelle wirklich
    /// liefert – ohne Kamera, ohne Licht, ohne Nebel. Damit lässt sich blockgenau prüfen, ob die
    /// Ressourcen richtig gelesen wurden.
    #[cfg(test)]
    pub(crate) fn sample(&self, state: u32, face: usize, u: f64, v: f64) -> (u8, u8, u8, u8) {
        let material_id = self.states.get(state as usize).copied().unwrap_or(0);
        let material = self
            .materials
            .get(material_id as usize)
            .unwrap_or(&self.materials[0]);
        let selected = material
            .elements
            .iter()
            .find_map(|element| element.faces[face.min(5)])
            .unwrap_or(Face {
                texture: 0,
                tint: None,
                uv: None,
                rotation: 0,
            });
        self.sample_face(selected, u, v, &BiomeTint::PLAINS)
    }

    pub(crate) fn raw(&self, path: &str) -> Option<Vec<u8>> {
        let mut archive = self.archive.lock().ok()?;
        read_zip(&mut archive, path, MAX_ARCHIVE_ASSET_BYTES)
    }

    /// Vanilla-Icon fuer einen Registry-Namen. Direkte Item-Texturen haben Vorrang; fuer
    /// Blockitems folgt der gleichnamige Block. Komplex gerenderte Item-Modelle fallen sichtbar
    /// auf die lila-schwarze Fehlertextur zurueck statt ein falsches Icon zu erfinden.
    #[cfg(feature = "items")]
    pub(crate) fn item_png(&self, name: &str) -> Vec<u8> {
        if let Ok(cache) = self.item_icons.lock() {
            if let Some(bytes) = cache.get(name) {
                return bytes.clone();
            }
        }
        let (namespace, path) = resource_id(name);
        let direct = format!("assets/{}/textures/item/{}.png", namespace, path);
        let bytes = self
            .raw(&direct)
            .or_else(|| {
                let state = self.block_defaults.get(name).copied()?;
                self.render_block_icon(state)
            })
            .unwrap_or_else(|| self.missing_png.clone());
        if let Ok(mut cache) = self.item_icons.lock() {
            cache.insert(name.to_string(), bytes.clone());
        }
        bytes
    }

    pub(crate) fn missing_png(&self) -> &[u8] {
        &self.missing_png
    }

    /// Blockitems als kleines GUI-Modell statt als flach aufgeklebter Seitentextur. Die
    /// Elementgeometrie und alle Pixel stammen aus demselben Vanilla-Modell wie die Welt.
    #[cfg(feature = "items")]
    fn render_block_icon(&self, state: u32) -> Option<Vec<u8>> {
        let material_id = *self.states.get(state as usize)?;
        let material = self.materials.get(material_id as usize)?;
        let mut quads = Vec::new();
        for element in &material.elements {
            for face_index in 0..6 {
                let Some(face) = element.faces[face_index] else {
                    continue;
                };
                let local = face_vertices(element.from, element.to, face_index);
                let vertices = local.map(|point| {
                    let transformed = add(matrix_vector(element.transform, point), element.offset);
                    IconVertex {
                        projected: icon_project(transformed),
                        uv: face_uv(point, element.from, element.to, face_index, face.uv),
                    }
                });
                let normal = matrix_vector(element.transform, face_normal(face_index));
                quads.push((face, dominant_face(normal), vertices));
            }
        }
        if quads.is_empty() {
            return None;
        }
        let (mut min_x, mut max_x, mut min_y, mut max_y) = (
            f64::INFINITY,
            f64::NEG_INFINITY,
            f64::INFINITY,
            f64::NEG_INFINITY,
        );
        for (_, _, vertices) in &quads {
            for vertex in vertices {
                min_x = min_x.min(vertex.projected[0]);
                max_x = max_x.max(vertex.projected[0]);
                min_y = min_y.min(vertex.projected[1]);
                max_y = max_y.max(vertex.projected[1]);
            }
        }
        let scale = (14.0 / (max_x - min_x).max(1e-6)).min(14.0 / (max_y - min_y).max(1e-6));
        let center_x = (min_x + max_x) * 0.5;
        let center_y = (min_y + max_y) * 0.5;
        for (_, _, vertices) in &mut quads {
            for vertex in vertices {
                vertex.projected[0] = 7.5 + (vertex.projected[0] - center_x) * scale;
                vertex.projected[1] = 7.5 + (vertex.projected[1] - center_y) * scale;
            }
        }

        let mut rgba = vec![0u8; 16 * 16 * 4];
        let mut depth = vec![f64::NEG_INFINITY; 16 * 16];
        for (face, world_face, vertices) in quads {
            raster_triangle(
                self,
                face,
                world_face,
                [vertices[0], vertices[1], vertices[2]],
                &mut rgba,
                &mut depth,
            );
            raster_triangle(
                self,
                face,
                world_face,
                [vertices[0], vertices[2], vertices[3]],
                &mut rgba,
                &mut depth,
            );
        }
        encode_rgba_png(16, 16, &rgba).ok()
    }

    #[cfg(test)]
    pub(crate) fn material_count(&self) -> usize {
        self.states.len()
    }
}

/// Offizielle Client-JARs tragen ihre Version selbst ein. Ein Resourcepack hat diese Datei nicht
/// und bleibt zulaessig; ist sie vorhanden, darf eine versehentlich gewaehlte andere Spielversion
/// aber nicht still mit den falschen Netzwerk-State-IDs gerendert werden.
fn verify_archive_version(archive: &mut ZipArchive<File>, protocol: &str) -> Result<(), String> {
    if archive.by_name("version.json").is_err() {
        return Ok(());
    }
    let bytes = read_zip(archive, "version.json", MAX_JSON_BYTES)
        .ok_or_else(|| "version.json der Client-JAR ist zu gross oder nicht lesbar".to_string())?;
    let json: Value = serde_json::from_slice(&bytes)
        .map_err(|error| format!("version.json der Client-JAR ist ungueltig: {}", error))?;
    let found = json
        .get("id")
        .and_then(Value::as_str)
        .ok_or_else(|| "version.json der Client-JAR enthaelt keine Versions-ID".to_string())?;
    if found != protocol {
        return Err(format!(
            "Client-JAR ist fuer Minecraft {}, ausgewaehlt ist aber --mc {}",
            found, protocol
        ));
    }
    Ok(())
}

struct Loader {
    archive: ZipArchive<File>,
    json: HashMap<String, Option<Value>>,
    models: HashMap<String, ResolvedModel>,
    texture_ids: HashMap<String, u16>,
    textures: Vec<Texture>,
    material_ids: HashMap<String, u16>,
    materials: Vec<Material>,
}

#[derive(Clone, Default)]
struct ResolvedModel {
    textures: HashMap<String, String>,
    elements: Vec<Value>,
}

#[derive(Clone)]
struct ModelUse {
    name: String,
    x: i32,
    y: i32,
}

impl Loader {
    fn new(archive: ZipArchive<File>) -> Loader {
        let mut rgba = vec![0u8; 16 * 16 * 4];
        for y in 0..16 {
            for x in 0..16 {
                let color = if (x / 4 + y / 4) % 2 == 0 {
                    (248, 0, 248)
                } else {
                    (20, 20, 20)
                };
                let at = (y * 16 + x) * 4;
                rgba[at..at + 4].copy_from_slice(&[color.0, color.1, color.2, 255]);
            }
        }
        Loader {
            archive,
            json: HashMap::new(),
            models: HashMap::new(),
            texture_ids: HashMap::new(),
            textures: vec![Texture {
                width: 16,
                frame_height: 16,
                rgba,
            }],
            material_ids: HashMap::new(),
            materials: Vec::new(),
        }
    }

    fn material_id(&mut self, block: &str, properties: &str) -> u16 {
        let uses = self.model_uses(block, properties);
        let mut key = String::new();
        for used in &uses {
            use std::fmt::Write as _;
            let _ = write!(key, "{}@{},{};", used.name, used.x, used.y);
        }
        if let Some(id) = self.material_ids.get(&key) {
            return *id;
        }
        let tint = tint_for(block);
        let mut elements = Vec::new();
        // Für den Ersatzweg weiter unten: das erste benutzte Modell samt seiner Texturliste.
        let first_model = uses.first().map(|used| used.name.clone());
        for used in uses {
            let model = self.resolve_model(&used.name, 0);
            for raw in &model.elements {
                let Some(from) = raw.get("from").and_then(vector3) else {
                    continue;
                };
                let Some(to) = raw.get("to").and_then(vector3) else {
                    continue;
                };
                let mut faces: [Option<Face>; 6] = std::array::from_fn(|_| None);
                if let Some(object) = raw.get("faces").and_then(Value::as_object) {
                    for (index, direction) in FACE_NAMES.iter().enumerate() {
                        let Some(face) = object.get(*direction) else {
                            continue;
                        };
                        let Some(reference) = face.get("texture").and_then(Value::as_str) else {
                            continue;
                        };
                        let name = resolve_texture(reference, &model.textures);
                        faces[index] = Some(Face {
                            texture: self.texture(&name),
                            // Das Modell sagt *ob*, die Blockliste sagt *womit*.
                            tint: face.get("tintindex").and(tint),
                            uv: face.get("uv").and_then(vector4),
                            rotation: face.get("rotation").and_then(Value::as_i64).unwrap_or(0)
                                as i32,
                        });
                    }
                }
                let (transform, offset) = element_transform(raw, used.x, used.y);
                elements.push(Element {
                    from,
                    to,
                    faces,
                    transform,
                    offset,
                });
            }
        }

        if elements.is_empty() {
            // Ein Modell ohne Flächen ist kein Fehler: **Flüssigkeiten** haben genau das.
            // `block/water.json` besteht in Vanilla nur aus `"particle": "block/water_still"`,
            // weil das Spiel Wasser und Lava eigens zeichnet. Vorher endete das hier bei der
            // Suche nach einer Textur namens `block/water` – die es nicht gibt – und jeder
            // See wurde lila-schwarz. Die im Modell genannte Partikeltextur ist genau die
            // Fläche, die gemeint ist.
            //
            // Danach erst der Ersatz für schlichte Resourcepacks, die ein Modell auslassen und
            // nur eine gleichnamige Blocktextur mitbringen. Fehlt auch die, ist die Fehlertextur
            // absichtlich sichtbar – lieber erkennbar falsch als still erfunden.
            let particle = first_model.and_then(|name| {
                let model = self.resolve_model(&name, 0);
                let reference = model.textures.get("particle")?.clone();
                let resolved = resolve_texture(&reference, &model.textures);
                (!resolved.is_empty()).then_some(resolved)
            });
            let direct = format!("{}:block/{}", resource_id(block).0, resource_id(block).1);
            let texture = match particle {
                Some(name) => self.texture(&name),
                None if self.texture_exists(&direct) => self.texture(&direct),
                None => 0,
            };
            elements.push(Element {
                from: [0.0, 0.0, 0.0],
                to: [16.0, 16.0, 16.0],
                faces: std::array::from_fn(|_| {
                    Some(Face {
                        texture,
                        // Wasser trägt seine Farbe nicht in der Textur, sondern bekommt sie vom
                        // Biom; ohne Einfärbung wäre es schlicht grau.
                        tint,
                        uv: None,
                        rotation: 0,
                    })
                }),
                transform: identity(),
                offset: [0.0; 3],
            });
        }
        if self.materials.len() >= u16::MAX as usize {
            return 0;
        }
        let id = self.materials.len() as u16;
        self.materials.push(Material { elements });
        self.material_ids.insert(key, id);
        id
    }

    fn model_uses(&mut self, block: &str, properties: &str) -> Vec<ModelUse> {
        let (namespace, path) = resource_id(block);
        let file = format!("assets/{}/blockstates/{}.json", namespace, path);
        let Some(json) = self.json(&file) else {
            return vec![ModelUse {
                name: format!("{}:block/{}", namespace, path),
                x: 0,
                y: 0,
            }];
        };
        let properties = parse_properties(properties);
        let mut uses = Vec::new();
        if let Some(variants) = json.get("variants").and_then(Value::as_object) {
            let mut best = None;
            for (key, value) in variants {
                if variant_matches(key, &properties) {
                    let score = if key.is_empty() {
                        0
                    } else {
                        key.split(',').count()
                    };
                    if best.as_ref().is_none_or(|(old, _)| score > *old) {
                        best = Some((score, value.clone()));
                    }
                }
            }
            if let Some((_, value)) = best {
                add_model_uses(&value, &mut uses);
            }
        }
        if let Some(parts) = json.get("multipart").and_then(Value::as_array) {
            for part in parts {
                if part
                    .get("when")
                    .is_none_or(|when| condition_matches(when, &properties))
                {
                    if let Some(apply) = part.get("apply") {
                        add_model_uses(apply, &mut uses);
                    }
                }
            }
        }
        if uses.is_empty() {
            uses.push(ModelUse {
                name: format!("{}:block/{}", namespace, path),
                x: 0,
                y: 0,
            });
        }
        uses
    }

    fn resolve_model(&mut self, name: &str, depth: usize) -> ResolvedModel {
        if let Some(model) = self.models.get(name) {
            return model.clone();
        }
        if depth > 24 {
            return ResolvedModel::default();
        }
        let (namespace, path) = resource_id(name);
        let file = format!("assets/{}/models/{}.json", namespace, path);
        let Some(json) = self.json(&file) else {
            return ResolvedModel::default();
        };
        let mut model = json
            .get("parent")
            .and_then(Value::as_str)
            .map(|parent| self.resolve_model(parent, depth + 1))
            .unwrap_or_default();
        if let Some(textures) = json.get("textures").and_then(Value::as_object) {
            for (key, value) in textures {
                if let Some(value) = value.as_str() {
                    model.textures.insert(key.clone(), value.to_string());
                }
            }
        }
        if let Some(elements) = json.get("elements").and_then(Value::as_array) {
            model.elements.clone_from(elements);
        }
        self.models.insert(name.to_string(), model.clone());
        model
    }

    fn json(&mut self, path: &str) -> Option<Value> {
        if !self.json.contains_key(path) {
            let parsed = read_zip(&mut self.archive, path, MAX_JSON_BYTES)
                .and_then(|bytes| serde_json::from_slice(&bytes).ok());
            self.json.insert(path.to_string(), parsed);
        }
        self.json.get(path).cloned().flatten()
    }

    fn texture_exists(&mut self, name: &str) -> bool {
        let (namespace, path) = resource_id(name);
        let file = format!("assets/{}/textures/{}.png", namespace, path);
        let exists = self.archive.by_name(&file).is_ok();
        exists
    }

    fn texture(&mut self, name: &str) -> u16 {
        if name.is_empty() {
            return 0;
        }
        if let Some(id) = self.texture_ids.get(name) {
            return *id;
        }
        let (namespace, path) = resource_id(name);
        let file = format!("assets/{}/textures/{}.png", namespace, path);
        let Some(bytes) = read_zip(&mut self.archive, &file, MAX_ARCHIVE_ASSET_BYTES) else {
            return 0;
        };
        let Ok(texture) = decode_png(&bytes) else {
            return 0;
        };
        if self.textures.len() >= u16::MAX as usize {
            return 0;
        }
        let id = self.textures.len() as u16;
        self.textures.push(texture);
        self.texture_ids.insert(name.to_string(), id);
        id
    }
}

fn state_table(protocol: &str) -> Result<String, String> {
    let bytes = match protocol {
        "1.21.1" => STATES_1_21_1,
        "1.21.11" => STATES_1_21_11,
        "26.1" => STATES_26_1,
        "26.2" => STATES_26_2,
        other => return Err(format!("Keine Block-State-Tabelle fuer {}", other)),
    };
    let mut text = String::new();
    GzDecoder::new(bytes)
        .read_to_string(&mut text)
        .map_err(|error| format!("Block-State-Tabelle konnte nicht gelesen werden: {}", error))?;
    Ok(text)
}

fn read_zip(archive: &mut ZipArchive<File>, path: &str, max_size: u64) -> Option<Vec<u8>> {
    let file = archive.by_name(path).ok()?;
    if file.size() > max_size {
        return None;
    }
    let mut bytes = Vec::with_capacity(file.size() as usize);
    file.take(max_size + 1).read_to_end(&mut bytes).ok()?;
    (bytes.len() as u64 <= max_size).then_some(bytes)
}

fn resource_id(name: &str) -> (&str, &str) {
    name.split_once(':').unwrap_or(("minecraft", name))
}

fn parse_properties(text: &str) -> Vec<(&str, &str)> {
    text.split(',')
        .filter_map(|pair| pair.split_once('='))
        .collect()
}

fn variant_matches(key: &str, properties: &[(&str, &str)]) -> bool {
    key.is_empty()
        || key.split(',').all(|pair| {
            pair.split_once('=').is_some_and(|(wanted, value)| {
                properties
                    .iter()
                    .any(|(name, actual)| *name == wanted && *actual == value)
            })
        })
}

fn condition_matches(condition: &Value, properties: &[(&str, &str)]) -> bool {
    let Some(object) = condition.as_object() else {
        return true;
    };
    if let Some(or) = object.get("OR").and_then(Value::as_array) {
        return or.iter().any(|part| condition_matches(part, properties));
    }
    if let Some(and) = object.get("AND").and_then(Value::as_array) {
        return and.iter().all(|part| condition_matches(part, properties));
    }
    object.iter().all(|(name, wanted)| {
        let Some(wanted) = wanted.as_str() else {
            return false;
        };
        properties.iter().any(|(actual, value)| {
            *actual == name && wanted.split('|').any(|choice| choice == *value)
        })
    })
}

fn add_model_uses(value: &Value, out: &mut Vec<ModelUse>) {
    let value = value
        .as_array()
        .and_then(|items| items.first())
        .unwrap_or(value);
    let Some(name) = value.get("model").and_then(Value::as_str) else {
        return;
    };
    out.push(ModelUse {
        name: name.to_string(),
        x: value.get("x").and_then(Value::as_i64).unwrap_or(0) as i32,
        y: value.get("y").and_then(Value::as_i64).unwrap_or(0) as i32,
    });
}

fn resolve_texture(reference: &str, variables: &HashMap<String, String>) -> String {
    let mut current = reference;
    for _ in 0..16 {
        let Some(key) = current.strip_prefix('#') else {
            return current.to_string();
        };
        let Some(next) = variables.get(key) else {
            return String::new();
        };
        current = next;
    }
    String::new()
}

fn vector3(value: &Value) -> Option<[f64; 3]> {
    let values = value.as_array()?;
    Some([
        values.first()?.as_f64()?,
        values.get(1)?.as_f64()?,
        values.get(2)?.as_f64()?,
    ])
}

fn vector4(value: &Value) -> Option<[f64; 4]> {
    let values = value.as_array()?;
    Some([
        values.first()?.as_f64()?,
        values.get(1)?.as_f64()?,
        values.get(2)?.as_f64()?,
        values.get(3)?.as_f64()?,
    ])
}

fn identity() -> [[f64; 3]; 3] {
    [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]]
}

fn element_transform(raw: &Value, x: i32, y: i32) -> ([[f64; 3]; 3], [f64; 3]) {
    let mut transform = identity();
    let mut offset = [0.0; 3];
    if let Some(rotation) = raw.get("rotation") {
        let origin = rotation
            .get("origin")
            .and_then(vector3)
            .unwrap_or([8.0, 8.0, 8.0]);
        let axis = rotation.get("axis").and_then(Value::as_str).unwrap_or("y");
        let angle = rotation.get("angle").and_then(Value::as_f64).unwrap_or(0.0);
        compose_rotation(
            &mut transform,
            &mut offset,
            rotation_matrix(axis, angle),
            origin,
        );
    }
    if x.rem_euclid(360) != 0 {
        compose_rotation(
            &mut transform,
            &mut offset,
            rotation_matrix("x", x as f64),
            [8.0; 3],
        );
    }
    if y.rem_euclid(360) != 0 {
        compose_rotation(
            &mut transform,
            &mut offset,
            rotation_matrix("y", y as f64),
            [8.0; 3],
        );
    }
    (transform, offset)
}

fn rotation_matrix(axis: &str, degrees: f64) -> [[f64; 3]; 3] {
    let (sin, cos) = degrees.to_radians().sin_cos();
    match axis {
        "x" => [[1.0, 0.0, 0.0], [0.0, cos, -sin], [0.0, sin, cos]],
        "z" => [[cos, -sin, 0.0], [sin, cos, 0.0], [0.0, 0.0, 1.0]],
        _ => [[cos, 0.0, sin], [0.0, 1.0, 0.0], [-sin, 0.0, cos]],
    }
}

fn compose_rotation(
    transform: &mut [[f64; 3]; 3],
    offset: &mut [f64; 3],
    rotation: [[f64; 3]; 3],
    origin: [f64; 3],
) {
    let rotated_origin = matrix_vector(rotation, origin);
    let rotated_offset = matrix_vector(rotation, *offset);
    *offset = [
        rotated_offset[0] + origin[0] - rotated_origin[0],
        rotated_offset[1] + origin[1] - rotated_origin[1],
        rotated_offset[2] + origin[2] - rotated_origin[2],
    ];
    *transform = matrix_matrix(rotation, *transform);
}

fn matrix_matrix(a: [[f64; 3]; 3], b: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|row| {
        std::array::from_fn(|column| (0..3).map(|index| a[row][index] * b[index][column]).sum())
    })
}

fn matrix_vector(matrix: [[f64; 3]; 3], vector: [f64; 3]) -> [f64; 3] {
    std::array::from_fn(|row| {
        matrix[row][0] * vector[0] + matrix[row][1] * vector[1] + matrix[row][2] * vector[2]
    })
}

fn transpose(matrix: [[f64; 3]; 3]) -> [[f64; 3]; 3] {
    std::array::from_fn(|row| std::array::from_fn(|column| matrix[column][row]))
}

fn subtract(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] - b[0], a[1] - b[1], a[2] - b[2]]
}

fn add_scaled(origin: [f64; 3], direction: [f64; 3], distance: f64) -> [f64; 3] {
    [
        origin[0] + direction[0] * distance,
        origin[1] + direction[1] * distance,
        origin[2] + direction[2] * distance,
    ]
}

#[cfg(any(feature = "items", test))]
fn add(a: [f64; 3], b: [f64; 3]) -> [f64; 3] {
    [a[0] + b[0], a[1] + b[1], a[2] + b[2]]
}

#[cfg(feature = "items")]
#[derive(Clone, Copy)]
struct IconVertex {
    /// Bildschirm-X, Bildschirm-Y und Tiefe (groesser = naeher).
    projected: [f64; 3],
    uv: (f64, f64),
}

#[cfg(feature = "items")]
fn icon_project(point: [f64; 3]) -> [f64; 3] {
    let x = point[0] - 8.0;
    let y = point[1] - 8.0;
    let z = point[2] - 8.0;
    [
        (x - z) * std::f64::consts::FRAC_1_SQRT_2,
        (x + z) * 0.353_553_390_593_273_8 - y * 0.866_025_403_784_438_6,
        (x + z) * 0.612_372_435_695_794_5 + y * 0.5,
    ]
}

#[cfg(feature = "items")]
fn face_vertices(from: [f64; 3], to: [f64; 3], face: usize) -> [[f64; 3]; 4] {
    let (x0, y0, z0) = (from[0], from[1], from[2]);
    let (x1, y1, z1) = (to[0], to[1], to[2]);
    match face {
        0 => [[x0, y0, z1], [x0, y0, z0], [x0, y1, z0], [x0, y1, z1]],
        1 => [[x1, y0, z0], [x1, y0, z1], [x1, y1, z1], [x1, y1, z0]],
        2 => [[x0, y0, z0], [x0, y0, z1], [x1, y0, z1], [x1, y0, z0]],
        3 => [[x0, y1, z1], [x0, y1, z0], [x1, y1, z0], [x1, y1, z1]],
        4 => [[x1, y0, z0], [x0, y0, z0], [x0, y1, z0], [x1, y1, z0]],
        _ => [[x0, y0, z1], [x1, y0, z1], [x1, y1, z1], [x0, y1, z1]],
    }
}

#[cfg(feature = "items")]
fn raster_triangle(
    assets: &Assets,
    face: Face,
    world_face: usize,
    vertices: [IconVertex; 3],
    rgba: &mut [u8],
    depth: &mut [f64],
) {
    let [a, b, c] = vertices;
    let denominator = (b.projected[1] - c.projected[1]) * (a.projected[0] - c.projected[0])
        + (c.projected[0] - b.projected[0]) * (a.projected[1] - c.projected[1]);
    if denominator.abs() < 1e-9 {
        return;
    }
    let min_x = a.projected[0]
        .min(b.projected[0])
        .min(c.projected[0])
        .floor()
        .clamp(0.0, 15.0) as usize;
    let max_x = a.projected[0]
        .max(b.projected[0])
        .max(c.projected[0])
        .ceil()
        .clamp(0.0, 15.0) as usize;
    let min_y = a.projected[1]
        .min(b.projected[1])
        .min(c.projected[1])
        .floor()
        .clamp(0.0, 15.0) as usize;
    let max_y = a.projected[1]
        .max(b.projected[1])
        .max(c.projected[1])
        .ceil()
        .clamp(0.0, 15.0) as usize;
    let light = match world_face {
        3 => 1.0,
        2 => 0.55,
        4 | 5 => 0.82,
        _ => 0.70,
    };
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            let px = x as f64 + 0.5;
            let py = y as f64 + 0.5;
            let wa = ((b.projected[1] - c.projected[1]) * (px - c.projected[0])
                + (c.projected[0] - b.projected[0]) * (py - c.projected[1]))
                / denominator;
            let wb = ((c.projected[1] - a.projected[1]) * (px - c.projected[0])
                + (a.projected[0] - c.projected[0]) * (py - c.projected[1]))
                / denominator;
            let wc = 1.0 - wa - wb;
            if wa < -1e-6 || wb < -1e-6 || wc < -1e-6 {
                continue;
            }
            let z = wa * a.projected[2] + wb * b.projected[2] + wc * c.projected[2];
            let at = y * 16 + x;
            if z <= depth[at] {
                continue;
            }
            let u = wa * a.uv.0 + wb * b.uv.0 + wc * c.uv.0;
            let v = wa * a.uv.1 + wb * b.uv.1 + wc * c.uv.1;
            // Ein Gegenstands-Icon hängt an keinem Ort in der Welt und hat deshalb kein Biom.
            // Vanilla zeichnet Inventar-Icons genauso: mit dem Ton der gemäßigten Ebene.
            let color = assets.sample_face(face, u, v, &BiomeTint::PLAINS);
            if color.3 < 16 {
                continue;
            }
            depth[at] = z;
            let pixel = at * 4;
            rgba[pixel..pixel + 4].copy_from_slice(&[
                (color.0 as f64 * light).min(255.0) as u8,
                (color.1 as f64 * light).min(255.0) as u8,
                (color.2 as f64 * light).min(255.0) as u8,
                color.3,
            ]);
        }
    }
}

/// Strahl gegen eine ungedrehte Model-AABB. Zurueck kommen Eintritt und Austritt jeweils mit
/// lokaler Modellseite. Beide werden gebraucht, wenn die Kamera innerhalb eines Elements steht
/// oder ein Modell nur auf einer Seite eine Flaeche besitzt.
fn ray_box(
    origin: [f64; 3],
    direction: [f64; 3],
    from: [f64; 3],
    to: [f64; 3],
) -> Option<(f64, usize, f64, usize)> {
    let min_faces = [0usize, 2, 4];
    let max_faces = [1usize, 3, 5];
    let mut near = f64::NEG_INFINITY;
    let mut far = f64::INFINITY;
    let mut near_face = 3usize;
    let mut far_face = 2usize;
    for axis in 0..3 {
        if direction[axis].abs() < 1e-12 {
            if origin[axis] < from[axis] || origin[axis] > to[axis] {
                return None;
            }
            continue;
        }
        let a = (from[axis] - origin[axis]) / direction[axis];
        let b = (to[axis] - origin[axis]) / direction[axis];
        let (entry, entry_face, exit, exit_face) = if a <= b {
            (a, min_faces[axis], b, max_faces[axis])
        } else {
            (b, max_faces[axis], a, min_faces[axis])
        };
        if entry > near {
            near = entry;
            near_face = entry_face;
        }
        if exit < far {
            far = exit;
            far_face = exit_face;
        }
        if near > far + 1e-9 {
            return None;
        }
    }
    Some((near, near_face, far, far_face))
}

fn face_uv(
    point: [f64; 3],
    from: [f64; 3],
    to: [f64; 3],
    face: usize,
    explicit: Option<[f64; 4]>,
) -> (f64, f64) {
    let fraction = |value: f64, low: f64, high: f64| {
        if (high - low).abs() < 1e-9 {
            0.5
        } else {
            ((value - low) / (high - low)).clamp(0.0, 1.0)
        }
    };
    let (fu, fv, automatic) = match face {
        0 => (
            fraction(point[2], from[2], to[2]),
            1.0 - fraction(point[1], from[1], to[1]),
            (point[2] / 16.0, 1.0 - point[1] / 16.0),
        ),
        1 => (
            1.0 - fraction(point[2], from[2], to[2]),
            1.0 - fraction(point[1], from[1], to[1]),
            (1.0 - point[2] / 16.0, 1.0 - point[1] / 16.0),
        ),
        2 => (
            fraction(point[0], from[0], to[0]),
            1.0 - fraction(point[2], from[2], to[2]),
            (point[0] / 16.0, 1.0 - point[2] / 16.0),
        ),
        3 => (
            fraction(point[0], from[0], to[0]),
            fraction(point[2], from[2], to[2]),
            (point[0] / 16.0, point[2] / 16.0),
        ),
        4 => (
            1.0 - fraction(point[0], from[0], to[0]),
            1.0 - fraction(point[1], from[1], to[1]),
            (1.0 - point[0] / 16.0, 1.0 - point[1] / 16.0),
        ),
        _ => (
            fraction(point[0], from[0], to[0]),
            1.0 - fraction(point[1], from[1], to[1]),
            (point[0] / 16.0, 1.0 - point[1] / 16.0),
        ),
    };
    match explicit {
        Some(uv) => (
            (uv[0] + (uv[2] - uv[0]) * fu) / 16.0,
            (uv[1] + (uv[3] - uv[1]) * fv) / 16.0,
        ),
        None => automatic,
    }
}

fn face_normal(face: usize) -> [f64; 3] {
    match face {
        0 => [-1.0, 0.0, 0.0],
        1 => [1.0, 0.0, 0.0],
        2 => [0.0, -1.0, 0.0],
        3 => [0.0, 1.0, 0.0],
        4 => [0.0, 0.0, -1.0],
        _ => [0.0, 0.0, 1.0],
    }
}

fn dominant_face(normal: [f64; 3]) -> usize {
    let absolute = [normal[0].abs(), normal[1].abs(), normal[2].abs()];
    if absolute[0] >= absolute[1] && absolute[0] >= absolute[2] {
        if normal[0] < 0.0 {
            0
        } else {
            1
        }
    } else if absolute[1] >= absolute[2] {
        if normal[1] < 0.0 {
            2
        } else {
            3
        }
    } else if normal[2] < 0.0 {
        4
    } else {
        5
    }
}

fn decode_png(bytes: &[u8]) -> Result<Texture, String> {
    let mut decoder = png::Decoder::new(Cursor::new(bytes));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().map_err(|error| error.to_string())?;
    let width = reader.info().width as usize;
    let height = reader.info().height as usize;
    let pixels = width
        .checked_mul(height)
        .ok_or_else(|| "PNG-Abmessungen laufen ueber".to_string())?;
    if width == 0 || height == 0 {
        return Err("leere PNG".to_string());
    }
    if width > MAX_TEXTURE_DIMENSION
        || height > MAX_TEXTURE_DIMENSION
        || pixels > MAX_TEXTURE_PIXELS
    {
        return Err(format!(
            "PNG ist mit {}x{} Pixeln zu gross (maximal {}x{} und {} Pixel)",
            width, height, MAX_TEXTURE_DIMENSION, MAX_TEXTURE_DIMENSION, MAX_TEXTURE_PIXELS
        ));
    }
    let output_size = reader.output_buffer_size();
    if output_size > MAX_TEXTURE_PIXELS * 4 {
        return Err("decodierte PNG ist zu gross".to_string());
    }
    let mut buffer = vec![0; output_size];
    let info = reader
        .next_frame(&mut buffer)
        .map_err(|error| error.to_string())?;
    let source = &buffer[..info.buffer_size()];
    let mut rgba = Vec::with_capacity(pixels * 4);
    match info.color_type {
        png::ColorType::Rgba => rgba.extend_from_slice(source),
        png::ColorType::Rgb => {
            for rgb in source.chunks(3) {
                rgba.extend_from_slice(&[rgb[0], rgb[1], rgb[2], 255]);
            }
        }
        png::ColorType::Grayscale => {
            for value in source {
                rgba.extend_from_slice(&[*value, *value, *value, 255]);
            }
        }
        png::ColorType::GrayscaleAlpha => {
            for value in source.chunks(2) {
                rgba.extend_from_slice(&[value[0], value[0], value[0], value[1]]);
            }
        }
        png::ColorType::Indexed => return Err("indizierte PNG nach Expansion uebrig".to_string()),
    }
    debug_assert_eq!(width, info.width as usize);
    debug_assert_eq!(height, info.height as usize);
    Ok(Texture {
        width,
        frame_height: height.min(width),
        rgba,
    })
}

pub(crate) fn encode_rgba_png(width: usize, height: usize, rgba: &[u8]) -> Result<Vec<u8>, String> {
    if rgba.len() != width.saturating_mul(height).saturating_mul(4) {
        return Err("falsche RGBA-Laenge".to_string());
    }
    let mut bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut bytes, width as u32, height as u32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        encoder.set_compression(png::Compression::Fast);
        let mut writer = encoder.write_header().map_err(|error| error.to_string())?;
        writer
            .write_image_data(rgba)
            .map_err(|error| error.to_string())?;
    }
    Ok(bytes)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn state_tabellen_sind_lueckenlos() {
        for (version, expected) in [
            ("1.21.1", 26_684),
            ("1.21.11", 29_671),
            ("26.1", 29_873),
            ("26.2", 32_366),
        ] {
            let table = state_table(version).unwrap();
            assert_eq!(table.lines().count(), expected, "{}", version);
            assert!(table.starts_with("0\tminecraft:air\t\t1"), "{}", version);

            let mut defaults = HashMap::<&str, usize>::new();
            let mut blocks = HashSet::new();
            for line in table.lines() {
                let fields: Vec<_> = line.split('\t').collect();
                assert_eq!(fields.len(), 4, "{}: {}", version, line);
                blocks.insert(fields[1]);
                if fields[3] == "1" {
                    *defaults.entry(fields[1]).or_default() += 1;
                } else {
                    assert_eq!(fields[3], "0", "{}: {}", version, line);
                }
            }
            assert_eq!(defaults.len(), blocks.len(), "{}", version);
            assert!(defaults.values().all(|count| *count == 1), "{}", version);
        }
    }

    #[test]
    fn varianten_werden_als_mengen_verglichen() {
        let properties = parse_properties("axis=x,persistent=true");
        assert!(variant_matches("persistent=true,axis=x", &properties));
        assert!(!variant_matches("axis=z", &properties));
    }

    #[test]
    fn png_roundtrip_hat_signatur() {
        let png = encode_rgba_png(1, 1, &[1, 2, 3, 255]).unwrap();
        assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n");
    }

    #[test]
    fn slab_trifft_ihre_echte_halbe_hoehe() {
        let hit = ray_box(
            [8.0, 20.0, 8.0],
            [0.0, -16.0, 0.0],
            [0.0, 0.0, 0.0],
            [16.0, 8.0, 16.0],
        )
        .unwrap();
        assert!((hit.0 - 0.75).abs() < 1e-9);
        assert_eq!(hit.1, 3); // Oberseite, nicht die Oberkante eines Vollwuerfels
    }

    #[test]
    fn elementrotation_bleibt_um_ihren_ursprung() {
        let raw = serde_json::json!({
            "rotation": {"origin": [8, 8, 8], "axis": "y", "angle": 90}
        });
        let (matrix, offset) = element_transform(&raw, 0, 0);
        let center = add(matrix_vector(matrix, [8.0, 8.0, 8.0]), offset);
        assert!(center.iter().all(|value| (*value - 8.0).abs() < 1e-9));
    }

    /// Die Biomfarben gegen die echten Farbkarten der Original-JAR.
    ///
    /// Die erwarteten Werte sind nicht aus diesem Code gewonnen, sondern unabhängig aus
    /// `colormap/grass.png` abgelesen (Vanillas `ColorMapColorUtil.get`). Der Wert für die Ebene
    /// ist zugleich die Gegenprobe auf die alte Festfarbe: Sie war `#91BD59` – und genau das
    /// liefert die Farbkarte für Temperatur 0,8 und Niederschlag 0,4. Die Umstellung ändert also
    /// gerade dort nichts, wo bisher zufällig richtig geraten wurde, und alles dort, wo nicht.
    ///
    /// Die JAR wird selbst gesucht statt über `AFK_POV_RESOURCES` hereingereicht: Diese Variable
    /// zeigt für `originale_client_jar` auf eine 26.2er JAR, und ein Testlauf, bei dem dieselbe
    /// Variable je nach Testnamen eine andere Version meinen muss, geht irgendwann schief. Welche
    /// Version es ist, spielt hier ohnehin keine Rolle: Die vier abgetasteten Punkte liefern in
    /// 1.21.1 und 26.2 dieselben Werte.
    ///
    /// `XDG_CONFIG_HOME=$(mktemp -d) cargo test --features pov-client -- --ignored biomfarben`
    #[test]
    #[ignore]
    fn biomfarben_kommen_aus_der_farbkarte() {
        use crate::pov::{BiomeParams, GrassModifier};
        let console = crate::console::Console::new(false, false, false);
        let version = "1.21.1";
        let path =
            crate::pov_resources::locate(&console, version, &crate::pov_resources::Source::Auto)
                .expect("Ressourcen beschaffen");
        let assets = Assets::load(&path, version).expect("Assets laden");

        let biome = |temperature: f32, downfall: f32| BiomeParams {
            temperature,
            downfall,
            ..BiomeParams::PLAINS
        };
        // Ebene, Wüste, Taiga, Dschungel – vier deutlich verschiedene Klimapunkte.
        assert_eq!(assets.biome_tint(&biome(0.8, 0.4)).grass, [0x91, 0xBD, 0x59]);
        assert_eq!(assets.biome_tint(&biome(2.0, 0.0)).grass, [0xBF, 0xB7, 0x55]);
        assert_eq!(assets.biome_tint(&biome(0.25, 0.8)).grass, [0x86, 0xB7, 0x83]);
        assert_eq!(assets.biome_tint(&biome(0.95, 0.9)).grass, [0x59, 0xC9, 0x3C]);

        // Eine ausdrücklich gesetzte Farbe schlägt die Farbkarte.
        let fixed = BiomeParams {
            grass: Some(0x123456),
            ..BiomeParams::PLAINS
        };
        assert_eq!(assets.biome_tint(&fixed).grass, [0x12, 0x34, 0x56]);

        // Der Sumpf färbt unabhängig von Temperatur und Niederschlag.
        let swamp = BiomeParams {
            modifier: GrassModifier::Swamp,
            ..BiomeParams::PLAINS
        };
        assert_eq!(assets.biome_tint(&swamp).grass, [0x6A, 0x70, 0x39]);

        // Wasser steht als Zahl in der Registry und braucht gar keine Farbkarte.
        let water = BiomeParams {
            water: 0x617B64,
            ..BiomeParams::PLAINS
        };
        assert_eq!(assets.biome_tint(&water).water, [0x61, 0x7B, 0x64]);
    }

    /// Manueller Integrationslauf gegen eine unveraenderte Original-Client-JAR:
    /// `AFK_POV_RESOURCES=/pfad/26.2.jar cargo test --features pov-client -- --ignored originale_client_jar`
    #[test]
    #[ignore]
    #[cfg(feature = "items")]
    fn originale_client_jar() {
        let path = std::env::var("AFK_POV_RESOURCES").expect("AFK_POV_RESOURCES fehlt");
        assert!(matches!(
            Assets::load(Path::new(&path), "26.1"),
            Err(error) if error.contains("ausgewaehlt ist aber --mc 26.1")
        ));
        let assets = Assets::load(Path::new(&path), "26.2").expect("Assets laden");
        assert_eq!(assets.material_count(), 32_366);
        assert!(assets
            .raw("assets/minecraft/textures/block/stone.png")
            .is_some());
        assert!(assets
            .raw("assets/minecraft/textures/gui/container/generic_54.png")
            .is_some());
        assert_ne!(assets.sample(1, 3, 0.5, 0.5).0, 248);
        let stone_icon = assets.item_png("minecraft:stone");
        assert_eq!(&stone_icon[..8], b"\x89PNG\r\n\x1a\n");
        // Beim zweiten Aufruf kommt dasselbe bereits gerenderte Icon aus dem Cache.
        assert_eq!(assets.item_png("minecraft:stone"), stone_icon);
    }
}
