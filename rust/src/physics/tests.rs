//! Physik gegen bekannte Vanilla-Werte.
//!
//! Die Zahlen sind keine Schätzungen: Fallkurve (0,08 Schwerkraft, ×0,98), Sprunghöhe
//! (Scheitel 1,2522 Blöcke), Gehtempo am Boden (0,21585 Blöcke je Tick im Gleichgewicht) und
//! Sprinttempo (0,28061) sind die Werte, die jeder Vanilla-Client für dieselben Eingaben liefert.

use super::blocks::{self, StateTable};
use super::*;
use std::collections::HashMap;

/// Eine kleine Testwelt aus Blockzuständen der Version 26.1.
struct TestWorld {
    blocks: HashMap<(i32, i32, i32), u32>,
    table: StateTable,
}

impl TestWorld {
    fn new() -> TestWorld {
        TestWorld {
            blocks: HashMap::new(),
            table: StateTable::for_version("26.1"),
        }
    }

    fn floor(mut self, y: i32, radius: i32) -> TestWorld {
        for x in -radius..=radius {
            for z in -radius..=radius {
                self.blocks.insert((x, y, z), STONE);
            }
        }
        self
    }

    fn set(mut self, x: i32, y: i32, z: i32, state: u32) -> TestWorld {
        self.blocks.insert((x, y, z), state);
        self
    }
}

impl Blocks for TestWorld {
    fn class_at(&self, x: i32, y: i32, z: i32) -> Option<u16> {
        Some(self.table.class(*self.blocks.get(&(x, y, z)).unwrap_or(&0)))
    }
}

/// Zustandsnummern aus `block-states-26.1`.
const STONE: u32 = 1;

fn state(name: &str, props: &str) -> u32 {
    use std::io::Read;
    let mut text = String::new();
    flate2::read::GzDecoder::new(&include_bytes!("../../data/block-states-26.1.txt.gz")[..])
        .read_to_string(&mut text)
        .unwrap();
    text.lines()
        .find(|line| {
            let f: Vec<&str> = line.split('\t').collect();
            f[1] == name && f[2] == props
        })
        .map(|line| line.split('\t').next().unwrap().parse().unwrap())
        .unwrap_or_else(|| panic!("{} {} fehlt", name, props))
}

fn body_at(x: f64, y: f64, z: f64) -> Body {
    let mut body = Body::new(false);
    body.teleport([x, y, z], [0.0; 3], 0.0, 0.0);
    body
}

fn run(body: &mut Body, keys: Keys, world: &TestWorld, ticks: usize) {
    for _ in 0..ticks {
        body.tick(&keys, world);
    }
}

#[test]
fn faellt_und_landet_genau_auf_dem_block() {
    let world = TestWorld::new().floor(64, 3);
    let mut body = body_at(0.5, 70.0, 0.5);
    let mut previous = body.pos[1];
    // Erster Tick: v = -0,08·0,98 nach der Bewegung; die Bewegung selbst ist noch 0.
    body.tick(&Keys::default(), &world);
    assert_eq!(body.pos[1], previous);
    // Vanilla rechnet 0,98 als `float`: -0,08 · 0,98f = -0,0784000015…
    assert_eq!(body.vel[1], -0.08 * 0.98f32 as f64);
    for _ in 0..60 {
        previous = body.pos[1];
        body.tick(&Keys::default(), &world);
        assert!(body.pos[1] <= previous);
    }
    assert_eq!(body.pos[1], 65.0, "steht genau auf dem Stein");
    assert!(body.on_ground);
    // Auch im Stand zieht die Schwerkraft: Vanilla-Spieler haben am Boden genau diesen Wert.
    assert_eq!(body.vel[1], -0.08 * 0.98f32 as f64);
}

#[test]
fn schwebt_nie_ueber_einer_kante() {
    // Das alte Verhalten: :go über eine Kante, und der Bot blieb in der Luft stehen.
    let world = TestWorld::new().floor(64, 1).floor(60, 20);
    let mut body = body_at(0.5, 65.0, 0.5);
    run(&mut body, Keys::default(), &world, 5);
    assert!(body.on_ground);
    let keys = Keys {
        forward: true,
        ..Keys::default()
    };
    run(&mut body, keys, &world, 20);
    run(&mut body, Keys::default(), &world, 40);
    assert_eq!(body.pos[1], 61.0, "ist die Kante hinuntergefallen");
}

#[test]
fn gehtempo_entspricht_vanilla() {
    let world = TestWorld::new().floor(64, 40);
    let mut body = body_at(0.5, 65.0, 0.5);
    run(&mut body, Keys::default(), &world, 3);
    let keys = Keys {
        forward: true,
        ..Keys::default()
    };
    run(&mut body, keys, &world, 60);
    let before = body.pos[2];
    body.tick(&keys, &world);
    let speed = body.pos[2] - before;
    // 0,1 · 0,98 / (1 − 0,546) = 0,21585 Blöcke je Tick ≈ 4,317 Blöcke je Sekunde.
    assert!((speed - 0.215_85).abs() < 1e-3, "{}", speed);
}

#[test]
fn sprinttempo_entspricht_vanilla() {
    let world = TestWorld::new().floor(64, 60);
    let mut body = body_at(0.5, 65.0, 0.5);
    run(&mut body, Keys::default(), &world, 3);
    let keys = Keys {
        forward: true,
        sprint: true,
        ..Keys::default()
    };
    run(&mut body, keys, &world, 60);
    assert!(body.sprinting);
    let before = body.pos[2];
    body.tick(&keys, &world);
    let speed = body.pos[2] - before;
    // 0,13 · 0,98 / 0,454 = 0,28062 ≈ 5,612 Blöcke je Sekunde.
    assert!((speed - 0.280_62).abs() < 1e-3, "{}", speed);
}

