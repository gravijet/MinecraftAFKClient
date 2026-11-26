//! Physik im laufenden Client: Tick, Tasten, Teleports und die Pakete an den Server.
//!
//! Gerechnet wird im 20-Hz-Client-Tick (siehe `client::client_tick`). Ein Bot, der still steht,
//! kostet dabei nichts: [`Body::settled`] lässt die Rechnung aus, solange weder eine Taste
//! gedrückt ist noch sich die Welt geändert hat; dann geht nur die Positionserinnerung alle
//! 20 Ticks hinaus – wie beim Vanilla-Client.

use super::{Body, Keys};
use crate::client::Shared;
use crate::protocol::buf::Writer;
use crate::protocol::values;
use std::sync::atomic::Ordering;
use std::sync::Mutex;

pub struct Driver {
    state: Mutex<State>,
}

struct State {
    body: Body,
    keys: Keys,
    /// Erst nach dem ersten Teleport des Servers gibt es eine Position.
    placed: bool,
    last_input: Option<Keys>,
    last_world_revision: u64,
}

impl Driver {
    pub fn new(legacy: bool) -> Driver {
        Driver {
            state: Mutex::new(State {
                body: Body::new(legacy),
                keys: Keys::default(),
                placed: false,
                last_input: None,
                last_world_revision: 0,
            }),
        }
    }

    /// Verbindung oder Welt weg: alles vergessen, Tasten loslassen.
    pub fn reset(&self) {
        let mut state = self.state.lock().unwrap();
        let legacy = state.body.legacy;
        state.body = Body::new(legacy);
        state.keys = Keys::default();
        state.placed = false;
        state.last_input = None;
    }

    pub fn set_keys(&self, change: impl FnOnce(&mut Keys)) {
        change(&mut self.state.lock().unwrap().keys);
    }

    /// Blickrichtung setzen; geht mit dem nächsten Tick hinaus.
    pub fn look(&self, yaw: f32, pitch: f32) {
        let mut state = self.state.lock().unwrap();
        state.body.yaw = yaw;
        state.body.pitch = pitch.clamp(-90.0, 90.0);
    }

    pub fn velocity(&self) -> [f64; 3] {
        self.state.lock().unwrap().body.vel
    }

    pub fn on_ground(&self) -> bool {
        self.state.lock().unwrap().body.on_ground
    }

    pub fn horizontal_collision(&self) -> bool {
        self.state.lock().unwrap().body.horizontal_collision
    }

    /// Server-Teleport (`setValuesFromPositionPacket`).
    pub fn teleport(&self, pos: [f64; 3], vel: Option<[f64; 3]>, yaw: f32, pitch: f32) {
        let mut state = self.state.lock().unwrap();
        let vel = vel.unwrap_or(state.body.vel);
        state.body.teleport(pos, vel, yaw, pitch);
        state.placed = true;
    }

    /// Bewegung von außen (Rückstoß: `SetEntityMotion` für uns selbst, Explosionen).
    pub fn push(&self, velocity: [f64; 3], add: bool) {
        let mut state = self.state.lock().unwrap();
        let body = &mut state.body;
        if add {
            for (axis, delta) in velocity.iter().enumerate() {
                body.vel[axis] += delta;
            }
        } else {
            body.vel = velocity;
        }
        body.wake();
    }

    /// `ClientboundPlayerAbilities`: fliegen, darf fliegen, Fluggeschwindigkeit, Gehtempo.
    pub fn abilities(&self, flying: bool, may_fly: bool, fly_speed: f32, walk_speed: f32) {
        let mut state = self.state.lock().unwrap();
        let body = &mut state.body;
        body.flying = flying;
        body.may_fly = may_fly;
        if fly_speed.is_finite() && fly_speed > 0.0 {
            body.fly_speed = fly_speed;
        }
        if walk_speed.is_finite() && walk_speed > 0.0 {
            body.walk_speed = walk_speed;
        }
        body.wake();
    }

    /// Zuschauermodus: keine Kollision, immer fliegend.
    pub fn spectator(&self, on: bool) {
        let mut state = self.state.lock().unwrap();
        state.body.no_physics = on;
        if on {
            state.body.flying = true;
        }
        state.body.wake();
    }
}

