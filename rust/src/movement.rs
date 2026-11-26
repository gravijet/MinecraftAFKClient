//! Bewegung auf Zuruf: laufen, springen, Kopf drehen, Heimatposition, Routen.
//!
//! Die Befehle drücken nur Tasten (`:go vor 3` hält „vorwärts“, bis drei Blöcke geschafft sind)
//! und drehen den Kopf. Gerechnet wird in der echten Spielerphysik ([`crate::physics`]): Der Bot
//! fällt, steigt Stufen hinauf, schwimmt und bleibt an Wänden hängen wie ein Spieler. Steht etwas
//! im Weg, springt er; kommt er trotzdem nicht weiter, bricht der Lauf mit einer Meldung ab.
//!
//! Kosten im Leerlauf: null. Ein Befehl startet einen kurzlebigen Thread, der die Tasten hält,
//! bis das Ziel erreicht ist.

use crate::client::{Shared, WORLD_LIMIT};
use crate::console::{Console, BOLD, CYAN, GRAY};
use crate::options;

use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::{Duration, Instant};

/// Ein Server-Tick. Genau so oft schickt auch ein echter Client seine Position.
const TICK: Duration = Duration::from_millis(50);
/// Als „angekommen" gilt, wer dem Ziel so nah ist (Blöcke). Laufen hat Schwung: Wer erst
/// loslässt, wenn er genau am Ziel steht, rutscht ein Stück darüber hinaus.
const ARRIVED: f64 = 0.3;
/// So lange ohne Fortschritt (trotz Springen) = etwas steht im Weg.
const STUCK_AFTER: Duration = Duration::from_secs(3);
/// Obergrenze für eine einzelne Laufanweisung (Blöcke) – gegen Tippfehler wie `:go vor 10000`.
const MAX_BLOCKS: f64 = 512.0;
/// Obergrenze für die Wegpunkte einer Route – hält Datei und Speicher winzig.
const MAX_ROUTE: usize = 64;
/// Nach dem Beitritt kommt die Position erst mit dem ersten Teleport; so lange warten wir darauf.
const POSITION_TIMEOUT: Duration = Duration::from_secs(20);

// ===================== Einstellungen (movement.json) =====================

/// Ein gemerkter Punkt inklusive Blickrichtung.
#[derive(Serialize, Deserialize, Clone, Copy, Default, PartialEq)]
#[serde(rename_all = "camelCase", default)]
pub struct Spot {
    pub x: f64,
    pub y: f64,
    pub z: f64,
    pub yaw: f32,
    pub pitch: f32,
}

impl Spot {
    /// Einen aus der Datei gelesenen Punkt brauchbar machen.
    ///
    /// `movement.json` ist eine gewöhnliche Textdatei; darin kann alles stehen – auch `1e400`,
    /// woraus JSON eine Unendlichkeit macht. Ein solcher Wert wanderte ungeprüft in jede
    /// Rechnung: Der Abstand zum Ziel war dann unendlich, die Schrittweite null, und `:home go`
    /// lief bis ins Zeitlimit, ohne sich einen Block zu bewegen.
    fn normalize(&mut self) {
        let coordinate = |value: f64| {
            if value.is_finite() {
                value.clamp(-WORLD_LIMIT, WORLD_LIMIT)
            } else {
                0.0
            }
        };
        self.x = coordinate(self.x);
        self.y = coordinate(self.y);
        self.z = coordinate(self.z);
        self.yaw = if self.yaw.is_finite() {
            wrap_degrees(self.yaw)
        } else {
            0.0
        };
        self.pitch = if self.pitch.is_finite() {
            self.pitch.clamp(-90.0, 90.0)
        } else {
            0.0
        };
    }

    fn describe(&self) -> String {
        format!(
            "x={:.1}  y={:.1}  z={:.1}  ·  Blick {:.0}° ({}) / {:.0}°",
            self.x,
            self.y,
            self.z,
            self.yaw,
            compass(self.yaw),
            self.pitch
        )
    }

    /// Abstand in der Ebene – die Höhe interessiert beim Laufen nicht.
    fn flat_distance(&self, other: &Spot) -> f64 {
        let (dx, dz) = (other.x - self.x, other.z - self.z);
        (dx * dx + dz * dz).sqrt()
    }
}

/// `~/.config/afksystems/movement.json` – eigene Datei, damit weder der schlanke Client noch der
/// Java-Client (der die `config.json` beim Speichern auf seine eigenen Felder reduziert) diese
/// Einstellungen je überschreiben kann. Der Java-Client mit Bewegung liest dieselbe Datei.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
struct Settings {
    /// Beim Beitritt automatisch zur Heimatposition laufen.
    home_enabled: bool,
    /// Sekunden nach dem Beitritt, bevor der Heimlauf beginnt (der Server soll erst „ankommen").
    home_delay_seconds: u64,
    /// Die Heimatposition; `null`, solange keine gesetzt ist.
    home: Option<Spot>,
    /// Wegpunkte auf dem Weg dorthin, in Reihenfolge. Leer = geradeaus laufen.
    route: Vec<Spot>,
    /// Blöcke pro Sekunde als Wahl der Gangart: unter 2 schleichen, über 4,4 sprinten, sonst
    /// gehen. Das Tempo selbst ergibt sich aus der Physik.
    walk_speed: f64,
    /// Grad je Tick beim Drehen. Ein Ruck um 180° in einem Tick fällt jedem Anticheat auf.
    turn_speed: f64,
    /// Notbremse: so lange darf ein einzelner Lauf höchstens dauern.
    max_walk_seconds: u64,
}

impl Default for Settings {
    fn default() -> Self {
        Settings {
            home_enabled: false,
            home_delay_seconds: 5,
            home: None,
            route: Vec::new(),
            walk_speed: 4.317,
            turn_speed: 25.0,
            max_walk_seconds: 60,
        }
    }
}

impl Settings {
    fn normalize(&mut self) {
        self.walk_speed = self.walk_speed.clamp(0.5, 5.612);
        self.turn_speed = self.turn_speed.clamp(5.0, 90.0);
        self.max_walk_seconds = self.max_walk_seconds.clamp(5, 600);
        self.home_delay_seconds = self.home_delay_seconds.min(3600);
        self.route.truncate(MAX_ROUTE);
        for point in &mut self.route {
            point.normalize();
        }
        if let Some(home) = &mut self.home {
            home.normalize();
        }
    }
}

