//! HugoAFKClient (Rust) – schlanker Minecraft-AFK-Client für **MC 26.1 (Protokoll 775)**.
//!
//! Gleiche Bedienung, gleiche Konfiguration und dieselben Konto-Dateien wie der Java-Client,
//! aber ohne JVM: ein natives Programm mit zwei Threads und wenigen MB Speicher.

mod auth;
mod buf;
mod client;
mod clock;
mod config;
mod conn;
mod console;
mod dns;
mod nbt;
mod proto;

use crate::client::Client;
use crate::config::{AutoCommand, Config};
use crate::console::{Console, Link, BOLD, CYAN, GRAY, GREEN};
use crate::proto::MINECRAFT_VERSION;
use std::path::{Path, PathBuf};

fn main() {
    let base = config_dir();
    let mut config = Config::load(&base.join("config.json"));

    let mut server_arg: Option<String> = None;
    let mut account_arg: Option<String> = None;
    let mut check = false;
    let mut headless = false;
    let mut args = std::env::args().skip(1);
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "-h" | "--help" => return print_usage(),
            "--check" => check = true,
            "--headless" => headless = true,
            "--server" => server_arg = args.next(),
            "--account" => account_arg = args.next(),
            other if !other.starts_with('-') => server_arg = Some(other.to_string()),
            _ => {}
        }
    }

    if check {
        let server = server_arg.unwrap_or_else(|| config.last_server.clone());
        return self_check(&base, &account_arg.unwrap_or(config.active_account), &server);
    }

    let console = if headless {
        Console::headless()
    } else {
        match Console::new() {
            Ok(console) => console,
            Err(e) => {
                eprintln!("Terminal konnte nicht initialisiert werden: {}", e);
                return;
            }
        }
    };
    console.set_color(config.color_output && !headless);
    print_header(&console);

    // ---- Anmeldung ----
    let preferred = account_arg.unwrap_or_else(|| config.active_account.clone());
    let mut account = match login(&console, &base, &preferred) {
        Ok(account) => account,
        Err(e) => {
            console.error(&format!("Login fehlgeschlagen: {}", e));
            console.close();
            return;
        }
    };
    config.active_account = account.name.clone();
    config.save();

    // ---- Server wählen ----
    let start = server_arg.unwrap_or_else(|| config.last_server.clone());
    let server = if headless {
        if start.trim().is_empty() {
            eprintln!("--headless braucht einen Server: hugoafk --headless <host[:port]>");
            return;
        }
        start
    } else {
        match pre_connect_menu(&console, &base, &mut config, &mut account, start) {
            Some(server) => server,
            None => {
                console.close();
                return;
            }
        }
    };
    // Im Dienstbetrieb nichts an der Konfiguration verstellen.
    if !headless {
        config.last_server = server.clone();
        config.save();
    }

    let (host, port, srv) = parse_host(&console, &server);
    print_help(&console);

    let client = Client::new(console.clone(), config.clone(), account, host, port, srv);

    if headless {
        // Kein Terminal, keine Eingabe: der Netz-Thread arbeitet, der Hauptthread schläft nur.
        loop {
            std::thread::sleep(std::time::Duration::from_secs(3600));
        }
    }

    input_loop(&console, &client, &base, &mut config);

    client.shutdown();
    console.close();
    println!("Tschuess!");
}

// ===================== Selbsttest =====================

