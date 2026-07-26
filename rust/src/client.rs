//! Der eigentliche AFK-Client: verbinden, verbunden bleiben, Chat senden/empfangen.
//!
//! Kick-Schutz ist rein protokollbasiert – genau das, was ein wartender Vanilla-Client tut:
//! `KeepAlive` sofort beantworten, `Ping`→`Pong`, Teleports bestätigen, erzwungene Resource-Packs
//! bestätigen (nicht laden), beim Beitritt `ClientInformation` senden, Cookies beantworten,
//! den Verhaltenskodex (neu in 26.1) bestätigen. **Keine** Anti-AFK-Bewegung.
//!
//! Threads: 1× Netz (liest und antwortet), 1× Sender (rate-limitiert), sonst nichts. Im Leerlauf
//! blockieren beide – kein Timer, kein Polling, praktisch 0 % CPU.

use crate::auth::{self, Account};
use crate::buf::{Reader, Writer};
use crate::config::Config;
use crate::conn::{self, PacketReader, PacketWriter};
use crate::console::{Console, Link};
use crate::proto::{config as cfg, game, handshake, login, pack_status, State};
use crate::proto::{MINECRAFT_VERSION, PROTOCOL_VERSION};
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

/// Warteschlange ausgehender Nachrichten – mit `clear()`, damit nach einem Reconnect keine
/// veralteten Zeilen nachträglich im Chat landen.
struct Queue {
    items: Mutex<VecDeque<String>>,
    signal: Condvar,
}

pub struct Shared {
    console: Console,
    config: Mutex<Config>,
    account: Mutex<Account>,
    writer: Mutex<Option<PacketWriter>>,
    /// Ziel: (Host, Port, SRV-Auflösung erlaubt)
    target: Mutex<(String, u16, bool)>,
    cookies: Mutex<HashMap<String, Vec<u8>>>,
    queue: Queue,

    /// Weckt den Befehls-Planer, sobald die Verbindung endet oder die Liste sich ändert –
    /// sonst schläft er blockierend bis zum nächsten Termin (0 % CPU im Leerlauf).
    idle: (Mutex<()>, Condvar),

    in_game: AtomicBool,
    running: AtomicBool,
    shutting_down: AtomicBool,
    /// Trennung wurde von uns ausgelöst (:reconnect, :server, Kontowechsel) -> ohne Backoff neu verbinden.
    intentional: AtomicBool,
    /// Zählt Verbindungs-Generationen: ein wartender Befehl erkennt daran, dass „seine"
    /// Verbindung längst tot ist, und feuert dann nicht mehr.
    generation: AtomicU32,
    /// Zählt Änderungen an der Befehlsliste – ein laufender Planer beendet sich dadurch.
    command_epoch: AtomicU32,
    /// Empfangene signierte Nachrichten, die der Server noch quittiert haben will.
    unacked: AtomicU32,
    /// Zeitstempel der letzten gesendeten Nachricht; der Server verlangt monotone Zeit.
    last_chat_ms: AtomicI64,
}

/// Ausweis eines Befehls-Planers: gilt nur für „seine" Verbindung und „seine" Befehlsliste.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Ticket {
    generation: u32,
    epoch: u32,
}

pub struct Client {
    shared: Arc<Shared>,
}

impl Client {
    pub fn new(console: Console, config: Config, account: Account, host: String, port: u16, srv: bool) -> Client {
        let shared = Arc::new(Shared {
            console,
            config: Mutex::new(config),
            account: Mutex::new(account),
            writer: Mutex::new(None),
            target: Mutex::new((host, port, srv)),
            cookies: Mutex::new(HashMap::new()),
            queue: Queue {
                items: Mutex::new(VecDeque::new()),
                signal: Condvar::new(),
            },
            idle: (Mutex::new(()), Condvar::new()),
            in_game: AtomicBool::new(false),
            running: AtomicBool::new(true),
            shutting_down: AtomicBool::new(false),
            intentional: AtomicBool::new(false),
            generation: AtomicU32::new(0),
            command_epoch: AtomicU32::new(0),
            unacked: AtomicU32::new(0),
            last_chat_ms: AtomicI64::new(0),
        });
        shared.update_status(Link::Offline);

        let net = Arc::clone(&shared);
        thread::Builder::new()
            .name("hugoafk-net".into())
            .spawn(move || net_loop(net))
            .expect("Netz-Thread");

        let sender = Arc::clone(&shared);
        thread::Builder::new()
            .name("hugoafk-sender".into())
            .spawn(move || sender_loop(sender))
            .expect("Sender-Thread");

        Client { shared }
    }

