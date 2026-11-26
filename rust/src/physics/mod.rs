//! Spielerphysik, nachgebaut aus dem unverschleierten 26.x-Client.
//!
//! Ein Tick hier ist ein Tick von `LocalPlayer`: Eingabe aufbereiten (`modifyInput`), springen
//! (`LivingEntity.aiStep`), reisen (`travel` → `travelInAir`/`travelInWater`/`travelInLava`),
//! bewegen mit Kollision und Stufen (`Entity.move`/`collide`, inklusive der Stufenkandidaten
//! aus `collectCandidateStepUpHeights`), Kantenschutz beim Schleichen
//! (`Player.maybeBackOffFromEdge`) und Blockwirkungen (Spinnennetz, Beerenbusch, Pulverschnee,
//! Schleim). Konstanten und Reihenfolge stehen so im Original; wo bewusst vereinfacht wurde,
//! steht ein `shortcut:`.
//!
//! Der Zustand wird nur gerechnet, solange sich etwas bewegt (siehe [`Body::settled`]): Ein
//! stehender Bot kostet keine Rechenzeit.

pub mod blocks;
pub mod driver;

use blocks::{flag, Class};

/// Achsenparallele Box in Weltkoordinaten.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Aabb {
    pub min: [f64; 3],
    pub max: [f64; 3],
}

impl Aabb {
    fn offset(self, d: [f64; 3]) -> Aabb {
        Aabb {
            min: [self.min[0] + d[0], self.min[1] + d[1], self.min[2] + d[2]],
            max: [self.max[0] + d[0], self.max[1] + d[1], self.max[2] + d[2]],
        }
    }

    /// `AABB.expandTowards`: in Bewegungsrichtung verlängern.
    fn expand_towards(self, d: [f64; 3]) -> Aabb {
        let mut out = self;
        for (axis, delta) in d.iter().enumerate() {
            if *delta < 0.0 {
                out.min[axis] += delta;
            } else {
                out.max[axis] += delta;
            }
        }
        out
    }

    fn deflate(self, by: f64) -> Aabb {
        Aabb {
            min: [self.min[0] + by, self.min[1] + by, self.min[2] + by],
            max: [self.max[0] - by, self.max[1] - by, self.max[2] - by],
        }
    }

    fn intersects(&self, other: &Aabb) -> bool {
        (0..3).all(|a| self.min[a] < other.max[a] && self.max[a] > other.min[a])
    }
}

/// Woher die Physik ihre Blöcke bekommt. `None` = Chunk nicht geladen.
pub trait Blocks {
    fn class_at(&self, x: i32, y: i32, z: i32) -> Option<u16>;
}

/// Gedrückte Tasten – wie `Input.keyPresses` des Vanilla-Clients.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Keys {
    pub forward: bool,
    pub backward: bool,
    pub left: bool,
    pub right: bool,
    pub jump: bool,
    pub shift: bool,
    pub sprint: bool,
}

impl Keys {
    pub fn any_movement(&self) -> bool {
        self.forward || self.backward || self.left || self.right || self.jump
    }

    /// Bitfeld von `ServerboundPlayerInputPacket` (ab 1.21.2).
    pub fn bits(&self) -> u8 {
        (self.forward as u8)
            | (self.backward as u8) << 1
            | (self.left as u8) << 2
            | (self.right as u8) << 3
            | (self.jump as u8) << 4
            | (self.shift as u8) << 5
            | (self.sprint as u8) << 6
    }
}

/// Ergebnis eines Ticks: was an den Server muss.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Report {
    pub moved: bool,
    pub rotated: bool,
    /// Boden- oder Wandkontakt hat sich geändert (`StatusOnly`).
    pub status_changed: bool,
    pub sprint_changed: bool,
}

const GRAVITY: f64 = 0.08;
const STEP_HEIGHT: f64 = 0.6;
const WIDTH: f64 = 0.6;
const HEIGHT_STANDING: f64 = 1.8;
const HEIGHT_CROUCHING: f64 = 1.5;
const WALK_SPEED: f32 = 0.1;
const SNEAK_FACTOR: f32 = 0.3;
const JUMP_POWER: f32 = 0.42;
const COLLISION_EPSILON: f64 = 1.0e-7;

