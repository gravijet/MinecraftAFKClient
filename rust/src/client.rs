//! Der eigentliche AFK-Client: verbinden, verbunden bleiben, Chat senden/empfangen.
//!
//! Kick-Schutz ist rein protokollbasiert – genau das, was ein wartender Vanilla-Client tut:
//! `KeepAlive` sofort beantworten, `Ping`→`Pong`, Teleports bestätigen, nicht ladbare Resource-Packs
//! ehrlich ablehnen, beim Beitritt `ClientInformation` senden, Cookies beantworten,
//! den Verhaltenskodex (ab 1.21.11) bestätigen. **Keine** Anti-AFK-Bewegung.
//!
//! Threads: 1× Netz (liest und antwortet), 1× Sender (rate-limitiert) sowie ab 1.21.2 während
//! einer Verbindung ein Tick-End-Sender. Im Leerlauf blockieren sie – kein Polling, praktisch
//! 0 % CPU.
//!
//! Die Protokollversion steckt in [`crate::protocol::Protocol`] und kommt aus `--mc`; alle
//! versionsabhängigen Stellen sind unten mit `proto.modern` bzw. über `proto.ids` markiert.
//!
//! Nur im Build mit `--features movement` kommt [`crate::movement`] dazu: gesteuerte Bewegung auf
//! Zuruf (`:go`, `:look`, `:home`). Ohne das Feature ist davon nichts einkompiliert.

use crate::auth::{self, Account};
use crate::console::Console;
use crate::net::{self, crypto, PacketReader, PacketWriter};
use crate::options::Options;
use crate::protocol::buf::{Reader, Writer};
use crate::protocol::CLIENT_COMMAND_RESPAWN;
use crate::protocol::{handshake, pack_status, ConfigIn, In, LoginIn, Protocol, State};
use crate::rules::{Event, Rules};
use crate::{net::dns, protocol::nbt};

use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Ab so vielen unquittierten **signierten** Nachrichten wird ungefragt quittiert – derselbe
/// Schwellwert wie im Vanilla-Client (der Server trennt erst bei 4096).
const ACK_THRESHOLD: u32 = 64;
/// Längengrenzen des Servers, gezählt wie dort: in UTF-16-Einheiten (siehe [`sanitize`]).
/// Chat-Nachricht 256, Befehl 32500.
const MAX_MESSAGE_CHARS: usize = 256;
const MAX_COMMAND_CHARS: usize = 32_500;
const DEFAULT_PORT: u16 = 25565;
/// Moderne Vanilla-Clients schließen jeden 50-ms-Client-Tick mit einem leeren Paket ab.
const CLIENT_TICK: Duration = Duration::from_millis(50);
/// Wie lange nach der Startposition höchstens auf den ersten Chunk-Stapel gewartet wird, bevor
/// die Welt trotzdem als geladen gilt.
///
/// Der Vanilla-Client wartet unbegrenzt (er zeigt „Welt wird geladen"). Ein Server, der gar keine
/// Chunks schickt – Limbo-Warteschlangen mancher Netzwerke tun das – ließe den Client damit
/// dauerhaft stumm: kein Chat, kein `--cmd`, keine Regel. Deshalb hier eine Notbremse.
const WORLD_LOAD_GRACE: Duration = Duration::from_secs(3);
/// So lange darf eine getippte Spielaktion auf das Ende der Ladephase warten – siehe
/// [`Shared::await_gameplay`]. Bewusst etwas mehr als [`WORLD_LOAD_GRACE`]: dann greift in
/// jedem Fall zuerst die Notbremse, und die Aktion geht doch noch raus.
pub(crate) const GAMEPLAY_WAIT: Duration = Duration::from_secs(4);
/// Der von Vanilla für die Client-Kennung definierte Payload-Kanal.
const BRAND_CHANNEL: &str = "minecraft:brand";
const BRAND_PREFIX: &str = "example.invalid";

/// So viele Zeilen dürfen höchstens warten. Mehr kann bei einem Mindestabstand von einer
/// Sekunde ohnehin niemand sinnvoll abarbeiten; ohne Grenze könnte eine dauerfeuernde
/// `--on chat:`-Regel den Speicher langsam volllaufen lassen.
const MAX_QUEUE: usize = 64;

/// Warteschlange einer Spielsitzung. Weltwechsel und beabsichtigte Transfers behalten
/// offene Zeilen; ein echter Verbindungsabbruch verwirft sie samt bereits entnommener Zeile.
struct Queue {
    epoch: AtomicU32,
    items: Mutex<VecDeque<String>>,
    signal: Condvar,
}

/// Spielerposition: x, y, z, Gierwinkel, Neigung.
pub type Position = (f64, f64, f64, f32, f32);

/// Nachbau von `ChunkBatchSizeCalculator` aus dem Vanilla-Client.
///
/// Der Server schickt Chunks nicht am Stück, sondern in Stapeln, und wartet nach jedem Stapel auf
/// eine Rückmeldung: `ServerboundChunkBatchReceived` mit der Zahl der Chunks, die der Client je
/// Tick verkraftet. Bleibt sie aus, zählt der `PlayerChunkSender` des Servers die offenen Stapel
/// hoch und **stellt die Chunk-Auslieferung ein**, sobald zehn unbestätigt sind. Genau das ist hier
/// vorher passiert: nach dem ersten Stapel kam nichts mehr.
///
/// Die Rechnung ist die des Originals, damit auch der gemeldete Wert derselbe ist: gemessene
/// Nanosekunden je Chunk, auf ein Drittel bis Dreifaches des bisherigen Mittels begrenzt, dann
/// gleitend gemittelt (das alte Mittel wiegt bis zu 49-fach). Der gewünschte Durchsatz ist das
/// Verhältnis eines Zeitbudgets von 7 ms zu diesem Mittel.
struct ChunkBatch {
    /// Zeitpunkt des `ChunkBatchStart`; `None`, solange kein Stapel offen ist.
    started: Option<Instant>,
    nanos_per_chunk: f64,
    /// Gewicht des bisherigen Mittels, wächst bis [`ChunkBatch::MAX_OLD_SAMPLES`].
    old_samples: f64,
}

impl ChunkBatch {
    /// Startannahme des Originals: 2 ms je Chunk – also anfangs 3,5 Chunks je Tick.
    const INITIAL_NANOS_PER_CHUNK: f64 = 2_000_000.0;
    const MAX_OLD_SAMPLES: f64 = 49.0;
    /// Ein einzelner Stapel darf das Mittel höchstens um den Faktor drei ziehen.
    const CLAMP: f64 = 3.0;
    /// Zeitbudget je Tick, das der Vanilla-Client fürs Einlesen von Chunks veranschlagt.
    const TICK_BUDGET_NANOS: f64 = 7_000_000.0;

    fn new() -> ChunkBatch {
        ChunkBatch {
            started: None,
            nanos_per_chunk: ChunkBatch::INITIAL_NANOS_PER_CHUNK,
            old_samples: 1.0,
        }
    }

    fn start(&mut self) {
        self.started = Some(Instant::now());
    }

    /// Stapel abgeschlossen. Ohne vorangegangenes `ChunkBatchStart` bleibt die Schätzung stehen –
    /// aus einem Zeitstempel von irgendwann vorhin ließe sich nichts Sinnvolles ableiten.
    fn finish(&mut self, size: i32) {
        let Some(started) = self.started.take() else {
            return;
        };
        if size <= 0 {
            return;
        }
        let measured = started.elapsed().as_nanos() as f64 / size as f64;
        let bounded = measured.clamp(
            self.nanos_per_chunk / ChunkBatch::CLAMP,
            self.nanos_per_chunk * ChunkBatch::CLAMP,
        );
        self.nanos_per_chunk =
            (self.nanos_per_chunk * self.old_samples + bounded) / (self.old_samples + 1.0);
        self.old_samples = (self.old_samples + 1.0).min(ChunkBatch::MAX_OLD_SAMPLES);
    }

    /// Gewünschte Chunks je Tick. Die Grenzen sind die, die der Server ohnehin anlegt; sie stehen
    /// hier, damit aus einer entarteten Messung kein `inf` aufs Kabel geht.
    fn desired_chunks_per_tick(&self) -> f32 {
        let desired = ChunkBatch::TICK_BUDGET_NANOS / self.nanos_per_chunk;
        if desired.is_finite() {
            (desired as f32).clamp(0.01, 64.0)
        } else {
            (ChunkBatch::TICK_BUDGET_NANOS / ChunkBatch::INITIAL_NANOS_PER_CHUNK) as f32
        }
    }
}

pub struct Shared {
    pub(crate) console: Console,
    /// Gewählte Protokollversion – fest für die ganze Laufzeit.
    pub(crate) proto: &'static Protocol,
    options: Options,
    account: Mutex<Account>,
    writer: Mutex<Option<PacketWriter>>,
    /// Ziel: (Host, Port, SRV-Auflösung erlaubt)
    target: Mutex<(String, u16, bool)>,
    cookies: Mutex<HashMap<String, Vec<u8>>>,
    queue: Queue,
    /// Makros aus `--on`. Ohne Regeln kostet das je Ereignis einen `is_empty()`-Test.
    rules: Rules,

    /// Weckt den Befehls-Planer, sobald die Verbindung endet – sonst schläft er blockierend bis
    /// zum nächsten Termin (0 % CPU im Leerlauf).
    idle: (Mutex<()>, Condvar),

    /// Zuletzt bekannte eigene Position; `None`, solange der Server noch keine geschickt hat.
    /// Der Netz-Thread schreibt sie bei jedem Teleport, der Bewegungs-Thread bei jedem Schritt.
    position: Mutex<Option<Position>>,
    /// Der erste Teleport ist nicht nur gelesen, sondern bereits bestätigt und gespiegelt.
    /// Trennt diese kurze Phase von `in_game`, das schon mit dem Login-Paket wahr wird.
    position_ready: AtomicBool,
    /// Ab 26.3: Teleport-Bestätigung für den nächsten Client-Tick. Der Vanilla-Client arbeitet
    /// Pakete erst im nächsten Frame ab (`Minecraft.runTick` → `processQueuedPackets`); eine
    /// sofortige Antwort traf den Server vor seinem ersten Spieler-Tick, wo
    /// `handleAcceptTeleportPacket` noch gegen Position 0 rechnet („moved too quickly"-Schleife).
    pending_accept: Mutex<Option<Writer>>,
    /// Bei modernen Protokollen ist auch der erste Chunk-Stapel vollständig empfangen und mit
    /// `PlayerLoaded` abgeschlossen. 1.21.1 kennt dieses Signal noch nicht.
    world_loaded: AtomicBool,
    /// Seit dem letzten Weltbeitritt kam mindestens ein vollständiger Chunk-Stapel an.
    chunk_batch_done: AtomicBool,
    /// `PlayerLoaded` wurde für diesen Weltbeitritt bereits gesendet – der Netz- und der
    /// Tick-Thread bewerben sich beide darum, senden darf genau einer.
    player_loaded_sent: AtomicBool,
    /// Beginn der laufenden Weltladephase (ms seit [`Shared::started`]); daran hängt die
    /// Notbremse [`WORLD_LOAD_GRACE`].
    world_load_since: AtomicU64,
    /// Bezugspunkt aller Millisekundenstempel oben. Ein `Instant` ist monoton – anders als die
    /// Systemuhr kann er nicht rückwärts springen.
    started: Instant,

    /// Die geladene Welt: Chunks und Blöcke – für die Physik, in der POV-Bauform auch fürs Bild.
    pub(crate) world: Mutex<crate::world::World>,
    /// Spielerphysik: Position, Geschwindigkeit, gedrückte Tasten.
    pub(crate) driver: crate::physics::driver::Driver,
    /// Gesteuerte Bewegung (`:go`, `:look`, `:home`).
    pub(crate) mover: crate::movement::Mover,

    /// Optionale Zustände (Anzeigetafel, Menüs, Gegenstände, POV, Tastenzustand, Anti-AFK).
    ///
    /// `extras` allein ist nur der gemeinsame Unterbau (Paket-IDs, Verteilerstelle); erst eine
    /// der darauf aufbauenden Funktionen liest hier etwas heraus.
    pub(crate) extras: crate::extras::Extras,

    /// Eigene Entitäts-Nummer aus dem Login-Paket. Das Schleich-Paket von 1.21.1 braucht sie.
    pub(crate) entity_id: std::sync::atomic::AtomicI32,

    pub(crate) in_game: AtomicBool,
    /// Diese Verbindung hat es bis in die Spielphase geschafft. Setzt den Reconnect-Backoff
    /// zurück: Wer eine Stunde drin war und dann fliegt, soll wieder nach der kurzen Grundzeit
    /// wiederkommen und nicht mit der langen Wartezeit von vorhin.
    reached_game: AtomicBool,
    /// Solange `true`, arbeiten Netz-, Sende- und Bewegungs-Threads weiter.
    pub(crate) running: AtomicBool,
    /// Trennung wurde von uns ausgelöst (Server-Transfer) -> ohne Backoff neu verbinden.
    intentional: AtomicBool,
    /// Zählt Verbindungs-Generationen: ein wartender Befehl erkennt daran, dass „seine"
    /// Verbindung längst tot ist, und feuert dann nicht mehr.
    pub(crate) generation: AtomicU32,
    /// Empfangene signierte Nachrichten, die der Server noch quittiert haben will.
    unacked: AtomicU32,
    /// Zeitstempel der letzten gesendeten Nachricht; der Server verlangt monotone Zeit.
    last_chat_ms: AtomicI64,
}

pub struct Client {
    shared: Arc<Shared>,
}

