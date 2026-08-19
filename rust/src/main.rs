//! AFKSystems (Rust) – schlanker Minecraft-AFK-Client, **eine Binary für alle unterstützten
//! Protokolle** (1.21.1, 1.21.11, 26.1, 26.2; Auswahl über `--mc`).
//!
//! Kein Menü, keine Konfigurationsdatei: alles steht im Startbefehl. Der Client meldet sich an,
//! tritt bei und gibt danach ausschließlich Chat auf der Standardausgabe aus – Statusmeldungen
//! gehen auf die Standardfehlerausgabe, Eingabezeilen gehen als Chat raus. Damit lässt er sich
//! ohne Terminal betreiben und von einem anderen Programm (z. B. einer Website) fernsteuern.
//!
//! Mehrere Bauformen aus derselben Quelle:
//! * `cargo build --release` – der schlanke AFK-Client, ohne jede Bewegung.
//! * `cargo build --release --features movement` – zusätzlich gesteuerte Bewegung
//!   (`:go`, `:look`, `:home`, `:route`, `:stop`, `:pos`), siehe [`movement`].
//! * `cargo build --release --features items` – Menüs und deren Gegenstände samt Lore.
//! * `cargo build --release --features pov-client` – automatisch gestartete Live-Ansicht.
//! * `cargo build --release --features premium` – Anzeigetafel, Menüs, Anti-AFK und Schleichen.
//! * `cargo build --release --features ultra` – alle Rust-Zusatzfunktionen.

#[cfg(feature = "antiafk")]
mod antiafk;
mod auth;
#[cfg(feature = "board")]
mod board;
mod buf;
mod client;
mod conn;
mod console;
mod dns;
#[cfg(feature = "extras")]
mod extras;
#[cfg(feature = "items")]
mod item_names;
#[cfg(feature = "items")]
mod items;
#[cfg(feature = "menu")]
mod menu;
#[cfg(feature = "movement")]
mod movement;
mod nbt;
mod options;
#[cfg(feature = "pov")]
mod pov;
mod proto;
mod proxy;
mod rules;

use crate::client::Client;
use crate::console::Console;
use crate::options::{Command, Options};
use crate::proto::Protocol;
use std::path::Path;

fn main() {
    let command = match options::parse(std::env::args().skip(1)) {
        Ok(command) => command,
        Err(message) => {
            eprintln!("{}", message);
            std::process::exit(2);
        }
    };

    match command {
        Command::Help => print_usage(),
        Command::Accounts => print_accounts(),
        Command::Login => add_account_only(),
        Command::Run(options) => run(*options),
    }
}

fn run(options: Options) {
    let console = Console::new(options.color, options.quiet, options.events);
    warn_about_unused_options(&console, &options);

    // Offline-Konten sind reine Rechenarbeit: kein Microsoft-Login, kein Verzeichnis, keine
    // gespeicherte Datei. Deshalb wird hier auch nichts migriert oder geladen.
    let account = if let Some(name) = &options.offline {
        match auth::offline(name) {
            Ok(account) => {
                console.info(&format!(
                    "Offline-Modus als '{}' (nur auf Servern ohne Konto-Prüfung).",
                    account.name
                ));
                account
            }
            Err(e) => {
                console.error(&format!("Offline-Name abgelehnt: {}", e));
                std::process::exit(1);
            }
        }
    } else {
        options::migrate();
        let base = options::dir();
        match sign_in(&console, &base, options.account.as_deref()) {
            Ok(account) => account,
            Err(e) => {
                console.error(&format!("Login fehlgeschlagen: {}", e));
                std::process::exit(1);
            }
        }
    };

    let client = Client::new(console.clone(), options, account);

    // Jede Eingabezeile geht in den Chat. Endet die Standardeingabe (Dienstbetrieb, Pipe, kein
    // Terminal), läuft der Client einfach ohne Eingabe weiter – die Netz-Threads arbeiten.
    while let Some(line) = console.read_line() {
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        #[cfg(feature = "local")]
        if let Some(rest) = line.strip_prefix(':') {
            let mut parts = rest.splitn(2, char::is_whitespace);
            let verb = parts.next().unwrap_or("").to_lowercase();
            let arg = parts.next().unwrap_or("").trim();
            client.local_command(&verb, arg);
            continue;
        }
        client.send_chat(line);
    }

    loop {
        std::thread::park();
    }
}