/// Ein Client-Tick. Rechnet, schreibt die Position zurück und schickt die fälligen Pakete.
pub fn tick(shared: &Shared) {
    let driver = &shared.driver;
    let mut state = driver.state.lock().unwrap();
    if !state.placed {
        return;
    }
    let keys = state.keys;
    let report = {
        let world = shared.world.lock().unwrap();
        let world_changed = world.revision != state.last_world_revision;
        state.last_world_revision = world.revision;
        if world_changed {
            state.body.wake();
        }
        if state.body.settled(&keys) {
            state.body.idle()
        } else {
            state.body.tick(&keys, &*world)
        }
    };
    let body = state.body.clone();
    let input_changed = state.last_input != Some(keys);
    let previous_shift = state.last_input.map(|keys| keys.shift);
    state.last_input = Some(keys);
    drop(state);

    shared.set_position((body.pos[0], body.pos[1], body.pos[2], body.yaw, body.pitch));

    let proto = shared.proto;
    let ids = &proto.ids;
    // Reihenfolge wie `LocalPlayer.sendChanges`: Eingabe, Sprint, Bewegung.
    if input_changed && proto.modern && ids.sb_player_input >= 0 {
        let mut w = Writer::packet(ids.sb_player_input);
        w.u8(keys.bits());
        shared.send(w);
    }
    if input_changed && !proto.modern && previous_shift != Some(keys.shift) {
        // Bis 1.21.1 meldet ein eigener Spielerbefehl das Schleichen.
        send_command(
            shared,
            if keys.shift {
                values::player_state::START_SNEAKING
            } else {
                values::player_state::STOP_SNEAKING
            },
        );
    }
    if report.sprint_changed {
        send_sprint(shared, body.sprinting);
    }
    send_movement(shared, &body, report);
}

/// Sprint an/aus. Die Aktionsnummern haben sich mit dem Wegfall der Schleich-Aktionen
/// verschoben: bis 1.21.1 3/4, danach 1/2 (`ServerboundPlayerCommandPacket.Action`).
fn send_sprint(shared: &Shared, on: bool) {
    let action = match (shared.proto.modern, on) {
        (true, true) => 1,
        (true, false) => 2,
        (false, true) => values::player_state::START_SPRINTING,
        (false, false) => values::player_state::STOP_SPRINTING,
    };
    send_command(shared, action);
}

/// `ServerboundPlayerCommand`: Entität, Aktion, Zusatzwert.
fn send_command(shared: &Shared, action: i32) {
    let ids = &shared.proto.ids;
    if ids.sb_player_command < 0 {
        return;
    }
    let mut w = Writer::packet(ids.sb_player_command);
    w.var_int(shared.entity_id.load(Ordering::Relaxed));
    w.var_int(action);
    w.var_int(0);
    shared.send(w);
}

fn send_movement(shared: &Shared, body: &Body, report: super::Report) {
    let ids = &shared.proto.ids;
    let flags_byte = |w: &mut Writer| {
        if shared.proto.modern {
            w.u8(body.on_ground as u8 | (body.horizontal_collision as u8) << 1);
        } else {
            w.bool(body.on_ground);
        }
    };
    let (id, pos, rot) = match (report.moved, report.rotated) {
        (true, true) => (ids.sb_move_player_pos_rot, true, true),
        (true, false) => (ids.sb_move_player_pos, true, false),
        (false, true) => (ids.sb_move_player_rot, false, true),
        (false, false) => {
            // 1.8.9 schickt jeden Tick mindestens den Bodenzustand; moderne Clients nur, wenn
            // er sich ändert.
            if report.status_changed || shared.proto.legacy {
                (ids.sb_move_player_status_only, false, false)
            } else {
                return;
            }
        }
    };
    if id < 0 {
        return;
    }
    let mut w = Writer::packet(id);
    if pos {
        w.f64(body.pos[0]);
        w.f64(body.pos[1]);
        w.f64(body.pos[2]);
    }
    if rot {
        w.f32(body.yaw);
        w.f32(body.pitch);
    }
    flags_byte(&mut w);
    shared.send(w);
}