#[derive(Clone, Debug)]
pub struct Body {
    pub pos: [f64; 3],
    pub vel: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub on_ground: bool,
    pub horizontal_collision: bool,
    pub sprinting: bool,
    pub flying: bool,
    pub may_fly: bool,
    pub fly_speed: f32,
    /// Bewegungsgeschwindigkeit als Attribut (Standard 0,1; Sprint ×1,3 kommt dazu).
    pub walk_speed: f32,
    /// Zuschauer: keine Kollision.
    pub no_physics: bool,
    /// 1.8.9: keine Hockhaltung mit kleinerer Box, alte Stufenlogik.
    pub legacy: bool,
    crouching: bool,
    no_jump_delay: u8,
    fall_distance: f64,
    stuck: Option<[f64; 3]>,
    in_water: bool,
    in_lava: bool,
    // zuletzt gesendet
    last_pos: [f64; 3],
    last_rot: (f32, f32),
    last_on_ground: bool,
    last_horizontal_collision: bool,
    last_sprinting: bool,
    reminder: u32,
    /// Ticks in Folge ohne jede Bewegung.
    quiet: u32,
}

impl Body {
    pub fn new(legacy: bool) -> Body {
        Body {
            pos: [0.0; 3],
            vel: [0.0; 3],
            yaw: 0.0,
            pitch: 0.0,
            on_ground: false,
            horizontal_collision: false,
            sprinting: false,
            flying: false,
            may_fly: false,
            fly_speed: 0.05,
            walk_speed: WALK_SPEED,
            no_physics: false,
            legacy,
            crouching: false,
            no_jump_delay: 0,
            fall_distance: 0.0,
            stuck: None,
            in_water: false,
            in_lava: false,
            last_pos: [0.0; 3],
            last_rot: (0.0, 0.0),
            last_on_ground: false,
            last_horizontal_collision: false,
            last_sprinting: false,
            reminder: 0,
            quiet: 0,
        }
    }

    /// Server-Teleport übernehmen (`setValuesFromPositionPacket`). Danach gilt die Position als
    /// gesendet – das Echo bzw. die Bestätigung schickt der Aufrufer.
    pub fn teleport(&mut self, pos: [f64; 3], vel: [f64; 3], yaw: f32, pitch: f32) {
        self.pos = pos;
        self.vel = vel;
        self.yaw = yaw;
        self.pitch = pitch;
        self.last_pos = pos;
        self.last_rot = (yaw, pitch);
        self.last_on_ground = false;
        self.on_ground = false;
        self.reminder = 0;
        self.quiet = 0;
        self.fall_distance = 0.0;
    }

    fn height(&self) -> f64 {
        if self.crouching && !self.legacy {
            HEIGHT_CROUCHING
        } else {
            HEIGHT_STANDING
        }
    }

    pub fn bounding_box(&self) -> Aabb {
        let half = WIDTH / 2.0;
        Aabb {
            min: [self.pos[0] - half, self.pos[1], self.pos[2] - half],
            max: [
                self.pos[0] + half,
                self.pos[1] + self.height(),
                self.pos[2] + half,
            ],
        }
    }

    /// Nach einer Änderung von außen (Rückstoß, Blockänderung, Fähigkeiten) wieder rechnen.
    pub fn wake(&mut self) {
        self.quiet = 0;
    }

    /// Ein Tick ohne Rechnung: nur die Positionserinnerung.
    pub fn idle(&mut self) -> Report {
        self.report(&Keys::default())
    }

    /// Nichts zu rechnen: am Boden, keine Geschwindigkeit, keine Taste, nichts geändert.
    /// Der Aufrufer meldet dann nur alle 20 Ticks die Position (wie `positionReminder`).
    pub fn settled(&self, keys: &Keys) -> bool {
        self.quiet >= 2 && !keys.any_movement() && self.stuck.is_none()
    }

    /// Ein Client-Tick. Gibt zurück, was gesendet werden muss.
    pub fn tick(&mut self, keys: &Keys, world: &impl Blocks) -> Report {
        let before = (self.pos, self.vel, self.on_ground);
        if !self.chunk_loaded(world) {
            // Vanilla bewegt einen Spieler in einem nicht geladenen Chunk nicht.
            return self.report(keys);
        }
        self.update_crouching(keys, world);
        self.update_fluids(world);
        self.update_sprint(keys);

        // LivingEntity.aiStep: kleine Restgeschwindigkeiten verschwinden.
        if self.vel[0] * self.vel[0] + self.vel[2] * self.vel[2] < 9.0e-6 {
            self.vel[0] = 0.0;
            self.vel[2] = 0.0;
        }
        if self.vel[1].abs() < 0.003 {
            self.vel[1] = 0.0;
        }
        if self.no_jump_delay > 0 {
            self.no_jump_delay -= 1;
        }

        let input = self.modify_input(keys);

        if self.flying {
            let mut lift = 0.0;
            if keys.shift {
                lift -= 1.0;
            }
            if keys.jump {
                lift += 1.0;
            }
            self.vel[1] += lift * self.fly_speed as f64 * 3.0;
        } else if self.in_water && keys.shift {
            self.vel[1] -= 0.04; // goDownInWater
        }

        // Springen
        if keys.jump && !self.flying {
            if self.in_water || self.in_lava {
                self.vel[1] += 0.04; // jumpInLiquid
            } else if self.on_ground && self.no_jump_delay == 0 {
                self.jump_from_ground(world);
                self.no_jump_delay = 10;
            }
        } else {
            self.no_jump_delay = 0;
        }

        self.travel(input, keys, world);
        self.apply_block_effects(world);
        if self.pos != before.0 || self.vel != before.1 || self.on_ground != before.2 {
            self.quiet = 0;
        } else {
            self.quiet = self.quiet.saturating_add(1);
        }
        self.report(keys)
    }