impl Client {
    /// Startet Netz- und Sender-Thread. Verbunden wird genau einmal; nur ein vom Server
    /// angeordneter Transfer darf das Ziel wechseln und eine neue Verbindung öffnen.
    pub fn new(console: Console, options: Options, account: Account) -> Client {
        let (host, port, srv) = parse_host(&options.server);
        let proto = options.protocol;
        let rules = Rules::new(options.rules.clone(), options.rule_cooldown_seconds);
        let shared = Arc::new(Shared {
            console,
            proto,
            account: Mutex::new(account),
            writer: Mutex::new(None),
            target: Mutex::new((host, port, srv)),
            cookies: Mutex::new(HashMap::new()),
            queue: Queue {
                epoch: AtomicU32::new(0),
                items: Mutex::new(VecDeque::new()),
                signal: Condvar::new(),
            },
            rules,
            idle: (Mutex::new(()), Condvar::new()),
            position: Mutex::new(None),
            position_ready: AtomicBool::new(false),
            pending_accept: Mutex::new(None),
            world_loaded: AtomicBool::new(false),
            chunk_batch_done: AtomicBool::new(false),
            player_loaded_sent: AtomicBool::new(false),
            world_load_since: AtomicU64::new(0),
            started: Instant::now(),
            world: Mutex::new(crate::world::World::new(proto.name, options.view_distance)),
            driver: crate::physics::driver::Driver::new(proto.legacy),
            mover: crate::movement::Mover::new(),
            extras: crate::extras::Extras::new(&options),
            entity_id: std::sync::atomic::AtomicI32::new(0),
            options,
            in_game: AtomicBool::new(false),
            reached_game: AtomicBool::new(false),
            running: AtomicBool::new(true),
            intentional: AtomicBool::new(false),
            generation: AtomicU32::new(0),
            unacked: AtomicU32::new(0),
            last_chat_ms: AtomicI64::new(0),
        });

        #[cfg(feature = "pov")]
        {
            // Erst den Viewer öffnen, dann die Ressourcen holen: Der Browser zeigt dadurch sofort
            // eine Seite und darauf, was gerade geladen wird, statt bis dahin gar nicht zu
            // antworten.
            crate::pov::start_web(&shared);
            crate::pov::start_assets(&shared);
        }
        // Wie oben, aber ohne den `pov`-Weltteil: derselbe Browser-Server, eigene Ressourcen-
        // Ablage (siehe [`crate::web::assets::WebAssets`]). Ohne `Pov` gibt es keinen
        // `web_running`-Merker – dieser Zweig läuft aber wie der obige genau einmal, beim
        // Anlegen des Clients.
        #[cfg(not(feature = "pov"))]
        {
            if let Some(address) = shared.options().pov_web {
                if let Err(error) = crate::web::start(&shared, address) {
                    shared.console.error(&format!("Browser-Menü: {}", error));
                }
            }
            crate::web::assets::load_for_web(&shared);
        }

        for (name, task) in [("afk-net", true), ("afk-sender", false)] {
            let owned = Arc::clone(&shared);
            let started = thread::Builder::new().name(name.into()).spawn(move || {
                if task {
                    net_loop(owned)
                } else {
                    sender_loop(owned)
                }
            });
            // Ohne diese beiden Threads gibt es keinen Client. Ein `expect` hätte hier mit
            // `panic = "abort"` nur einen Abbruch ohne lesbare Ursache hinterlassen.
            if let Err(e) = started {
                shared.console.error(&format!(
                    "Thread '{}' liess sich nicht starten: {}",
                    name, e
                ));
                shared.console.flush(Duration::from_secs(1));
                std::process::exit(1);
            }
        }

        Client { shared }
    }

    /// Nachricht oder Befehl in die rate-limitierte Warteschlange stellen.
    ///
    /// Auch dann, wenn der Beitritt noch läuft: Der Sender-Thread wartet ohnehin auf die
    /// Spielphase, bevor er eine Zeile hinausschickt. Sie hier wegzuwerfen hieße, dass ein Panel,
    /// das den Client startet und sofort eine Zeile schreibt, sie verliert – und zwar abhängig
    /// davon, wer von beiden gerade schneller war.
    pub fn send_chat(&self, input: &str) {
        if !self.shared.ready_for_gameplay() {
            self.shared
                .console
                .info("Noch nicht im Spiel – die Zeile geht raus, sobald der Beitritt steht.");
        }
        if !self.shared.queue.push(input.to_string()) {
            self.shared
                .console
                .warn("Sendewarteschlange voll – die älteste Zeile ist herausgefallen.");
        }
    }

    /// Örtlicher Befehl aus der Eingabeschleife (alles mit `:` vorn). Zusatzbefehle bekommen
    /// zuerst die Gelegenheit; alles Übrige geht an die Bewegung.
    pub fn local_command(&self, verb: &str, arg: &str) {
        // Ohne Zusatzteile und ohne Bewegung nimmt niemand das Argument entgegen.
        if matches!(verb, "help" | "hilfe" | "?") {
            return self.local_help();
        }
        if matches!(verb, "pos" | "position") {
            return match self.shared.position() {
                Some((x, y, z, yaw, pitch)) => self.shared.console.info(&format!(
                    "x={:.2}  y={:.2}  z={:.2}  ·  Blick {:.1}° / {:.1}°",
                    x, y, z, yaw, pitch
                )),
                None => self
                    .shared
                    .console
                    .error("Position noch unbekannt (nicht im Spiel?)."),
            };
        }
        if matches!(verb, "world" | "welt") {
            let (chunks, sections, bytes) = self.shared.world.lock().unwrap().footprint();
            let dimension = self.shared.world.lock().unwrap().dimension.name.clone();
            self.shared.console.info(&format!(
                "Welt {}: {} Chunks, {} gefüllte Abschnitte, {} kB",
                dimension,
                chunks,
                sections,
                bytes / 1024
            ));
            #[cfg(feature = "pov")]
            if let Some(assets) = self.shared.extras.pov.assets() {
                let (textures, texture_bytes, materials, elements) = assets.footprint();
                self.shared.console.info(&format!(
                    "Texturen: {} geladen ({} kB), {} Materialien mit {} Flächen",
                    textures,
                    texture_bytes / 1024,
                    materials,
                    elements
                ));
            }
            return;
        }
        if crate::extras::command(&self.shared, verb, arg) {
            return;
        }
        crate::movement::command(&self.shared, verb, arg);
    }

    /// `:help` liegt absichtlich hier statt im Bewegungsmodul: auch Items- und POV-Dateien haben
    /// örtliche Befehle, obwohl dort kein Byte Bewegungslogik einkompiliert ist.
    fn local_help(&self) {
        let console = &self.shared.console;
        console.print("");
        console.print(&console.paint(
            crate::console::BOLD,
            "  Befehle (alles mit ':' vorn, alles andere geht in den Chat)",
        ));
        for line in [
            ":go vor|zurück|links|rechts [blöcke]   laufen (Richtung relativ zum Blick)",
            ":look <gier> [neigung] · nord|ost|…    Kopf drehen",
            ":jump [richtung]                      springen",
            ":home set|on|off|go|delay|speed        Heimatposition",
            ":route rec|stop|add|del|go|clear       Wegpunkte zur Heimatposition",
            ":stop                                  Bewegung abbrechen",
        ] {
            console.print(&format!("    {}", line));
        }
        for line in crate::extras::help_lines() {
            console.print(&format!("    {}", line));
        }
        console.print("    :pos                                   Position anzeigen");
        console.print("    :world                                 geladene Welt und ihr Speicher");
        console.print(&console.paint(
            crate::console::GRAY,
            "    /befehl geht als Serverbefehl raus, alles andere als Chat.",
        ));
    }
}

impl Queue {
    /// `false` = die Warteschlange war voll und die älteste Zeile ist herausgefallen.
    fn push(&self, item: String) -> bool {
        let mut items = self.items.lock().unwrap();
        let dropped = items.len() >= MAX_QUEUE;
        if dropped {
            items.pop_front();
        }
        items.push_back(item);
        drop(items);
        self.signal.notify_one();
        !dropped
    }

    fn clear(&self) {
        let mut items = self.items.lock().unwrap();
        self.epoch.fetch_add(1, Ordering::SeqCst);
        items.clear();
    }
}

impl Shared {
    pub(crate) fn send(&self, packet: Writer) {
        let mut guard = self.writer.lock().unwrap();
        if let Some(writer) = guard.as_mut() {
            if writer.send(packet).is_err() {
                // `write_all` wiederholt nur Unterbrechungen selbst; was hier ankommt, ist ein
                // echter Fehler oder der Schreib-Zeitablauf – in beiden Fällen kann schon ein
                // halber Rahmen draußen sein. Weiterschreiben hieße Müll schicken, also Socket
                // zu und die Verbindung sauber neu aufbauen lassen.
                writer.shutdown();
                *guard = None;
            }
        }
    }

    /// Leeres Paket ohne kurzlebigen Paketpuffer senden (Tick-Ende und Player-Loaded).
    fn send_empty(&self, packet_id: i32) {
        let mut guard = self.writer.lock().unwrap();
        if let Some(writer) = guard.as_mut() {
            if writer.send_empty(packet_id).is_err() {
                writer.shutdown();
                *guard = None;
            }
        }
    }

    /// Senden und danach den Schreiber umschalten (Verschlüsselung, Kompression).
    ///
    /// Beides muss unter **einer** Sperre passieren. Zwischen zwei getrennten Aufrufen könnte ein
    /// anderer Thread ein Paket dazwischenschieben; das ginge dann im alten Zustand hinaus,
    /// während der Server bereits umgeschaltet hat – und die Verbindung wäre ohne erkennbaren
    /// Grund weg.
    fn send_then(&self, packet: Writer, configure: impl FnOnce(&mut PacketWriter)) {
        let mut guard = self.writer.lock().unwrap();
        if let Some(writer) = guard.as_mut() {
            if writer.send(packet).is_err() {
                writer.shutdown();
                *guard = None;
                return;
            }
            configure(writer);
        }
    }

    /// Startargumente – die Zusatzteile lesen daraus ihre eigenen Einstellungen.
    pub(crate) fn options(&self) -> &Options {
        &self.options
    }

    pub(crate) fn position(&self) -> Option<Position> {
        *self.position.lock().unwrap()
    }

    /// Spielpakete mit einer Spieleraktion sind erst sinnvoll, wenn Login **und** der erste
    /// autoritative Positionsteleport verarbeitet wurden.
    pub(crate) fn ready_for_gameplay(&self) -> bool {
        self.in_game.load(Ordering::Relaxed)
            && self.position_ready.load(Ordering::SeqCst)
            && self.world_loaded.load(Ordering::SeqCst)
    }

    /// Warten, bis Spielaktionen möglich sind – aber nur, wenn wir überhaupt schon im Spiel sind.
    ///
    /// Zwischen „Server hat uns abgesetzt" und „Umgebung ist da" liegen auf einem echten Server
    /// ein paar Dutzend Millisekunden. Eine in genau diesem Fenster getippte Aktion mit einer
    /// Fehlermeldung wegzuwerfen, ist für den, der sie getippt hat, nicht zu unterscheiden von
    /// „geht nicht" – deshalb wird die kurze Ladezeit hier abgewartet.
    ///
    /// Wer dagegen noch gar nicht im Spiel ist (Login, Konfigurationsphase), bekommt weiterhin
    /// sofort eine Absage: Da ist nichts, worauf sich zu warten lohnte.
    ///
    /// Nur Bauformen mit örtlichen Spielaktionen rufen das auf; der schlanke Client stellt seine
    /// Chat-Zeilen einfach in die Warteschlange, und die wartet ohnehin.
    pub(crate) fn await_gameplay(&self, timeout: Duration) -> bool {
        if self.ready_for_gameplay() {
            return true;
        }
        if !self.in_game.load(Ordering::SeqCst) {
            return false;
        }
        let deadline = Instant::now() + timeout;
        let mut guard = self.idle.0.lock().unwrap();
        loop {
            if self.ready_for_gameplay() {
                return true;
            }
            if !self.in_game.load(Ordering::SeqCst) || !self.running.load(Ordering::Relaxed) {
                return false;
            }
            let left = deadline.saturating_duration_since(Instant::now());
            if left.is_zero() {
                return false;
            }
            // Der Weckruf kommt aus dem Netz-Thread; die Obergrenze ist nur die Rückfallebene,
            // falls er genau zwischen Prüfung und Warten fällt.
            let (next, _) = self
                .idle
                .1
                .wait_timeout(guard, left.min(Duration::from_millis(50)))
                .unwrap();
            guard = next;
        }
    }

    /// Auf den ersten autoritativen Positionsteleport dieser Verbindung warten.
    ///
    /// Das ist eine Zustandswartezeit, kein Polling: Der Netz-Thread weckt sie nach dem
    /// bestätigten Teleport oder beim Verbindungsende. So gelangen keine Benutzeraktionen in
    /// die kurze Phase zwischen Spiel-Login und der vom Server gesetzten Startposition.
    fn wait_for_gameplay(&self, generation: u32) -> bool {
        let mut guard = self.idle.0.lock().unwrap();
        loop {
            if !self.valid(generation) {
                return false;
            }
            if self.ready_for_gameplay() {
                return true;
            }
            guard = self.idle.1.wait(guard).unwrap();
        }
    }

    pub(crate) fn set_position(&self, position: Position) {
        *self.position.lock().unwrap() = Some(position);
    }

    /// Millisekunden seit dem Programmstart – Bezugspunkt der Zeitstempel in [`Shared`].
    fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    /// Eine neue Welt beginnt: Beitritt, Respawn/Weltwechsel oder Rückkehr aus der
    /// Konfigurationsphase. Danach ist die Welt wieder „am Laden", und die Notbremse
    /// [`WORLD_LOAD_GRACE`] zählt von vorn.
    ///
    /// `forget_position` sagt, ob auch die bestätigte Startposition verfällt. Das darf nur, wo
    /// **sicher** eine neue kommt – nach einem Login also, denn dort schickt der Server sie
    /// unmittelbar hinterher. Bei einem Respawn bleibt sie stehen: Ein Plugin, das ein
    /// Respawn-Paket ohne Teleport schickt, ließe den Client sonst dauerhaft stumm, weil die
    /// Spielphase nie wieder erreicht würde.
    fn restart_world_load(&self, forget_position: bool) {
        if forget_position {
            self.position_ready.store(false, Ordering::SeqCst);
            *self.pending_accept.lock().unwrap() = None;
        }
        self.world_loaded.store(false, Ordering::SeqCst);
        self.chunk_batch_done.store(false, Ordering::SeqCst);
        self.player_loaded_sent.store(false, Ordering::SeqCst);
        self.world_load_since
            .store(self.elapsed_ms(), Ordering::SeqCst);
    }

    /// Wartet die Welt schon länger als [`WORLD_LOAD_GRACE`] auf ihren ersten Chunk-Stapel?
    fn world_load_overdue(&self) -> bool {
        let since = self.world_load_since.load(Ordering::SeqCst);
        self.elapsed_ms().saturating_sub(since) >= WORLD_LOAD_GRACE.as_millis() as u64
    }

    /// Chat-/Serverzeile anzeigen – und, falls es Chat-Regeln gibt, gegen sie halten.
    fn display(&self, text: &str) {
        self.console.chat(text);
        if self.rules.watches_chat() {
            self.run_rules(&Event::Chat(text));
        }
    }

    /// Ereignis melden (`--events`) und passende `--on`-Regeln auslösen.
    fn trigger(&self, event: Event, detail: &str) {
        self.console.event(event.name(), detail);
        self.run_rules(&event);
    }

    /// Aktionen der passenden Regeln in die normale Sendewarteschlange stellen – damit gelten
    /// für sie derselbe Mindestabstand und dieselben Längengrenzen wie für Eingaben.
    fn run_rules(&self, event: &Event) {
        for action in self.rules.fire(event) {
            self.console
                .info(&format!("Regel ({}): {}", event.name(), action));
            // Läuft die Warteschlange über, ist die älteste Zeile ohnehin die uninteressanteste.
            let _ = self.queue.push(action);
        }
    }