/// Das Panel schickt allen Bauformen dieselben Argumente. Damit `--antiafk` im schlanken Build
/// nicht still verpufft, wird hier einmal gesagt, was dieser Build nicht kann.
fn warn_about_unused_options(console: &Console, options: &Options) {
    if !options.ignored.is_empty() {
        console.warn(&format!(
            "{} gibt es nur im Java-Client; wird ignoriert (dieser Client verbindet nie \
             automatisch neu).",
            options.ignored.join(", ")
        ));
    }
    #[cfg(feature = "antiafk")]
    let _ = options;
    #[cfg(not(feature = "antiafk"))]
    {
        if options.antiafk_seconds > 0 {
            console.warn("--antiafk braucht den Premium- oder Ultra-Client; wird ignoriert.");
        }
    }
    #[cfg(not(feature = "state"))]
    {
        if options.sneak {
            console.warn("--sneak braucht den Premium- oder Ultra-Client; wird ignoriert.");
        }
    }
    #[cfg(not(feature = "pov"))]
    {
        if options.pov_size.is_some() || options.pov_autostart.is_some() {
            console.warn("--pov/--pov-size brauchen die POV- oder Ultra-Datei; wird ignoriert.");
        }
    }
    #[cfg(all(feature = "state", feature = "pov"))]
    let _ = console;
}

// ===================== Anmeldung =====================

/// Sorgt für ein angemeldetes Konto: das gewünschte, sonst das erste gespeicherte, sonst ein
/// neues über den Microsoft-Gerätecode.
fn sign_in(console: &Console, base: &Path, preferred: Option<&str>) -> auth::Res<auth::Account> {
    if let Some(name) = auth::migrate_legacy(base) {
        console.info(&format!(
            "Bestehende Anmeldung als Konto '{}' übernommen.",
            name
        ));
    }

    let accounts = auth::list(base);
    let target = match preferred {
        Some(name) if accounts.iter().any(|a| a == name) => Some(name.to_string()),
        Some(name) => {
            return Err(format!(
                "Konto '{}' gibt es nicht. Vorhanden: {}",
                name,
                if accounts.is_empty() {
                    "keins (mit --login anlegen)".to_string()
                } else {
                    accounts.join(", ")
                }
            )
            .into())
        }
        None => accounts.first().cloned(),
    };

    let Some(target) = target else {
        return device_code_login(console, base);
    };

    console.info(&format!("Melde Konto '{}' an ...", target));
    match auth::load(base, &target) {
        Ok(account) => Ok(account),
        Err(e) => {
            console.error(&format!(
                "Konto '{}' ließ sich nicht anmelden: {}",
                target, e
            ));
            device_code_login(console, base)
        }
    }
}

/// Microsoft-Login über den Gerätecode. Die Anweisungen gehen auf die Standardfehlerausgabe,
/// damit die Standardausgabe auch hier nur Chat enthält.
fn device_code_login(console: &Console, base: &Path) -> auth::Res<auth::Account> {
    let account = auth::add(base, |code| {
        console.print("");
        console.print("==================  Microsoft-Login  ==================");
        console.print(&format!(
            "  1. Öffne im Browser:  {}",
            code.verification_uri
        ));
        console.print(&format!("  2. Gib diesen Code ein: {}", code.user_code));
        console.print("=======================================================");
        console.info("Warte auf Anmeldung ...");
    })?;
    console.ok(&format!("Angemeldet als: {}", account.name));
    Ok(account)
}

/// `--login`: nur anmelden und beenden.
fn add_account_only() {
    let console = Console::new(true, false, false);
    options::migrate();
    match device_code_login(&console, &options::dir()) {
        Ok(account) => println!("{}", account.name),
        Err(e) => {
            console.error(&format!("Login fehlgeschlagen: {}", e));
            std::process::exit(1);
        }
    }
}

/// `--accounts`: gespeicherte Konten auflisten (eines je Zeile, direkt weiterverwendbar).
fn print_accounts() {
    options::migrate();
    for name in auth::list(&options::dir()) {
        println!("{}", name);
    }
}

// ===================== Hilfe =====================