    /// Was `sendPosition` in diesem Tick senden würde. Setzt die „zuletzt gesendet"-Werte.
    fn report(&mut self, _keys: &Keys) -> Report {
        let d = [
            self.pos[0] - self.last_pos[0],
            self.pos[1] - self.last_pos[1],
            self.pos[2] - self.last_pos[2],
        ];
        self.reminder += 1;
        let moved =
            d[0] * d[0] + d[1] * d[1] + d[2] * d[2] > 2.0e-4 * 2.0e-4 || self.reminder >= 20;
        let rotated = self.yaw != self.last_rot.0 || self.pitch != self.last_rot.1;
        let status_changed = self.on_ground != self.last_on_ground
            || self.horizontal_collision != self.last_horizontal_collision;
        let sprint_changed = self.sprinting != self.last_sprinting;
        if moved {
            self.last_pos = self.pos;
            self.reminder = 0;
        }
        if rotated {
            self.last_rot = (self.yaw, self.pitch);
        }
        self.last_on_ground = self.on_ground;
        self.last_horizontal_collision = self.horizontal_collision;
        self.last_sprinting = self.sprinting;
        Report {
            moved,
            rotated,
            status_changed,
            sprint_changed,
        }
    }

    fn chunk_loaded(&self, world: &impl Blocks) -> bool {
        world
            .class_at(
                self.pos[0].floor() as i32,
                self.pos[1].floor() as i32,
                self.pos[2].floor() as i32,
            )
            .is_some()
    }

    fn class_at(&self, world: &impl Blocks, x: i32, y: i32, z: i32) -> Class {
        blocks::class(world.class_at(x, y, z).unwrap_or_else(blocks::air_class))
    }

    fn update_crouching(&mut self, keys: &Keys, world: &impl Blocks) {
        if self.legacy {
            self.crouching = keys.shift && !self.flying;
            return;
        }
        // `LocalPlayer.aiStep`: hocken, wenn Shift gedrückt und die Hockbox passt – oder wenn die
        // Stehbox nicht mehr passt.
        let fits = |height: f64| {
            let half = WIDTH / 2.0;
            let b = Aabb {
                min: [self.pos[0] - half, self.pos[1], self.pos[2] - half],
                max: [self.pos[0] + half, self.pos[1] + height, self.pos[2] + half],
            }
            .deflate(1.0e-7);
            no_collision(world, &b)
        };
        self.crouching =
            !self.flying && fits(HEIGHT_CROUCHING) && (keys.shift || !fits(HEIGHT_STANDING));
    }

    fn update_fluids(&mut self, world: &impl Blocks) {
        let b = self.bounding_box().deflate(0.001);
        let (mut water, mut lava) = (false, false);
        for_blocks(&b, |x, y, z| {
            let class = self.class_at(world, x, y, z);
            if class.fluid_amount == 0 {
                return;
            }
            let above = self.class_at(world, x, y + 1, z);
            let same_above = (class.has(flag::WATER) && above.has(flag::WATER))
                || (class.has(flag::LAVA) && above.has(flag::LAVA));
            let height = if same_above {
                1.0
            } else {
                class.fluid_amount as f64 / 9.0
            };
            if y as f64 + height >= b.min[1] {
                water |= class.has(flag::WATER);
                lava |= class.has(flag::LAVA);
            }
        });
        self.in_water = water;
        self.in_lava = lava && !water;
        if self.in_water {
            self.fall_distance = 0.0;
        }
    }