    /// Zeitstempel für die nächste Nachricht – nie kleiner als der vorige, sonst trennt der
    /// Server mit `out_of_order_chat`.
    fn next_chat_time(&self) -> i64 {
        let now = now_ms();
        let mut last = self.last_chat_ms.load(Ordering::Relaxed);
        loop {
            let stamp = now.max(last + 1);
            match self.last_chat_ms.compare_exchange_weak(
                last,
                stamp,
                Ordering::Relaxed,
                Ordering::Relaxed,
            ) {
                Ok(_) => return stamp,
                Err(current) => last = current,
            }
        }
    }

    /// Höchstens `duration` warten, ohne zu pollen. `false` = Verbindung beendet oder Programm
    /// beendet: der Aufrufer soll aufhören.
    pub(crate) fn wait(&self, duration: Duration, generation: u32) -> bool {
        let mut left = duration;
        let mut guard = self.idle.0.lock().unwrap();
        loop {
            if !self.valid(generation) {
                return false;
            }
            if left.is_zero() {
                return true;
            }
            let started = Instant::now();
            let (next, result) = self.idle.1.wait_timeout(guard, left).unwrap();
            guard = next;
            if result.timed_out() {
                return self.valid(generation);
            }
            left = left.saturating_sub(started.elapsed());
        }
    }

    fn valid(&self, generation: u32) -> bool {
        self.running.load(Ordering::Relaxed) && self.generation.load(Ordering::SeqCst) == generation
    }

    /// Alle Wartenden wecken (Verbindung beendet, Programmende).
    fn wake(&self) {
        let _guard = self.idle.0.lock().unwrap();
        self.idle.1.notify_all();
    }
}

// ===================== Sender-Thread =====================

/// Sendet Nachrichten mit Mindestabstand (gegen Spam-Kick) – schläft ansonsten blockierend.
///
/// Der Mindestabstand wird **vor** dem Senden abgewartet, nicht blind danach. Das ist derselbe
/// Abstand, macht aber zwei Dinge besser: Wer eine Minute lang nichts schickt, ist seine nächste
/// Zeile ohne Wartezeit los, und beim Beenden hängt der Thread nicht in einem Schlaf fest, den
/// niemand mehr braucht (mit `--chat-delay` sind das im Extremfall Tage).
fn sender_loop(shared: Arc<Shared>) {
    let delay = Duration::from_millis(shared.options.chat_min_delay_ms);
    let mut next_allowed = Instant::now();
    while shared.running.load(Ordering::Relaxed) {
        let (input, epoch) = {
            let mut items = shared.queue.items.lock().unwrap();
            loop {
                if !shared.running.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(item) = items.pop_front() {
                    break (item, shared.queue.epoch.load(Ordering::SeqCst));
                }
                items = shared.queue.signal.wait(items).unwrap();
            }
        };

        // Die entnommene Zeile bleibt während eines Weltwechsels im Besitz des Senders.
        // Ein `continue` der äußeren Schleife würde sie unbemerkt verlieren.
        loop {
            if !shared.running.load(Ordering::Relaxed)
                || shared.queue.epoch.load(Ordering::SeqCst) != epoch
            {
                break;
            }
            let generation = shared.generation.load(Ordering::SeqCst);
            if !shared.wait_for_gameplay(generation) {
                continue;
            }
            if !sleep_until(&shared, next_allowed) {
                return;
            }
            if shared.queue.epoch.load(Ordering::SeqCst) != epoch {
                break;
            }
            if shared.generation.load(Ordering::SeqCst) != generation
                || !shared.ready_for_gameplay()
            {
                continue;
            }
            // Erst unmittelbar vor dem Versand vorbereiten: einen Chat-Quittungs-Offset
            // dürfen wir erst verbrauchen, wenn die Nachricht tatsächlich gesendet wird.
            let Some(outgoing) = prepare(&input, shared.proto.legacy) else {
                break;
            };
            match outgoing {
                Outgoing::Command(command) => {
                    let mut w = Writer::packet(shared.proto.ids.sb_chat_command);
                    if shared.proto.legacy {
                        w.string(&format!("/{}", command));
                    } else {
                        w.string(&command);
                    }
                    shared.send(w);
                }
                Outgoing::Message(message) => {
                    let offset = shared.unacked.swap(0, Ordering::Relaxed);
                    shared.send(chat_packet(&shared, &message, offset));
                }
            }
            next_allowed = Instant::now() + delay;
            break;
        }
    }
}

/// Bis `until` schlafen, in kurzen Scheiben. `false` = das Programm endet gerade.
fn sleep_until(shared: &Shared, until: Instant) -> bool {
    loop {
        if !shared.running.load(Ordering::Relaxed) {
            return false;
        }
        let left = until.saturating_duration_since(Instant::now());
        if left.is_zero() {
            return true;
        }
        thread::sleep(left.min(Duration::from_millis(200)));
    }
}

enum Outgoing {
    Command(String),
    Message(String),
}

/// Eingabe in ein sendbares Paket übersetzen. `None`, wenn nach dem Säubern nichts übrig ist.
fn prepare(input: &str, legacy: bool) -> Option<Outgoing> {
    if let Some(command) = input.strip_prefix('/') {
        // In 1.8.9 laufen Befehle als gewöhnliche Chatnachricht; deren 100-Zeichen-Grenze zählt
        // den führenden Schrägstrich mit.
        let command = sanitize(command, if legacy { 99 } else { MAX_COMMAND_CHARS });
        return (!command.is_empty()).then_some(Outgoing::Command(command));
    }
    let message = sanitize(input, if legacy { 100 } else { MAX_MESSAGE_CHARS });
    (!message.is_empty()).then_some(Outgoing::Message(message))
}

/// Zeichen entfernen, die der Server verbietet (§, Steuerzeichen, DEL), und auf die
/// erlaubte Länge kürzen. Ohne das trennt er mit `illegal_chat_characters` bzw. der
/// Paket-Decoder bricht ab.
///
/// Gezählt wird in **UTF-16-Einheiten**, nicht in Zeichen. Das ist kein Detail:
///
/// * Der Server prüft die Länge mit Javas `String.length()`, und dort zählt jedes Zeichen über
///   U+FFFF – also jedes Emoji – als **zwei**.
/// * Sein Paketleser deckelt zusätzlich die Bytezahl auf das Dreifache der erlaubten Länge.
///
/// Mit Zeichen gezählt ging eine Zeile aus 200 Emoji glatt durch: 200 ≤ 256. Auf dem Kabel waren
/// das aber 400 Java-Zeichen und 800 Byte – über beiden Grenzen. Der Server brach schon beim
/// Dekodieren ab, und die Verbindung war weg, weil jemand Emoji geschrieben hat.
///
/// In UTF-16-Einheiten gezählt sind beide Grenzen zugleich eingehalten: Ein Zeichen der
/// Grundebene ist eine Einheit und höchstens drei Byte, jedes darüber zwei Einheiten und vier
/// Byte. Die Bytezahl bleibt damit immer unter dem Dreifachen der Einheitenzahl.
fn sanitize(input: &str, limit: usize) -> String {
    let mut out = String::with_capacity(input.len().min(limit * 3));
    let mut units = 0usize;
    for c in input
        .chars()
        .filter(|c| *c != '\u{a7}' && *c >= ' ' && *c != '\u{7f}')
    {
        let width = c.len_utf16();
        if units + width > limit {
            break;
        }
        units += width;
        out.push(c);
    }
    // `trim` kann nur kürzen – an den Grenzen ändert sich dadurch nichts.
    let trimmed = out.trim();
    if trimmed.len() == out.len() {
        out
    } else {
        trimmed.to_string()
    }
}

fn chat_packet(shared: &Shared, message: &str, offset: u32) -> Writer {
    let mut w = Writer::packet(shared.proto.ids.sb_chat);
    w.string(message);
    if shared.proto.legacy {
        return w;
    }
    w.i64(shared.next_chat_time());
    w.i64(0); // salt
    w.bool(false); // keine Signatur (unsignierter Chat)
    w.var_int(offset as i32);
    w.raw(&[0, 0, 0]); // Bitset der zuletzt gesehenen Nachrichten (20 Bit = 3 Byte)
    if shared.proto.modern {
        w.u8(0); // Prüfsumme 0 = „bitte nicht prüfen" (gibt es erst ab 1.21.11)
    }
    w
}

// ===================== Netz-Thread =====================

/// Ab dieser Verbindungsdauer gilt ein Transfer nicht mehr als Teil einer Weiterreich-Schleife.
/// Wer nach Minuten regulär auf einen anderen Unterserver geschickt wird, soll nicht wegen
/// irgendwelcher Transfers von vorhin ausgebremst werden.
const TRANSFER_CHAIN_RESET: Duration = Duration::from_secs(10);

fn net_loop(shared: Arc<Shared>) {
    // Ein Server, der uns im Kreis weiterreicht, darf keine Endlosschleife auf voller Last
    // erzeugen: je Transfer in Folge wird ein Stück länger gewartet.
    let mut transfers: u32 = 0;
    // Fehlversuche *in Folge* – ein erreichtes Spiel setzt sie zurück.
    let mut attempts: u32 = 0;
    // Die nächste Verbindung folgt einem Transfer: Der Handshake meldet dann `TRANSFER` statt
    // `LOGIN`, genau wie der Vanilla-Client (Server mit `accepts-transfers=false` lehnen ab).
    let mut after_transfer = false;
    while shared.running.load(Ordering::Relaxed) {
        let started = Instant::now();
        let result = run_connection(&shared, after_transfer);
        let lasted = started.elapsed();

        shared.in_game.store(false, Ordering::SeqCst);
        shared.restart_world_load(true);
        shared.generation.fetch_add(1, Ordering::SeqCst);
        *shared.writer.lock().unwrap() = None;
        *shared.position.lock().unwrap() = None;
        *shared.pending_accept.lock().unwrap() = None;
        // Wartende Befehls-Planer erkennen an der neuen Generation, dass sie fertig sind.
        shared.wake();
        shared.rules.reset();
        crate::movement::on_disconnect(&shared);
        shared.driver.reset();
        shared.world.lock().unwrap().clear();
        crate::extras::on_disconnect(&shared);

        let transferred = shared.intentional.swap(false, Ordering::SeqCst);
        after_transfer = transferred;
        if !transferred {
            match &result {
                Err(e) => {
                    shared.console.error(&format!("Getrennt: {}", e));
                    shared.console.event("disconnect", e);
                }
                Ok(()) => shared.console.event("disconnect", ""),
            }
            // Ein Kick beendet die Sitzung. Auch eine vom Sender bereits entnommene Zeile
            // wird durch die neue Queue-Epoche ungültig; niemals blind nach einem Kick senden.
            shared.queue.clear();
            shared.wake();
        }
        if transferred {
            // Server-Transfer ist ein ausdrücklicher Protokollwechsel, kein Reconnect nach
            // einem Kick. Deshalb wird nur in diesem Fall weiterverbunden.
            //
            // Gezählt werden nur *schnell* aufeinanderfolgende Transfers. Ohne diese Bedingung
            // summierte der Zähler alle Transfers der ganzen Laufzeit auf: Wer über Stunden
            // regulär zwischen Unterservern wechselt, bekam ab dem vierten Wechsel eine Warnung
            // und eine immer längere Wartezeit vor einem völlig normalen Vorgang.
            transfers = if lasted >= TRANSFER_CHAIN_RESET {
                1
            } else {
                transfers.saturating_add(1)
            };
            if transfers > 3 {
                let wait = Duration::from_millis(250 * u64::from(transfers.min(20)));
                shared.console.warn(&format!(
                    "{}. Transfer in Folge – warte {} ms.",
                    transfers,
                    wait.as_millis()
                ));
                thread::sleep(wait);
            }
            continue;
        }
        if let Some(policy) = shared.options.reconnect {
            // Eine Verbindung, die es bis ins Spiel geschafft hat, war keine Fehlkonfiguration:
            // Sie fängt wieder bei der Grundwartezeit an. Nur *aufeinanderfolgende* Fehlversuche
            // verlängern die Wartezeit, sonst stünde nach einem Tag Laufzeit die Obergrenze auch
            // hinter einem einzelnen Wackler.
            if shared.reached_game.swap(false, Ordering::SeqCst) {
                attempts = 0;
            }
            attempts = attempts.saturating_add(1);
            if policy.attempts != 0 && attempts > policy.attempts {
                shared.console.error(&format!(
                    "Nach {} Versuchen keine Verbindung – beende.",
                    policy.attempts
                ));
            } else {
                // Ein Kick ist das Ende der Sitzung: Die Cookies gehören zu ihr und dürfen nicht
                // in die nächste hinüberlaufen. Nur der Transfer oben behält sie – genau dafür
                // gibt es sie. Der Java-Client trennt beides seit jeher an derselben Stelle.
                shared.cookies.lock().unwrap().clear();
                let wait = backoff(&policy, attempts);
                shared.console.warn(&format!(
                    "Neuer Verbindungsversuch {} in {} s.",
                    attempts,
                    wait.as_secs()
                ));
                shared.console.event(
                    "reconnect",
                    &format!("versuch={} in={}", attempts, wait.as_secs()),
                );
                // Unterbrechbar warten: Ein Programmende soll nicht bis zu einer Minute lang in
                // einem Schlaf hängen, den niemand mehr braucht.
                let generation = shared.generation.load(Ordering::SeqCst);
                if shared.wait(wait, generation) {
                    continue;
                }
                // `wait` sagt „nein“ nur beim Programmende – dann fällt der Ablauf unten durch
                // und beendet den Prozess wie ohne Reconnect.
            }
        } else {
            shared
                .console
                .info("Verbindung beendet – kein automatischer Reconnect.");
        }
        shared.running.store(false, Ordering::SeqCst);
        shared.queue.signal.notify_all();
        shared.wake();
        // `main` kann blockierend auf Terminal-/Pipe-Eingabe warten. Nur den Netz-Thread zu
        // beenden ließe den Prozess deshalb nach einem Kick scheinbar weiterlaufen. Ein
        // Verbindungsabbruch ist für einen einzelnen AFK-Prozess ein Fehlerstatus; ein
        // Dienst/Panel kann ihn dadurch ebenfalls zuverlässig erkennen.
        //
        // Vorher aber die noch wartenden Zeilen beider Ströme hinausschreiben: Die Ausgabe geht
        // über eigene Threads (siehe [`Console::chat`]), und `exit` wartet auf keinen Thread.
        // Gerade die letzten Zeilen vor einem Kick sind die interessanten.
        shared.console.flush(Duration::from_secs(2));
        std::process::exit(1);
    }
}

/// Wartezeit vor dem `attempt`-ten Versuch in Folge: die Grundzeit, je vorangegangenem
/// Fehlversuch verdoppelt und auf die Obergrenze gedeckelt – dieselbe Rechnung wie im
/// Java-Client, damit ein Panel beiden dasselbe zutraut.
///
/// Der Schiebeschritt ist gedeckelt und die Multiplikation sättigend: Sonst wäre schon der
/// 64. Fehlversuch in Folge ein Überlauf, und aus einer Minute Wartezeit würde ein Sekundentakt
/// gegen einen Server, der ohnehin gerade nicht kann.
fn backoff(policy: &crate::options::Reconnect, attempt: u32) -> Duration {
    let doublings = attempt.saturating_sub(1).min(32);
    let seconds = policy
        .delay_seconds
        .saturating_mul(1u64 << doublings)
        .min(policy.max_backoff_seconds);
    Duration::from_secs(seconds)
}