// ===================== Mover =====================

pub struct Mover {
    settings: Mutex<Settings>,
    file: PathBuf,
    /// Laufende Aufgabe. Jede neue Aufgabe zählt hoch und beendet damit stillschweigend die vorige –
    /// es bewegt sich immer höchstens ein Thread.
    job: AtomicU32,
    /// Nummer der tatsächlich laufenden Aufgabe, `0` = keine. Anders als `job` kann damit ein
    /// Server-Teleport nur dann eine Warnung ausgeben, wenn er wirklich Bewegung abbricht.
    active: AtomicU32,
    /// Zwischen `:route rec` und `:route stop`: die bisher erreichten Punkte. Bewusst **nicht** in
    /// den Einstellungen – eine halbe Aufzeichnung soll keinen Neustart überleben.
    recording: Mutex<Option<Vec<Spot>>>,
}

impl Mover {
    pub fn new() -> Mover {
        let file = options::dir().join("movement.json");
        let mut settings: Settings = std::fs::read_to_string(&file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        settings.normalize();
        Mover {
            settings: Mutex::new(settings),
            file,
            job: AtomicU32::new(0),
            active: AtomicU32::new(0),
            recording: Mutex::new(None),
        }
    }

    /// Laufende Bewegung abbrechen (Trennung, `:stop`, neuer Befehl).
    pub fn stop(&self) -> bool {
        self.job.fetch_add(1, Ordering::SeqCst);
        self.active.swap(0, Ordering::SeqCst) != 0
    }

    pub fn is_active(&self) -> bool {
        self.active.load(Ordering::SeqCst) != 0
    }

    fn claim(&self) -> u32 {
        let id = self.job.fetch_add(1, Ordering::SeqCst) + 1;
        self.active.store(id, Ordering::SeqCst);
        id
    }

    fn finish(&self, id: u32) {
        let _ = self
            .active
            .compare_exchange(id, 0, Ordering::SeqCst, Ordering::SeqCst);
    }

    fn get(&self) -> Settings {
        self.settings.lock().unwrap().clone()
    }

    /// Einstellungen ändern und sofort speichern (unteilbar – siehe
    /// [`crate::options::write_atomic`]).
    fn edit(&self, change: impl FnOnce(&mut Settings)) {
        let text = {
            let mut settings = self.settings.lock().unwrap();
            change(&mut settings);
            settings.normalize();
            serde_json::to_string_pretty(&*settings).ok()
        };
        // Erst die Sperre loslassen, dann auf die Platte schreiben.
        //
        // An dieser Sperre hängt beim Beitritt auch der Netz-Thread (er liest die
        // Heimatposition), und ein Schreibvorgang kann dauern – auf einem ausgelasteten oder
        // netzgebundenen Dateisystem lange genug, um ein KeepAlive zu verpassen.
        if let Some(text) = text {
            options::write_atomic(&self.file, &text);
        }
    }
}

/// Ausweis eines Bewegungs-Threads: gilt nur für „seine" Aufgabe auf „seiner" Verbindung.
#[derive(Clone, Copy)]
struct Job {
    id: u32,
    generation: u32,
}

impl Job {
    fn connection_alive(&self, shared: &Shared) -> bool {
        shared.running.load(Ordering::Relaxed)
            && shared.in_game.load(Ordering::Relaxed)
            && shared.generation.load(Ordering::SeqCst) == self.generation
            && shared.mover.job.load(Ordering::SeqCst) == self.id
    }