    fn update_sprint(&mut self, keys: &Keys) {
        // Vereinfachtes `shouldStopRunSprinting`: ohne Vorwärtsimpuls, beim Schleichen oder an
        // einer Wand endet der Sprint.
        // shortcut: Hunger und "minorHorizontalCollision" fehlen; erst nachrüsten, wenn ein
        // Server darüber stolpert.
        let forward = keys.forward && !keys.backward;
        if keys.sprint && forward && !self.crouching {
            self.sprinting = true;
        }
        if self.sprinting && (!forward || self.crouching || self.horizontal_collision) {
            self.sprinting = false;
        }
    }

    /// `LocalPlayer.modifyInput` samt `KeyboardInput`: Richtung normalisieren, ×0,98, beim
    /// Schleichen ×0,3, dann auf das Einheitsquadrat strecken.
    fn modify_input(&self, keys: &Keys) -> [f32; 2] {
        let impulse = |a: bool, b: bool| (a as i32 - b as i32) as f32;
        let mut v = [
            impulse(keys.left, keys.right),
            impulse(keys.forward, keys.backward),
        ];
        let len = (v[0] * v[0] + v[1] * v[1]).sqrt();
        if len == 0.0 {
            return [0.0, 0.0];
        }
        v = [v[0] / len * 0.98, v[1] / len * 0.98];
        if self.crouching {
            v = [v[0] * SNEAK_FACTOR, v[1] * SNEAK_FACTOR];
        }
        let length = (v[0] * v[0] + v[1] * v[1]).sqrt();
        let dir = [v[0] / length, v[1] / length];
        let (ax, ay) = (dir[0].abs(), dir[1].abs());
        let tan = if ay > ax { ax / ay } else { ay / ax };
        let to_square = (1.0 + tan * tan).sqrt();
        let scaled = (length * to_square).min(1.0);
        [dir[0] * scaled, dir[1] * scaled]
    }

    fn jump_from_ground(&mut self, world: &impl Blocks) {
        let power = JUMP_POWER * self.block_jump_factor(world);
        if power <= 1.0e-5 {
            return;
        }
        self.vel[1] = self.vel[1].max(power as f64);
        if self.sprinting {
            let angle = self.yaw * (std::f32::consts::PI / 180.0);
            self.vel[0] += -mth_sin(angle) as f64 * 0.2;
            self.vel[2] += mth_cos(angle) as f64 * 0.2;
        }
    }

    fn feet(&self) -> (i32, i32, i32) {
        (
            self.pos[0].floor() as i32,
            self.pos[1].floor() as i32,
            self.pos[2].floor() as i32,
        )
    }

    /// `getBlockPosBelowThatAffectsMyMovement`: der Block einen halben Block unter den Füßen.
    fn below(&self) -> (i32, i32, i32) {
        (
            self.pos[0].floor() as i32,
            (self.pos[1] - 0.500_001).floor() as i32,
            self.pos[2].floor() as i32,
        )
    }

    fn block_jump_factor(&self, world: &impl Blocks) -> f32 {
        let (x, y, z) = self.feet();
        let at = self.class_at(world, x, y, z).jump_factor;
        if at == 1.0 {
            let (x, y, z) = self.below();
            self.class_at(world, x, y, z).jump_factor
        } else {
            at
        }
    }

    fn block_speed_factor(&self, world: &impl Blocks) -> f64 {
        let (x, y, z) = self.feet();
        let at = self.class_at(world, x, y, z);
        if at.has(flag::WATER) || at.has(flag::BUBBLE_COLUMN) {
            return at.speed_factor as f64;
        }
        if at.speed_factor == 1.0 {
            let (x, y, z) = self.below();
            self.class_at(world, x, y, z).speed_factor as f64
        } else {
            at.speed_factor as f64
        }
    }

    fn on_climbable(&self, world: &impl Blocks) -> bool {
        let (x, y, z) = self.feet();
        !self.flying && self.class_at(world, x, y, z).has(flag::CLIMBABLE)
    }

    fn speed(&self) -> f32 {
        if self.sprinting {
            self.walk_speed * 1.3
        } else {
            self.walk_speed
        }
    }

    fn flying_speed(&self) -> f32 {
        if self.flying {
            if self.sprinting {
                self.fly_speed * 2.0
            } else {
                self.fly_speed
            }
        } else if self.sprinting {
            0.025_999_999
        } else {
            0.02
        }
    }

    /// `Entity.moveRelative`.
    fn move_relative(&mut self, speed: f32, input: [f32; 2]) {
        let (x, z) = (input[0] as f64, input[1] as f64);
        let len_sq = x * x + z * z;
        if len_sq < 1.0e-7 {
            return;
        }
        let (mut x, mut z) = if len_sq > 1.0 {
            let len = len_sq.sqrt();
            (x / len, z / len)
        } else {
            (x, z)
        };
        x *= speed as f64;
        z *= speed as f64;
        let angle = self.yaw * (std::f32::consts::PI / 180.0);
        let (sin, cos) = (mth_sin(angle) as f64, mth_cos(angle) as f64);
        self.vel[0] += x * cos - z * sin;
        self.vel[2] += z * cos + x * sin;
    }

