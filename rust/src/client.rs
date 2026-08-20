//! Der eigentliche AFK-Client: verbinden, verbunden bleiben, Chat senden/empfangen.
//!
//! Kick-Schutz ist rein protokollbasiert – genau das, was ein wartender Vanilla-Client tut:
//! `KeepAlive` sofort beantworten, `Ping`→`Pong`, Teleports bestätigen, erzwungene Resource-Packs
//! bestätigen (nicht laden), beim Beitritt `ClientInformation` senden, Cookies beantworten,
//! den Verhaltenskodex (ab 1.21.11) bestätigen. **Keine** Anti-AFK-Bewegung.
//!
//! Threads: 1× Netz (liest und antwortet), 1× Sender (rate-limitiert), sonst nichts. Im Leerlauf
//! blockieren beide – kein Timer, kein Polling, praktisch 0 % CPU.
//!
//! Die Protokollversion steckt in [`crate::proto::Protocol`] und kommt aus `--mc`; alle
//! versionsabhängigen Stellen sind unten mit `proto.modern` bzw. über `proto.game` markiert.
//!
//! Nur im Build mit `--features movement` kommt [`crate::movement`] dazu: gesteuerte Bewegung auf
//! Zuruf (`:go`, `:look`, `:home`). Ohne das Feature ist davon nichts einkompiliert.

use crate::auth::{self, Account};
use crate::buf::{Reader, Writer};
use crate::conn::{self, PacketReader, PacketWriter};
use crate::console::Console;
use crate::options::Options;
use crate::proto::CLIENT_COMMAND_RESPAWN;
use crate::proto::{config as cfg, handshake, login, pack_status, In, Protocol, State};
use crate::rules::{Event, Rules};
use crate::{dns, nbt};