fn run_connection(shared: &Arc<Shared>, after_transfer: bool) -> Result<(), String> {
    let (host, port, srv_allowed) = shared.target.lock().unwrap().clone();

    // SRV nur beim Standardport – exakt wie MCProtocolLib im Java-Client. Manche Server haben
    // überhaupt keinen A-Record und sind nur über SRV erreichbar.
    let (real_host, real_port) = if srv_allowed && port == DEFAULT_PORT {
        dns::resolve_srv(&host).unwrap_or((host.clone(), port))
    } else {
        (host.clone(), port)
    };

    let proxy = shared.options.proxy.as_ref();
    shared.console.note(&format!(
        "Verbinde zu {}:{} (MC {}){} ...",
        real_host,
        real_port,
        shared.proto.name,
        match proxy {
            Some(proxy) => format!(" über {}", proxy.describe()),
            None => String::new(),
        }
    ));
    shared.console.event(
        "connecting",
        &format!(
            "host={} port={} mc={}",
            real_host, real_port, shared.proto.name
        ),
    );

    let (mut reader, writer) =
        net::connect(&real_host, real_port, proxy).map_err(|e| e.to_string())?;
    *shared.writer.lock().unwrap() = Some(writer);
    start_client_ticks(shared)?;
    // Cookies werden hier bewusst **nicht** geleert.
    //
    // Genau dafür gibt es sie: Server A legt eines ab, schickt einen Transfer, und Server B
    // fragt es beim Login ab (so laufen Anmeldung und Warteschlange auf großen Netzwerken).
    // Der Vanilla-Client reicht sie aus demselben Grund an die neue Verbindung weiter. Sie
    // vorher zu löschen hieß: Server B bekam auf jede Abfrage „habe ich nicht" – und schickte
    // uns zurück oder gleich hinaus. Die einzige Verbindung, die dieser Client von sich aus neu
    // aufbaut, ist ohnehin die nach einem Transfer.

    // Handshake + Login-Start
    let (username, profile_id) = {
        let account = shared.account.lock().unwrap();
        (account.name.clone(), account.profile_id)
    };

    // Im Handshake steht die Adresse, unter der wir den Server ansprechen. Normalerweise ist das
    // das echte Ziel; mit --fakehost steht dort etwas anderes (manche Proxys leiten danach weiter,
    // und genau dafür gibt es die Option). Die TCP-Verbindung geht davon unberührt zum echten Ziel.
    let (handshake_host, handshake_port) = match &shared.options.fakehost {
        // Port 0 heißt „nicht angegeben" – dann bleibt der echte.
        Some((host, 0)) => (host.clone(), real_port),
        Some((host, port)) => (host.clone(), *port),
        None => (real_host.clone(), real_port),
    };

    let mut intention = Writer::packet(handshake::SB_INTENTION);
    intention.var_int(shared.proto.version);
    intention.string(&handshake_host);
    intention.u16(handshake_port);
    intention.var_int(if after_transfer && !shared.proto.legacy {
        handshake::INTENT_TRANSFER
    } else {
        handshake::INTENT_LOGIN
    });
    shared.send(intention);

    let mut hello = Writer::packet(shared.proto.ids.login_sb_hello);
    hello.string(&username);
    if !shared.proto.legacy {
        hello.uuid(&profile_id);
    }
    shared.send(hello);

    let mut session = Session {
        state: State::Login,
        joined: false,
        dead: false,
        chunk_batch: ChunkBatch::new(),
        last_signature: None,
        pending_keys: None,
        pending_threshold: None,
    };

    let mut packet: Vec<u8> = Vec::with_capacity(1024);
    loop {
        if !shared.running.load(Ordering::Relaxed) {
            return Ok(());
        }
        match reader.read_packet(&mut packet) {
            Ok(()) => {}
            Err(e) if e.kind() == io::ErrorKind::UnexpectedEof => {
                return Err("Verbindung vom Server geschlossen".to_string())
            }
            Err(e) => return Err(e.to_string()),
        }

        let mut r = Reader::new(&packet);
        let id = r.var_int().map_err(|e| e.to_string())?;

        let outcome = match session.state {
            State::Login => handle_login(shared, &mut session, &mut reader, id, &mut r),
            State::Configuration => handle_config(shared, &mut session, id, &mut r),
            State::Game => handle_game(shared, &mut session, id, &mut r),
        };

        match outcome {
            Ok(true) => return Ok(()), // sauber getrennt (Disconnect/Transfer)
            Ok(false) => {}
            Err(e) => return Err(e),
        }
    }
}

/// Der Client-Tick: derselbe 50-ms-Takt, in dem auch ein echter Client arbeitet.
///
/// Aufgaben, alle dem Vanilla-Client abgeschaut:
/// * **Teleport-Bestätigung** ab 26.3 (siehe [`Shared::pending_accept`]).
/// * **Physik und Bewegungspakete** ([`crate::physics::driver::tick`]), inklusive der
///   Positionserinnerung alle 20 Ticks.
/// * **Tick-Ende** – ab 1.21.2 schließt jeder Tick mit einem leeren Paket ab.
/// * **Notbremse der Ladephase** – falls der Server nie einen Chunk-Stapel schickt.
///
/// Der Thread lebt genau eine TCP-Verbindung lang (erkannt an der Generation) und taktet gegen
/// einen **festen Termin**. Vorher wurde nach jeder Runde volle 50 ms gewartet: der Takt lief
/// dadurch dauerhaft langsamer als 20 Hz, und zwar genau um die Arbeitszeit je Runde.
/// Verpasste Ticks werden nicht nachgeholt – nach einem Standby zwanzig Pakete auf einmal
/// nachzuschieben, täte kein echter Client.
fn start_client_ticks(shared: &Arc<Shared>) -> Result<(), String> {
    let generation = shared.generation.load(Ordering::SeqCst);
    let owned = Arc::clone(shared);
    thread::Builder::new()
        .name("afk-client-tick".into())
        .spawn(move || {
            let mut next = Instant::now() + CLIENT_TICK;
            loop {
                if !owned.wait(next.saturating_duration_since(Instant::now()), generation) {
                    return;
                }
                let now = Instant::now();
                next = if next + CLIENT_TICK > now {
                    next + CLIENT_TICK
                } else {
                    now + CLIENT_TICK // hinterher: Takt neu aufsetzen statt aufholen
                };
                client_tick(&owned);
            }
        })
        .map(|_| ())
        .map_err(|e| format!("Client-Tick-Thread liess sich nicht starten: {}", e))
}

/// Eine Runde des Client-Ticks.
fn client_tick(shared: &Arc<Shared>) {
    // Die Notbremse zuerst: ohne sie käme der Client auf einem Server ohne Chunks nie in die
    // Spielphase, und die beiden Pakete darunter gingen nie hinaus.
    send_pending_accept(shared);
    finish_world_load(shared);
    if !shared.ready_for_gameplay() {
        return;
    }
    crate::physics::driver::tick(shared);
    if shared.proto.ids.sb_client_tick_end >= 0 {
        shared.send_empty(shared.proto.ids.sb_client_tick_end);
    }
}

/// Die aufgeschobene Teleport-Bestätigung senden (siehe [`Shared::pending_accept`]); erst
/// danach gilt die Position als bestätigt.
fn send_pending_accept(shared: &Arc<Shared>) {
    let Some(accept) = shared.pending_accept.lock().unwrap().take() else {
        return;
    };
    shared.send(accept);
    shared.position_ready.store(true, Ordering::SeqCst);
    shared.wake();
}

struct Session {
    state: State,
    /// Erstes Login-Paket dieser TCP-Verbindung = echter Beitritt zum Proxy. Jedes weitere ist
    /// nur ein Wechsel zwischen Unterservern und zählt bewusst NICHT als neuer Beitritt.
    joined: bool,
    dead: bool,
    /// Durchsatzschätzung für die Chunk-Stapel. Sie gehört dem Netz-Thread allein – nur er liest
    /// Pakete, und nur aus ihnen speist sie sich.
    chunk_batch: ChunkBatch,
    /// Signatur der letzten gezählten Nachricht. Der Server zählt zwei gleiche Signaturen
    /// direkt hintereinander nur einmal – der Vanilla-Client macht es genauso.
    last_signature: Option<Box<[u8; 256]>>,
    /// Ab 26.4: ausgehandelte Schlüssel, bis `ApplyOptions` sie scharf schaltet.
    pending_keys: Option<crypto::SessionKeys>,
    /// Ab 26.4: angekündigte Kompressionsschwelle, bis ein Options-Bit sie einschaltet.
    pending_threshold: Option<i32>,
}

// ===================== Login-Phase =====================

/// Bits in `ApplyOptions`/`LoginFinished` ab 26.4 (`ConnectionOptionsPacket`).
const OPTION_ENCRYPTION: u8 = 1;
const OPTION_COMPRESSION: u8 = 2;

fn handle_login(
    shared: &Arc<Shared>,
    session: &mut Session,
    reader: &mut PacketReader,
    id: i32,
    r: &mut Reader,
) -> Result<bool, String> {
    let ids = &shared.proto.ids;
    let err = |e: io::Error| e.to_string();
    match shared.proto.login_incoming(id) {
        // Bis 26.3: RSA-Schlüssel, eigener AES-Schlüssel, der zugleich IV beider Richtungen ist.
        LoginIn::Hello => {
            let server_id = r.string().map_err(err)?;
            let public_key = r.byte_array().map_err(err)?;
            let challenge = r.byte_array().map_err(err)?;
            let should_authenticate = r.bool().unwrap_or(true);

            let mut secret = [0u8; 16];
            rand::rngs::OsRng.fill_bytes(&mut secret);
            if should_authenticate {
                let hash = auth::server_hash(&server_id, &secret, &public_key);
                let ipv4 = reader.local_addr().map_err(err)?.is_ipv4();
                shared.account.lock().unwrap().join_server(&hash, ipv4)?;
            }
            let key = RsaPublicKey::from_public_key_der(&public_key)
                .map_err(|e| format!("Server-Schlüssel unlesbar: {}", e))?;
            let mut rng = rand::rngs::OsRng;
            let encrypted_secret = key
                .encrypt(&mut rng, Pkcs1v15Encrypt, &secret)
                .map_err(|e| e.to_string())?;
            let encrypted_challenge = key
                .encrypt(&mut rng, Pkcs1v15Encrypt, &challenge)
                .map_err(|e| e.to_string())?;
            let mut w = Writer::packet(ids.login_sb_key);
            w.byte_array(&encrypted_secret);
            w.byte_array(&encrypted_challenge);
            // Ab jetzt ist alles verschlüsselt – Senden und Umschalten unter derselben Sperre.
            shared.send_then(w, |writer| writer.enable_encryption(&secret, &secret));
            reader.enable_encryption(&secret, &secret);
        }
        // Ab 26.4: X25519-Tausch. Verschlüsselt wird erst auf `ApplyOptions` hin.
        LoginIn::Key => {
            let salt = r.i64().map_err(err)?;
            let server_id = r.string().map_err(err)?;
            let server_key = r.byte_array().map_err(err)?;
            let authenticate = r.bool().map_err(err)?;
            let name = shared.account.lock().unwrap().name.clone();
            let keys = crypto::exchange(&crypto::Offer {
                salt,
                server_id: &server_id,
                server_key_der: &server_key,
                authenticate,
                client_name: &name,
                protocol: shared.proto.version,
            })?;
            if authenticate {
                let ipv4 = reader.local_addr().map_err(err)?.is_ipv4();
                shared
                    .account
                    .lock()
                    .unwrap()
                    .join_server(&keys.auth_digest, ipv4)?;
            }
            let mut w = Writer::packet(ids.login_sb_key);
            w.byte_array(&keys.client_key_der);
            w.raw(&keys.signature);
            shared.send(w);
            session.pending_keys = Some(keys);
        }
        LoginIn::Compression => {
            let threshold = r.var_int().map_err(err)?;
            if shared.proto.key_exchange {
                // Ab 26.4 gilt die Schwelle erst, wenn ein Options-Bit sie einschaltet.
                session.pending_threshold = Some(threshold);
            } else {
                reader.set_threshold(threshold);
                if let Some(writer) = shared.writer.lock().unwrap().as_mut() {
                    writer.set_threshold(threshold);
                }
            }
        }
        LoginIn::ApplyOptions => {
            let options = r.u8().map_err(err)?;
            let answer = Writer::packet(ids.login_sb_options_applied);
            apply_connection_options(shared, session, reader, options, answer)?;
        }
        LoginIn::Finished => {
            if shared.proto.legacy {
                // Protokoll 47 geht nach LoginSuccess unmittelbar in die Spielphase.
                session.state = State::Game;
                return Ok(false);
            }
            let answer = Writer::packet(ids.login_sb_acknowledged);
            if shared.proto.key_exchange {
                skip_game_profile(r).map_err(err)?;
                r.uuid().map_err(err)?; // Sitzungs-ID des Servers (nur Telemetrie)
                let options = r.u8().map_err(err)?;
                apply_connection_options(shared, session, reader, options, answer)?;
            } else {
                shared.send(answer);
            }
            session.state = State::Configuration;
            // Genau wie der Vanilla-Client: Kennung, (ab 26.4) leere Mod-Liste, Einstellungen.
            shared.send(client_brand(shared.proto, ids.config_sb_custom_payload));
            if shared.proto.key_exchange {
                shared.send(empty_mod_list(ids.config_sb_custom_payload));
            }
            shared.send(client_information(shared, ids.config_sb_client_information));
        }
        LoginIn::Disconnect => {
            // In der Login-Phase kommt der Grund noch als JSON-Text, nicht als NBT.
            let reason = match r.string() {
                Ok(json) => nbt::render_json(&json, shared.console.fmt()),
                Err(_) => "unbekannt".to_string(),
            };
            return Err(reason);
        }
        LoginIn::CustomQuery => {
            let transaction = r.var_int().map_err(err)?;
            let mut w = Writer::packet(ids.login_sb_custom_query_answer);
            w.var_int(transaction);
            w.bool(false); // wir verstehen die Anfrage nicht -> leere Antwort
            shared.send(w);
        }
        LoginIn::CookieRequest => {
            let key = r.string().map_err(err)?;
            shared.send(cookie_response(shared, ids.login_sb_cookie_response, &key));
        }
        LoginIn::Ignored => {}
    }
    Ok(false)
}