/// `--check`: prüft ohne Terminal-Rohmodus und ohne Serverbeitritt, ob Konto, Token-Kette,
/// SRV-Auflösung und TCP-Verbindung stimmen. Praktisch, um Fehler einzugrenzen.
fn self_check(base: &Path, preferred: &str, server: &str) {
    println!("HugoAFKClient (Rust) – Selbsttest");
    println!("  Konfiguration: {}", base.display());

    if let Some(name) = auth::migrate_legacy(base) {
        println!("  auth.json als Konto '{}' übernommen", name);
    }
    let accounts = auth::list(base);
    println!("  Konten: {:?}", accounts);

    let target = if !preferred.is_empty() && accounts.iter().any(|a| a == preferred) {
        preferred.to_string()
    } else {
        match accounts.first() {
            Some(name) => name.clone(),
            None => {
                println!("  [!] Kein Konto vorhanden – bitte den Client normal starten und anmelden.");
                return;
            }
        }
    };

    print!("  Anmeldung '{}' ... ", target);
    let _ = std::io::Write::flush(&mut std::io::stdout());
    let mut account = match auth::load(base, &target) {
        Ok(account) => {
            println!("ok");
            account
        }
        Err(e) => {
            println!("FEHLER: {}", e);
            return;
        }
    };
    println!(
        "    Spieler: {}  UUID: {}",
        account.name,
        buf::uuid_to_dashed(&account.profile_id)
    );
    match account.token() {
        Ok(token) => println!("    Minecraft-Token: gültig ({} Zeichen)", token.len()),
        Err(e) => println!("    [!] Token: {}", e),
    }

    if server.is_empty() {
        println!("  Kein Server konfiguriert – Verbindungstest übersprungen.");
        return;
    }

    let value = server.trim();
    let (host, port, srv) = match value.rsplit_once(':') {
        Some((h, p)) if !h.contains(':') => (h.to_string(), p.parse().unwrap_or(25565), false),
        _ => (value.to_string(), 25565u16, true),
    };
    let (real_host, real_port) = if srv && port == 25565 {
        match dns::resolve_srv(&host) {
            Some((h, p)) => {
                println!("  SRV: _minecraft._tcp.{} -> {}:{}", host, h, p);
                (h, p)
            }
            None => {
                println!("  SRV: kein Eintrag für {} (nutze {}:{})", host, host, port);
                (host.clone(), port)
            }
        }
    } else {
        (host.clone(), port)
    };

    print!("  TCP {}:{} ... ", real_host, real_port);
    let _ = std::io::Write::flush(&mut std::io::stdout());
    match std::net::TcpStream::connect((real_host.as_str(), real_port)) {
        Ok(_) => println!("erreichbar"),
        Err(e) => println!("FEHLER: {}", e),
    }
}

// ===================== Anmeldung =====================

/// Sorgt für ein gültiges aktives Konto: bevorzugtes Konto, sonst das erste vorhandene,
/// sonst Device-Code-Login.
fn login(console: &Console, base: &Path, preferred: &str) -> auth::Res<auth::Account> {
    if let Some(name) = auth::migrate_legacy(base) {
        console.info(&format!(
            "Bestehende Anmeldung als Konto '{}' übernommen.",
            name
        ));
    }

    let accounts = auth::list(base);
    let target = if !preferred.is_empty() && accounts.iter().any(|a| a == preferred) {
        Some(preferred.to_string())
    } else {
        accounts.first().cloned()
    };

    let Some(target) = target else {
        return add_account(console, base);
    };

    console.info(&format!("Melde Konto '{}' an ...", target));
    match auth::load(base, &target) {
        Ok(account) => Ok(account),
        Err(e) => {
            console.error(&format!("Konto '{}' ließ sich nicht anmelden: {}", target, e));
            // Andere gespeicherte Konten probieren, bevor wir den Nutzer zum Login zwingen.
            for other in accounts.iter().filter(|a| **a != target) {
                if let Ok(account) = auth::load(base, other) {
                    console.info(&format!("Nutze stattdessen Konto '{}'.", other));
                    return Ok(account);
                }
            }
            add_account(console, base)
        }
    }
}

fn add_account(console: &Console, base: &Path) -> auth::Res<auth::Account> {
    let account = auth::add(base, |code| {
        console.print("");
        console.print("==================  Microsoft-Login  ==================");
        console.print(&format!("  1. Öffne im Browser:  {}", code.verification_uri));
        console.print(&format!("  2. Gib diesen Code ein: {}", code.user_code));
        console.print("=======================================================");
        console.info("Warte auf Anmeldung ...");
    })?;
    console.info(&format!("Angemeldet als: {}", account.name));
    Ok(account)
}

// ===================== Menüs =====================

fn pre_connect_menu(
    console: &Console,
    base: &Path,
    config: &mut Config,
    account: &mut auth::Account,
    mut server: String,
) -> Option<String> {
    print_menu(console, config, account, &server);
    loop {
        console.set_status(
            Link::Offline,
            &format!(
                "nicht verbunden  ·  {}  ·  {}",
                if server.is_empty() { "kein Server" } else { &server },
                account.name
            ),
        );
        let line = console.read_line("❯ ")?;
        let line = line.trim().to_string();

        if line.is_empty() {
            if !server.is_empty() {
                return Some(server);
            }
            console.error("Keine Server-IP. Gib host[:port] ein oder :quit.");
            continue;
        }
        if let Some(rest) = line.strip_prefix(':') {
            let (cmd, arg) = split_command(rest);
            match cmd.as_str() {
                "quit" | "exit" => return None,
                "account" => {
                    if let Some(new_account) = account_menu(console, base, config, &account.name) {
                        *account = new_account;
                    }
                }
                "server" => {
                    if arg.is_empty() {
                        console.error("Nutzung: :server <host[:port]>");
                    } else {
                        server = arg;
                    }
                }
                "cmd" | "cmds" | "befehle" => {
                    commands_menu(console, config, &arg);
                }
                "help" => print_menu(console, config, account, &server),
                other => console.error(&format!("Unbekannt: :{} (siehe :help)", other)),
            }
        } else {
            return Some(line);
        }
    }
}