    /// Nachricht oder Befehl in die rate-limitierte Warteschlange stellen.
    pub fn send_chat(&self, input: &str) {
        if !self.shared.in_game.load(Ordering::Relaxed) {
            self.shared.console.error("Nicht verbunden – Nachricht nicht gesendet.");
            return;
        }
        self.shared.queue.push(input.to_string());
    }

    pub fn reconnect_now(&self) {
        self.shared.console.info("Verbinde neu ...");
        self.shared.drop_connection(true);
    }

    pub fn account_name(&self) -> String {
        self.shared.account.lock().unwrap().name.clone()
    }

    /// Konto wechseln und mit dem neuen Konto sofort neu verbinden.
    pub fn set_account(&self, account: Account) {
        *self.shared.account.lock().unwrap() = account;
        self.shared.console.note("Konto gewechselt – verbinde neu ...");
        self.shared.drop_connection(true);
    }

    pub fn switch_server(&self, host: String, port: u16, srv: bool) {
        self.shared
            .console
            .note(&format!("Wechsle zu {}:{} ...", host, port));
        *self.shared.target.lock().unwrap() = (host, port, srv);
        self.shared.drop_connection(true);
    }

    /// Geänderte Konfiguration übernehmen. Läuft gerade ein Befehls-Planer, wird er beendet
    /// und – wenn wir im Spiel sind – mit der neuen Liste neu gestartet.
    pub fn update_config(&self, config: Config) {
        *self.shared.config.lock().unwrap() = config;
        self.shared.command_epoch.fetch_add(1, Ordering::SeqCst);
        self.shared.wake();
        if self.shared.in_game.load(Ordering::Relaxed) {
            start_commands(&self.shared);
        }
    }

    pub fn shutdown(&self) {
        self.shared.shutting_down.store(true, Ordering::SeqCst);
        self.shared.running.store(false, Ordering::SeqCst);
        self.shared.intentional.store(true, Ordering::SeqCst);
        self.shared.queue.signal.notify_all();
        self.shared.wake();
        if let Some(writer) = self.shared.writer.lock().unwrap().as_ref() {
            writer.shutdown();
        }
    }
}

impl Queue {
    fn push(&self, item: String) {
        self.items.lock().unwrap().push_back(item);
        self.signal.notify_one();
    }

    fn clear(&self) {
        self.items.lock().unwrap().clear();
    }
}

impl Shared {
    fn send(&self, packet: Writer) {
        if let Some(writer) = self.writer.lock().unwrap().as_mut() {
            let _ = writer.send(packet);
        }
    }

    /// Verbindung hart schließen; der Netz-Thread merkt das am Lesefehler und verbindet neu.
    fn drop_connection(&self, intentional: bool) {
        self.intentional.store(intentional, Ordering::SeqCst);
        if let Some(writer) = self.writer.lock().unwrap().as_ref() {
            writer.shutdown();
        }
    }

    /// Chat-/Serverzeile anzeigen (mit Uhrzeit davor).
    fn display(&self, text: &str) {
        self.console.chat(text);
    }