    fn alive(&self, shared: &Shared) -> bool {
        self.connection_alive(shared) && shared.ready_for_gameplay()
    }
}

enum Task {
    /// Zum Punkt (x|z) laufen. `record` = das Ziel als Wegpunkt merken, falls gerade aufgezeichnet
    /// wird (so entsteht eine Route: einmal mit `:go` vorlaufen, der Client merkt sich die Ecken).
    WalkTo {
        x: f64,
        z: f64,
        record: bool,
        label: String,
    },
    /// Nur den Kopf drehen.
    Turn { yaw: f32, pitch: f32 },
    /// Springen – auf der Stelle oder mit Drift in eine Richtung.
    Jump { dx: f64, dz: f64 },
    /// Heimatposition: auf die Position warten, Verzögerung abwarten, die Route abgehen,
    /// zum Ziel laufen, dort in die gemerkte Richtung schauen.
    Home {
        waypoints: Vec<Spot>,
        target: Spot,
        delay: Duration,
    },
}

enum Outcome {
    Arrived,
    /// Kein Fortschritt mehr – vermutlich eine Wand.
    Stuck,
    /// Notbremse `maxWalkSeconds`.
    Timeout,
    /// Verbindung weg, `:stop` oder ein neuerer Befehl.
    Cancelled,
    /// Der Server hat uns noch keine Position geschickt.
    Unknown,
}

fn spawn(shared: &Arc<Shared>, task: Task) {
    let job = Job {
        id: shared.mover.claim(),
        generation: shared.generation.load(Ordering::SeqCst),
    };
    let shared = Arc::clone(shared);
    thread::Builder::new()
        .name("afk-move".into())
        .spawn(move || {
            run(&shared, job, task);
            shared.mover.finish(job.id);
        })
        .ok();
}

fn run(shared: &Arc<Shared>, job: Job, task: Task) {
    // Ein Befehl, der genau zwischen dem Teleport des Servers und der fertig geladenen Umgebung
    // abgesetzt wird, soll nicht als „abgebrochen" enden, sondern die paar Millisekunden abwarten.
    // Wer noch gar nicht im Spiel ist, kommt sofort zurück und läuft in die übliche Absage;
    // `Home` bringt darüber hinaus seine eigene, längere Wartezeit mit ([`wait_for_position`]).
    let _ = shared.await_gameplay(crate::client::GAMEPLAY_WAIT);
    let settings = shared.mover.get();
    match task {
        Task::Turn { yaw, pitch } => {
            report(
                shared,
                turn_to(shared, job, &settings, yaw, pitch),
                "Drehen",
            );
        }
        Task::Jump { dx, dz } => {
            let keys = match (dx, dz, shared.position()) {
                (dx, dz, Some((_, _, _, yaw, _))) if dx != 0.0 || dz != 0.0 => {
                    keys_for(yaw_towards(dx, dz) - yaw)
                }
                _ => (false, false, false, false),
            };
            report(shared, jump(shared, job, keys), "Sprung");
        }
        Task::WalkTo {
            x,
            z,
            record,
            label,
        } => {
            let outcome = walk_to(shared, job, &settings, x, z, false);
            if record && matches!(outcome, Outcome::Arrived) {
                record_point(shared);
            }
            report(shared, outcome, &label);
        }
        Task::Home {
            waypoints,
            target,
            delay,
        } => {
            // Nach dem Beitritt kennt der Client seine Position erst nach dem ersten Teleport.
            if !wait_for_position(shared, job) || !nap(shared, job, delay) {
                return;
            }
            shared.console.note(&if waypoints.is_empty() {
                "Laufe zur Heimatposition ...".to_string()
            } else {
                format!(
                    "Laufe die Route ({} Wegpunkte) zur Heimatposition ...",
                    waypoints.len()
                )
            });

            let mut outcome = Outcome::Arrived;
            let mut label = String::from("Heimatposition");
            for (index, point) in waypoints.iter().enumerate() {
                // Wegpunkte kennen ihre Höhe – damit geht es Treppen hinauf und hinunter.
                outcome = walk_to(shared, job, &settings, point.x, point.z, true);
                if !matches!(outcome, Outcome::Arrived) {
                    label = format!("Route (Wegpunkt {}/{})", index + 1, waypoints.len());
                    break;
                }
            }
            if matches!(outcome, Outcome::Arrived) {
                outcome = walk_to(shared, job, &settings, target.x, target.z, true);
            }
            if matches!(outcome, Outcome::Arrived) {
                outcome = turn_to(shared, job, &settings, target.yaw, target.pitch);
            }
            report(shared, outcome, &label);
        }
    }
}

/// Nach einem erreichten `:go`-Ziel: die neue Position an die laufende Aufzeichnung hängen.
fn record_point(shared: &Arc<Shared>) {
    let Some((x, y, z, yaw, pitch)) = shared.position() else {
        return;
    };
    let mut guard = shared.mover.recording.lock().unwrap();
    let Some(points) = guard.as_mut() else {
        return;
    };
    if points.len() >= MAX_ROUTE {
        return shared.console.warn(&format!(
            "Route: mehr als {} Wegpunkte gehen nicht.",
            MAX_ROUTE
        ));
    }
    points.push(Spot {
        x,
        y,
        z,
        yaw,
        pitch,
    });
    shared
        .console
        .info(&format!("Wegpunkt {} aufgezeichnet.", points.len()));
}

fn report(shared: &Arc<Shared>, outcome: Outcome, label: &str) {
    match outcome {
        Outcome::Arrived => shared.console.ok(&format!("{}: fertig.", label)),
        Outcome::Stuck => shared.console.warn(&format!(
            "{}: komme trotz Springen nicht weiter – abgebrochen.",
            label
        )),
        Outcome::Timeout => shared
            .console
            .warn(&format!("{}: Zeitlimit erreicht – abgebrochen.", label)),
        Outcome::Unknown => shared
            .console
            .error(&format!("{}: Position unbekannt – abgebrochen.", label)),
        Outcome::Cancelled => {}
    }
}

// ===================== Laufen und Drehen =====================
//
// Alles hier drückt nur Tasten und dreht den Kopf. Wohin der Bot dabei wirklich kommt, rechnet
// die Physik (siehe [`crate::physics`]): Er fällt Kanten hinunter, steigt Stufen hinauf, bleibt
// an Wänden hängen und springt – wie ein Spieler, der dieselben Tasten drückt.

/// Ein Physik-Tick abwarten.
fn sleep_tick(next: Instant) -> Instant {
    let now = Instant::now();
    if next > now {
        thread::sleep(next - now);
    }
    next.max(now) + TICK
}

/// Tasten loslassen – am Ende jeder Bewegung, auch bei Abbruch. Schleichen und Sprinten gehen
/// auf das zurück, was `:sneak`/`:sprint` eingestellt haben.
fn release(shared: &Shared) {
    let sneaking = shared.extras.sneaking();
    let sprinting = shared.extras.sprinting();
    shared.driver.set_keys(|keys| {
        keys.forward = false;
        keys.backward = false;
        keys.left = false;
        keys.right = false;
        keys.jump = false;
        keys.shift = sneaking;
        keys.sprint = sprinting;
    });
}

/// Blickrichtung zu einem Punkt (Minecraft-Winkel: 0 = Süden, 90 = Westen).
fn yaw_towards(dx: f64, dz: f64) -> f32 {
    (-dx).atan2(dz).to_degrees() as f32
}

/// Die Tastenkombination, deren Laufrichtung relativ zum Blick `relative` Grad am nächsten
/// kommt. Wie ein Spieler mit WASD: vorwärts, seitwärts, schräg, rückwärts.
fn keys_for(relative: f32) -> (bool, bool, bool, bool) {
    // (vor, zurück, links, rechts) je 45°-Sektor, beginnend bei 0° = vorwärts.
    const SECTORS: [(bool, bool, bool, bool); 8] = [
        (true, false, false, false), // 0
        (true, false, false, true),  // 45: vorne rechts
        (false, false, false, true), // 90: rechts
        (false, true, false, true),  // 135
        (false, true, false, false), // 180
        (false, true, true, false),  // -135
        (false, false, true, false), // -90: links
        (true, false, true, false),  // -45
    ];
    let index = ((wrap_degrees(relative) + 360.0 + 22.5) / 45.0) as usize % 8;
    SECTORS[index]
}

/// Zum Punkt (x|z) laufen.
///
/// `face`: zum Ziel drehen und vorwärts laufen (Heimweg, Route). Sonst bleibt der Blick, und es
/// wird mit den passenden Tasten seitwärts oder rückwärts gegangen (`:go links 3`).
/// Steht etwas im Weg, wird gesprungen (eine Stufe von einem Block schafft jeder Sprung); hilft
/// auch das nicht, endet der Lauf mit [`Outcome::Stuck`].
fn walk_to(
    shared: &Arc<Shared>,
    job: Job,
    settings: &Settings,
    tx: f64,
    tz: f64,
    face: bool,
) -> Outcome {
    let limit = Duration::from_secs(settings.max_walk_seconds);
    let started = Instant::now();
    let mut closest = f64::MAX;
    let mut closest_at = Instant::now();
    let mut next = Instant::now();
    // Kurze Strecken (`:go vor 0.2`) brauchen eine feinere Schwelle als lange.
    let arrived = match shared.position() {
        Some((x, _, z, _, _)) => ARRIVED.min(((tx - x).powi(2) + (tz - z).powi(2)).sqrt() / 2.0),
        None => ARRIVED,
    };
    let outcome = loop {
        if !job.alive(shared) {
            break Outcome::Cancelled;
        }
        let Some((x, _, z, yaw, pitch)) = shared.position() else {
            break Outcome::Unknown;
        };
        let (dx, dz) = (tx - x, tz - z);
        let distance = (dx * dx + dz * dz).sqrt();
        if distance <= arrived {
            break Outcome::Arrived;
        }
        if started.elapsed() > limit {
            break Outcome::Timeout;
        }
        if distance < closest - 0.05 {
            closest = distance;
            closest_at = Instant::now();
        } else if closest_at.elapsed() > STUCK_AFTER {
            break Outcome::Stuck;
        }

        let target = yaw_towards(dx, dz);
        let mut yaw = yaw;
        if face {
            let step = settings.turn_speed as f32;
            yaw += wrap_degrees(target - yaw).clamp(-step, step);
            shared.driver.look(yaw, pitch);
        }
        let relative = target - yaw;
        let (forward, backward, left, right) = if face && wrap_degrees(relative).abs() > 60.0 {
            (false, false, false, false) // erst drehen, dann laufen
        } else {
            keys_for(relative)
        };
        // Wie die automatische Sprunghilfe des Spiels: an einer Wand am Boden springen.
        let jump = shared.driver.horizontal_collision() && shared.driver.on_ground();
        shared.driver.set_keys(|keys| {
            keys.forward = forward;
            keys.backward = backward;
            keys.left = left;
            keys.right = right;
            keys.jump = jump;
            // Langsames Gehen bedeutet hier Schleichen – das ist das einzige echte Langsam.
            keys.shift = settings.walk_speed < 2.0 || shared.extras.sneaking();
            keys.sprint = settings.walk_speed > 4.4 || shared.extras.sprinting();
        });
        next = sleep_tick(next);
    };
    release(shared);
    // Ausrollen lassen: Die Physik bremst ab, die Position ist danach die echte.
    nap(shared, job, Duration::from_millis(250));
    outcome
}

/// Springen – auf der Stelle oder mit einer Laufrichtung.
fn jump(shared: &Arc<Shared>, job: Job, keys: (bool, bool, bool, bool)) -> Outcome {
    let mut next = Instant::now();
    let (forward, backward, left, right) = keys;
    shared.driver.set_keys(|k| {
        k.jump = true;
        k.forward = forward;
        k.backward = backward;
        k.left = left;
        k.right = right;
    });
    // Abheben abwarten, dann die Landung.
    let mut left_ground = false;
    for _ in 0..40 {
        if !job.alive(shared) {
            release(shared);
            return Outcome::Cancelled;
        }
        next = sleep_tick(next);
        let grounded = shared.driver.on_ground();
        if !grounded {
            left_ground = true;
            shared.driver.set_keys(|k| k.jump = false);
        } else if left_ground {
            break;
        }
    }
    release(shared);
    Outcome::Arrived
}

/// Kopf gleichmäßig drehen.
fn turn_to(shared: &Arc<Shared>, job: Job, settings: &Settings, yaw: f32, pitch: f32) -> Outcome {
    let mut next = Instant::now();
    let step = settings.turn_speed as f32;
    loop {
        if !job.alive(shared) {
            return Outcome::Cancelled;
        }
        let Some((_, _, _, now_yaw, now_pitch)) = shared.position() else {
            return Outcome::Unknown;
        };
        let dy = wrap_degrees(yaw - now_yaw);
        let dp = pitch - now_pitch;
        if dy.abs() < 0.5 && dp.abs() < 0.5 {
            shared.driver.look(now_yaw + dy, pitch);
            return Outcome::Arrived;
        }
        shared.driver.look(
            now_yaw + dy.clamp(-step, step),
            now_pitch + dp.clamp(-step, step),
        );
        next = sleep_tick(next);
    }
}

/// Blickrichtung sofort setzen (Anti-AFK); geht mit dem nächsten Tick hinaus.
pub(crate) fn send_rotation(shared: &Shared, yaw: f32, pitch: f32) {
    shared.driver.look(yaw, pitch);
}

/// Unterbrechbar warten.
fn nap(shared: &Arc<Shared>, job: Job, duration: Duration) -> bool {
    let until = Instant::now() + duration;
    while Instant::now() < until {
        if !job.connection_alive(shared) {
            return false;
        }
        thread::sleep(TICK.min(until.saturating_duration_since(Instant::now())));
    }
    true
}

/// Nach dem Beitritt kommt die Position erst mit dem ersten Teleport.
fn wait_for_position(shared: &Arc<Shared>, job: Job) -> bool {
    let until = Instant::now() + POSITION_TIMEOUT;
    while Instant::now() < until {
        if !job.connection_alive(shared) {
            return false;
        }
        if shared.ready_for_gameplay() && shared.position().is_some() {
            return true;
        }
        thread::sleep(TICK);
    }
    false
}

// ===================== Haken aus dem Client =====================

/// Nach jedem Beitritt (auch nach einem Unterserver-Wechsel – dort ist die Welt eine andere und
/// der Server setzt uns neu ab).
pub fn on_join(shared: &Arc<Shared>) {
    let settings = shared.mover.get();
    let (true, Some(target)) = (settings.home_enabled, settings.home) else {
        return;
    };
    spawn(
        shared,
        Task::Home {
            waypoints: settings.route,
            target,
            delay: Duration::from_secs(settings.home_delay_seconds),
        },
    );
}

/// Verbindung beendet: laufende Bewegung verwerfen.
pub fn on_disconnect(shared: &Shared) {
    shared.mover.stop();
}

// ===================== Befehle =====================

/// Richtung relativ zur Blickrichtung – genau wie W/A/S/D im echten Client.
#[derive(Clone, Copy)]
enum Direction {
    Forward,
    Back,
    Left,
    Right,
}

impl Direction {
    fn parse(word: &str) -> Option<Direction> {
        match word {
            "vor" | "vorne" | "vorwaerts" | "vorwärts" | "w" | "forward" | "f" => {
                Some(Direction::Forward)
            }
            "zurueck" | "zurück" | "rueckwaerts" | "rückwärts" | "s" | "back" | "b" => {
                Some(Direction::Back)
            }
            "links" | "a" | "left" | "l" => Some(Direction::Left),
            "rechts" | "d" | "right" | "r" => Some(Direction::Right),
            _ => None,
        }
    }