    fn travel(&mut self, input: [f32; 2], keys: &Keys, world: &impl Blocks) {
        let original_vy = self.vel[1];
        if !self.flying && self.in_water {
            self.travel_in_water(input, keys, world);
        } else if !self.flying && self.in_lava {
            self.travel_in_lava(input, keys, world);
        } else {
            self.travel_in_air(input, keys, world);
        }
        if self.flying {
            self.vel[1] = original_vy * 0.6;
        }
    }

    fn travel_in_air(&mut self, input: [f32; 2], keys: &Keys, world: &impl Blocks) {
        let friction = if self.on_ground {
            let (x, y, z) = self.below();
            self.class_at(world, x, y, z).friction
        } else {
            1.0
        };
        let speed = if self.on_ground {
            if friction > 0.6 {
                self.speed() * (0.216_000_02 / (friction * friction * friction))
            } else {
                self.speed()
            }
        } else {
            self.flying_speed()
        };
        self.move_relative(speed, input);
        if self.on_climbable(world) {
            self.fall_distance = 0.0;
            self.vel[0] = self.vel[0].clamp(-0.15, 0.15);
            self.vel[2] = self.vel[2].clamp(-0.15, 0.15);
            self.vel[1] = self.vel[1].max(-0.15);
            if self.vel[1] < 0.0 && keys.shift {
                let (x, y, z) = self.feet();
                if !self.class_at(world, x, y, z).has(flag::SCAFFOLDING) {
                    self.vel[1] = 0.0;
                }
            }
        }
        self.move_by(self.vel, keys, world);
        let mut movement = self.vel;
        if (self.horizontal_collision || keys.jump) && self.on_climbable(world) {
            movement[1] = 0.2;
        }
        let mut vy = movement[1];
        if !self.flying || !self.no_physics {
            vy -= GRAVITY;
        }
        let horizontal = friction as f64 * 0.91f32 as f64;
        self.vel = [
            movement[0] * horizontal,
            vy * 0.98f32 as f64,
            movement[2] * horizontal,
        ];
    }

    fn travel_in_water(&mut self, input: [f32; 2], keys: &Keys, world: &impl Blocks) {
        let falling = self.vel[1] <= 0.0;
        let old_y = self.pos[1];
        let slow_down = if self.sprinting { 0.9 } else { 0.8 };
        self.move_relative(0.02, input);
        self.move_by(self.vel, keys, world);
        let mut movement = self.vel;
        if self.horizontal_collision && self.on_climbable(world) {
            movement[1] = 0.2;
        }
        movement = [
            movement[0] * slow_down,
            movement[1] * 0.8,
            movement[2] * slow_down,
        ];
        self.vel = fluid_falling_adjusted(movement, falling, self.sprinting);
        self.jump_out_of_fluid(keys, old_y, world);
    }

    fn travel_in_lava(&mut self, input: [f32; 2], keys: &Keys, world: &impl Blocks) {
        let falling = self.vel[1] <= 0.0;
        let old_y = self.pos[1];
        self.move_relative(0.02, input);
        self.move_by(self.vel, keys, world);
        // shortcut: "tief genug zum Schwimmen" wird nicht unterschieden – Spieler stehen selten
        // in flacher Lava; dann gilt der flache Fall.
        self.vel = [self.vel[0] * 0.5, self.vel[1] * 0.8, self.vel[2] * 0.5];
        self.vel = fluid_falling_adjusted(self.vel, falling, self.sprinting);
        self.vel[1] -= GRAVITY / 4.0;
        self.jump_out_of_fluid(keys, old_y, world);
    }

    fn jump_out_of_fluid(&mut self, keys: &Keys, old_y: f64, world: &impl Blocks) {
        if keys.jump && self.horizontal_collision {
            let probe = self.bounding_box().offset([
                self.vel[0],
                self.vel[1] + 0.6 - self.pos[1] + old_y,
                self.vel[2],
            ]);
            if no_collision(world, &probe) && !self.has_fluid(world, &probe) {
                self.vel[1] = 0.3;
            }
        }
    }

    fn has_fluid(&self, world: &impl Blocks, b: &Aabb) -> bool {
        let mut found = false;
        for_blocks(b, |x, y, z| {
            found |= self.class_at(world, x, y, z).fluid_amount > 0;
        });
        found
    }

