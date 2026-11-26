//! Bewegungseigenschaften jedes Blockzustands, erzeugt aus Mojangs eigenem Code
//! (`data/physics/Dump.java` → `data/physics/generate.py`).
//!
//! Die Tabellen liegen unkomprimiert in der Binärdatei: Nur gelesene Seiten werden geladen, und
//! sie belegen keinen Heap. Je Blockzustand eine 16-Bit-Klassennummer; je Klasse Reibung,
//! Geschwindigkeits- und Sprungfaktor, Merkmale und die Kollisionsboxen.

use super::Aabb;

static CLASSES: &[u8] = include_bytes!("../../data/physics/classes.bin");

/// Merkmale einer Klasse (gleiche Bits wie in `data/physics/generate.py`; die dort zusätzlich
/// gesetzten Bits für fallendes Wasser, Honig und Seelensand wertet die Physik nicht aus –
/// Seelensand und Honig bremsen über ihren Geschwindigkeitsfaktor).
pub mod flag {
    pub const WATER: u16 = 1 << 0;
    pub const LAVA: u16 = 1 << 1;
    pub const CLIMBABLE: u16 = 1 << 3;
    pub const COBWEB: u16 = 1 << 4;
    pub const BERRY_BUSH: u16 = 1 << 5;
    pub const POWDER_SNOW: u16 = 1 << 6;
    pub const SLIME: u16 = 1 << 7;
    pub const BED: u16 = 1 << 8;
    pub const SCAFFOLDING: u16 = 1 << 10;
    pub const BUBBLE_COLUMN: u16 = 1 << 11;
}

/// Klassennummer je Blockzustand für eine Protokollversion.
pub struct StateTable(&'static [u8]);

impl StateTable {
    pub fn for_version(name: &str) -> StateTable {
        StateTable(match name {
            "1.8.9" => include_bytes!("../../data/physics/1.8.9.bin"),
            "1.21.1" => include_bytes!("../../data/physics/1.21.1.bin"),
            "1.21.11" => include_bytes!("../../data/physics/1.21.11.bin"),
            "26.1" => include_bytes!("../../data/physics/26.1.bin"),
            "26.2" => include_bytes!("../../data/physics/26.2.bin"),
            "26.3" => include_bytes!("../../data/physics/26.3.bin"),
            "26.4" => include_bytes!("../../data/physics/26.4.bin"),
            _ => &[],
        })
    }

    /// Klasse eines Blockzustands. Unbekannte Zustände (ein Server mit Mods, ein zu neuer
    /// Bericht) gelten als voller Block: lieber anstoßen als durch eine Wand fallen.
    pub fn class(&self, state: u32) -> u16 {
        let at = state as usize * 2;
        match self.0.get(at..at + 2) {
            Some(bytes) => u16::from_le_bytes([bytes[0], bytes[1]]),
            None => full_cube_class(),
        }
    }
}

/// Ein Eintrag aus `classes.bin`.
#[derive(Clone, Copy)]
pub struct Class {
    pub friction: f32,
    pub speed_factor: f32,
    pub jump_factor: f32,
    pub flags: u16,
    /// Flüssigkeitsstand 1..8 (8 = Quelle oder fallend), 0 = keine Flüssigkeit.
    pub fluid_amount: u8,
    boxes: &'static [u8],
}

const HEADER: usize = 4 * 3 + 2 + 1 + 1;
const BOX: usize = 6 * 4;

impl Class {
    pub fn has(&self, flag: u16) -> bool {
        self.flags & flag != 0
    }

    #[cfg(test)]
    pub fn is_empty(&self) -> bool {
        self.boxes.is_empty()
    }

    /// Kollisionsboxen in Blockkoordinaten (0..1, Zäune bis 1,5).
    pub fn boxes(&self) -> impl Iterator<Item = Aabb> + '_ {
        self.boxes.chunks_exact(BOX).map(|b| {
            let f = |i: usize| f32::from_le_bytes([b[i], b[i + 1], b[i + 2], b[i + 3]]) as f64;
            Aabb {
                min: [f(0), f(4), f(8)],
                max: [f(12), f(16), f(20)],
            }
        })
    }
}