    fn name(self) -> &'static str {
        match self {
            Direction::Forward => "vorwärts",
            Direction::Back => "rückwärts",
            Direction::Left => "links",
            Direction::Right => "rechts",
        }
    }

    /// Einheitsvektor (x|z) bei gegebenem Gierwinkel. Minecraft: Gierwinkel 0 = Süden (+Z),
    /// 90 = Westen (−X); vorwärts ist also (−sin, cos), rechts davon (−cos, −sin).
    fn vector(self, yaw: f32) -> (f64, f64) {
        let (sin, cos) = (yaw as f64).to_radians().sin_cos();
        match self {
            Direction::Forward => (-sin, cos),
            Direction::Back => (sin, -cos),
            Direction::Right => (-cos, -sin),
            Direction::Left => (cos, sin),
        }
    }
}

/// Einstiegspunkt aus der Eingabeschleife. `verb` ist der `:`-Befehl ohne Doppelpunkt.
pub fn command(shared: &Arc<Shared>, verb: &str, arg: &str) {
    match verb {
        "go" | "geh" | "gehe" | "lauf" | "laufe" => go(shared, arg),
        "look" | "schau" | "dreh" | "drehe" => look(shared, arg),
        "home" | "heim" => home(shared, arg),
        "route" | "weg" | "strecke" => route(shared, arg),
        "jump" | "spring" | "springe" => jump_command(shared, arg),
        "fall" | "fallen" => fall_command(shared),
        "stop" | "halt" => {
            shared.mover.stop();
            shared.console.info("Bewegung gestoppt.");
        }
        _ => shared
            .console
            .error(&format!("Unbekannter Befehl: :{} (siehe :help)", verb)),
    }
}