    /// `Entity.move` für eigene Bewegung.
    fn move_by(&mut self, delta: [f64; 3], keys: &Keys, world: &impl Blocks) {
        if self.no_physics {
            self.pos = [
                self.pos[0] + delta[0],
                self.pos[1] + delta[1],
                self.pos[2] + delta[2],
            ];
            self.horizontal_collision = false;
            self.on_ground = false;
            return;
        }
        let mut delta = delta;
        if let Some(stuck) = self.stuck.take() {
            delta = [
                delta[0] * stuck[0],
                delta[1] * stuck[1],
                delta[2] * stuck[2],
            ];
            self.vel = [0.0; 3];
        }
        delta = self.back_off_from_edge(delta, keys, world);
        let movement = self.collide(delta, world);
        let length = movement.iter().map(|v| v * v).sum::<f64>();
        let wanted = delta.iter().map(|v| v * v).sum::<f64>();
        if length > 1.0e-7 || wanted - length < 1.0e-7 {
            self.pos = [
                self.pos[0] + movement[0],
                self.pos[1] + movement[1],
                self.pos[2] + movement[2],
            ];
        }
        let x_collision = !mth_equal(delta[0], movement[0]);
        let z_collision = !mth_equal(delta[2], movement[2]);
        self.horizontal_collision = x_collision || z_collision;
        let vertical = delta[1] != movement[1];
        if delta[1].abs() > 0.0 || true {
            self.on_ground = vertical && delta[1] < 0.0;
        }
        if self.on_ground {
            self.fall_distance = 0.0;
        } else if movement[1] < 0.0 {
            self.fall_distance -= movement[1];
        }

        if x_collision {
            self.vel[0] = 0.0;
        }
        if z_collision {
            self.vel[2] = 0.0;
        }
        if vertical {
            let (x, y, z) = self.below_on();
            let landed_on = self.class_at(world, x, y, z);
            self.vel[1] = if delta[1] < 0.0 && !keys.shift && landed_on.has(flag::SLIME) {
                -self.vel[1]
            } else if delta[1] < 0.0 && !keys.shift && landed_on.has(flag::BED) {
                -self.vel[1] * 0.66
            } else {
                0.0
            };
        }
        if self.on_ground && !keys.shift {
            // `SlimeBlock.stepOn`
            let (x, y, z) = self.below_on();
            if self.class_at(world, x, y, z).has(flag::SLIME) && self.vel[1].abs() < 0.1 {
                let factor = 0.4 + self.vel[1].abs() * 0.2;
                self.vel[0] *= factor;
                self.vel[2] *= factor;
            }
        }
        let factor = self.block_speed_factor(world);
        self.vel[0] *= factor;
        self.vel[2] *= factor;
    }

    /// `getOnPosLegacy`: der Block direkt unter den Füßen (0,2 tief).
    fn below_on(&self) -> (i32, i32, i32) {
        (
            self.pos[0].floor() as i32,
            (self.pos[1] - 0.2).floor() as i32,
            self.pos[2].floor() as i32,
        )
    }

    /// `Player.maybeBackOffFromEdge`: Beim Schleichen nicht über eine Kante laufen.
    fn back_off_from_edge(&self, delta: [f64; 3], keys: &Keys, world: &impl Blocks) -> [f64; 3] {
        let max_down = STEP_HEIGHT;
        if self.flying || delta[1] > 0.0 || !keys.shift || !self.is_above_ground(world, max_down) {
            return delta;
        }
        let (mut dx, mut dz) = (delta[0], delta[2]);
        let step_x = dx.signum() * 0.05;
        let step_z = dz.signum() * 0.05;
        while dx != 0.0 && self.can_fall_at_least(world, dx, 0.0, max_down) {
            if dx.abs() <= 0.05 {
                dx = 0.0;
                break;
            }
            dx -= step_x;
        }
        while dz != 0.0 && self.can_fall_at_least(world, 0.0, dz, max_down) {
            if dz.abs() <= 0.05 {
                dz = 0.0;
                break;
            }
            dz -= step_z;
        }
        while dx != 0.0 && dz != 0.0 && self.can_fall_at_least(world, dx, dz, max_down) {
            if dx.abs() <= 0.05 {
                dx = 0.0;
            } else {
                dx -= step_x;
            }
            if dz.abs() <= 0.05 {
                dz = 0.0;
            } else {
                dz -= step_z;
            }
        }
        [dx, delta[1], dz]
    }

    fn is_above_ground(&self, world: &impl Blocks, max_down: f64) -> bool {
        self.on_ground
            || self.fall_distance < max_down
                && !self.can_fall_at_least(world, 0.0, 0.0, max_down - self.fall_distance)
    }