/// `ClientHandshakePacketListenerImpl.applyConnectionOptions` (26.4): eingehend umschalten,
/// Antwort noch im alten Zustand senden, dann ausgehend umschalten.
fn apply_connection_options(
    shared: &Arc<Shared>,
    session: &mut Session,
    reader: &mut PacketReader,
    options: u8,
    answer: Writer,
) -> Result<(), String> {
    let keys = if options & OPTION_ENCRYPTION != 0 {
        Some(
            session
                .pending_keys
                .take()
                .ok_or("Server schaltet Verschlüsselung ohne Schlüsseltausch ein")?,
        )
    } else {
        None
    };
    let threshold = if options & OPTION_COMPRESSION != 0 {
        Some(
            session
                .pending_threshold
                .ok_or("Server schaltet Kompression ohne Schwelle ein")?,
        )
    } else {
        None
    };
    if let Some(keys) = &keys {
        reader.enable_encryption(&keys.inbound_key, &keys.inbound_iv);
    }
    if let Some(threshold) = threshold {
        reader.set_threshold(threshold);
    }
    shared.send_then(answer, |writer| {
        if let Some(keys) = &keys {
            writer.enable_encryption(&keys.outbound_key, &keys.outbound_iv);
        }
        if let Some(threshold) = threshold {
            writer.set_threshold(threshold);
        }
    });
    Ok(())
}

/// `ByteBufCodecs.GAME_PROFILE`: UUID, Name, Eigenschaften (Name, Wert, optionale Signatur).
fn skip_game_profile(r: &mut Reader) -> io::Result<()> {
    r.uuid()?;
    r.string()?;
    let properties = r.var_int()?;
    if !(0..=16).contains(&properties) {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "zu viele Profil-Eigenschaften",
        ));
    }
    for _ in 0..properties {
        r.string()?;
        r.string()?;
        if r.bool()? {
            r.string()?;
        }
    }
    Ok(())
}

// ===================== Konfigurations-Phase =====================

/// So lange wartet die Bestätigung des Verhaltenskodex. Ein Mensch liest ihn erst; ein Client,
/// der in derselben Millisekunde antwortet, trifft auf Servern bis 26.3 ein Wettrennen im
/// Server (MC-312695: `handleAcceptCodeOfConduct` lief dort ohne Wechsel auf den
/// Hauptthread und konnte den Server abstürzen lassen). Mit Abstand ist die Aufgabe des
/// Servers sicher angelaufen, bevor die Antwort ankommt.
const CODE_OF_CONDUCT_DELAY: Duration = Duration::from_millis(1500);

fn handle_config(
    shared: &Arc<Shared>,
    session: &mut Session,
    id: i32,
    r: &mut Reader,
) -> Result<bool, String> {
    let ids = &shared.proto.ids;
    let err = |e: io::Error| e.to_string();
    match shared.proto.config_incoming(id) {
        ConfigIn::KeepAlive => {
            let ping = r.i64().map_err(err)?;
            let mut w = Writer::packet(ids.config_sb_keep_alive);
            w.i64(ping);
            shared.send(w);
        }
        ConfigIn::Ping => {
            let ping = r.i32().map_err(err)?;
            let mut w = Writer::packet(ids.config_sb_pong);
            w.i32(ping);
            shared.send(w);
        }
        ConfigIn::Finish => {
            shared.send(Writer::packet(ids.config_sb_finish));
            session.state = State::Game;
        }
        ConfigIn::SelectKnownPacks => {
            // Wir kennen keine Packs -> leere Liste. Der Server schickt daraufhin alle
            // Registry-Daten selbst.
            let mut w = Writer::packet(ids.config_sb_select_known_packs);
            w.var_int(0);
            shared.send(w);
        }
        ConfigIn::CodeOfConduct => accept_code_of_conduct(shared),
        ConfigIn::RegistryData => crate::extras::registry(shared, r),
        ConfigIn::ResourcePackPush => {
            let pack = r.uuid().map_err(err)?;
            acknowledge_resource_pack(shared, ids.config_sb_resource_pack, &pack);
        }
        ConfigIn::StoreCookie => store_cookie(shared, r)?,
        ConfigIn::CookieRequest => {
            let key = r.string().map_err(err)?;
            shared.send(cookie_response(shared, ids.config_sb_cookie_response, &key));
        }
        ConfigIn::Transfer => return transfer(shared, r),
        ConfigIn::Disconnect => return Err(disconnect_reason(shared, r)),
        ConfigIn::Ignored => {}
    }
    Ok(false)
}

/// Den Verhaltenskodex bestätigen – genau einmal je Anfrage, mit Abstand (siehe
/// [`CODE_OF_CONDUCT_DELAY`]) und nur, solange dieselbe Verbindung noch besteht.
fn accept_code_of_conduct(shared: &Arc<Shared>) {
    let generation = shared.generation.load(Ordering::SeqCst);
    let owned = Arc::clone(shared);
    let spawned = thread::Builder::new()
        .name("afk-conduct".into())
        .spawn(move || {
            if owned.wait(CODE_OF_CONDUCT_DELAY, generation) && owned.valid(generation) {
                owned.send(Writer::packet(
                    owned.proto.ids.config_sb_accept_code_of_conduct,
                ));
            }
        });
    if spawned.is_err() {
        // Ohne Thread lieber sofort bestätigen als gar nicht – sonst kommt niemand ins Spiel.
        shared.send(Writer::packet(
            shared.proto.ids.config_sb_accept_code_of_conduct,
        ));
    }
}

// ===================== Spiel-Phase =====================

fn handle_game(
    shared: &Arc<Shared>,
    session: &mut Session,
    id: i32,
    r: &mut Reader,
) -> Result<bool, String> {
    let game = &shared.proto.ids;
    match shared.proto.incoming(id) {
        // KeepAlive zuerst und sofort beantworten – das ist der eigentliche Schutz gegen
        // disconnect.timeout. Alles andere (Anzeige o. Ä.) kommt danach.
        In::KeepAlive => {
            let ping = if shared.proto.legacy {
                r.var_int().map_err(|e| e.to_string())? as i64
            } else {
                r.i64().map_err(|e| e.to_string())?
            };
            let mut w = Writer::packet(game.sb_keep_alive);
            if shared.proto.legacy {
                w.var_int(ping as i32);
            } else {
                w.i64(ping);
            }
            shared.send(w);
        }
        In::Ping => {
            let ping = r.i32().map_err(|e| e.to_string())?;
            let mut w = Writer::packet(game.sb_pong);
            w.i32(ping);
            shared.send(w);
        }
        // Chunk-Flusskontrolle. Sie steht bewusst hier und nicht in einer Ausbaustufe: auch der
        // schlanke Client, der keinen einzigen Chunk liest, muss den Stapel bestätigen – sonst
        // hört der Server auf zu senden und wartet auf eine Antwort, die nie kommt.
        In::ChunkBatchStart => session.chunk_batch.start(),
        In::ChunkBatchFinished => {
            let size = r.var_int().map_err(|e| e.to_string())?;
            acknowledge_chunk_batch(shared, session, size);
        }
        In::Login => {
            // Erstes Feld ist die eigene Entitäts-Nummer. Der schlanke Client braucht sie nicht,
            // das Schleich-Paket von 1.21.1 und die POV-Ansicht dagegen schon.
            let own_entity = r.i32().ok();
            if let Some(entity) = own_entity {
                shared
                    .entity_id
                    .store(entity, std::sync::atomic::Ordering::Relaxed);
            }
            #[cfg(feature = "pov")]
            if let Some(entity) = own_entity {
                crate::pov::login(shared, entity);
            }
            crate::world::login(shared, r);
            let first_join = !session.joined;
            session.joined = true;
            on_join(shared, session, first_join);
        }
        // Ein Respawn ist entweder die Wiedergeburt nach dem Tod oder ein Weltwechsel. Beides
        // unterscheiden wir am eigenen Zustand statt am Paketinhalt: dessen Aufbau ist in jeder
        // Version ein anderer, `session.dead` dagegen ist eindeutig – und kostet nichts.
        In::Respawn => {
            crate::world::respawn(shared, r);
            #[cfg(feature = "pov")]
            crate::pov::respawn(shared);
            // Ein Respawn setzt die Welt neu auf – der Server schickt gleich einen neuen
            // Chunk-Stapel. Der Vanilla-Client durchläuft die Ladephase deshalb ein zweites Mal
            // und meldet danach erneut `PlayerLoaded`. Die bestätigte Position bleibt dabei
            // stehen (siehe [`Shared::restart_world_load`]).
            shared.restart_world_load(false);
            if !session.dead {
                shared.console.info("Welt gewechselt.");
                shared.trigger(Event::World, "");
            }
        }
        In::SystemChat => {
            // Eine unlesbare Chat-Komponente ist kein Grund, die Verbindung zu beenden: Pakete
            // sind einzeln gerahmt, das nächste beginnt ohnehin an einer bekannten Stelle.
            // Vorher hat eine einzige seltsame Zeile eines Plugins den ganzen Client abgemeldet.
            let line = if shared.proto.legacy {
                r.string()
                    .ok()
                    .map(|json| nbt::render_json(&json, shared.console.fmt()))
            } else {
                nbt::read_network(r)
                    .ok()
                    .map(|component| nbt::render(&component, shared.console.fmt()))
            };
            if let Some(line) = line {
                if !line.trim().is_empty() {
                    shared.display(&line);
                }
            }
        }
        In::PlayerChat => {
            let chat = parse_player_chat(shared.proto.modern, shared.console.fmt(), r);
            // Nur SIGNIERTE Nachrichten führt der Server in seiner Quittungsliste. Zählte man
            // unsignierte mit (Plugin-/Proxy-Chat!), wäre unser Offset größer als das, was der
            // Server erwartet – und er trennt mit „chat_validation_failed".
            if let Some(signature) = chat.signature {
                if session.last_signature.as_deref() != Some(signature.as_ref()) {
                    session.last_signature = Some(signature);
                    shared.unacked.fetch_add(1, Ordering::Relaxed);
                }
            }
            if let Some(line) = chat.line {
                shared.display(&line);
            }
            maybe_acknowledge(shared);
        }
        In::Position => handle_position(shared, r)?,
        In::SetHealth => {
            let health = r.f32().map_err(|e| e.to_string())?;
            if health <= 0.0 && !session.dead {
                session.dead = true;
                shared.console.error("Gestorben – respawne automatisch.");
                let mut w = Writer::packet(game.sb_client_command);
                w.var_int(CLIENT_COMMAND_RESPAWN);
                shared.send(w);
                shared.trigger(Event::Death, "");
            } else if health > 0.0 {
                session.dead = false;
            }
        }
        In::ResourcePackPush => {
            if shared.proto.legacy {
                let _url = r.string().map_err(|e| e.to_string())?;
                let hash = r.string().map_err(|e| e.to_string())?;
                acknowledge_legacy_resource_pack(shared, game.sb_resource_pack, &hash);
            } else {
                let pack = r.uuid().map_err(|e| e.to_string())?;
                acknowledge_resource_pack(shared, game.sb_resource_pack, &pack);
            }
        }
        In::StartConfiguration => {
            shared.driver.reset();
            // Der Server holt uns zurück in die Konfigurationsphase (z. B. Ressourcen-Neuladen).
            shared.in_game.store(false, Ordering::SeqCst);
            shared.restart_world_load(true);
            // Wir verlassen die Welt: die alte Position gilt nicht mehr, und alles, was sich
            // darauf stützt (Bewegung, Live-Ansicht), soll das sofort merken.
            *shared.position.lock().unwrap() = None;
            shared.send(Writer::packet(game.sb_configuration_acknowledged));
            session.state = State::Configuration;
        }
        In::StoreCookie => store_cookie(shared, r)?,
        In::CookieRequest => {
            let key = r.string().map_err(|e| e.to_string())?;
            shared.send(cookie_response(shared, game.sb_cookie_response, &key));
        }
        In::Transfer => return transfer(shared, r),
        In::Disconnect => return Err(disconnect_reason(shared, r)),
        In::EntityMotion => entity_motion(shared, r).map_err(|e| e.to_string())?,
        In::Explode => explosion(shared, r).map_err(|e| e.to_string())?,
        In::Abilities => {
            let flags = r.u8().map_err(|e| e.to_string())?;
            let fly_speed = r.f32().map_err(|e| e.to_string())?;
            let _fov_speed = r.f32().map_err(|e| e.to_string())?;
            // Bit 2: fliegt gerade, Bit 4: darf fliegen (`ClientboundPlayerAbilitiesPacket`).
            shared
                .driver
                .abilities(flags & 2 != 0, flags & 4 != 0, fly_speed, 0.1);
        }
        In::GameEvent => {
            let event = r.u8().map_err(|e| e.to_string())?;
            let value = r.f32().map_err(|e| e.to_string())?;
            // Ereignis 3 = Spielmodus geändert; 3.0 = Zuschauer.
            if event == 3 {
                shared.driver.spectator(value == 3.0);
            }
        }

        // Alles Weitere gibt es nur in einer Zusatz-Bauform. Im schlanken Build existieren
        // diese Zweige gar nicht.
        other => crate::extras::incoming(shared, other, r),
    }
    Ok(false)
}

/// Beitritt verarbeiten. `first_join` = erstes Login-Paket dieser Verbindung, also der echte
/// Beitritt zum (Velocity/BungeeCord-)Proxy. Nur dann starten die Befehle.
fn on_join(shared: &Arc<Shared>, session: &mut Session, first_join: bool) {
    shared.in_game.store(true, Ordering::SeqCst);
    shared.reached_game.store(true, Ordering::SeqCst);
    // Der Server beginnt mit einer frischen Quittungsliste – unser Zähler muss mit.
    shared.unacked.store(0, Ordering::Relaxed);
    // Offene Befehle gehören weiterhin dieser Sitzung, auch beim Unterserverwechsel.
    // Die alte Position gilt nicht mehr: der Server setzt uns gleich neu ab (auch bei einem
    // Unterserver-Wechsel, dort ist es sogar eine andere Welt).
    *shared.position.lock().unwrap() = None;
    shared.restart_world_load(true);
    session.dead = false;
    session.last_signature = None;

    // Moderne Clients haben Einstellungen und Kennung schon in der Konfigurationsphase gemeldet;
    // 1.8.9 kennt sie nicht und schickt beides nach `JoinGame` (`NetHandlerPlayClient`).
    if shared.proto.legacy {
        shared.send(client_information(
            shared,
            shared.proto.ids.sb_client_information,
        ));
        shared.send(client_brand(
            shared.proto,
            shared.proto.ids.sb_custom_payload,
        ));
    }

    if first_join {
        let name = shared.account.lock().unwrap().name.clone();
        shared
            .console
            .ok(&format!("Verbunden und im Spiel als {}.", name));
        start_commands(shared);
        shared.trigger(Event::Join, &format!("name={}", name));
    } else {
        shared
            .console
            .info("Unterserver gewechselt (zählt nicht als neuer Beitritt).");
        // Ein anderer Unterserver ist eine andere Welt – für `--on world` zählt das.
        shared.trigger(Event::World, "grund=unterserver");
    }

    // Heimatposition nach JEDEM Beitritt – anders als bei den Befehlen zählt hier auch der
    // Unterserver-Wechsel, denn dort landen wir in einer anderen Welt an einer anderen Stelle.
    crate::movement::on_join(shared);
    crate::extras::on_join(shared);
}