    /// Zustandszeile unter dem Eingabefeld nachziehen.
    fn update_status(&self, link: Link) {
        let account = self.account.lock().unwrap().name.clone();
        let (host, port, _) = self.target.lock().unwrap().clone();
        let server = if host.is_empty() {
            "kein Server".to_string()
        } else if port == DEFAULT_PORT {
            host
        } else {
            format!("{}:{}", host, port)
        };
        let state = match link {
            Link::Online => "verbunden",
            Link::Connecting => "verbinde",
            Link::Offline => "getrennt",
        };
        self.console.set_status(
            link,
            &format!("{}  ·  {}  ·  {}  ·  :help", state, server, account),
        );
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

    fn ticket(&self) -> Ticket {
        Ticket {
            generation: self.generation.load(Ordering::SeqCst),
            epoch: self.command_epoch.load(Ordering::SeqCst),
        }
    }

    fn valid(&self, ticket: Ticket) -> bool {
        self.running.load(Ordering::Relaxed) && self.ticket() == ticket
    }

    /// Höchstens `duration` warten, ohne zu pollen. `false` = Verbindung beendet, Programm
    /// beendet oder Befehlsliste geändert: der Aufrufer soll aufhören.
    fn wait(&self, duration: Duration, ticket: Ticket) -> bool {
        let mut left = duration;
        let mut guard = self.idle.0.lock().unwrap();
        loop {
            if !self.valid(ticket) {
                return false;
            }
            if left.is_zero() {
                return true;
            }
            let started = Instant::now();
            let (next, result) = self.idle.1.wait_timeout(guard, left).unwrap();
            guard = next;
            if result.timed_out() {
                return self.valid(ticket);
            }
            left = left.saturating_sub(started.elapsed());
        }
    }

    /// Alle Wartenden wecken (Verbindung beendet, Liste geändert, Programmende).
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
                let mut w = Writer::packet(game::SB_CHAT_COMMAND);
                w.string(&command);
                shared.send(w);
            }
            Outgoing::Message(message) => {
                let offset = shared.unacked.swap(0, Ordering::Relaxed);
                shared.send(chat_packet(&shared, &message, offset));
            }
        }

        let delay = shared.config.lock().unwrap().chat_min_delay_ms.max(200);
        thread::sleep(Duration::from_millis(delay));
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
    let mut w = Writer::packet(game::SB_CHAT);
    w.string(message);
    w.i64(shared.next_chat_time());
    w.i64(0); // salt
    w.bool(false); // keine Signatur (unsignierter Chat)
    w.var_int(offset as i32);
    w.raw(&[0, 0, 0]); // Bitset der zuletzt gesehenen Nachrichten (20 Bit = 3 Byte)
    w.u8(0); // Prüfsumme 0 = „bitte nicht prüfen"
    w
}

// ===================== Netz-Thread =====================

fn net_loop(shared: Arc<Shared>) {
    let mut attempts: u32 = 0;

    while shared.running.load(Ordering::Relaxed) {
        let result = run_connection(&shared);

        shared.in_game.store(false, Ordering::SeqCst);
        shared.generation.fetch_add(1, Ordering::SeqCst);
        *shared.writer.lock().unwrap() = None;
        // Wartende Befehls-Planer erkennen an der neuen Generation, dass sie fertig sind.
        shared.wake();

        if shared.shutting_down.load(Ordering::Relaxed) {
            return;
        }
        shared.update_status(Link::Offline);
        if let Err(e) = result {
            shared.console.error(&format!("Getrennt: {}", e));
        }

        if shared.intentional.swap(false, Ordering::SeqCst) {
            attempts = 0; // vom Nutzer ausgelöst: sofort neu verbinden
            continue;
        }

        let (auto, base, max) = {
            let config = shared.config.lock().unwrap();
            (
                config.auto_reconnect,
                config.reconnect_delay_seconds,
                config.max_backoff_seconds,
            )
        };
        if !auto {
            shared
                .console
                .warn("Auto-Reconnect ist aus – mit :reconnect neu verbinden.");
            return;
        }

        attempts += 1;
        let exponent = (attempts - 1).min(6);
        let delay = (base.saturating_mul(1 << exponent)).clamp(1, max);
        shared
            .console
            .info(&format!("Reconnect-Versuch {} in {} s ...", attempts, delay));

        // In Sekundenschritten schlafen, damit :quit nicht bis zu 60 s hängt.
        for _ in 0..delay {
            if !shared.running.load(Ordering::Relaxed) {
                return;
            }
            thread::sleep(Duration::from_secs(1));
        }
    }
}