fn go(shared: &Arc<Shared>, arg: &str) {
    let mut parts = arg.split_whitespace();
    let Some(word) = parts.next() else {
        return usage_go(&shared.console);
    };
    let word = word.to_lowercase();
    if word == "stop" || word == "halt" {
        shared.mover.stop();
        return shared.console.info("Bewegung gestoppt.");
    }
    let Some(direction) = Direction::parse(&word) else {
        return usage_go(&shared.console);
    };
    // Ohne Angabe genau ein Block – das ist die häufigste Feinkorrektur.
    let blocks = match parts.next() {
        Some(text) => match parse_number(text) {
            Some(value) if value > 0.0 && value <= MAX_BLOCKS => value,
            _ => {
                return shared.console.error(&format!(
                    "Anzahl muss zwischen 0 und {} liegen.",
                    MAX_BLOCKS
                ))
            }
        },
        None => 1.0,
    };

    let Some((x, _, z, yaw, _)) = shared.position() else {
        return shared
            .console
            .error("Position noch unbekannt (nicht im Spiel?).");
    };
    let (fx, fz) = direction.vector(yaw);
    shared.console.note(&format!(
        "Gehe {:.1} Blöcke {} ...",
        blocks,
        direction.name()
    ));
    spawn(
        shared,
        Task::WalkTo {
            x: x + fx * blocks,
            z: z + fz * blocks,
            // Läuft gerade eine Aufzeichnung, wird genau hieraus die Route.
            record: true,
            label: format!("{:.1} Blöcke {}", blocks, direction.name()),
        },
    );
}

/// `:jump` (auf der Stelle) oder `:jump vor|zurück|links|rechts` (im Sprung ein Stück weiter).
fn jump_command(shared: &Arc<Shared>, arg: &str) {
    let word = arg.split_whitespace().next().unwrap_or("").to_lowercase();
    let (dx, dz) = if word.is_empty() {
        (0.0, 0.0)
    } else {
        let Some(direction) = Direction::parse(&word) else {
            shared
                .console
                .error("Nutzung: :jump   oder   :jump vor|zurück|links|rechts");
            return;
        };
        let Some((_, _, _, yaw, _)) = shared.position() else {
            return shared
                .console
                .error("Position noch unbekannt (nicht im Spiel?).");
        };
        direction.vector(yaw)
    };
    if shared.position().is_none() {
        return shared
            .console
            .error("Position noch unbekannt (nicht im Spiel?).");
    }
    shared.console.note("Springe ...");
    spawn(shared, Task::Jump { dx, dz });
}

/// Fallen passiert von selbst – die Physik zieht den Bot nach unten, sobald nichts trägt.
fn fall_command(shared: &Arc<Shared>) {
    shared
        .console
        .info("Fallen passiert von selbst: Steht nichts unter dem Bot, fällt er wie ein Spieler.");
}

fn look(shared: &Arc<Shared>, arg: &str) {
    let Some((_, _, _, yaw, pitch)) = shared.position() else {
        return shared
            .console
            .error("Position noch unbekannt (nicht im Spiel?).");
    };
    let mut parts = arg.split_whitespace();
    let Some(first) = parts.next() else {
        return usage_look(&shared.console);
    };
    let word = first.to_lowercase();

    // Zwei Zahlen = absolute Blickrichtung (Gierwinkel, Neigung).
    let target = if let Some(absolute) = parse_number(&word) {
        let pitch = parts.next().and_then(parse_number).unwrap_or(pitch as f64);
        (absolute as f32, pitch as f32)
    } else {
        // Sonst: Himmelsrichtung oder relative Drehung um <grad> (Standard 90 bzw. 30).
        let amount = parts.next().and_then(parse_number);
        match word.as_str() {
            "nord" | "norden" | "north" | "n" => (180.0, pitch),
            "sued" | "süd" | "sueden" | "süden" | "south" => (0.0, pitch),
            "ost" | "osten" | "east" | "e" => (-90.0, pitch),
            "west" | "westen" | "w" => (90.0, pitch),
            "gerade" | "mitte" | "level" => (yaw, 0.0),
            "links" | "left" | "l" => (yaw - amount.unwrap_or(90.0) as f32, pitch),
            "rechts" | "right" | "r" => (yaw + amount.unwrap_or(90.0) as f32, pitch),
            "hoch" | "up" | "oben" => (yaw, pitch - amount.unwrap_or(30.0) as f32),
            "runter" | "down" | "unten" => (yaw, pitch + amount.unwrap_or(30.0) as f32),
            "um" | "umdrehen" | "back" => (yaw + 180.0, pitch),
            _ => return usage_look(&shared.console),
        }
    };

    let (target_yaw, target_pitch) = (wrap_degrees(target.0), target.1.clamp(-90.0, 90.0));
    shared.console.note(&format!(
        "Drehe auf {:.0}° ({}) / {:.0}° ...",
        target_yaw,
        compass(target_yaw),
        target_pitch
    ));
    spawn(
        shared,
        Task::Turn {
            yaw: target_yaw,
            pitch: target_pitch,
        },
    );
}