/// Konten verwalten. Liefert das neue Konto, wenn gewechselt wurde – sonst `None`.
fn account_menu(
    console: &Console,
    base: &Path,
    config: &mut Config,
    current: &str,
) -> Option<auth::Account> {
    loop {
        let accounts = auth::list(base);
        console.print("");
        console.print(&console.paint(BOLD, "=== Konten ==="));
        for (i, name) in accounts.iter().enumerate() {
            let active = if name == current {
                console.paint(CYAN, "  (aktiv)")
            } else {
                String::new()
            };
            console.print(&format!("  {}) {}{}", i + 1, name, active));
        }
        console.print(&console.paint(
            GRAY,
            "  n) neues Konto (Microsoft-Login)   r <nr>) entfernen   [Enter] zurück",
        ));

        let line = console.read_line("Konto ❯ ")?.trim().to_string();
        if line.is_empty() {
            return None;
        }

        if line.eq_ignore_ascii_case("n") {
            match add_account(console, base) {
                Ok(account) => {
                    config.active_account = account.name.clone();
                    config.save();
                    return Some(account);
                }
                Err(e) => console.error(&format!("Login fehlgeschlagen: {}", e)),
            }
        } else if let Some(rest) = line.strip_prefix(['r', 'R']) {
            match parse_index(rest.trim(), accounts.len()) {
                Some(index) => {
                    let name = &accounts[index];
                    if auth::remove(base, name) {
                        console.info(&format!("Entfernt: {}", name));
                    }
                }
                None => console.error("Nutzung: r <nr>"),
            }
        } else if let Some(index) = parse_index(&line, accounts.len()) {
            match auth::load(base, &accounts[index]) {
                Ok(account) => {
                    config.active_account = account.name.clone();
                    config.save();
                    console.info(&format!("Aktives Konto: {}", account.name));
                    return Some(account);
                }
                Err(e) => console.error(&format!("Konto ließ sich nicht anmelden: {}", e)),
            }
        } else {
            console.error("Ungültige Eingabe.");
        }
    }
}

// ===================== Befehlsliste =====================

/// Wiederkehrende Befehle verwalten. `true`, wenn sich etwas geändert hat.
fn commands_menu(console: &Console, config: &mut Config, arg: &str) -> bool {
    if !arg.is_empty() {
        return edit_commands(console, config, arg);
    }
    let mut changed = false;
    loop {
        print_commands(console, config);
        let Some(line) = console.read_line("Befehle ❯ ") else {
            return changed;
        };
        let line = line.trim().to_string();
        if line.is_empty() {
            return changed;
        }
        changed |= edit_commands(console, config, &line);
    }
}

/// Ein Bearbeitungsschritt: `add <sek> <befehl>`, `del <nr>`, `on|off <nr>`, `delay <nr> <sek>`.
fn edit_commands(console: &Console, config: &mut Config, input: &str) -> bool {
    let (verb, rest) = split_command(input);
    let count = config.commands.len();

    let changed = match verb.as_str() {
        "add" | "neu" | "+" => {
            let (interval, command) = split_command(&rest);
            match (interval.parse::<u64>(), command.is_empty()) {
                (Ok(seconds), false) => {
                    config.commands.push(AutoCommand::new(&command, 4, seconds));
                    console.ok(&format!(
                        "Hinzugefügt: {}",
                        config.commands.last().map(AutoCommand::describe).unwrap_or_default()
                    ));
                    true
                }
                _ => {
                    console.error("Nutzung: add <sekunden> <befehl>    z. B. add 300 /afk");
                    console.info("Sekunden = Wiederholungsintervall, 0 = nur einmal je Beitritt.");
                    false
                }
            }
        }
        "del" | "rm" | "r" | "-" => match parse_index(&rest, count) {
            Some(index) => {
                let removed = config.commands.remove(index);
                console.info(&format!("Entfernt: {}", removed.command));
                true
            }
            None => {
                console.error("Nutzung: del <nr>");
                false
            }
        },
        "on" | "off" | "an" | "aus" => match parse_index(&rest, count) {
            Some(index) => {
                let enabled = matches!(verb.as_str(), "on" | "an");
                config.commands[index].enabled = enabled;
                console.info(&format!(
                    "{}: {}",
                    if enabled { "Aktiv" } else { "Aus" },
                    config.commands[index].command
                ));
                true
            }
            None => {
                console.error("Nutzung: on <nr>   bzw.   off <nr>");
                false
            }
        },
        "delay" | "start" => {
            let (number, seconds) = split_command(&rest);
            match (parse_index(&number, count), seconds.trim().parse::<u64>()) {
                (Some(index), Ok(seconds)) => {
                    config.commands[index].delay_seconds = seconds;
                    console.info(&format!(
                        "Startverzögerung: {}",
                        config.commands[index].describe()
                    ));
                    true
                }
                _ => {
                    console.error("Nutzung: delay <nr> <sekunden>");
                    false
                }
            }
        }
        "list" | "" => false,
        other => {
            console.error(&format!("Unbekannt: {}", other));
            false
        }
    };

    if changed {
        config.save();
    }
    changed
}