/// Wiederkehrende Befehle (`--cmd`) nach dem echten Beitritt.
///
/// **Ein** Thread für alle Einträge: er schläft blockierend bis zum nächsten Termin (kein
/// Polling, 0 % CPU im Leerlauf) und beendet sich, sobald die Verbindung endet. Ohne `--cmd`
/// wird gar kein Thread gestartet.
fn start_commands(shared: &Arc<Shared>) {
    if shared.options.commands.is_empty() {
        return;
    }
    let generation = shared.generation.load(Ordering::SeqCst);
    let shared = Arc::clone(shared);
    thread::Builder::new()
        .name("afk-cmds".into())
        .spawn(move || {
            // Erst nach der Startposition beginnen: ein Null-Delay-Befehl darf nicht zwischen
            // Spiel-Login und Teleport in einem noch unvollständigen Weltzustand landen.
            if !shared.wait_for_gameplay(generation) {
                return;
            }
            let list = &shared.options.commands;
            let start = Instant::now();
            // Nächster Termin je Eintrag, gemessen ab dem Beitritt. `None` = erledigt.
            let mut due: Vec<Option<Duration>> = list
                .iter()
                .map(|c| Some(Duration::from_secs(c.delay_seconds)))
                .collect();

            loop {
                // Frühesten offenen Termin nehmen; gibt es keinen mehr, ist der Thread fertig.
                let Some((index, at)) = due
                    .iter()
                    .enumerate()
                    .filter_map(|(i, due)| due.map(|at| (i, at)))
                    .min_by_key(|(_, at)| *at)
                else {
                    return;
                };

                if !shared.wait(at.saturating_sub(start.elapsed()), generation) {
                    return;
                }
                // Ein Konfigurationswechsel kann die Spielphase kurz verlassen. Erst mit der
                // neuen Startposition darf ein fälliger Befehl wieder in die Sendewarteschlange.
                if !shared.wait_for_gameplay(generation) {
                    return;
                }
                if shared.generation.load(Ordering::SeqCst) != generation {
                    continue;
                }

                let command = &list[index];
                shared.console.info(&format!("Befehl: {}", command.command));
                shared.queue.push(command.command.clone());

                due[index] = match command.repeat_seconds {
                    0 => None,
                    repeat => {
                        // Verpasste Termine überspringen (z. B. nach einem Standby des Rechners),
                        // damit nicht mehrere Wiederholungen auf einmal nachfeuern.
                        let repeat = Duration::from_secs(repeat);
                        let elapsed = start.elapsed();
                        let mut next = at + repeat;
                        while next <= elapsed {
                            next += repeat;
                        }
                        Some(next)
                    }
                };
            }
        })
        .ok();
}

/// Teleport des Servers übernehmen und bestätigen.
///
/// Das Paketformat hat sich mit 1.21.2 geändert: vorher standen die Koordinaten vorn und die
/// Teleport-Nummer hinten, heute umgekehrt und mit Bewegungsvektor dazwischen. Die Bedeutung der
/// „relativ"-Bits ist in beiden gleich (0=x, 1=y, 2=z, 3=Gierwinkel, 4=Neigung).
fn handle_position(shared: &Arc<Shared>, r: &mut Reader) -> Result<(), String> {
    // Während Bestätigung und Rückspiegelung darf kein Tick-/Aktions-Thread dazwischenfunken.
    shared.position_ready.store(false, Ordering::SeqCst);
    let err = |e: io::Error| e.to_string();

    // Ab 1.21.2 bringt der Teleport eine Geschwindigkeit mit; davor bleibt sie auf relativen
    // Achsen erhalten und wird auf absoluten genullt.
    let mut motion = [0.0f64; 3];
    let (id, x, y, z, yaw, pitch, flags) = if shared.proto.modern {
        let id = r.var_int().map_err(err)?;
        let (x, y, z) = (
            r.f64().map_err(err)?,
            r.f64().map_err(err)?,
            r.f64().map_err(err)?,
        );
        motion = [
            r.f64().map_err(err)?,
            r.f64().map_err(err)?,
            r.f64().map_err(err)?,
        ];
        let yaw = r.f32().map_err(err)?;
        let pitch = r.f32().map_err(err)?;
        (id, x, y, z, yaw, pitch, r.i32().map_err(err)?)
    } else if shared.proto.legacy {
        let (x, y, z) = (
            r.f64().map_err(err)?,
            r.f64().map_err(err)?,
            r.f64().map_err(err)?,
        );
        let yaw = r.f32().map_err(err)?;
        let pitch = r.f32().map_err(err)?;
        (-1, x, y, z, yaw, pitch, r.u8().map_err(err)? as i32)
    } else {
        let (x, y, z) = (
            r.f64().map_err(err)?,
            r.f64().map_err(err)?,
            r.f64().map_err(err)?,
        );
        let yaw = r.f32().map_err(err)?;
        let pitch = r.f32().map_err(err)?;
        let flags = r.u8().map_err(err)? as i32;
        (r.var_int().map_err(err)?, x, y, z, yaw, pitch, flags)
    };

    let previous_position = shared.position();
    // Die erste Position setzt uns nur in die Welt. Jeder spätere Teleport ist dagegen eine
    // autoritative Servervorgabe (Korrektur, Plugin-Teleport, Portal): eine laufende lokale
    // Bewegung darf danach nicht weiter ihre alte Absicht gegen den Server durchsetzen.
    if previous_position.is_some() && shared.mover.stop() {
        shared
            .console
            .warn("Server hat die Position vorgegeben – laufende Bewegung gestoppt.");
    }
    let previous = previous_position.unwrap_or((0.0, 0.0, 0.0, 0.0, 0.0));
    let relative = |bit: i32, old: f64, value: f64| -> f64 {
        if flags & (1 << bit) != 0 {
            old + value
        } else {
            value
        }
    };
    let new = sane(
        (
            relative(0, previous.0, x),
            relative(1, previous.1, y),
            relative(2, previous.2, z),
            relative(3, previous.3 as f64, yaw as f64) as f32,
            relative(4, previous.4 as f64, pitch as f64) as f32,
        ),
        previous,
    );
    shared.set_position(new);
    let current = shared.driver.velocity();
    let velocity: [f64; 3] = std::array::from_fn(|axis| {
        let relative_position = flags & (1 << axis) != 0;
        if shared.proto.modern {
            // Bits 5–7: Geschwindigkeit relativ (`Relative.DELTA_*`).
            if flags & (1 << (axis + 5)) != 0 {
                current[axis] + motion[axis]
            } else {
                motion[axis]
            }
        } else if relative_position {
            current[axis]
        } else {
            0.0
        }
    });
    shared
        .driver
        .teleport([new.0, new.1, new.2], Some(velocity), new.3, new.4);

    // Teleport bestätigen (Pflicht, sonst Rubberband/Kick). Ab 26.3 trägt die Bestätigung die
    // übernommene Position selbst; davor spiegelt der Vanilla-Client sie mit einem eigenen
    // `MovePlayer.PosRot` zurück – mit „nicht am Boden" und ohne Wandkontakt (Flags 0).
    let ids = &shared.proto.ids;
    if shared.proto.teleport_ack_position {
        let mut accept = Writer::packet(ids.sb_accept_teleportation);
        accept.var_int(id);
        accept.f64(new.0);
        accept.f64(new.1);
        accept.f64(new.2);
        accept.f32(new.3);
        accept.f32(new.4);
        // Ein zweiter Teleport vor dem Tick ersetzt den ersten: Der Server wartet nur auf die
        // jüngste Nummer. `position_ready` setzt erst `send_pending_accept`.
        *shared.pending_accept.lock().unwrap() = Some(accept);
        crate::extras::on_position(shared, previous_position.is_none());
        return Ok(());
    } else {
        if id >= 0 {
            let mut accept = Writer::packet(ids.sb_accept_teleportation);
            accept.var_int(id);
            shared.send(accept);
        }
        let mut echo = Writer::packet(ids.sb_move_player_pos_rot);
        echo.f64(new.0);
        echo.f64(new.1);
        echo.f64(new.2);
        echo.f32(new.3);
        echo.f32(new.4);
        echo.u8(0);
        shared.send(echo);
    }
    shared.position_ready.store(true, Ordering::SeqCst);
    shared.wake();
    // Mit der Startposition kann die Ladephase enden – in 1.21.1 sofort, ab 1.21.4, sobald auch
    // der erste Chunk-Stapel da ist (den kann der Server schon vorher geschickt haben).
    finish_world_load(shared);
    crate::extras::on_position(shared, previous_position.is_none());
    Ok(())
}

/// Die Weltgrenze von Minecraft. Alles darüber hinaus ist keine Position mehr, sondern ein
/// Rechenfehler auf der Gegenseite.
pub(crate) const WORLD_LIMIT: f64 = 3.2e7;

/// Eine vom Server gemeldete Position brauchbar machen.
///
/// Sie geht ungeprüft in jede weitere Rechnung ein – und wandert als Bewegungspaket auch wieder
/// zurück zum Server. Ein `NaN` darin (aus einem kaputten Plugin oder einem übergelaufenen
/// relativen Teleport) steckte damit dauerhaft im Zustand: `:go` rechnete anschließend nur noch
/// mit `NaN`, die Live-Ansicht zeichnete nichts mehr, und der Server trennte wegen einer
/// unmöglichen Bewegung. Unbrauchbare Werte werden deshalb durch die bisherigen ersetzt.
fn sane(new: Position, previous: Position) -> Position {
    let coordinate = |value: f64, fallback: f64| {
        if value.is_finite() && value.abs() <= WORLD_LIMIT {
            value
        } else {
            fallback
        }
    };
    let angle = |value: f32, fallback: f32| if value.is_finite() { value } else { fallback };
    (
        coordinate(new.0, previous.0),
        coordinate(new.1, previous.1),
        coordinate(new.2, previous.2),
        angle(new.3, previous.3),
        // `PositionMoveRotation.calculateAbsolute` begrenzt die Neigung genauso.
        angle(new.4, previous.4).clamp(-90.0, 90.0),
    )
}

/// Ergebnis eines Spieler-Chat-Pakets.
struct PlayerChat {
    /// Signatur, falls die Nachricht signiert war – nur solche Nachrichten muss man quittieren.
    signature: Option<Box<[u8; 256]>>,
    /// Fertig gerenderte Anzeigezeile; `None`, wenn das Paket nicht lesbar war.
    line: Option<String>,
}

/// Spieler-Chat lesen. Wir prüfen keine Signaturen (wir sind nur Zuhörer), müssen die Felder
/// aber vollständig durchlaufen, um an Name und Inhalt zu kommen.
///
/// Der Kopf (bis einschließlich Signatur) wird getrennt gelesen: Selbst wenn der Rest des
/// Pakets nicht verstanden wird, bleibt die Quittungszählung dadurch korrekt.
fn parse_player_chat(modern: bool, format: nbt::Fmt, r: &mut Reader) -> PlayerChat {
    let mut chat = PlayerChat {
        signature: None,
        line: None,
    };
    // globalIndex gibt es erst ab 1.21.11; davor beginnt das Paket direkt mit dem Absender.
    if modern && r.var_int().is_err() {
        return chat;
    }
    if r.uuid().is_err() || r.var_int().is_err() {
        return chat; // Absender, index
    }
    match r.bool() {
        Ok(true) => match r.bytes(256) {
            Ok(bytes) => {
                let mut signature = Box::new([0u8; 256]);
                signature.copy_from_slice(bytes);
                chat.signature = Some(signature);
            }
            Err(_) => return chat,
        },
        Ok(false) => {}
        Err(_) => return chat,
    }
    chat.line = parse_chat_body(format, r);
    chat
}

fn parse_chat_body(format: nbt::Fmt, r: &mut Reader) -> Option<String> {
    let content = r.string().ok()?;
    r.i64().ok()?; // timestamp
    r.i64().ok()?; // salt

    // Der Server führt höchstens zwanzig zuletzt gesehene Nachrichten mit. Eine andere Zahl
    // heißt, dass der Lesezeiger schon falsch steht – dann lieber gar keine Zeile anzeigen als
    // eine aus zufällig gelesenen Bytes. (Vorher wurde die Zahl auf 20 gestaucht und einfach
    // weitergelesen.)
    let seen = r.var_int().ok()?;
    if !(0..=20).contains(&seen) {
        return None;
    }
    for _ in 0..seen {
        let id = r.var_int().ok()? - 1;
        if id == -1 {
            r.bytes(256).ok()?;
        }
    }

    let unsigned = if r.bool().ok()? {
        Some(nbt::read_network(r).ok()?)
    } else {
        None
    };

    // filterMask: 0 = ungefiltert, 1 = ganz gefiltert, 2 = teilweise. Nur im dritten Fall folgt
    // ein Bitfeld (Long-Array) – und genau das fehlte hier. Auf einem Server mit eingeschaltetem
    // Chatfilter stand der Lesezeiger danach mitten im Paket, und die Zeile fiel wortlos weg.
    match r.var_int().ok()? {
        0 | 1 => {}
        2 => {
            // Ein Bitfeld über höchstens 256 Zeichen braucht vier Longs; mehr ist keine Filterung.
            let longs = r.var_int().ok()?;
            if !(0..=64).contains(&longs) {
                return None;
            }
            r.bytes(longs as usize * 8).ok()?;
        }
        _ => return None,
    }

    // chatType: 0 = eingebettete Definition, sonst Registry-ID + 1.
    if r.var_int().ok()? == 0 {
        for _ in 0..2 {
            // translationKey
            r.string().ok()?;
            // Eine Chat-Verzierung kennt drei mögliche Parameter. Eine andere Zahl heißt, dass
            // der Lesezeiger schon falsch steht – dann lieber keine Zeile als eine erfundene.
            // (Vorher wurde die Zahl auf 16 gestaucht und einfach weitergelesen.)
            let params = r.var_int().ok()?;
            if !(0..=16).contains(&params) {
                return None;
            }
            for _ in 0..params {
                r.var_int().ok()?;
            }
            nbt::read_network(r).ok()?; // style
        }
    }

    let name = nbt::read_network(r).ok()?;
    let body = match unsigned {
        Some(component) => nbt::render(&component, format),
        None => nbt::render(&nbt::Nbt::Str(content), format),
    };
    Some(format!("<{}> {}", nbt::render(&name, format), body))
}

// ===================== Weltladephase =====================