fn print_usage() {
    println!(
        "AFKSystems {} – {}\n\
         \n\
         Aufruf:  afk <host[:port]> [optionen]\n\
         \n\
         Konto:\n\
         \x20 -a, --account <name>        gespeichertes Microsoft-Konto (Standard: das erste)\n\
         \x20     --offline <name>        Offline-/Cracked-Konto (nur ohne Konto-Prüfung)\n\
         \x20     --login                 Microsoft-Konto anmelden und beenden\n\
         \x20     --accounts              gespeicherte Konten auflisten und beenden\n\
         \n\
         Verbindung:\n\
         \x20 -s, --server <host[:port]>  Serveradresse (geht auch ohne -s als erstes Argument)\n\
         \x20 -m, --mc <version>          Protokoll: {}  (Standard: {})\n\
         \x20     --proxy <adresse>       socks5://[nutzer:pass@]host:port oder http://...\n\
         \x20     --fakehost <host[:port]> diese Adresse im Handshake statt der echten\n\
         \x20     --view-distance <2-32>  gemeldete Sichtweite in Chunks (Standard: {})\n\
         \n\
         Befehle und Makros:\n\
         \x20 -c, --cmd [sek:]<befehl>    Befehl nach dem Beitritt, mehrfach angebbar.\n\
         \x20                             Ohne 'sek:' einmalig, sonst alle 'sek' Sekunden.\n\
         \x20                             Beispiel: -c 300:/afk\n\
         \x20     --join-delay <sek>      Wartezeit nach dem Beitritt vor dem ersten Befehl (4)\n\
         \x20     --on <ausloeser>=<aktion>  join | world | death | chat:<text>\n\
         \x20                             Beispiel: --on chat:du bist afk=/lobby\n\
         \x20     --on-cooldown <sek>     Sperrzeit je Regel (3)\n\
         \x20     --chat-delay <ms>       Mindestabstand ausgehender Nachrichten (1000)\n\
         \n\
         Ausgabe:\n\
         \x20     --no-color              keine Farben\n\
         \x20 -q, --quiet                 keine Statusmeldungen, nur Chat\n\
         \x20     --events                zusaetzlich '@event ...'-Zeilen zum Mitlesen\n\
         \x20 -h, --help                  diese Hilfe\n\
         {}{}\n\
         Beispiel:\n\
         \x20 afk mc.example.net --mc 26.1 -c 300:/afk --on death=/spawn\n\
         \n\
         Ausgabe: Chat auf der Standardausgabe, alles andere auf der Standardfehlerausgabe.\n\
         Eingabe: jede Zeile geht als Chat raus, mit '/' vorn als Serverbefehl.\n\
         Konten:  {}{}",
        env!("CARGO_PKG_VERSION"),
        if cfg!(feature = "ultra") {
            "Ultra-AFK-Client fuer Minecraft"
        } else if cfg!(feature = "pov-client") {
            "POV-AFK-Client fuer Minecraft"
        } else if cfg!(feature = "items") && cfg!(feature = "premium") {
            "Premium-AFK-Client mit Gegenstaenden"
        } else if cfg!(feature = "premium") {
            "Premium-AFK-Client fuer Minecraft"
        } else if cfg!(feature = "items") {
            "AFK-Client mit Gegenstaenden"
        } else if cfg!(feature = "movement") {
            "Minecraft-AFK-Client mit Bewegung"
        } else {
            "schlanker Minecraft-AFK-Client"
        },
        Protocol::names(),
        proto::DEFAULT.name,
        options::DEFAULT_VIEW_DISTANCE,
        if cfg!(feature = "antiafk") {
            "\nPremium:\n\
             \x20     --antiafk <sek>         alle <sek> eine kleine Bewegung (min. 15, 0 = aus)\n\
             \x20     --sneak                 beim Beitritt geduckt bleiben\n"
        } else {
            ""
        },
        if cfg!(feature = "pov") {
            "\nLive-Ansicht:\n\
             \x20     --pov an|aus            Ansicht beim Beitritt starten (POV an, Ultra aus)\n\
             \x20     --pov-size <b>x<h>      Bildgroesse in Pixeln, z. B. 160x80 (Standard 64x32)\n\
             \x20     --pov-fps <1-20>        Bilder je Sekunde (Standard 8)\n"
        } else {
            ""
        },
        options::dir().display(),
        if cfg!(feature = "local") {
            if cfg!(feature = "movement") {
                "\nOertliche Befehle beginnen mit ':' – ':help' listet sie alle auf.\n\
                 Bewegung (:go, :look, :home) merkt sich movement.json im selben Verzeichnis."
            } else {
                "\nOertliche Befehle beginnen mit ':' – ':help' listet sie alle auf."
            }
        } else {
            ""
        }
    );
}