fn home(shared: &Arc<Shared>, arg: &str) {
    let (verb, rest) = match arg.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb.to_lowercase(), rest.trim().to_string()),
        None => (arg.trim().to_lowercase(), String::new()),
    };

    match verb.as_str() {
        "" | "status" | "list" => print_home(shared),
        "on" | "an" | "ein" => {
            if shared.mover.get().home.is_none() {
                return shared
                    .console
                    .error("Keine Heimatposition gesetzt – erst  :home set  (siehe :home).");
            }
            shared.mover.edit(|s| s.home_enabled = true);
            shared
                .console
                .ok("Heimatposition aktiv – bei jedem Beitritt wird dorthin gelaufen.");
        }
        "off" | "aus" => {
            shared.mover.edit(|s| s.home_enabled = false);
            shared.console.info("Heimatposition aus.");
        }
        "set" | "setze" => set_home(shared, &rest),
        "clear" | "loeschen" | "löschen" | "del" => {
            shared.mover.edit(|s| {
                s.home = None;
                s.home_enabled = false;
            });
            shared.console.info("Heimatposition gelöscht.");
        }
        "go" | "los" | "jetzt" => go_home(shared),
        "delay" => match parse_number(&rest) {
            Some(seconds) if (0.0..=3600.0).contains(&seconds) => {
                shared.mover.edit(|s| s.home_delay_seconds = seconds as u64);
                shared.console.info(&format!(
                    "Startverzögerung: {} s nach dem Beitritt.",
                    seconds as u64
                ));
            }
            _ => shared.console.error("Nutzung: :home delay <sekunden>"),
        },
        "speed" | "tempo" => match parse_number(&rest) {
            Some(speed) => {
                shared.mover.edit(|s| s.walk_speed = speed);
                shared.console.info(&format!(
                    "Laufgeschwindigkeit: {:.3} Blöcke/s (4,317 = Gehen, 5,612 = Sprinten).",
                    shared.mover.get().walk_speed
                ));
            }
            None => shared
                .console
                .error("Nutzung: :home speed <blöcke pro sekunde>"),
        },
        other => shared
            .console
            .error(&format!("Unbekannt: :home {} (siehe :home)", other)),
    }
}

/// Jetzt sofort heimlaufen – über die Route, falls eine aufgezeichnet ist.
fn go_home(shared: &Arc<Shared>) {
    let settings = shared.mover.get();
    let Some(target) = settings.home else {
        return shared
            .console
            .error("Keine Heimatposition gesetzt (:home set).");
    };
    spawn(
        shared,
        Task::Home {
            waypoints: settings.route,
            target,
            delay: Duration::ZERO,
        },
    );
}

/// `:home set` (aktuelle Position) oder `:home set <x> <y> <z> [gier] [neigung]`.
fn set_home(shared: &Arc<Shared>, rest: &str) {
    let numbers: Vec<f64> = rest.split_whitespace().filter_map(parse_number).collect();
    let spot = if numbers.is_empty() {
        let Some((x, y, z, yaw, pitch)) = shared.position() else {
            return shared
                .console
                .error("Position noch unbekannt (nicht im Spiel?).");
        };
        Spot {
            x,
            y,
            z,
            yaw,
            pitch,
        }
    } else if numbers.len() >= 3 {
        Spot {
            x: numbers[0],
            y: numbers[1],
            z: numbers[2],
            yaw: numbers.get(3).copied().unwrap_or(0.0) as f32,
            pitch: numbers.get(4).copied().unwrap_or(0.0) as f32,
        }
    } else {
        return shared
            .console
            .error("Nutzung: :home set   oder   :home set <x> <y> <z> [gier] [neigung]");
    };

    shared.mover.edit(|s| s.home = Some(spot));
    shared
        .console
        .ok(&format!("Heimatposition: {}", spot.describe()));
    if !shared.mover.get().home_enabled {
        shared
            .console
            .info("Noch nicht aktiv – mit  :home on  beim Beitritt automatisch dorthin laufen.");
    }
}

fn print_home(shared: &Arc<Shared>) {
    let settings = shared.mover.get();
    let console = &shared.console;
    console.print("");
    console.print(&console.paint(BOLD, "  Heimatposition"));
    match settings.home {
        Some(spot) => {
            let mark = if settings.home_enabled {
                console.paint(CYAN, "● an ")
            } else {
                console.paint(GRAY, "○ aus")
            };
            console.print(&format!("    {}  {}", mark, spot.describe()));
            console.print(&console.paint(
                GRAY,
                &format!(
                    "    Start {} s nach dem Beitritt  ·  {:.3} Blöcke/s",
                    settings.home_delay_seconds, settings.walk_speed
                ),
            ));
        }
        None => console.print(&console.paint(
            GRAY,
            "    (keine – mit  :home set  die aktuelle Position übernehmen)",
        )),
    }
    console.print(&console.paint(
        GRAY,
        "    :home set   :home on|off   :home go   :home delay <sek>   :home clear",
    ));
    if !settings.route.is_empty() {
        console.print(&console.paint(
            GRAY,
            &format!(
                "    Route: {} Wegpunkte werden vorher abgelaufen (:route)",
                settings.route.len()
            ),
        ));
    }
}

// ===================== Route =====================