    fn can_fall_at_least(&self, world: &impl Blocks, dx: f64, dz: f64, min_height: f64) -> bool {
        let b = self.bounding_box();
        let probe = Aabb {
            min: [
                b.min[0] + 1.0e-7 + dx,
                b.min[1] - min_height - 1.0e-7,
                b.min[2] + 1.0e-7 + dz,
            ],
            max: [b.max[0] - 1.0e-7 + dx, b.min[1], b.max[2] - 1.0e-7 + dz],
        };
        no_collision(world, &probe)
    }

    /// `Entity.collide`: erst normal, dann – am Boden und mit Wandkontakt – über die
    /// Stufenkandidaten bis 0,6 Blöcke.
    fn collide(&self, movement: [f64; 3], world: &impl Blocks) -> [f64; 3] {
        let aabb = self.bounding_box();
        let step = if movement.iter().all(|v| *v == 0.0) {
            movement
        } else {
            collide_box(world, movement, &aabb)
        };
        let x_c = movement[0] != step[0];
        let y_c = movement[1] != step[1];
        let z_c = movement[2] != step[2];
        let grounded_after = y_c && movement[1] < 0.0;
        if !(grounded_after || self.on_ground) || !(x_c || z_c) {
            return step;
        }
        let grounded = if grounded_after {
            aabb.offset([0.0, step[1], 0.0])
        } else {
            aabb
        };
        let mut up = grounded.expand_towards([movement[0], STEP_HEIGHT, movement[2]]);
        if !grounded_after {
            up = up.expand_towards([0.0, -1.0e-5, 0.0]);
        }
        let colliders = collect_boxes(world, &up);
        let skip = step[1] as f32;
        for candidate in step_candidates(&grounded, &colliders, STEP_HEIGHT as f32, skip) {
            let from_ground = collide_with(
                [movement[0], candidate as f64, movement[2]],
                &grounded,
                &colliders,
            );
            if horizontal_sq(&from_ground) > horizontal_sq(&step) {
                let to_ground = aabb.min[1] - grounded.min[1];
                return [from_ground[0], from_ground[1] - to_ground, from_ground[2]];
            }
        }
        step
    }

    /// `applyEffectsFromBlocks`: Spinnennetz & Co. bremsen die nächste Bewegung.
    fn apply_block_effects(&mut self, world: &impl Blocks) {
        let b = self.bounding_box().deflate(1.0e-5);
        let mut stuck = None;
        for_blocks(&b, |x, y, z| {
            let class = self.class_at(world, x, y, z);
            if class.has(flag::COBWEB) {
                stuck = Some([0.25, 0.05, 0.25]);
            } else if class.has(flag::BERRY_BUSH) && stuck.is_none() {
                stuck = Some([0.8, 0.75, 0.8]);
            } else if class.has(flag::POWDER_SNOW) && stuck.is_none() {
                stuck = Some([0.9, 1.5, 0.9]);
            }
        });
        self.stuck = stuck;
    }
}

fn horizontal_sq(v: &[f64; 3]) -> f64 {
    v[0] * v[0] + v[2] * v[2]
}

/// `Mth.equal`.
fn mth_equal(a: f64, b: f64) -> bool {
    (b - a).abs() < 1.0e-5
}

/// `Mth.sin`/`Mth.cos`: Tabelle mit 65 536 Einträgen – hier ohne Tabelle, aber mit demselben
/// Index und damit bitgleich.
fn mth_sin(value: f32) -> f32 {
    let index = ((value * 10_430.378) as i64 as i32 & 65_535) as f64;
    (index * std::f64::consts::PI * 2.0 / 65_536.0).sin() as f32
}

fn mth_cos(value: f32) -> f32 {
    let index = ((value * 10_430.378 + 16_384.0) as i64 as i32 & 65_535) as f64;
    (index * std::f64::consts::PI * 2.0 / 65_536.0).sin() as f32
}

/// `LivingEntity.getFluidFallingAdjustedMovement` mit Schwerkraft 0,08.
fn fluid_falling_adjusted(movement: [f64; 3], falling: bool, sprinting: bool) -> [f64; 3] {
    if sprinting {
        return movement;
    }
    let y = if falling
        && (movement[1] - 0.005).abs() >= 0.003
        && (movement[1] - GRAVITY / 16.0).abs() < 0.003
    {
        -0.003
    } else {
        movement[1] - GRAVITY / 16.0
    };
    [movement[0], y, movement[2]]
}