#[test]
fn sprung_erreicht_vanilla_scheitel() {
    let world = TestWorld::new().floor(64, 3);
    let mut body = body_at(0.5, 65.0, 0.5);
    run(&mut body, Keys::default(), &world, 3);
    let jump = Keys {
        jump: true,
        ..Keys::default()
    };
    body.tick(&jump, &world);
    let mut top: f64 = body.pos[1];
    for _ in 0..30 {
        body.tick(&Keys::default(), &world);
        top = top.max(body.pos[1]);
    }
    assert!(
        (top - 65.0 - 1.2522).abs() < 1e-3,
        "Scheitel {}",
        top - 65.0
    );
    assert_eq!(body.pos[1], 65.0);
}

#[test]
fn stufe_von_einer_halben_platte_wird_hinaufgestiegen() {
    let slab = state("minecraft:oak_slab", "type=bottom,waterlogged=false");
    let mut world = TestWorld::new().floor(64, 30);
    for z in 2..30 {
        world = world.set(0, 65, z, slab);
    }
    let mut body = body_at(0.5, 65.0, 0.5);
    run(&mut body, Keys::default(), &world, 3);
    let keys = Keys {
        forward: true,
        ..Keys::default()
    };
    run(&mut body, keys, &world, 20);
    assert_eq!(body.pos[1], 65.5, "auf der Platte");
}

#[test]
fn ein_voller_block_braucht_einen_sprung() {
    let mut world = TestWorld::new().floor(64, 30);
    for z in 2..30 {
        world = world.set(0, 65, z, STONE);
    }
    let mut body = body_at(0.5, 65.0, 0.5);
    run(&mut body, Keys::default(), &world, 3);
    let walk = Keys {
        forward: true,
        ..Keys::default()
    };
    run(&mut body, walk, &world, 20);
    assert_eq!(body.pos[1], 65.0, "ohne Sprung kein Hinaufsteigen");
    assert!(body.horizontal_collision);
    assert!(body.pos[2] <= 1.7 + 1e-9, "die Wand hält: {}", body.pos[2]);
    let walk_and_jump = Keys {
        forward: true,
        jump: true,
        ..Keys::default()
    };
    run(&mut body, walk_and_jump, &world, 20);
    run(&mut body, walk, &world, 10);
    assert_eq!(body.pos[1], 66.0, "mit Sprung oben");
}

#[test]
fn schleichen_haelt_an_der_kante() {
    let world = TestWorld::new().floor(64, 1).floor(55, 6);
    let mut body = body_at(0.5, 65.0, 0.5);
    run(&mut body, Keys::default(), &world, 3);
    let keys = Keys {
        forward: true,
        shift: true,
        ..Keys::default()
    };
    run(&mut body, keys, &world, 80);
    assert_eq!(body.pos[1], 65.0, "nicht hinuntergefallen");
    // Die Box (0,3 Halbbreite) darf höchstens bis knapp an die Kante (z = 2) hinausragen.
    assert!(body.pos[2] < 2.3 + 1e-6, "{}", body.pos[2]);
}

#[test]
fn im_wasser_sinkt_es_langsam() {
    let water = state("minecraft:water", "level=0");
    let mut world = TestWorld::new().floor(50, 3);
    for y in 51..70 {
        for x in -2..=2 {
            for z in -2..=2 {
                world = world.set(x, y, z, water);
            }
        }
    }
    let mut body = body_at(0.5, 65.0, 0.5);
    run(&mut body, Keys::default(), &world, 40);
    let before = body.pos[1];
    body.tick(&Keys::default(), &world);
    let sink = before - body.pos[1];
    // Endgeschwindigkeit im Wasser: 0,005 / 0,2 = 0,025 Blöcke je Tick.
    assert!(sink > 0.0 && sink < 0.03, "{}", sink);
}

#[test]
fn stehender_bot_rechnet_nicht() {
    let world = TestWorld::new().floor(64, 3);
    let mut body = body_at(0.5, 65.0, 0.5);
    run(&mut body, Keys::default(), &world, 5);
    assert!(body.settled(&Keys::default()));
    let walk = Keys {
        forward: true,
        ..Keys::default()
    };
    assert!(!body.settled(&walk));
}

#[test]
fn positionserinnerung_alle_zwanzig_ticks() {
    let world = TestWorld::new().floor(64, 3);
    let mut body = body_at(0.5, 65.0, 0.5);
    run(&mut body, Keys::default(), &world, 5);
    let mut reminders = 0;
    for _ in 0..100 {
        if body.idle().moved {
            reminders += 1;
        }
    }
    assert_eq!(reminders, 5);
}

#[test]
fn eingabe_bits_wie_vanilla() {
    let keys = Keys {
        forward: true,
        jump: true,
        sprint: true,
        ..Keys::default()
    };
    assert_eq!(keys.bits(), 1 | 16 | 64);
}

#[test]
fn spinnennetz_bremst() {
    let web = state("minecraft:cobweb", "");
    let world = TestWorld::new().floor(64, 5).set(0, 65, 1, web);
    let mut body = body_at(0.5, 65.0, 0.5);
    run(&mut body, Keys::default(), &world, 3);
    let keys = Keys {
        forward: true,
        ..Keys::default()
    };
    run(&mut body, keys, &world, 40);
    assert!(body.pos[2] < 2.5, "im Netz gefangen: {}", body.pos[2]);
}

#[test]
fn blockklassen_sind_vorhanden() {
    assert!(!blocks::class(blocks::full_cube_class()).is_empty());
}