fn print_commands(console: &Console, config: &Config) {
    console.print("");
    console.print(&console.paint(BOLD, "  Wiederkehrende Befehle"));
    if config.commands.is_empty() {
        console.print(&console.paint(GRAY, "    (keine – mit  add <sekunden> <befehl>  anlegen)"));
    }
    for (index, command) in config.commands.iter().enumerate() {
        let mark = if command.enabled {
            console.paint(GREEN, "●")
        } else {
            console.paint(GRAY, "○")
        };
        console.print(&format!(
            "    {} {})  {}",
            mark,
            index + 1,
            command.describe()
        ));
    }
    console.print(&console.paint(
        GRAY,
        "    add <sek> <befehl>   del <nr>   on|off <nr>   delay <nr> <sek>   [Enter] zurück",
    ));
}

// ===================== Eingabeschleife =====================

fn input_loop(console: &Console, client: &Client, base: &Path, config: &mut Config) {
    while let Some(line) = console.read_line("❯ ") {
        let line = line.trim().to_string();
        if line.is_empty() {
            continue;
        }
        let Some(rest) = line.strip_prefix(':') else {
            client.send_chat(&line);
            continue;
        };

        let (cmd, arg) = split_command(rest);
        match cmd.as_str() {
            "quit" | "exit" => return,
            "reconnect" => client.reconnect_now(),
            "server" => {
                if arg.is_empty() {
                    console.error("Nutzung: :server <host[:port]>");
                } else {
                    let (host, port, srv) = parse_host(console, &arg);
                    config.last_server = arg;
                    config.save();
                    client.switch_server(host, port, srv);
                }
            }
            "account" => {
                if let Some(account) = account_menu(console, base, config, &client.account_name()) {
                    client.set_account(account);
                }
            }
            "cmd" | "cmds" | "befehle" => {
                if commands_menu(console, config, &arg) {
                    // Der Client hält eine eigene Kopie – sonst liefe der Planer weiter alt.
                    client.update_config(config.clone());
                }
            }
            "clear" | "cls" => console.clear_screen(),
            "help" => print_help(console),
            other => console.error(&format!("Unbekannter Befehl: :{} (siehe :help)", other)),
        }
    }
}

// ===================== Hilfen =====================

fn split_command(rest: &str) -> (String, String) {
    let mut parts = rest.splitn(2, char::is_whitespace);
    let cmd = parts.next().unwrap_or("").to_lowercase();
    let arg = parts.next().unwrap_or("").trim().to_string();
    (cmd, arg)
}

fn parse_index(text: &str, size: usize) -> Option<usize> {
    let index = text.trim().parse::<usize>().ok()?;
    if index >= 1 && index <= size {
        Some(index - 1)
    } else {
        None
    }
}

/// `host`, `host:port` oder `[::1]:port` zerlegen. Der dritte Rückgabewert sagt, ob eine
/// SRV-Auflösung versucht werden darf (nur wenn kein Port angegeben wurde).
fn parse_host(console: &Console, input: &str) -> (String, u16, bool) {
    let value = input.trim();

    if let Some(end) = value.strip_prefix('[').and_then(|v| v.find(']')) {
        let host = value[1..end + 1].to_string();
        let rest = &value[end + 2..];
        if let Some(port) = rest.strip_prefix(':') {
            return (host, parse_port(console, port), false);
        }
        return (host, 25565, false);
    }

    match value.rsplit_once(':') {
        Some((host, port)) if !host.contains(':') => {
            (host.to_string(), parse_port(console, port), false)
        }
        _ => (value.to_string(), 25565, true),
    }
}