/// Den Weltbeitritt abschließen, sobald beides steht: die vom Server gesetzte Startposition und
/// die Welt um uns herum.
///
/// Ab 1.21.4 gehört dazu ein `ServerboundPlayerLoaded`. Der Server hält den Spieler bis dahin in
/// einem geschützten Zustand (kein Schaden, keine Partikel) und setzt ihn erst nach 30 Sekunden
/// von sich aus frei – wer das Paket nie schickt, hängt also eine halbe Minute in der Schwebe.
/// 1.21.1 kennt das Paket noch nicht; dort ist die Welt mit der Startposition fertig.
///
/// Aufgerufen von beiden Seiten, die den Zustand ändern können: vom Netz-Thread (Position,
/// Chunk-Stapel) und vom Tick-Thread (Notbremse). Gesendet wird trotzdem genau einmal.
fn finish_world_load(shared: &Arc<Shared>) {
    if shared.world_loaded.load(Ordering::SeqCst) {
        return;
    }
    if !shared.in_game.load(Ordering::SeqCst) || !shared.position_ready.load(Ordering::SeqCst) {
        return;
    }
    let packet = shared.proto.ids.sb_player_loaded;
    if packet >= 0
        && !shared.chunk_batch_done.load(Ordering::SeqCst)
        && !shared.world_load_overdue()
    {
        return;
    }
    // Erst den Zuschlag holen, dann senden: sonst schickten Netz- und Tick-Thread das Paket
    // beide, und der Server bekäme zweimal dieselbe Meldung.
    if shared
        .player_loaded_sent
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        return;
    }
    if packet >= 0 {
        shared.send_empty(packet);
    }
    // Erst jetzt gilt die Welt als geladen – vorher dürfte eine Nachricht vor dem `PlayerLoaded`
    // hinausgehen, und das ist keine Reihenfolge, die ein echter Client je erzeugt.
    shared.world_loaded.store(true, Ordering::SeqCst);
    shared.wake();
}

/// Chunk-Stapel bestätigen. Das ist Flusskontrolle des Servers, keine POV-Funktion: ohne diese
/// Antwort hört der `PlayerChunkSender` nach zehn offenen Stapeln auf, überhaupt noch Chunks zu
/// schicken – siehe [`ChunkBatch`].
fn acknowledge_chunk_batch(shared: &Arc<Shared>, session: &mut Session, size: i32) {
    session.chunk_batch.finish(size);
    let mut w = Writer::packet(shared.proto.ids.sb_chunk_batch_received);
    w.f32(session.chunk_batch.desired_chunks_per_tick());
    shared.send(w);
    shared.chunk_batch_done.store(true, Ordering::SeqCst);
    finish_world_load(shared);
}

/// Empfangene Chat-Nachrichten regelmäßig quittieren (sonst kickt der Server irgendwann).
fn maybe_acknowledge(shared: &Arc<Shared>) {
    if shared.unacked.load(Ordering::Relaxed) >= ACK_THRESHOLD {
        let offset = shared.unacked.swap(0, Ordering::Relaxed);
        if offset > 0 {
            let mut w = Writer::packet(shared.proto.ids.sb_chat_ack);
            w.var_int(offset as i32);
            shared.send(w);
        }
    }
}

// ===================== gemeinsame Bausteine =====================

/// Spieleinstellungen wie ein echter Client (manche Server erwarten das vor dem Spielbeitritt).
///
/// Die gemeldete Sichtweite ist der wirksamste Sparhebel des ganzen Clients: ohne Live-Ansicht
/// wird kein einziger Chunk gelesen, der Server schickt sie aber trotzdem – und jedes Byte davon
/// muss entschlüsselt und entpackt werden. Mit der kleinsten erlaubten Sichtweite (2) fällt der
/// allergrößte Teil dieses Verkehrs einfach weg. Siehe [`crate::options::DEFAULT_VIEW_DISTANCE`].
fn client_information(shared: &Shared, packet_id: i32) -> Writer {
    let mut w = Writer::packet(packet_id);
    // Minecraft-Sprachkennungen sind durchgehend kleingeschrieben (`en_us`, `de_de`) – das sind
    // die Namen der Sprachdateien im Client. Ein „de_DE" nach BCP-47-Art gibt es dort nicht und
    // verrät damit auf den ersten Blick einen Fremdclient.
    w.string("de_de");
    w.u8(shared.options.view_distance.clamp(2, 32));
    w.var_int(0); // ChatVisibility: FULL
    w.bool(true); // Chatfarben
    w.u8(0x7F); // alle Skin-Teile sichtbar
    if shared.proto.legacy {
        return w;
    }
    w.var_int(1); // Haupthand: rechts
    w.bool(false); // Textfilterung
    w.bool(true); // in der Serverliste sichtbar
    if shared.proto.modern {
        w.var_int(0); // Partikel: ALL (gibt es erst ab 1.21.11)
    }
    w
}

/// Client-Kennung (`minecraft:brand`, in 1.8.9 `MC|Brand`).
///
/// Moderne Clients schicken sie einmal in der Konfigurationsphase direkt nach dem Login, 1.8.9
/// nach dem Beitritt. `packet_id` ist die Custom-Payload-ID der jeweiligen Phase.
fn client_brand(protocol: &Protocol, packet_id: i32) -> Writer {
    let mut w = Writer::packet(packet_id);
    w.string(if protocol.legacy {
        "MC|Brand"
    } else {
        BRAND_CHANNEL
    });
    w.string(&format!("{} {}", BRAND_PREFIX, protocol.name));
    w
}

/// Ab 26.4 meldet der Vanilla-Client nach der Kennung seine (leere) Mod-Liste.
fn empty_mod_list(packet_id: i32) -> Writer {
    let mut w = Writer::packet(packet_id);
    w.string("minecraft:mod_list");
    w.var_int(0);
    w
}

/// Ein Server-Resource-Pack für die Spielsitzung bestätigen.
///
/// Der AFK-Client zeichnet keine Servertexturen und muss die Datei daher nicht herunterladen.
/// Trotzdem erwarten Server mit verpflichtendem Pack denselben Abschluss wie vom Java-Client:
/// erst `ACCEPTED`, dann `SUCCESSFULLY_LOADED`. Ein ehrliches `DECLINED` klingt naheliegend, führt
/// dort aber unmittelbar zu `multiplayer.requiredTexturePrompt.disconnect` – der Bot darf dann
/// überhaupt nicht beitreten, obwohl keine seiner Funktionen das Pack benötigt.
fn acknowledge_resource_pack(shared: &Arc<Shared>, packet_id: i32, pack: &[u8; 16]) {
    for answer in resource_pack_answers(packet_id, pack) {
        shared.send(answer);
    }
}

fn resource_pack_answers(packet_id: i32, pack: &[u8; 16]) -> [Writer; 2] {
    let answer = |status| {
        let mut w = Writer::packet(packet_id);
        w.uuid(pack);
        w.var_int(status);
        w
    };
    [
        answer(pack_status::ACCEPTED),
        answer(pack_status::SUCCESSFULLY_LOADED),
    ]
}

fn acknowledge_legacy_resource_pack(shared: &Arc<Shared>, packet_id: i32, hash: &str) {
    for status in [pack_status::ACCEPTED, pack_status::SUCCESSFULLY_LOADED] {
        let mut answer = Writer::packet(packet_id);
        answer.string(hash);
        answer.var_int(status);
        shared.send(answer);
    }
}

/// So viele Cookies hebt der Client je Verbindung auf, und so groß darf eines höchstens sein.
///
/// Die Grenzen sind dieselben, die auch der Vanilla-Client zieht. Ohne sie könnte ein Server
/// beliebig viele Cookies unter immer neuen Namen ablegen und den Speicher so still volllaufen
/// lassen – geleert wird die Ablage nämlich erst beim nächsten Verbindungsaufbau.
const MAX_COOKIES: usize = 64;
const MAX_COOKIE_BYTES: usize = 5120;

fn store_cookie(shared: &Arc<Shared>, r: &mut Reader) -> Result<(), String> {
    let key = r.string().map_err(|e| e.to_string())?;
    let payload = r.byte_array().map_err(|e| e.to_string())?;
    if payload.len() > MAX_COOKIE_BYTES {
        return Err(format!(
            "Cookie '{}' ist zu groß ({} Byte)",
            key,
            payload.len()
        ));
    }
    let mut cookies = shared.cookies.lock().unwrap();
    // Ein bereits bekanntes Cookie darf immer aktualisiert werden; nur neue zählen gegen die Zahl.
    if cookies.len() < MAX_COOKIES || cookies.contains_key(&key) {
        cookies.insert(key, payload);
    }
    Ok(())
}

fn cookie_response(shared: &Arc<Shared>, packet_id: i32, key: &str) -> Writer {
    let mut w = Writer::packet(packet_id);
    w.string(key);
    match shared.cookies.lock().unwrap().get(key) {
        Some(payload) => {
            w.bool(true);
            w.byte_array(payload);
        }
        None => w.bool(false),
    }
    w
}

/// Server-Transfer: neues Ziel übernehmen und die Verbindung beenden – der Netz-Thread
/// verbindet sofort (ohne Backoff) zum neuen Ziel.
fn transfer(shared: &Arc<Shared>, r: &mut Reader) -> Result<bool, String> {
    let host = r.string().map_err(|e| e.to_string())?;
    // `as u16` hätte einen unsinnigen Port stillschweigend beschnitten – und wir wären dann
    // auf irgendeinen Port gelaufen, statt den Fehler zu nennen.
    let raw = r.var_int().map_err(|e| e.to_string())?;
    let port =
        u16::try_from(raw).map_err(|_| format!("Server-Transfer mit unmöglichem Port: {}", raw))?;
    if host.trim().is_empty() {
        return Err("Server-Transfer ohne Zieladresse".to_string());
    }
    shared
        .console
        .info(&format!("Server-Transfer zu {}:{}", host, port));
    shared
        .console
        .event("transfer", &format!("host={} port={}", host, port));
    *shared.target.lock().unwrap() = (host, port, false);
    shared.intentional.store(true, Ordering::SeqCst);
    Ok(true)
}

fn disconnect_reason(shared: &Arc<Shared>, r: &mut Reader) -> String {
    if shared.proto.legacy {
        return r
            .string()
            .map(|json| nbt::render_json(&json, shared.console.fmt()))
            .unwrap_or_else(|_| "unbekannt".to_string());
    }
    match nbt::read_network(r) {
        Ok(component) => nbt::render(&component, shared.console.fmt()),
        Err(_) => "unbekannt".to_string(),
    }
}

/// `host`, `host:port` oder `[::1]:port` zerlegen. Der dritte Rückgabewert sagt, ob eine
/// SRV-Auflösung versucht werden darf (nur wenn kein Port angegeben wurde).
fn parse_host(input: &str) -> (String, u16, bool) {
    let value = input.trim();

    if let Some(end) = value.strip_prefix('[').and_then(|v| v.find(']')) {
        let host = value[1..end + 1].to_string();
        let rest = &value[end + 2..];
        if let Some(port) = rest.strip_prefix(':') {
            return (host, port.trim().parse().unwrap_or(DEFAULT_PORT), false);
        }
        return (host, DEFAULT_PORT, false);
    }

    match value.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => (
            host.to_string(),
            port.trim().parse().unwrap_or(DEFAULT_PORT),
            false,
        ),
        _ => (value.to_string(), DEFAULT_PORT, true),
    }
}

fn now_ms() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|d| d.as_millis() as i64)
        .unwrap_or(0)
}

/// `SetEntityMotion`: Rückstoß für uns selbst. Ab 1.21.9 als „LpVec3“, davor drei Shorts in
/// 1/8000 Block je Tick.
fn entity_motion(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    let id = r.var_int()?;
    if id != shared.entity_id.load(Ordering::Relaxed) {
        return Ok(());
    }
    let velocity = if shared.proto.modern {
        read_lp_vec3(r)?
    } else {
        let mut v = [0.0; 3];
        for axis in &mut v {
            *axis = r.i16()? as f64 / 8000.0;
        }
        v
    };
    shared.driver.push(velocity, false);
    Ok(())
}

/// `LpVec3.read` aus dem 26.x-Client.
fn read_lp_vec3(r: &mut Reader) -> io::Result<[f64; 3]> {
    let lowest = r.u8()? as u64;
    if lowest == 0 {
        return Ok([0.0; 3]);
    }
    let middle = r.u8()? as u64;
    let highest = r.i32()? as u32 as u64;
    let buffer = highest << 16 | middle << 8 | lowest;
    let mut scale = lowest & 3;
    if lowest & 4 != 0 {
        scale |= (r.var_int()? as u32 as u64) << 2;
    }
    let unpack = |value: u64| ((value & 32767) as f64).min(32766.0) * 2.0 / 32766.0 - 1.0;
    let scale = scale as f64;
    Ok([
        unpack(buffer >> 3) * scale,
        unpack(buffer >> 18) * scale,
        unpack(buffer >> 33) * scale,
    ])
}