/// Alle Blockpositionen, die eine Box berührt.
fn for_blocks(b: &Aabb, mut f: impl FnMut(i32, i32, i32)) {
    let (x0, x1) = (b.min[0].floor() as i32, b.max[0].floor() as i32);
    let (y0, y1) = (b.min[1].floor() as i32, b.max[1].floor() as i32);
    let (z0, z1) = (b.min[2].floor() as i32, b.max[2].floor() as i32);
    for x in x0..=x1 {
        for y in y0..=y1 {
            for z in z0..=z1 {
                f(x, y, z);
            }
        }
    }
}

/// Kollisionsboxen aller Blöcke im Bereich (in Weltkoordinaten). Wie `getBlockCollisions`
/// nimmt das auch Blöcke unmittelbar am Rand mit (Zäune ragen 0,5 nach oben hinaus).
fn collect_boxes(world: &impl Blocks, area: &Aabb) -> Vec<Aabb> {
    let mut out = Vec::new();
    let grown = Aabb {
        min: [area.min[0] - 1.0, area.min[1] - 1.0, area.min[2] - 1.0],
        max: [area.max[0] + 1.0, area.max[1] + 1.0, area.max[2] + 1.0],
    };
    for_blocks(&grown, |x, y, z| {
        let class = match world.class_at(x, y, z) {
            Some(class) => blocks::class(class),
            // Nicht geladener Chunk: wie ein voller Block (der Server hätte ihn uns sonst geschickt).
            None => blocks::class(blocks::full_cube_class()),
        };
        for b in class.boxes() {
            let placed = b.offset([x as f64, y as f64, z as f64]);
            if placed.intersects(area) {
                out.push(placed);
            }
        }
    });
    out
}

fn no_collision(world: &impl Blocks, area: &Aabb) -> bool {
    collect_boxes(world, area).is_empty()
}

fn collide_box(world: &impl Blocks, movement: [f64; 3], b: &Aabb) -> [f64; 3] {
    let colliders = collect_boxes(world, &b.expand_towards(movement));
    collide_with(movement, b, &colliders)
}

/// `collideWithShapes`: Y zuerst, dann die Achse mit der größeren Bewegung zuletzt.
fn collide_with(movement: [f64; 3], b: &Aabb, shapes: &[Aabb]) -> [f64; 3] {
    if shapes.is_empty() {
        return movement;
    }
    let order: [usize; 3] = if movement[0].abs() < movement[2].abs() {
        [1, 2, 0]
    } else {
        [1, 0, 2]
    };
    let mut resolved = [0.0; 3];
    for axis in order {
        let wanted = movement[axis];
        if wanted != 0.0 {
            resolved[axis] = collide_axis(axis, &b.offset(resolved), shapes, wanted);
        }
    }
    resolved
}

/// `Shapes.collide` für Boxen: so weit, bis die erste Box im Weg ist.
fn collide_axis(axis: usize, b: &Aabb, shapes: &[Aabb], mut distance: f64) -> f64 {
    let (o1, o2) = match axis {
        0 => (1, 2),
        1 => (0, 2),
        _ => (0, 1),
    };
    for s in shapes {
        if distance.abs() < COLLISION_EPSILON {
            return 0.0;
        }
        let overlaps = b.max[o1] - COLLISION_EPSILON > s.min[o1]
            && b.min[o1] + COLLISION_EPSILON < s.max[o1]
            && b.max[o2] - COLLISION_EPSILON > s.min[o2]
            && b.min[o2] + COLLISION_EPSILON < s.max[o2];
        if !overlaps {
            continue;
        }
        if distance > 0.0 && b.max[axis] - COLLISION_EPSILON <= s.min[axis] {
            distance = distance.min(s.min[axis] - b.max[axis]);
        } else if distance < 0.0 && b.min[axis] + COLLISION_EPSILON >= s.max[axis] {
            distance = distance.max(s.max[axis] - b.min[axis]);
        }
    }
    distance
}

/// `collectCandidateStepUpHeights`: jede Oberkante/Unterkante in Reichweite, aufsteigend.
fn step_candidates(b: &Aabb, shapes: &[Aabb], max: f32, skip: f32) -> Vec<f32> {
    let mut out: Vec<f32> = Vec::with_capacity(4);
    for s in shapes {
        for coord in [s.min[1], s.max[1]] {
            let relative = (coord - b.min[1]) as f32;
            if relative < 0.0 || relative == skip || relative > max {
                continue;
            }
            if !out.contains(&relative) {
                out.push(relative);
            }
        }
    }
    out.sort_by(|a, b| a.total_cmp(b));
    out
}

#[cfg(test)]
mod tests;