fn parse_port(console: &Console, text: &str) -> u16 {
    match text.trim().parse::<u16>() {
        Ok(port) => port,
        Err(_) => {
            console.error("Ungültiger Port – benutze 25565.");
            25565
        }
    }
}

fn config_dir() -> PathBuf {
    if let Ok(xdg) = std::env::var("XDG_CONFIG_HOME") {
        if !xdg.trim().is_empty() {
            return PathBuf::from(xdg).join("hugoafk");
        }
    }
    let home = std::env::var("USERPROFILE")
        .or_else(|_| std::env::var("HOME"))
        .unwrap_or_else(|_| ".".to_string());
    PathBuf::from(home).join(".config").join("hugoafk")
}

// ===================== Ausgabe =====================

fn print_header(console: &Console) {
    // Rahmenbreite aus dem Text berechnen – so sitzt er immer bündig, egal wie lang die
    // Versionsnummer ist.
    let suffix = format!("   ·   Rust   ·   Minecraft {}", MINECRAFT_VERSION);
    let bar = "─".repeat("HugoAFKClient".len() + suffix.chars().count() + 4);
    console.print("");
    console.print(&console.paint(CYAN, &format!("  ╭{}╮", bar)));
    console.print(&format!(
        "{}  {}{}  {}",
        console.paint(CYAN, "  │"),
        console.paint(BOLD, "HugoAFKClient"),
        console.paint(GRAY, &suffix),
        console.paint(CYAN, "│")
    ));
    console.print(&console.paint(CYAN, &format!("  ╰{}╯", bar)));
}

fn print_menu(console: &Console, config: &Config, account: &auth::Account, server: &str) {
    let shown = if server.is_empty() {
        console.paint(GRAY, "(keiner)")
    } else {
        server.to_string()
    };
    console.print("");
    console.print(&format!(
        "{} {}",
        console.paint(GRAY, "  Konto   "),
        account.name
    ));
    console.print(&format!("{} {}", console.paint(GRAY, "  Server  "), shown));
    let commands = config.active_commands();
    let summary = if commands.is_empty() {
        console.paint(GRAY, "(keine)")
    } else {
        commands
            .iter()
            .map(|c| c.command.clone())
            .collect::<Vec<_>>()
            .join(", ")
    };
    console.print(&format!("{} {}", console.paint(GRAY, "  Befehle "), summary));
    console.print("");
    console.print(&console.paint(
        GRAY,
        "  [Enter] verbinden   <ip> verbinden   :account   :server <ip>   :cmd   :quit",
    ));
}

fn print_help(console: &Console) {
    console.print("");
    console.print(&console.paint(BOLD, "  Bedienung"));
    let row = |key: &str, text: &str| {
        console.print(&format!(
            "    {}  {}",
            console.paint(CYAN, &format!("{:<14}", key)),
            console.paint(GRAY, text)
        ));
    };
    row("<text>", "in den Chat schreiben");
    row("/befehl", "Serverbefehl senden");
    row(":cmd", "wiederkehrende Befehle (z. B. /afk alle 5 min)");
    row(":reconnect", "neu verbinden");
    row(":server <ip>", "Server wechseln");
    row(":account", "Konto wechseln/verwalten");
    row(":clear", "Bildschirm leeren");
    row(":quit", "beenden");
    console.print(&console.paint(
        GRAY,
        "    ↑↓ Verlauf · Strg+U Zeile löschen · Strg+W Wort löschen · Strg+L Bildschirm",
    ));
}

fn print_usage() {
    println!(
        "HugoAFKClient (Rust) – schlanker Minecraft-AFK-Client für MC {}\n\
         \n\
         Aufruf: hugoafk [optionen] [host[:port]]\n\
         \n\
         Optionen:\n\
         \x20 --server <host[:port]>   Server-Adresse\n\
         \x20 --account <name>         Startkonto wählen\n\
         \x20 --check                  Selbsttest (Konto, SRV, Erreichbarkeit)\n\
         \x20 --headless               ohne Terminal-Eingabe (Dienst-/Hintergrundbetrieb)\n\
         \x20 -h, --help               diese Hilfe\n\
         \n\
         Konfiguration: ~/.config/hugoafk/ (config.json, accounts/) – dieselbe wie beim Java-Client.",
        MINECRAFT_VERSION
    );
}