use rand::RngCore;
use rsa::pkcs8::DecodePublicKey;
use rsa::{Pkcs1v15Encrypt, RsaPublicKey};
use std::collections::HashMap;
use std::collections::VecDeque;
use std::io;
use std::sync::atomic::{AtomicBool, AtomicI64, AtomicU32, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

/// Ab so vielen unquittierten **signierten** Nachrichten wird ungefragt quittiert – derselbe
/// Schwellwert wie im Vanilla-Client (der Server trennt erst bei 4096).
const ACK_THRESHOLD: u32 = 64;
/// Längengrenzen des Servers: Chat-Nachricht 256 Zeichen, Befehl 32500.
const MAX_MESSAGE_CHARS: usize = 256;
const MAX_COMMAND_CHARS: usize = 32_500;
const DEFAULT_PORT: u16 = 25565;

/// So viele Zeilen dürfen höchstens warten. Mehr kann bei einem Mindestabstand von einer
/// Sekunde ohnehin niemand sinnvoll abarbeiten; ohne Grenze könnte eine dauerfeuernde
/// `--on chat:`-Regel den Speicher langsam volllaufen lassen.
const MAX_QUEUE: usize = 64;

/// Warteschlange ausgehender Nachrichten – mit `clear()`, damit bei einem Server-Transfer keine
/// veralteten Zeilen nachträglich im Chat landen.
struct Queue {
    items: Mutex<VecDeque<String>>,
    signal: Condvar,
}

/// Spielerposition: x, y, z, Gierwinkel, Neigung.
pub type Position = (f64, f64, f64, f32, f32);

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

    /// Gesteuerte Bewegung (`:go`, `:look`, `:home`) – nur im Build mit `--features movement`.
    #[cfg(feature = "movement")]
    pub(crate) mover: crate::movement::Mover,

    /// Optionale Zustände (Anzeigetafel, Menüs, Gegenstände, POV, Tastenzustand, Anti-AFK).
    ///
    /// `extras` allein ist nur der gemeinsame Unterbau (Paket-IDs, Verteilerstelle); erst eine
    /// der darauf aufbauenden Funktionen liest hier etwas heraus.
    #[cfg(feature = "extras")]
    #[cfg_attr(
        not(any(
            feature = "board",
            feature = "menu",
            feature = "pov",
            feature = "state",
            feature = "antiafk"
        )),
        allow(dead_code)
    )]
    pub(crate) extras: crate::extras::Extras,

    /// Eigene Entitäts-Nummer aus dem Login-Paket. Das Schleich-Paket von 1.21.1 braucht sie.
    #[cfg(feature = "state")]
    pub(crate) entity_id: std::sync::atomic::AtomicI32,

    pub(crate) in_game: AtomicBool,
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
                items: Mutex::new(VecDeque::new()),
                signal: Condvar::new(),
            },
            rules,
            idle: (Mutex::new(()), Condvar::new()),
            position: Mutex::new(None),
            #[cfg(feature = "movement")]
            mover: crate::movement::Mover::new(),
            #[cfg(feature = "extras")]
            extras: crate::extras::Extras::new(&options),
            #[cfg(feature = "state")]
            entity_id: std::sync::atomic::AtomicI32::new(0),
            options,
            in_game: AtomicBool::new(false),
            running: AtomicBool::new(true),
            intentional: AtomicBool::new(false),
            generation: AtomicU32::new(0),
            unacked: AtomicU32::new(0),
            last_chat_ms: AtomicI64::new(0),
        });

        for (name, task) in [("afk-net", true), ("afk-sender", false)] {
            let shared = Arc::clone(&shared);
            thread::Builder::new()
                .name(name.into())
                .spawn(move || {
                    if task {
                        net_loop(shared)
                    } else {
                        sender_loop(shared)
                    }
                })
                .expect("Thread");
        }

        Client { shared }
    }

    /// Nachricht oder Befehl in die rate-limitierte Warteschlange stellen.
    pub fn send_chat(&self, input: &str) {
        if !self.shared.in_game.load(Ordering::Relaxed) {
            self.shared
                .console
                .error("Nicht verbunden – Nachricht nicht gesendet.");
            return;
        }
        if !self.shared.queue.push(input.to_string()) {
            self.shared
                .console
                .warn("Sendewarteschlange voll – die älteste Zeile ist herausgefallen.");
        }
    }

    /// Örtlicher Befehl aus der Eingabeschleife (alles mit `:` vorn). Zusatzbefehle bekommen
    /// zuerst die Gelegenheit; alles Übrige geht an die Bewegung.
    #[cfg(feature = "local")]
    pub fn local_command(&self, verb: &str, arg: &str) {
        // Ohne Zusatzteile und ohne Bewegung nimmt niemand das Argument entgegen.
        #[cfg(not(any(feature = "extras", feature = "movement")))]
        let _ = arg;
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
        #[cfg(feature = "extras")]
        if crate::extras::command(&self.shared, verb, arg) {
            return;
        }
        #[cfg(feature = "movement")]
        crate::movement::command(&self.shared, verb, arg);
        #[cfg(not(feature = "movement"))]
        self.shared
            .console
            .error("Unbekannter örtlicher Befehl. :help zeigt die verfügbaren Befehle.");
    }

    /// `:help` liegt absichtlich hier statt im Bewegungsmodul: auch Items- und POV-Dateien haben
    /// örtliche Befehle, obwohl dort kein Byte Bewegungslogik einkompiliert ist.
    #[cfg(feature = "local")]
    fn local_help(&self) {
        let console = &self.shared.console;
        console.print("");
        console.print(&console.paint(
            crate::console::BOLD,
            "  Befehle (alles mit ':' vorn, alles andere geht in den Chat)",
        ));
        #[cfg(feature = "movement")]
        for line in [
            ":go vor|zurück|links|rechts [blöcke]   laufen (Richtung relativ zum Blick)",
            ":look <gier> [neigung] · nord|ost|…    Kopf drehen",
            ":jump [richtung]  ·  :fall             springen · fallen lassen",
            ":home set|on|off|go|delay|speed        Heimatposition",
            ":route rec|stop|add|del|go|clear       Wegpunkte zur Heimatposition",
            ":stop                                  Bewegung abbrechen",
        ] {
            console.print(&format!("    {}", line));
        }
        #[cfg(feature = "extras")]
        for line in crate::extras::help_lines() {
            console.print(&format!("    {}", line));
        }
        console.print("    :pos                                   Position anzeigen");
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
        self.items.lock().unwrap().clear();
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

    /// Startargumente – die Zusatzteile lesen daraus ihre eigenen Einstellungen.
    #[cfg(feature = "pov")]
    pub(crate) fn options(&self) -> &Options {
        &self.options
    }

    pub(crate) fn position(&self) -> Option<Position> {
        *self.position.lock().unwrap()
    }

    pub(crate) fn set_position(&self, position: Position) {
        *self.position.lock().unwrap() = Some(position);
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
fn sender_loop(shared: Arc<Shared>) {
    while shared.running.load(Ordering::Relaxed) {
        let input = {
            let mut items = shared.queue.items.lock().unwrap();
            loop {
                if !shared.running.load(Ordering::Relaxed) {
                    return;
                }
                if let Some(item) = items.pop_front() {
                    break item;
                }
                items = shared.queue.signal.wait(items).unwrap();
            }
        };

        if !shared.in_game.load(Ordering::Relaxed) {
            continue; // gerade nicht verbunden: still verwerfen
        }

        // Erst säubern, dann senden: sonst könnte eine Nachricht wegfallen, deren
        // Quittungs-Offset schon verbraucht wäre.
        let Some(outgoing) = prepare(&input) else {
            continue;
        };
        match outgoing {
            // Befehle tragen KEINE Quittung (das Paket hat kein lastSeenMessages-Feld),
            // der Offset darf hier also nicht verbraucht werden.
            Outgoing::Command(command) => {
                let mut w = Writer::packet(shared.proto.game.sb_chat_command);
                w.string(&command);
                shared.send(w);
            }
            Outgoing::Message(message) => {
                let offset = shared.unacked.swap(0, Ordering::Relaxed);
                shared.send(chat_packet(&shared, &message, offset));
            }
        }

        thread::sleep(Duration::from_millis(shared.options.chat_min_delay_ms));
    }
}

enum Outgoing {
    Command(String),
    Message(String),
}

/// Eingabe in ein sendbares Paket übersetzen. `None`, wenn nach dem Säubern nichts übrig ist.
fn prepare(input: &str) -> Option<Outgoing> {
    if let Some(command) = input.strip_prefix('/') {
        let command = sanitize(command, MAX_COMMAND_CHARS);
        return (!command.is_empty()).then_some(Outgoing::Command(command));
    }
    let message = sanitize(input, MAX_MESSAGE_CHARS);
    (!message.is_empty()).then_some(Outgoing::Message(message))
}

/// Zeichen entfernen, die der Server verbietet (§, Steuerzeichen, DEL), und auf die
/// erlaubte Länge kürzen. Ohne das trennt er mit `illegal_chat_characters` bzw. der
/// Paket-Decoder bricht ab.
fn sanitize(input: &str, limit: usize) -> String {
    input
        .chars()
        .filter(|c| *c != '\u{a7}' && *c >= ' ' && *c != '\u{7f}')
        .take(limit)
        .collect::<String>()
        .trim()
        .to_string()
}

fn chat_packet(shared: &Shared, message: &str, offset: u32) -> Writer {
    let mut w = Writer::packet(shared.proto.game.sb_chat);
    w.string(message);
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
    while shared.running.load(Ordering::Relaxed) {
        let started = Instant::now();
        let result = run_connection(&shared);
        let lasted = started.elapsed();

        shared.in_game.store(false, Ordering::SeqCst);
        shared.generation.fetch_add(1, Ordering::SeqCst);
        *shared.writer.lock().unwrap() = None;
        *shared.position.lock().unwrap() = None;
        // Wartende Befehls-Planer erkennen an der neuen Generation, dass sie fertig sind.
        shared.wake();
        shared.rules.reset();
        #[cfg(feature = "movement")]
        crate::movement::on_disconnect(&shared);
        #[cfg(feature = "extras")]
        crate::extras::on_disconnect(&shared);

        match &result {
            Err(e) => {
                shared.console.error(&format!("Getrennt: {}", e));
                shared.console.event("disconnect", e);
            }
            Ok(()) => shared.console.event("disconnect", ""),
        }

        if shared.intentional.swap(false, Ordering::SeqCst) {
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
        shared
            .console
            .info("Verbindung beendet – kein automatischer Reconnect.");
        shared.running.store(false, Ordering::SeqCst);
        shared.queue.signal.notify_all();
        shared.wake();
        // `main` kann blockierend auf Terminal-/Pipe-Eingabe warten. Nur den Netz-Thread zu
        // beenden ließe den Prozess deshalb nach einem Kick scheinbar weiterlaufen. Ein
        // Verbindungsabbruch ist für einen einzelnen AFK-Prozess ein Fehlerstatus; ein
        // Dienst/Panel kann ihn dadurch ebenfalls zuverlässig erkennen.
        //
        // Vorher aber die noch wartenden Chatzeilen hinausschreiben: Der Chat geht über einen
        // eigenen Thread (siehe [`Console::chat`]), und `exit` wartet auf keinen Thread. Gerade
        // die letzten Zeilen vor einem Kick sind die interessanten.
        shared.console.flush_chat(Duration::from_secs(2));
        std::process::exit(1);
    }
}

fn run_connection(shared: &Arc<Shared>) -> Result<(), String> {
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
        conn::connect(&real_host, real_port, proxy).map_err(|e| e.to_string())?;
    *shared.writer.lock().unwrap() = Some(writer);
    shared.cookies.lock().unwrap().clear();

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
    intention.var_int(handshake::INTENT_LOGIN);
    shared.send(intention);

    let mut hello = Writer::packet(login::SB_HELLO);
    hello.string(&username);
    hello.uuid(&profile_id);
    shared.send(hello);

    let mut session = Session {
        state: State::Login,
        joined: false,
        dead: false,
        last_signature: None,
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

struct Session {
    state: State,
    /// Erstes Login-Paket dieser TCP-Verbindung = echter Beitritt zum Proxy. Jedes weitere ist
    /// nur ein Wechsel zwischen Unterservern und zählt bewusst NICHT als neuer Beitritt.
    joined: bool,
    dead: bool,
    /// Signatur der letzten gezählten Nachricht. Der Server zählt zwei gleiche Signaturen
    /// direkt hintereinander nur einmal – der Vanilla-Client macht es genauso.
    last_signature: Option<Box<[u8; 256]>>,
}

// ===================== Login-Phase =====================

fn handle_login(
    shared: &Arc<Shared>,
    session: &mut Session,
    reader: &mut PacketReader,
    id: i32,
    r: &mut Reader,
) -> Result<bool, String> {
    match id {
        login::CB_HELLO => {
            let server_id = r.string().map_err(|e| e.to_string())?;
            let public_key = r.byte_array().map_err(|e| e.to_string())?;
            let challenge = r.byte_array().map_err(|e| e.to_string())?;
            let should_authenticate = r.bool().unwrap_or(true);

            let mut secret = [0u8; 16];
            rand::rngs::OsRng.fill_bytes(&mut secret);

            if should_authenticate {
                let hash = auth::server_hash(&server_id, &secret, &public_key);
                shared.account.lock().unwrap().join_server(&hash)?;
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

            let mut w = Writer::packet(login::SB_KEY);
            w.byte_array(&encrypted_secret);
            w.byte_array(&encrypted_challenge);
            shared.send(w);

            // Ab jetzt ist alles verschlüsselt – erst senden, dann umschalten.
            if let Some(writer) = shared.writer.lock().unwrap().as_mut() {
                writer.enable_encryption(&secret);
            }
            reader.enable_encryption(&secret);
        }
        login::CB_COMPRESSION => {
            let threshold = r.var_int().map_err(|e| e.to_string())?;
            reader.set_threshold(threshold);
            if let Some(writer) = shared.writer.lock().unwrap().as_mut() {
                writer.set_threshold(threshold);
            }
        }
        login::CB_FINISHED => {
            shared.send(Writer::packet(login::SB_ACKNOWLEDGED));
            session.state = State::Configuration;
            shared.send(client_information(shared, cfg::SB_CLIENT_INFORMATION));
        }
        login::CB_DISCONNECT => {
            // In der Login-Phase kommt der Grund noch als JSON-Text, nicht als NBT.
            let reason = r.string().unwrap_or_else(|_| "unbekannt".to_string());
            return Err(reason);
        }
        login::CB_CUSTOM_QUERY => {
            let transaction = r.var_int().map_err(|e| e.to_string())?;
            let mut w = Writer::packet(login::SB_CUSTOM_QUERY_ANSWER);
            w.var_int(transaction);
            w.bool(false); // wir verstehen die Anfrage nicht -> leere Antwort
            shared.send(w);
        }
        login::CB_COOKIE_REQUEST => {
            let key = r.string().map_err(|e| e.to_string())?;
            shared.send(cookie_response(shared, login::SB_COOKIE_RESPONSE, &key));
        }
        _ => {}
    }
    Ok(false)
}

// ===================== Konfigurations-Phase =====================
//
// Die IDs dieser Phase sind in allen unterstützten Versionen gleich; nur den Verhaltenskodex
// gibt es erst ab 1.21.11 (in 1.21.1 hat die Phase so viele Pakete gar nicht).

fn handle_config(
    shared: &Arc<Shared>,
    session: &mut Session,
    id: i32,
    r: &mut Reader,
) -> Result<bool, String> {
    match id {
        cfg::CB_KEEP_ALIVE => {
            let ping = r.i64().map_err(|e| e.to_string())?;
            let mut w = Writer::packet(cfg::SB_KEEP_ALIVE);
            w.i64(ping);
            shared.send(w);
        }
        cfg::CB_PING => {
            let ping = r.i32().map_err(|e| e.to_string())?;
            let mut w = Writer::packet(cfg::SB_PONG);
            w.i32(ping);
            shared.send(w);
        }
        cfg::CB_FINISH => {
            shared.send(Writer::packet(cfg::SB_FINISH));
            session.state = State::Game;
        }
        cfg::CB_SELECT_KNOWN_PACKS => {
            // Wir kennen keine Packs -> leere Liste. Der Server schickt daraufhin alle
            // Registry-Daten selbst (die wir ohnehin nur überspringen).
            let mut w = Writer::packet(cfg::SB_SELECT_KNOWN_PACKS);
            w.var_int(0);
            shared.send(w);
        }
        cfg::CB_CODE_OF_CONDUCT if shared.proto.modern => {
            // Ab 1.21.11: ohne Bestätigung lässt der Server niemanden ins Spiel.
            shared.send(Writer::packet(cfg::SB_ACCEPT_CODE_OF_CONDUCT));
        }
        #[cfg(feature = "extras")]
        cfg::CB_REGISTRY_DATA => crate::extras::registry(shared, r),
        cfg::CB_RESOURCE_PACK_PUSH => {
            let pack = r.uuid().map_err(|e| e.to_string())?;
            acknowledge_resource_pack(shared, cfg::SB_RESOURCE_PACK, &pack);
        }
        cfg::CB_STORE_COOKIE => store_cookie(shared, r)?,
        cfg::CB_COOKIE_REQUEST => {
            let key = r.string().map_err(|e| e.to_string())?;
            shared.send(cookie_response(shared, cfg::SB_COOKIE_RESPONSE, &key));
        }
        cfg::CB_TRANSFER => return transfer(shared, r),
        cfg::CB_DISCONNECT => return Err(disconnect_reason(shared, r)),
        _ => {}
    }
    Ok(false)
}

// ===================== Spiel-Phase =====================

fn handle_game(
    shared: &Arc<Shared>,
    session: &mut Session,
    id: i32,
    r: &mut Reader,
) -> Result<bool, String> {
    let game = &shared.proto.game;
    match shared.proto.incoming(id) {
        // KeepAlive zuerst und sofort beantworten – das ist der eigentliche Schutz gegen
        // disconnect.timeout. Alles andere (Anzeige o. Ä.) kommt danach.
        In::KeepAlive => {
            let ping = r.i64().map_err(|e| e.to_string())?;
            let mut w = Writer::packet(game.sb_keep_alive);
            w.i64(ping);
            shared.send(w);
        }
        In::Ping => {
            let ping = r.i32().map_err(|e| e.to_string())?;
            let mut w = Writer::packet(game.sb_pong);
            w.i32(ping);
            shared.send(w);
        }
        In::Login => {
            // Erstes Feld ist die eigene Entitäts-Nummer. Der schlanke Client braucht sie nicht,
            // das Schleich-Paket von 1.21.1 und die POV-Ansicht dagegen schon.
            #[cfg(any(feature = "state", feature = "pov"))]
            let own_entity = r.i32().ok();
            #[cfg(feature = "state")]
            if let Some(entity) = own_entity {
                shared
                    .entity_id
                    .store(entity, std::sync::atomic::Ordering::Relaxed);
            }
            #[cfg(feature = "pov")]
            if let Some(entity) = own_entity {
                crate::pov::login(shared, entity, r);
            }
            let first_join = !session.joined;
            session.joined = true;
            on_join(shared, session, first_join);
        }
        // Ein Respawn ist entweder die Wiedergeburt nach dem Tod oder ein Weltwechsel. Beides
        // unterscheiden wir am eigenen Zustand statt am Paketinhalt: dessen Aufbau ist in jeder
        // Version ein anderer, `session.dead` dagegen ist eindeutig – und kostet nichts.
        In::Respawn => {
            #[cfg(feature = "pov")]
            crate::pov::respawn(shared, r);
            if !session.dead {
                shared.console.info("Welt gewechselt.");
                shared.trigger(Event::World, "");
            }
        }
        In::SystemChat => {
            let component = nbt::read_network(r).map_err(|e| e.to_string())?;
            let line = nbt::render(&component, shared.console.fmt());
            if !line.trim().is_empty() {
                shared.display(&line);
            }
        }
        In::PlayerChat => {
            let chat = parse_player_chat(shared, r);
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
            let pack = r.uuid().map_err(|e| e.to_string())?;
            acknowledge_resource_pack(shared, game.sb_resource_pack, &pack);
        }
        In::StartConfiguration => {
            // Der Server holt uns zurück in die Konfigurationsphase (z. B. Ressourcen-Neuladen).
            shared.in_game.store(false, Ordering::SeqCst);
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

        // Alles Weitere gibt es nur in einer Zusatz-Bauform. Im schlanken Build existieren
        // diese Zweige gar nicht.
        #[cfg(feature = "extras")]
        other => crate::extras::incoming(shared, other, r),

        #[cfg(not(feature = "extras"))]
        In::Ignored => {}
    }
    Ok(false)
}

/// Beitritt verarbeiten. `first_join` = erstes Login-Paket dieser Verbindung, also der echte
/// Beitritt zum (Velocity/BungeeCord-)Proxy. Nur dann starten die Befehle.
fn on_join(shared: &Arc<Shared>, session: &mut Session, first_join: bool) {
    shared.in_game.store(true, Ordering::SeqCst);
    // Der Server beginnt mit einer frischen Quittungsliste – unser Zähler muss mit.
    shared.unacked.store(0, Ordering::Relaxed);
    shared.queue.clear();
    // Die alte Position gilt nicht mehr: der Server setzt uns gleich neu ab (auch bei einem
    // Unterserver-Wechsel, dort ist es sogar eine andere Welt).
    *shared.position.lock().unwrap() = None;
    session.dead = false;
    session.last_signature = None;

    shared.send(client_information(
        shared,
        shared.proto.game.sb_client_information,
    ));

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
    #[cfg(feature = "movement")]
    crate::movement::on_join(shared);
    #[cfg(feature = "extras")]
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
                // Serverwechsel o. Ä.: kurz warten, statt ins Leere zu senden.
                if !shared.in_game.load(Ordering::Relaxed) {
                    if !shared.wait(Duration::from_secs(2), generation) {
                        return;
                    }
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
    let err = |e: io::Error| e.to_string();

    let (id, x, y, z, yaw, pitch, flags) = if shared.proto.modern {
        let id = r.var_int().map_err(err)?;
        let (x, y, z) = (
            r.f64().map_err(err)?,
            r.f64().map_err(err)?,
            r.f64().map_err(err)?,
        );
        // Bewegungsvektor interessiert uns nicht, muss aber übersprungen werden.
        for _ in 0..3 {
            r.f64().map_err(err)?;
        }
        let yaw = r.f32().map_err(err)?;
        let pitch = r.f32().map_err(err)?;
        (id, x, y, z, yaw, pitch, r.i32().map_err(err)?)
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

    let previous = shared.position().unwrap_or((0.0, 0.0, 0.0, 0.0, 0.0));
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

    // Teleport bestätigen (Pflicht, sonst Rubberband/Kick) und die vorgegebene Position EINMAL
    // zurückspiegeln – das ist die Antwort auf den Teleport, keine Eigenbewegung.
    let mut accept = Writer::packet(shared.proto.game.sb_accept_teleportation);
    accept.var_int(id);
    shared.send(accept);

    // Das Bewegungspaket ist in allen unterstützten Versionen bytegleich: ab 1.21.4 steht dort
    // ein Flag-Byte statt des alten `onGround`-Bool – 0x01 bedeutet in beiden „am Boden".
    let mut move_packet = Writer::packet(shared.proto.game.sb_move_player_pos_rot);
    move_packet.f64(new.0);
    move_packet.f64(new.1);
    move_packet.f64(new.2);
    move_packet.f32(new.3);
    move_packet.f32(new.4);
    move_packet.u8(0x01);
    shared.send(move_packet);
    Ok(())
}

/// Die Weltgrenze von Minecraft. Alles darüber hinaus ist keine Position mehr, sondern ein
/// Rechenfehler auf der Gegenseite.
const WORLD_LIMIT: f64 = 3.2e7;

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
        angle(new.4, previous.4),
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
fn parse_player_chat(shared: &Arc<Shared>, r: &mut Reader) -> PlayerChat {
    let mut chat = PlayerChat {
        signature: None,
        line: None,
    };
    // globalIndex gibt es erst ab 1.21.11; davor beginnt das Paket direkt mit dem Absender.
    if shared.proto.modern && r.var_int().is_err() {
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
    chat.line = parse_chat_body(shared, r);
    chat
}

fn parse_chat_body(shared: &Arc<Shared>, r: &mut Reader) -> Option<String> {
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
    r.var_int().ok()?; // filterMask

    // chatType: 0 = eingebettete Definition, sonst Registry-ID + 1.
    if r.var_int().ok()? == 0 {
        for _ in 0..2 {
            r.string().ok()?; // translationKey
            let params = r.var_int().ok()?.clamp(0, 16);
            for _ in 0..params {
                r.var_int().ok()?;
            }
            nbt::read_network(r).ok()?; // style
        }
    }

    let name = nbt::read_network(r).ok()?;
    let format = shared.console.fmt();
    let body = match unsigned {
        Some(component) => nbt::render(&component, format),
        None => nbt::render(&nbt::Nbt::Str(content), format),
    };
    Some(format!("<{}> {}", nbt::render(&name, format), body))
}

/// Empfangene Chat-Nachrichten regelmäßig quittieren (sonst kickt der Server irgendwann).
fn maybe_acknowledge(shared: &Arc<Shared>) {
    if shared.unacked.load(Ordering::Relaxed) >= ACK_THRESHOLD {
        let offset = shared.unacked.swap(0, Ordering::Relaxed);
        if offset > 0 {
            let mut w = Writer::packet(shared.proto.game.sb_chat_ack);
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
    w.string("de_DE");
    w.u8(shared.options.view_distance.clamp(2, 32));
    w.var_int(0); // ChatVisibility: FULL
    w.bool(true); // Chatfarben
    w.u8(0x7F); // alle Skin-Teile sichtbar
    w.var_int(1); // Haupthand: rechts
    w.bool(false); // Textfilterung
    w.bool(true); // in der Serverliste sichtbar
    if shared.proto.modern {
        w.var_int(0); // Partikel: ALL (gibt es erst ab 1.21.11)
    }
    w
}

/// Resource-Pack NICHT laden, aber bestätigen -> kein Kick bei erzwungenem Pack.
fn acknowledge_resource_pack(shared: &Arc<Shared>, packet_id: i32, pack: &[u8; 16]) {
    for status in [pack_status::ACCEPTED, pack_status::SUCCESSFULLY_LOADED] {
        let mut w = Writer::packet(packet_id);
        w.uuid(pack);
        w.var_int(status);
        shared.send(w);
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
        return Err(format!("Cookie '{}' ist zu groß ({} Byte)", key, payload.len()));
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
    let port = u16::try_from(raw)
        .map_err(|_| format!("Server-Transfer mit unmöglichem Port: {}", raw))?;
    if host.trim().is_empty() {
        return Err("Server-Transfer ohne Zieladresse".to_string());
    }
    shared
        .console
        .info(&format!("Server-Transfer zu {}:{}", host, port));
    *shared.target.lock().unwrap() = (host, port, false);
    shared.intentional.store(true, Ordering::SeqCst);
    Ok(true)
}

fn disconnect_reason(shared: &Arc<Shared>, r: &mut Reader) -> String {
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn befehle_und_nachrichten_werden_unterschieden() {
        assert!(matches!(prepare("/afk"), Some(Outgoing::Command(c)) if c == "afk"));
        assert!(matches!(prepare("hallo"), Some(Outgoing::Message(m)) if m == "hallo"));
        // Nichts Sendbares übrig: darf keinen Quittungs-Offset verbrauchen.
        assert!(prepare("/").is_none());
        assert!(prepare("   ").is_none());
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
        assert_eq!(sane((1.0, 2.0, 3.0, 4.0, 5.0), alt), (1.0, 2.0, 3.0, 4.0, 5.0));
        assert_eq!(sane((f64::NAN, 2.0, 3.0, 4.0, 5.0), alt).0, alt.0);
        assert_eq!(sane((1.0, f64::INFINITY, 3.0, 4.0, 5.0), alt).1, alt.1);
        assert_eq!(sane((1.0, 2.0, 1e300, 4.0, 5.0), alt).2, alt.2);
        assert_eq!(sane((1.0, 2.0, 3.0, f32::NAN, 5.0), alt).3, alt.3);
        assert_eq!(sane((1.0, 2.0, 3.0, 4.0, f32::NAN), alt).4, alt.4);
        // Genau auf der Weltgrenze bleibt gültig.
        assert_eq!(sane((WORLD_LIMIT, 2.0, 3.0, 4.0, 5.0), alt).0, WORLD_LIMIT);
    }

    /// Die Paket-IDs jeder Version müssen sich eindeutig zuordnen lassen – ein Tippfehler in der
    /// Tabelle (zwei gleiche IDs) würde sonst still das falsche Paket verarbeiten.
    #[test]
    fn paket_ids_sind_je_version_eindeutig() {
        for p in crate::proto::PROTOCOLS {
            let g = &p.game;
            #[allow(unused_mut)]
            let mut ids = vec![
                g.cb_cookie_request,
                g.cb_disconnect,
                g.cb_keep_alive,
                g.cb_login,
                g.cb_ping,
                g.cb_player_chat,
                g.cb_player_position,
                g.cb_resource_pack_push,
                g.cb_respawn,
                g.cb_set_health,
                g.cb_start_configuration,
                g.cb_store_cookie,
                g.cb_system_chat,
                g.cb_transfer,
            ];
            // Zusatz-IDs müssen sich in dieselbe Menge einreihen: eine Überschneidung mit einem
            // der obigen Pakete würde still das falsche Paket verarbeiten.
            #[cfg(feature = "extras")]
            {
                let e = &p.extra;
                #[cfg(feature = "menu")]
                ids.extend_from_slice(&[
                    e.cb_container_close,
                    e.cb_container_set_content,
                    e.cb_container_set_slot,
                    e.cb_open_screen,
                ]);
                #[cfg(feature = "board")]
                ids.extend_from_slice(&[
                    e.cb_reset_score,
                    e.cb_set_display_objective,
                    e.cb_set_objective,
                    e.cb_set_player_team,
                    e.cb_set_score,
                ]);
                #[cfg(feature = "pov")]
                ids.extend_from_slice(&[
                    e.cb_level_chunk,
                    e.cb_forget_level_chunk,
                    e.cb_block_update,
                    e.cb_section_blocks_update,
                    e.cb_chunk_batch_finished,
                    e.cb_add_entity,
                    e.cb_remove_entities,
                    e.cb_move_entity_pos,
                    e.cb_move_entity_pos_rot,
                    e.cb_teleport_entity,
                ]);
            }
            for (i, a) in ids.iter().enumerate() {
                for b in &ids[i + 1..] {
                    assert_ne!(a, b, "doppelte Paket-ID in {}", p.name);
                }
                assert_ne!(p.incoming(*a), In::Ignored, "unbekannte ID in {}", p.name);
            }
        }
    }
}