/// Wegpunkte statt Wegfindung: Ecken, Türen und Treppen kennt der Nutzer, der Client läuft sie nur
/// nach. Aufgezeichnet wird, indem man die Strecke einmal mit `:go` abläuft – jedes erreichte Ziel
/// wird ein Wegpunkt.
fn route(shared: &Arc<Shared>, arg: &str) {
    let (verb, rest) = match arg.split_once(char::is_whitespace) {
        Some((verb, rest)) => (verb.to_lowercase(), rest.trim().to_string()),
        None => (arg.trim().to_lowercase(), String::new()),
    };
    let console = &shared.console;

    match verb.as_str() {
        "" | "status" | "list" => print_route(shared),

        "rec" | "record" | "aufnahme" | "start" => {
            if shared.position().is_none() {
                return console.error("Position noch unbekannt (nicht im Spiel?).");
            }
            let previous = shared.mover.recording.lock().unwrap().replace(Vec::new());
            if previous.is_some() {
                console.info("Vorige Aufzeichnung verworfen.");
            }
            console.ok("Aufzeichnung läuft.");
            console.info(
                "Laufe die Strecke jetzt mit  :go vor 5  usw. ab – jedes Ziel wird ein Wegpunkt.",
            );
            console.info("Am Ziel angekommen:  :route stop");
        }

        "stop" | "ende" | "fertig" => {
            let Some(points) = shared.mover.recording.lock().unwrap().take() else {
                return console.error("Es läuft keine Aufzeichnung (:route rec).");
            };
            if points.is_empty() {
                return console.warn("Nichts aufgezeichnet – Route unverändert.");
            }
            let here = shared.position();
            shared.mover.edit(|s| {
                // Ohne Heimatposition wird der Endpunkt der Aufzeichnung zum Ziel.
                if s.home.is_none() {
                    if let Some((x, y, z, yaw, pitch)) = here {
                        s.home = Some(Spot {
                            x,
                            y,
                            z,
                            yaw,
                            pitch,
                        });
                    }
                }
                s.route = points;
                // Der letzte Wegpunkt ist das Ziel selbst – als Zwischenstopp wäre er überflüssig.
                if let (Some(last), Some(home)) = (s.route.last().copied(), s.home) {
                    if last.flat_distance(&home) < 1.0 {
                        s.route.pop();
                    }
                }
            });
            let settings = shared.mover.get();
            console.ok(&format!(
                "Route gespeichert: {} Wegpunkte + Ziel.",
                settings.route.len()
            ));
            if !settings.home_enabled {
                console.info("Noch nicht aktiv – mit  :home on  bei jedem Beitritt ablaufen.");
            }
        }

        "add" | "punkt" | "+" => {
            let Some((x, y, z, yaw, pitch)) = shared.position() else {
                return console.error("Position noch unbekannt (nicht im Spiel?).");
            };
            let spot = Spot {
                x,
                y,
                z,
                yaw,
                pitch,
            };
            let mut guard = shared.mover.recording.lock().unwrap();
            if let Some(points) = guard.as_mut() {
                if points.len() >= MAX_ROUTE {
                    return console
                        .error(&format!("Mehr als {} Wegpunkte gehen nicht.", MAX_ROUTE));
                }
                points.push(spot);
                console.ok(&format!("Wegpunkt {} aufgezeichnet.", points.len()));
            } else {
                drop(guard);
                if shared.mover.get().route.len() >= MAX_ROUTE {
                    return console
                        .error(&format!("Mehr als {} Wegpunkte gehen nicht.", MAX_ROUTE));
                }
                shared.mover.edit(|s| s.route.push(spot));
                console.ok(&format!(
                    "Wegpunkt {} angehängt.",
                    shared.mover.get().route.len()
                ));
            }
        }

        "del" | "rm" | "-" => {
            let count = shared.mover.get().route.len();
            if count == 0 {
                return console.error("Die Route ist leer.");
            }
            // Ohne Nummer den letzten – das ist beim Nachbessern der häufigste Fall.
            let index = match rest.trim() {
                "" => count - 1,
                text => match text.parse::<usize>() {
                    Ok(number) if number >= 1 && number <= count => number - 1,
                    _ => return console.error(&format!("Nutzung: :route del <1..{}>", count)),
                },
            };
            shared.mover.edit(|s| {
                if index < s.route.len() {
                    s.route.remove(index);
                }
            });
            console.info(&format!("Wegpunkt {} entfernt.", index + 1));
        }

        "clear" | "leeren" | "loeschen" | "löschen" => {
            *shared.mover.recording.lock().unwrap() = None;
            shared.mover.edit(|s| s.route.clear());
            console.info("Route gelöscht – es wird wieder geradeaus zur Heimatposition gelaufen.");
        }

        "go" | "los" | "jetzt" => go_home(shared),

        other => console.error(&format!("Unbekannt: :route {} (siehe :route)", other)),
    }
}

fn print_route(shared: &Arc<Shared>) {
    let settings = shared.mover.get();
    let recording = shared.mover.recording.lock().unwrap().clone();
    let console = &shared.console;

    console.print("");
    console.print(&console.paint(BOLD, "  Route zur Heimatposition"));
    if let Some(points) = &recording {
        console.print(&console.paint(
            CYAN,
            &format!(
                "    ● Aufzeichnung läuft – {} Wegpunkte. Beenden mit  :route stop",
                points.len()
            ),
        ));
    }
    let points = recording.as_ref().unwrap_or(&settings.route);
    if points.is_empty() {
        console.print(&console.paint(
            GRAY,
            "    (keine – ohne Route wird geradeaus zur Heimatposition gelaufen)",
        ));
    }
    for (index, point) in points.iter().enumerate() {
        console.print(&format!(
            "    {})  x={:.1}  y={:.1}  z={:.1}",
            index + 1,
            point.x,
            point.y,
            point.z
        ));
    }
    match settings.home {
        Some(home) => console.print(&format!(
            "    {})  {}  {}",
            points.len() + 1,
            console.paint(CYAN, "Ziel"),
            home.describe()
        )),
        None => console.print(&console.paint(
            GRAY,
            "    (noch kein Ziel – :home set  oder  :route stop am Zielpunkt)",
        )),
    }
    console.print(&console.paint(
        GRAY,
        "    :route rec   :route stop   :route add   :route del [nr]   :route go   :route clear",
    ));
}

fn usage_go(console: &Console) {
    console.error("Nutzung: :go vor|zurück|links|rechts [blöcke]     z. B.  :go vor 5");
    console.info(
        "Richtung ist relativ zum Blick (wie W/A/S/D). Ohne Zahl = 1 Block. :stop bricht ab.",
    );
}