fn run_connection(shared: &Arc<Shared>) -> Result<(), String> {
    let (host, port, srv_allowed) = shared.target.lock().unwrap().clone();

    // SRV nur beim Standardport – exakt wie MCProtocolLib im Java-Client. Manche Server (z. B.
    // hugosmp.net) haben überhaupt keinen A-Record und sind nur über SRV erreichbar.
    let (real_host, real_port) = if srv_allowed && port == DEFAULT_PORT {
        dns::resolve_srv(&host).unwrap_or((host.clone(), port))
    } else {
        (host.clone(), port)
    };

    shared.update_status(Link::Connecting);
    shared.console.note(&format!(
        "Verbinde zu {}:{} (MC {}) ...",
        real_host, real_port, MINECRAFT_VERSION
    ));

    let (mut reader, writer) =
        conn::connect(&format!("{}:{}", real_host, real_port)).map_err(|e| e.to_string())?;
    *shared.writer.lock().unwrap() = Some(writer);
    shared.cookies.lock().unwrap().clear();

    // Handshake + Login-Start
    let (username, profile_id) = {
        let account = shared.account.lock().unwrap();
        (account.name.clone(), account.profile_id)
    };

    let mut intention = Writer::packet(handshake::SB_INTENTION);
    intention.var_int(PROTOCOL_VERSION);
    intention.string(&real_host);
    intention.u16(real_port);
    intention.var_int(handshake::INTENT_LOGIN);
    shared.send(intention);

    let mut hello = Writer::packet(login::SB_HELLO);
    hello.string(&username);
    hello.uuid(&profile_id);
    shared.send(hello);

    let mut session = Session {
        state: State::Login,
        joined: false,
        position: None,
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
            Ok(true) => return Ok(()),   // sauber getrennt (Disconnect/Transfer)
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
    position: Option<(f64, f64, f64, f32, f32)>,
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
            shared.send(client_information(cfg::SB_CLIENT_INFORMATION));
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
        cfg::CB_CODE_OF_CONDUCT => {
            // Neu in 26.1: ohne Bestätigung lässt der Server niemanden ins Spiel.
            shared.send(Writer::packet(cfg::SB_ACCEPT_CODE_OF_CONDUCT));
        }
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
    match id {
        // KeepAlive zuerst und sofort beantworten – das ist der eigentliche Schutz gegen
        // disconnect.timeout. Alles andere (Anzeige o. Ä.) kommt danach.
        game::CB_KEEP_ALIVE => {
            let ping = r.i64().map_err(|e| e.to_string())?;
            let mut w = Writer::packet(game::SB_KEEP_ALIVE);
            w.i64(ping);
            shared.send(w);
        }
        game::CB_PING => {
            let ping = r.i32().map_err(|e| e.to_string())?;
            let mut w = Writer::packet(game::SB_PONG);
            w.i32(ping);
            shared.send(w);
        }
        game::CB_LOGIN => {
            let first_join = !session.joined;
            session.joined = true;
            on_join(shared, session, first_join);
        }
        game::CB_SYSTEM_CHAT => {
            let component = nbt::read_network(r).map_err(|e| e.to_string())?;
            let line = nbt::render(&component, shared.console.is_color());
            if !line.trim().is_empty() {
                shared.display(&line);
            }
        }
        game::CB_PLAYER_CHAT => {
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
        game::CB_PLAYER_POSITION => handle_position(shared, session, r)?,
        game::CB_SET_HEALTH => {
            let health = r.f32().map_err(|e| e.to_string())?;
            if health <= 0.0 && !session.dead {
                session.dead = true;
                shared.console.error("Gestorben – respawne automatisch.");
                let mut w = Writer::packet(game::SB_CLIENT_COMMAND);
                w.var_int(game::CLIENT_COMMAND_RESPAWN);
                shared.send(w);
            } else if health > 0.0 {
                session.dead = false;
            }
        }
        game::CB_RESOURCE_PACK_PUSH => {
            let pack = r.uuid().map_err(|e| e.to_string())?;
            acknowledge_resource_pack(shared, game::SB_RESOURCE_PACK, &pack);
        }
        game::CB_START_CONFIGURATION => {
            // Der Server holt uns zurück in die Konfigurationsphase (z. B. Ressourcen-Neuladen).
            shared.in_game.store(false, Ordering::SeqCst);
            shared.send(Writer::packet(game::SB_CONFIGURATION_ACKNOWLEDGED));
            session.state = State::Configuration;
        }
        game::CB_STORE_COOKIE => store_cookie(shared, r)?,
        game::CB_COOKIE_REQUEST => {
            let key = r.string().map_err(|e| e.to_string())?;
            shared.send(cookie_response(shared, game::SB_COOKIE_RESPONSE, &key));
        }
        game::CB_TRANSFER => return transfer(shared, r),
        game::CB_DISCONNECT => return Err(disconnect_reason(shared, r)),
        _ => {}
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
    session.position = None;
    session.dead = false;
    session.last_signature = None;

    shared.send(client_information(game::SB_CLIENT_INFORMATION));
    shared.update_status(Link::Online);

    if first_join {
        let name = shared.account.lock().unwrap().name.clone();
        shared
            .console
            .ok(&format!("Verbunden und im Spiel als {}.", name));
        start_commands(shared);
    } else {
        shared
            .console
            .info("Unterserver gewechselt (zählt nicht als neuer Beitritt).");
    }
}

/// Wiederkehrende Befehle nach dem echten Beitritt.
///
/// **Ein** Thread für alle Einträge: er schläft blockierend bis zum nächsten Termin (kein
/// Polling, 0 % CPU im Leerlauf) und beendet sich, sobald die Verbindung endet oder die Liste
/// geändert wird. Ohne Befehle wird gar kein Thread gestartet.
fn start_commands(shared: &Arc<Shared>) {
    let list = shared.config.lock().unwrap().active_commands();
    if list.is_empty() {
        return;
    }
    let ticket = shared.ticket();
    let shared = Arc::clone(shared);
    thread::Builder::new()
        .name("hugoafk-cmds".into())
        .spawn(move || {
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

                if !shared.wait(at.saturating_sub(start.elapsed()), ticket) {
                    return;
                }
                // Serverwechsel o. Ä.: kurz warten, statt ins Leere zu senden.
                if !shared.in_game.load(Ordering::Relaxed) {
                    if !shared.wait(Duration::from_secs(2), ticket) {
                        return;
                    }
                    continue;
                }

                let command = &list[index];
                shared.console.note(&format!("Befehl: {}", command.command));
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

fn handle_position(shared: &Arc<Shared>, session: &mut Session, r: &mut Reader) -> Result<(), String> {
    let id = r.var_int().map_err(|e| e.to_string())?;
    let (x, y, z) = (
        r.f64().map_err(|e| e.to_string())?,
        r.f64().map_err(|e| e.to_string())?,
        r.f64().map_err(|e| e.to_string())?,
    );
    // Delta-Bewegung interessiert uns nicht, muss aber übersprungen werden.
    for _ in 0..3 {
        r.f64().map_err(|e| e.to_string())?;
    }
    let yaw = r.f32().map_err(|e| e.to_string())?;
    let pitch = r.f32().map_err(|e| e.to_string())?;
    let flags = r.i32().map_err(|e| e.to_string())?;

    // Bit je Element: X, Y, Z, Y_ROT, X_ROT (relativ = zum bisherigen Wert addieren).
    let previous = session.position.unwrap_or((0.0, 0.0, 0.0, 0.0, 0.0));
    let relative = |bit: i32, old: f64, value: f64| -> f64 {
        if flags & (1 << bit) != 0 {
            old + value
        } else {
            value
        }
    };
    let new = (
        relative(0, previous.0, x),
        relative(1, previous.1, y),
        relative(2, previous.2, z),
        relative(3, previous.3 as f64, yaw as f64) as f32,
        relative(4, previous.4 as f64, pitch as f64) as f32,
    );
    session.position = Some(new);

    // Teleport bestätigen (Pflicht, sonst Rubberband/Kick) und die vorgegebene Position EINMAL
    // zurückspiegeln – das ist die Antwort auf den Teleport, keine Eigenbewegung.
    let mut accept = Writer::packet(game::SB_ACCEPT_TELEPORTATION);
    accept.var_int(id);
    shared.send(accept);

    let mut move_packet = Writer::packet(game::SB_MOVE_PLAYER_POS_ROT);
    move_packet.f64(new.0);
    move_packet.f64(new.1);
    move_packet.f64(new.2);
    move_packet.f32(new.3);
    move_packet.f32(new.4);
    move_packet.u8(0x01); // onGround
    shared.send(move_packet);
    Ok(())
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
    if r.var_int().is_err() || r.uuid().is_err() || r.var_int().is_err() {
        return chat; // globalIndex, Absender, index
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

    let seen = r.var_int().ok()?.clamp(0, 20);
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
    let color = shared.console.is_color();
    let body = match unsigned {
        Some(component) => nbt::render(&component, color),
        None => nbt::render(&nbt::Nbt::Str(content), color),
    };
    Some(format!("<{}> {}", nbt::render(&name, color), body))
}

/// Empfangene Chat-Nachrichten regelmäßig quittieren (sonst kickt der Server irgendwann).
fn maybe_acknowledge(shared: &Arc<Shared>) {
    if shared.unacked.load(Ordering::Relaxed) >= ACK_THRESHOLD {
        let offset = shared.unacked.swap(0, Ordering::Relaxed);
        if offset > 0 {
            let mut w = Writer::packet(game::SB_CHAT_ACK);
            w.var_int(offset as i32);
            shared.send(w);
        }
    }
}

// ===================== gemeinsame Bausteine =====================

/// Spieleinstellungen wie ein echter Client (manche Server erwarten das vor dem Spielbeitritt).
fn client_information(packet_id: i32) -> Writer {
    let mut w = Writer::packet(packet_id);
    w.string("de_DE");
    w.u8(8); // Sichtweite (Chunks) – wir laden ohnehin nichts
    w.var_int(0); // ChatVisibility: FULL
    w.bool(true); // Chatfarben
    w.u8(0x7F); // alle Skin-Teile sichtbar
    w.var_int(1); // Haupthand: rechts
    w.bool(false); // Textfilterung
    w.bool(true); // in der Serverliste sichtbar
    w.var_int(0); // Partikel: ALL
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

fn store_cookie(shared: &Arc<Shared>, r: &mut Reader) -> Result<(), String> {
    let key = r.string().map_err(|e| e.to_string())?;
    let payload = r.byte_array().map_err(|e| e.to_string())?;
    shared.cookies.lock().unwrap().insert(key, payload);
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
    let port = r.var_int().map_err(|e| e.to_string())? as u16;
    shared
        .console
        .info(&format!("Server-Transfer zu {}:{}", host, port));
    *shared.target.lock().unwrap() = (host, port, false);
    shared.intentional.store(true, Ordering::SeqCst);
    Ok(true)
}

fn disconnect_reason(shared: &Arc<Shared>, r: &mut Reader) -> String {
    match nbt::read_network(r) {
        Ok(component) => nbt::render(&component, shared.console.is_color()),
        Err(_) => "unbekannt".to_string(),
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
}