/// Explosionsrückstoß steht bis 26.3 im Explosionspaket selbst (`playerKnockback`); 26.4 schickt
/// ihn als eigenes `SetEntityMotion`.
fn explosion(shared: &Arc<Shared>, r: &mut Reader) -> io::Result<()> {
    // shortcut: nur 26.2/26.3 sind gegen den Codec geprüft; ältere Explosionsformate lassen den
    // Rückstoß aus (der Server korrigiert dann per Teleport).
    if !matches!(shared.proto.name, "26.2" | "26.3") {
        return Ok(());
    }
    for _ in 0..3 {
        r.f64()?;
    }
    r.f32()?;
    r.i32()?;
    if r.bool()? {
        let knockback = [r.f64()?, r.f64()?, r.f64()?];
        shared.driver.push(knockback, true);
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sitzungsende_verwirft_auch_bereits_entnommene_zeilen() {
        let queue = Queue {
            epoch: AtomicU32::new(0),
            items: Mutex::new(VecDeque::new()),
            signal: Condvar::new(),
        };
        queue.push("/zahlung".into());
        let (in_flight, epoch) = {
            let mut items = queue.items.lock().unwrap();
            (
                items.pop_front().unwrap(),
                queue.epoch.load(Ordering::SeqCst),
            )
        };
        queue.push("/alte-sitzung".into());
        queue.clear();
        queue.push("/neue-sitzung".into());
        assert_eq!(in_flight, "/zahlung");
        assert_ne!(epoch, queue.epoch.load(Ordering::SeqCst));
        assert_eq!(
            queue.items.lock().unwrap().pop_front().unwrap(),
            "/neue-sitzung"
        );
    }

    fn policy(delay: u64, max: u64) -> crate::options::Reconnect {
        crate::options::Reconnect {
            delay_seconds: delay,
            max_backoff_seconds: max,
            attempts: 0,
        }
    }

    /// HugoSMP und andere Server lassen den Spieler ohne ihr verpflichtendes Resource-Pack gar
    /// nicht erst hinein. Der AFK-Client braucht die Bilder nicht, muss den zweistufigen
    /// Vanilla-Ablauf aber vollständig quittieren – genau wie die Java-Bauform.
    #[test]
    fn verpflichtendes_resource_pack_wird_bestaetigt() {
        let pack = [0xAB; 16];
        let answers = resource_pack_answers(49, &pack);
        assert_eq!(&answers[0].data[..17], &[vec![49], pack.to_vec()].concat());
        assert_eq!(answers[0].data[17], pack_status::ACCEPTED as u8);
        assert_eq!(&answers[1].data[..17], &[vec![49], pack.to_vec()].concat());
        assert_eq!(answers[1].data[17], pack_status::SUCCESSFULLY_LOADED as u8);
    }

    /// Die Wartezeit verdoppelt sich je Fehlversuch und bleibt dann an der Obergrenze stehen.
    #[test]
    fn backoff_verdoppelt_bis_zur_obergrenze() {
        let p = policy(5, 60);
        let seconds = |attempt| backoff(&p, attempt).as_secs();
        assert_eq!(seconds(1), 5, "der erste Versuch wartet die Grundzeit");
        assert_eq!(seconds(2), 10);
        assert_eq!(seconds(3), 20);
        assert_eq!(seconds(4), 40);
        assert_eq!(seconds(5), 60, "ab hier deckelt die Obergrenze");
        assert_eq!(seconds(6), 60);
    }

    /// Ohne Deckelung wäre der 64. Fehlversuch in Folge ein Schiebe-Überlauf – und aus einer
    /// Minute Wartezeit würde ausgerechnet dann ein Sekundentakt, wenn ein Server lange weg ist.
    #[test]
    fn backoff_laeuft_auch_nach_sehr_vielen_versuchen_nicht_ueber() {
        let p = policy(5, 60);
        for attempt in [32u32, 33, 64, 1000, u32::MAX] {
            assert_eq!(
                backoff(&p, attempt).as_secs(),
                60,
                "Versuch {} muss an der Obergrenze bleiben",
                attempt
            );
        }
    }

    /// § und Steuerzeichen führen sonst zum Kick „illegal_chat_characters".
    #[test]
    fn verbotene_zeichen_fallen_weg() {
        assert_eq!(sanitize("hallo §cwelt", MAX_MESSAGE_CHARS), "hallo cwelt");
        assert_eq!(sanitize("a\u{7f}b\u{1}c", MAX_MESSAGE_CHARS), "abc");
        assert_eq!(sanitize("  abstand  ", MAX_MESSAGE_CHARS), "abstand");
        assert_eq!(sanitize("äöü", MAX_MESSAGE_CHARS), "äöü");
    }

    /// Der Server liest genau 256 Zeichen – längere Nachrichten lassen den Decoder abbrechen.
    #[test]
    fn nachricht_wird_auf_serverlaenge_gekuerzt() {
        let long = "ä".repeat(400);
        assert_eq!(
            sanitize(&long, MAX_MESSAGE_CHARS).chars().count(),
            MAX_MESSAGE_CHARS
        );
    }

    /// Der Server zählt mit Javas `String.length()` (Emoji = zwei) und deckelt die Bytezahl auf
    /// das Dreifache. Nach Zeichen gezählt ging eine Zeile aus 200 Emoji durch – auf dem Kabel
    /// waren das 400 Java-Zeichen und 800 Byte, und der Server brach beim Dekodieren ab.
    #[test]
    fn emoji_sprengen_die_laengengrenze_nicht_mehr() {
        for (text, name) in [
            ("\u{1F389}".repeat(400), "nur Emoji"),
            ("ä".repeat(400), "Umlaute"),
            ("a".repeat(400), "ASCII"),
            (
                format!("{}{}", "\u{1F389}".repeat(200), "a".repeat(200)),
                "gemischt",
            ),
        ] {
            let out = sanitize(&text, MAX_MESSAGE_CHARS);
            let units: usize = out.chars().map(char::len_utf16).sum();
            assert!(units <= MAX_MESSAGE_CHARS, "{}: {} Einheiten", name, units);
            // Und damit automatisch auch unter der Bytegrenze des Paketlesers.
            assert!(
                out.len() <= MAX_MESSAGE_CHARS * 3,
                "{}: {} Byte",
                name,
                out.len()
            );
            // Ein Emoji darf dabei nicht in der Mitte zerschnitten werden.
            assert!(
                out.chars().all(|c| c != char::REPLACEMENT_CHARACTER),
                "{}",
                name
            );
        }
        // Die Grenze wird auch wirklich ausgeschöpft, nicht vorsichtshalber unterboten.
        let voll = sanitize(&"\u{1F389}".repeat(400), MAX_MESSAGE_CHARS);
        assert_eq!(voll.chars().count(), MAX_MESSAGE_CHARS / 2);
    }

    #[test]
    fn befehle_und_nachrichten_werden_unterschieden() {
        assert!(matches!(prepare("/afk", false), Some(Outgoing::Command(c)) if c == "afk"));
        assert!(matches!(prepare("hallo", false), Some(Outgoing::Message(m)) if m == "hallo"));
        // Nichts Sendbares übrig: darf keinen Quittungs-Offset verbrauchen.
        assert!(prepare("/", false).is_none());
        assert!(prepare("   ", false).is_none());
        assert!(
            matches!(prepare(&format!("/{}", "a".repeat(200)), true), Some(Outgoing::Command(c)) if c.len() == 99)
        );
    }

    #[test]
    fn host_und_port_werden_zerlegt() {
        // Ohne Port darf (und muss) SRV gefragt werden, mit Port nicht.
        assert_eq!(
            parse_host("mc.example.net"),
            ("mc.example.net".into(), 25565, true)
        );
        assert_eq!(
            parse_host("mc.example.net:25566"),
            ("mc.example.net".into(), 25566, false)
        );
        assert_eq!(parse_host("[::1]:25566"), ("::1".into(), 25566, false));
    }

    /// Ein `NaN` aus einem kaputten Teleport blieb dauerhaft im Zustand stehen: jede weitere
    /// Rechnung lieferte danach wieder `NaN`, und der Server trennte wegen unmöglicher Bewegung.
    #[test]
    fn unmoegliche_positionen_werden_abgefangen() {
        let alt = (10.0, 64.0, -20.0, 90.0, 0.0);
        assert_eq!(
            sane((1.0, 2.0, 3.0, 4.0, 5.0), alt),
            (1.0, 2.0, 3.0, 4.0, 5.0)
        );
        assert_eq!(sane((f64::NAN, 2.0, 3.0, 4.0, 5.0), alt).0, alt.0);
        assert_eq!(sane((1.0, f64::INFINITY, 3.0, 4.0, 5.0), alt).1, alt.1);
        assert_eq!(sane((1.0, 2.0, 1e300, 4.0, 5.0), alt).2, alt.2);
        assert_eq!(sane((1.0, 2.0, 3.0, f32::NAN, 5.0), alt).3, alt.3);
        assert_eq!(sane((1.0, 2.0, 3.0, 4.0, f32::NAN), alt).4, alt.4);
        // Genau auf der Weltgrenze bleibt gültig.
        assert_eq!(sane((WORLD_LIMIT, 2.0, 3.0, 4.0, 5.0), alt).0, WORLD_LIMIT);
    }

    /// Netzwerk-NBT einer einfachen Textkomponente – so schickt der Server Namen und Inhalte.
    fn nbt_text(w: &mut Writer, text: &str) {
        w.u8(10); // TAG_Compound ohne Wurzelnamen
        w.u8(8); // TAG_String
        w.u16(4);
        w.raw(b"text");
        w.u16(text.len() as u16);
        w.raw(text.as_bytes());
        w.u8(0); // Ende des Compounds
    }

    /// Ein `ClientboundPlayerChatPacket`, wie es wirklich vom Netz kommt.
    ///
    /// `filter` ist die Filterangabe des Servers: 0 = ungefiltert, 1 = ganz gefiltert,
    /// 2 = teilweise – und nur bei 2 folgt ein Bitfeld.
    fn player_chat(
        modern: bool,
        content: &str,
        sender: &str,
        filter: i32,
        signed: bool,
    ) -> Vec<u8> {
        let mut w = Writer::default();
        if modern {
            w.var_int(7); // globalIndex (erst ab 1.21.11)
        }
        w.uuid(&[1u8; 16]); // Absender
        w.var_int(0); // index
        w.bool(signed);
        if signed {
            w.raw(&[0xAB; 256]);
        }
        w.string(content);
        w.i64(1); // timestamp
        w.i64(2); // salt
        w.var_int(0); // keine zuletzt gesehenen Nachrichten
        w.bool(false); // kein abweichender unsignierter Inhalt
        w.var_int(filter);
        if filter == 2 {
            w.var_int(1); // Bitfeld: ein Long
            w.i64(0);
        }
        w.var_int(1); // chatType: Registry-Nummer 0 (+1)
        nbt_text(&mut w, sender);
        w.bool(false); // kein targetName
        w.data
    }

    #[test]
    fn spieler_chat_wird_gelesen() {
        for modern in [false, true] {
            let data = player_chat(modern, "Hallo Welt", "Hugo", 0, false);
            let mut r = Reader::new(&data);
            let chat = parse_player_chat(modern, nbt::Fmt::Plain, &mut r);
            assert!(chat.signature.is_none(), "unsigniert, modern={}", modern);
            assert_eq!(
                chat.line.as_deref(),
                Some("<Hugo> Hallo Welt"),
                "modern={}",
                modern
            );
        }
    }

    /// Schickt der Server ein teilweise gefiltertes Paket, folgt hinter der Filterangabe ein
    /// Bitfeld. Es fehlte – der Lesezeiger stand danach mitten im Paket, und die Zeile fiel
    /// wortlos weg. Auf einem Server mit eingeschaltetem Chatfilter also jede Zeile.
    #[test]
    fn teilweise_gefilterter_chat_bleibt_lesbar() {
        for filter in [0, 1, 2] {
            let data = player_chat(true, "Hallo", "Hugo", filter, false);
            let mut r = Reader::new(&data);
            assert_eq!(
                parse_player_chat(true, nbt::Fmt::Plain, &mut r)
                    .line
                    .as_deref(),
                Some("<Hugo> Hallo"),
                "Filter {}",
                filter
            );
        }
    }

    /// Nur SIGNIERTE Nachrichten führt der Server in seiner Quittungsliste – zählt der Client
    /// unsignierte mit, trennt der Server mit `chat_validation_failed`.
    #[test]
    fn nur_signierte_nachrichten_tragen_eine_signatur() {
        let data = player_chat(true, "Hallo", "Hugo", 0, true);
        let mut r = Reader::new(&data);
        let chat = parse_player_chat(true, nbt::Fmt::Plain, &mut r);
        assert!(chat.signature.is_some());
        assert_eq!(chat.line.as_deref(), Some("<Hugo> Hallo"));
    }

    /// Ein abgeschnittenes oder unsinniges Paket darf keine erfundene Zeile ergeben – und
    /// erst recht nicht abstürzen (der Netz-Thread läuft mit `panic = "abort"`).
    #[test]
    fn kaputter_spieler_chat_ergibt_keine_zeile() {
        // Das letzte Feld (`targetName`) liest der Client gar nicht mehr – bis dahin muss aber
        // jede Verkürzung ohne Zeile ausgehen statt mit einer aus zufälligen Bytes.
        let voll = player_chat(true, "Hallo", "Hugo", 0, false);
        for kurz in 0..voll.len() - 1 {
            let mut r = Reader::new(&voll[..kurz]);
            let chat = parse_player_chat(true, nbt::Fmt::Plain, &mut r);
            assert!(
                chat.line.is_none(),
                "abgeschnitten bei {} ergab eine Zeile",
                kurz
            );
        }

        let mut seed = 0x1234_5678_9ABC_DEF0u64;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..4000 {
            let len = (random() % 512) as usize;
            let data: Vec<u8> = (0..len).map(|_| random() as u8).collect();
            for modern in [false, true] {
                let mut r = Reader::new(&data);
                let _ = parse_player_chat(modern, nbt::Fmt::Plain, &mut r);
            }
        }
    }

    /// Jede ausgewertete Spielphasen-ID muss sich eindeutig zurück auf ihre Paketart abbilden –
    /// zwei gleiche IDs würden still das falsche Paket verarbeiten.
    #[test]
    fn paket_ids_sind_je_version_eindeutig() {
        for p in crate::protocol::PROTOCOLS {
            let mut seen = std::collections::HashMap::new();
            for id in 0..256 {
                let kind = p.incoming(id);
                if kind == In::Ignored {
                    continue;
                }
                if let Some(other) = seen.insert(kind, id) {
                    panic!("{}: {:?} unter {} und {}", p.name, kind, other, id);
                }
            }
            for required in [In::KeepAlive, In::Login, In::Position, In::Disconnect] {
                assert!(
                    seen.contains_key(&required),
                    "{}: {:?} fehlt",
                    p.name,
                    required
                );
            }
        }
    }

    /// Die Schätzung muss der des Vanilla-Clients folgen: sie beginnt bei 3,5 Chunks je Tick,
    /// steigt bei schnellen Stapeln und lässt sich von einem einzelnen Ausreißer nicht mehr als
    /// um den Faktor drei ziehen.
    #[test]
    fn chunk_durchsatz_folgt_dem_vanilla_schaetzer() {
        let mut batch = ChunkBatch::new();
        assert!((batch.desired_chunks_per_tick() - 3.5).abs() < 1e-4);

        // Ohne vorangegangenes Start-Paket bleibt die Schätzung, wie sie war.
        batch.finish(25);
        assert!((batch.desired_chunks_per_tick() - 3.5).abs() < 1e-4);

        // Ein sehr schneller Stapel: der Ausreißer wird auf ein Drittel des Mittels begrenzt,
        // und das alte Mittel wiegt noch voll mit.
        batch.start();
        batch.finish(4096);
        let expected = (2_000_000.0 + 2_000_000.0 / 3.0) / 2.0;
        assert!(
            (batch.nanos_per_chunk - expected).abs() < 1.0,
            "{}",
            batch.nanos_per_chunk
        );

        // Ein leerer Stapel darf die Schätzung nicht anfassen (und nicht durch null teilen).
        let before = batch.nanos_per_chunk;
        batch.start();
        batch.finish(0);
        assert_eq!(batch.nanos_per_chunk, before);

        // Und der gemeldete Wert bleibt in den Grenzen, die der Server ohnehin anlegt.
        for _ in 0..200 {
            batch.start();
            batch.finish(4096);
        }
        let desired = batch.desired_chunks_per_tick();
        assert!((0.01..=64.0).contains(&desired), "{}", desired);
    }

    #[test]
    fn client_brand_hat_exakt_den_vanilla_payload_aufbau() {
        for protocol in crate::protocol::PROTOCOLS {
            let packet = client_brand(protocol, protocol.ids.sb_custom_payload);
            let mut r = Reader::new(&packet.data);
            assert_eq!(r.var_int().unwrap(), protocol.ids.sb_custom_payload);
            assert_eq!(
                r.string().unwrap(),
                if protocol.legacy {
                    "MC|Brand"
                } else {
                    BRAND_CHANNEL
                }
            );
            assert_eq!(
                r.string().unwrap(),
                format!("{} {}", BRAND_PREFIX, protocol.name)
            );
            assert_eq!(r.remaining(), 0);
        }
    }
}