fn usage_look(console: &Console) {
    console.error("Nutzung: :look <gier> [neigung]  ·  :look links|rechts|hoch|runter [grad]");
    console.info("Auch: :look nord|ost|süd|west  ·  :look um  ·  :look gerade (Neigung 0)");
}

/// Zahl mit Punkt oder Komma („2,5").
fn parse_number(text: &str) -> Option<f64> {
    let value: f64 = text.trim().replace(',', ".").parse().ok()?;
    value.is_finite().then_some(value)
}

/// Gierwinkel auf (−180, 180] normieren – der kürzere Drehweg lässt sich so direkt ablesen.
fn wrap_degrees(value: f32) -> f32 {
    let mut wrapped = value % 360.0;
    if wrapped > 180.0 {
        wrapped -= 360.0;
    }
    if wrapped <= -180.0 {
        wrapped += 360.0;
    }
    wrapped
}

/// Himmelsrichtung zum Gierwinkel (Minecraft: 0 = Süden, 90 = Westen).
fn compass(yaw: f32) -> &'static str {
    const NAMES: [&str; 8] = [
        "Süd", "Südwest", "West", "Nordwest", "Nord", "Nordost", "Ost", "Südost",
    ];
    let index = ((wrap_degrees(yaw) / 45.0).round() as i32).rem_euclid(8) as usize;
    NAMES[index]
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Minecraft: Gierwinkel 0 = Süden (+Z), 90 = Westen (−X). Daran hängt jede Laufrichtung.
    #[test]
    fn richtungen_folgen_dem_blick() {
        let close = |a: f64, b: f64| assert!((a - b).abs() < 1e-9, "{} != {}", a, b);

        // Blick nach Süden: vorwärts = +Z, rechts = −X (Westen).
        let (x, z) = Direction::Forward.vector(0.0);
        close(x, 0.0);
        close(z, 1.0);
        let (x, z) = Direction::Right.vector(0.0);
        close(x, -1.0);
        close(z, 0.0);

        // Blick nach Westen (90°): vorwärts = −X, rechts = −Z (Norden).
        let (x, z) = Direction::Forward.vector(90.0);
        close(x, -1.0);
        close(z, 0.0);
        let (x, z) = Direction::Right.vector(90.0);
        close(x, 0.0);
        close(z, -1.0);

        // Rückwärts ist die Gegenrichtung von vorwärts, links die von rechts.
        let (fx, fz) = Direction::Forward.vector(37.0);
        let (bx, bz) = Direction::Back.vector(37.0);
        close(fx, -bx);
        close(fz, -bz);
    }

    /// Ohne kürzesten Weg würde der Kopf für 10° einmal komplett herumfahren.
    #[test]
    fn drehung_nimmt_den_kuerzeren_weg() {
        assert_eq!(wrap_degrees(190.0), -170.0);
        assert_eq!(wrap_degrees(-190.0), 170.0);
        assert_eq!(wrap_degrees(180.0), 180.0);
        assert_eq!(wrap_degrees(360.0), 0.0);
        // von 170° nach −170° sind es +20°, nicht −340°.
        assert_eq!(wrap_degrees(-170.0 - 170.0), 20.0);
    }

    #[test]
    fn himmelsrichtungen_stimmen() {
        assert_eq!(compass(0.0), "Süd");
        assert_eq!(compass(90.0), "West");
        assert_eq!(compass(180.0), "Nord");
        assert_eq!(compass(-90.0), "Ost");
        assert_eq!(compass(270.0), "Ost");
    }

    /// `movement.json` ist eine gewöhnliche Textdatei – darin kann auch `1e400` stehen, und
    /// JSON macht daraus eine Unendlichkeit. Ungeprüft war der Abstand zum Ziel danach unendlich
    /// und die Schrittweite null: `:home go` lief bis ins Zeitlimit, ohne sich zu bewegen.
    #[test]
    fn unmoegliche_punkte_aus_der_datei_werden_geradegerueckt() {
        let kaputt = Spot {
            x: f64::INFINITY,
            y: f64::NAN,
            z: 1e300,
            yaw: f64::INFINITY as f32,
            pitch: 400.0,
        };
        let mut settings = Settings {
            home: Some(kaputt),
            route: vec![kaputt],
            ..Settings::default()
        };
        settings.normalize();
        for point in settings.route.iter().chain(settings.home.iter()) {
            assert!(
                point.x.is_finite() && point.x.abs() <= WORLD_LIMIT,
                "{}",
                point.x
            );
            assert!(
                point.y.is_finite() && point.y.abs() <= WORLD_LIMIT,
                "{}",
                point.y
            );
            assert!(
                point.z.is_finite() && point.z.abs() <= WORLD_LIMIT,
                "{}",
                point.z
            );
            assert!(
                point.yaw.is_finite() && point.yaw.abs() <= 180.0,
                "{}",
                point.yaw
            );
            assert!((-90.0..=90.0).contains(&point.pitch), "{}", point.pitch);
        }
    }

    /// Eine zu lange Route würde die Datei und den Heimlauf sinnlos aufblähen.
    /// WASD wie im Spiel: Die Taste, deren Laufrichtung dem Ziel am nächsten kommt.
    #[test]
    fn tasten_folgen_der_zielrichtung() {
        assert_eq!(keys_for(0.0), (true, false, false, false));
        assert_eq!(keys_for(-90.0), (false, false, true, false));
        assert_eq!(keys_for(90.0), (false, false, false, true));
        assert_eq!(keys_for(180.0), (false, true, false, false));
        assert_eq!(keys_for(-180.0), (false, true, false, false));
        assert_eq!(keys_for(44.0), (true, false, false, true));
        // Süden = Gierwinkel 0, Osten (+x) = -90.
        assert!((yaw_towards(0.0, 1.0) - 0.0).abs() < 1e-4);
        assert!((yaw_towards(1.0, 0.0) + 90.0).abs() < 1e-4);
    }

    #[test]
    fn route_bleibt_begrenzt() {
        let mut settings = Settings {
            route: vec![Spot::default(); MAX_ROUTE + 10],
            ..Settings::default()
        };
        settings.normalize();
        assert_eq!(settings.route.len(), MAX_ROUTE);
    }

    #[test]
    fn zahlen_auch_mit_komma() {
        assert_eq!(parse_number("2,5"), Some(2.5));
        assert_eq!(parse_number(" -3 "), Some(-3.0));
        assert_eq!(parse_number("x"), None);
    }
}