/// Start jeder Klasse in `classes.bin`, einmal ausgerechnet (gut 2 KB).
fn offsets() -> &'static [u32] {
    static OFFSETS: std::sync::OnceLock<Vec<u32>> = std::sync::OnceLock::new();
    OFFSETS.get_or_init(|| {
        let count = u16::from_le_bytes([CLASSES[0], CLASSES[1]]) as usize;
        let mut out = Vec::with_capacity(count);
        let mut at = 2usize;
        for _ in 0..count {
            out.push(at as u32);
            let boxes = CLASSES[at + HEADER - 1] as usize;
            at += HEADER + boxes * BOX;
        }
        out
    })
}

pub fn class(index: u16) -> Class {
    let Some(&at) = offsets().get(index as usize) else {
        return class(full_cube_class());
    };
    let at = at as usize;
    let f = |i: usize| {
        f32::from_le_bytes([
            CLASSES[at + i],
            CLASSES[at + i + 1],
            CLASSES[at + i + 2],
            CLASSES[at + i + 3],
        ])
    };
    let boxes = CLASSES[at + HEADER - 1] as usize;
    Class {
        friction: f(0),
        speed_factor: f(4),
        jump_factor: f(8),
        flags: u16::from_le_bytes([CLASSES[at + 12], CLASSES[at + 13]]),
        fluid_amount: CLASSES[at + 14],
        boxes: &CLASSES[at + HEADER..at + HEADER + boxes * BOX],
    }
}

/// Die Klasse von Stein (Zustand 1 in jeder modernen Version): ein voller Würfel mit
/// Standardreibung.
pub fn full_cube_class() -> u16 {
    static STONE: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
    *STONE.get_or_init(|| StateTable::for_version("26.1").class(1))
}

/// Die Klasse von Luft (Zustand 0 in jeder Version).
pub fn air_class() -> u16 {
    static AIR: std::sync::OnceLock<u16> = std::sync::OnceLock::new();
    *AIR.get_or_init(|| StateTable::for_version("26.1").class(0))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stein_ist_ein_voller_wuerfel_und_luft_ist_leer() {
        let stone = class(full_cube_class());
        let boxes: Vec<Aabb> = stone.boxes().collect();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].min, [0.0, 0.0, 0.0]);
        assert_eq!(boxes[0].max, [1.0, 1.0, 1.0]);
        assert!((stone.friction - 0.6).abs() < 1e-6);
        assert!(class(air_class()).is_empty());
    }

    #[test]
    fn jede_version_hat_eine_tabelle() {
        for protocol in crate::protocol::PROTOCOLS {
            let table = StateTable::for_version(protocol.name);
            assert!(!table.0.is_empty(), "{}", protocol.name);
            assert_eq!(table.class(0), air_class(), "{}", protocol.name);
        }
    }

    /// Stichproben gegen bekannte Vanilla-Werte (Zustandsnummern aus dem 26.1-Bericht).
    #[test]
    fn bekannte_bloecke_haben_vanilla_werte() {
        let table = StateTable::for_version("26.1");
        let find = |name: &str, props: &str| -> u32 {
            use std::io::Read;
            let mut text = String::new();
            flate2::read::GzDecoder::new(
                &include_bytes!("../../data/block-states-26.1.txt.gz")[..],
            )
            .read_to_string(&mut text)
            .unwrap();
            text.lines()
                .find(|line| {
                    let f: Vec<&str> = line.split('\t').collect();
                    f[1] == name && f[2] == props
                })
                .map(|line| line.split('\t').next().unwrap().parse().unwrap())
                .unwrap()
        };
        let ice = class(table.class(find("minecraft:ice", "")));
        assert!((ice.friction - 0.98).abs() < 1e-6);
        let soul_sand = class(table.class(find("minecraft:soul_sand", "")));
        assert!((soul_sand.speed_factor - 0.4).abs() < 1e-6);
        let water = class(table.class(find("minecraft:water", "level=0")));
        assert!(water.has(flag::WATER) && water.is_empty() && water.fluid_amount == 8);
        let ladder = class(table.class(find("minecraft:ladder", "facing=north,waterlogged=false")));
        assert!(ladder.has(flag::CLIMBABLE));
        let slab = class(table.class(find("minecraft:oak_slab", "type=bottom,waterlogged=false")));
        let boxes: Vec<Aabb> = slab.boxes().collect();
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].max[1], 0.5);
        let fence = class(table.class(find(
            "minecraft:oak_fence",
            "east=false,north=false,south=false,waterlogged=false,west=false",
        )));
        assert_eq!(fence.boxes().next().unwrap().max[1], 1.5);
    }
}
