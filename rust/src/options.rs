//! Startargumente. Der Client hat weder Menü noch Konfigurationsdatei: alles, was er wissen
//! muss, steht im Startbefehl. Gespeichert wird nur, was gespeichert werden muss – die
//! Microsoft-Konten unter `~/.config/afksystems/accounts/`.

use crate::proto::{Protocol, DEFAULT};
use std::path::PathBuf;

/// Untergrenze für Wiederholungen: schneller löst nur der Spam-Schutz des Servers aus.
const MIN_REPEAT_SECONDS: u64 = 5;
/// Untergrenze für den Abstand zweier ausgehender Nachrichten.
const MIN_CHAT_DELAY_MS: u64 = 200;

/// Verzeichnis mit `accounts/` (und `movement.json` im Bewegungs-Build). Reine Pfadauskunft:
/// liegt nur das alte `hugoafk`-Verzeichnis vor, wird dessen Pfad geliefert.
pub fn dir() -> PathBuf {
    let base = config_base();
    let dir = base.join("afksystems");
    let legacy = base.join("hugoafk");
    if !dir.exists() && legacy.is_dir() {
        return legacy;
    }
    dir
}

/// Einmalige Umbenennung `hugoafk` -> `afksystems`, damit bestehende Anmeldungen erhalten
/// bleiben. Wird nur aufgerufen, wenn tatsächlich mit Konten gearbeitet wird; schlägt sie fehl,
/// arbeitet [`dir`] einfach am alten Ort weiter.
pub fn migrate() {
    let base = config_base();
    let dir = base.join("afksystems");
    let legacy = base.join("hugoafk");
    if !dir.exists() && legacy.is_dir() {
        let _ = std::fs::rename(&legacy, &dir);
    }
}

fn config_base() -> PathBuf {
    match std::env::var("XDG_CONFIG_HOME") {
        Ok(xdg) if !xdg.trim().is_empty() => PathBuf::from(xdg),
        _ => {
            let home = std::env::var("USERPROFILE")
                .or_else(|_| std::env::var("HOME"))
                .unwrap_or_else(|_| ".".to_string());
            PathBuf::from(home).join(".config")
        }
    }
}

/// Ein Befehl, der nach dem Beitritt läuft. `repeat_seconds = 0` heißt: nur einmal je Beitritt.
#[derive(Clone)]
pub struct AutoCommand {
    /// Was gesendet wird, z. B. `/afk`. Ohne `/` geht es als normale Chat-Nachricht raus.
    pub command: String,
    /// Sekunden nach dem Beitritt bis zur ersten Ausführung.
    pub delay_seconds: u64,
    /// Wiederholung in Sekunden; 0 = nur einmal je Beitritt.
    pub repeat_seconds: u64,
}

/// Alles, was der Client zur Laufzeit braucht.
#[derive(Clone)]
pub struct Options {
    pub server: String,
    pub account: Option<String>,
    pub protocol: &'static Protocol,
    pub commands: Vec<AutoCommand>,

    pub auto_reconnect: bool,
    pub reconnect_delay_seconds: u64,
    pub max_backoff_seconds: u64,

    pub color: bool,
    /// Keine Statusmeldungen – nur noch Chat auf der Standardausgabe.
    pub quiet: bool,
    pub chat_min_delay_ms: u64,
}

impl Default for Options {
    fn default() -> Self {
        Options {
            server: String::new(),
            account: None,
            protocol: DEFAULT,
            commands: Vec::new(),
            auto_reconnect: true,
            reconnect_delay_seconds: 5,
            max_backoff_seconds: 60,
            color: true,
            quiet: false,
            chat_min_delay_ms: 1000,
        }
    }
}

/// Was der Aufruf verlangt.
pub enum Command {
    /// Verbinden und laufen.
    Run(Box<Options>),
    /// Microsoft-Konto anmelden und beenden.
    Login,
    /// Gespeicherte Konten auflisten und beenden.
    Accounts,
    Help,
}

/// Startargumente auswerten. Der Fehlertext ist bereits für den Nutzer formuliert.
pub fn parse<I: IntoIterator<Item = String>>(args: I) -> Result<Command, String> {
    let mut o = Options::default();
    // Wartezeit nach dem Beitritt, bevor der erste Befehl rausgeht. Der Server braucht einen
    // Moment, bis er Chat von uns überhaupt annimmt.
    let mut join_delay = 4u64;
    let mut args = args.into_iter().peekable();

    while let Some(arg) = args.next() {
        let mut value = |name: &str| -> Result<String, String> {
            args.next()
                .ok_or_else(|| format!("{} braucht einen Wert.", name))
        };

        match arg.as_str() {
            "-h" | "--help" => return Ok(Command::Help),
            "--login" => return Ok(Command::Login),
            "--accounts" => return Ok(Command::Accounts),

            "-s" | "--server" => o.server = value("--server")?,
            "-a" | "--account" => o.account = Some(value("--account")?),
            "-m" | "--mc" | "--version" => {
                let name = value("--mc")?;
                o.protocol = Protocol::find(&name).ok_or_else(|| {
                    format!("Unbekannte Version '{}'. Möglich: {}", name, Protocol::names())
                })?;
            }
            "-c" | "--cmd" => o.commands.push(parse_command(&value("--cmd")?)?),
            "--join-delay" => join_delay = number(&value("--join-delay")?, "--join-delay")?,
            "--reconnect-delay" => {
                o.reconnect_delay_seconds =
                    number(&value("--reconnect-delay")?, "--reconnect-delay")?.max(1)
            }
            "--max-backoff" => {
                o.max_backoff_seconds = number(&value("--max-backoff")?, "--max-backoff")?.max(1)
            }
            "--chat-delay" => {
                o.chat_min_delay_ms =
                    number(&value("--chat-delay")?, "--chat-delay")?.max(MIN_CHAT_DELAY_MS)
            }
            "--no-reconnect" => o.auto_reconnect = false,
            "--no-color" => o.color = false,
            "-q" | "--quiet" => o.quiet = true,

            other if !other.starts_with('-') && o.server.is_empty() => o.server = other.to_string(),
            other => return Err(format!("Unbekannte Option '{}'. --help zeigt alle.", other)),
        }
    }

    if o.server.trim().is_empty() {
        return Err("Kein Server angegeben. Beispiel: afk --server mc.example.net".to_string());
    }
    o.server = o.server.trim().to_string();
    for command in &mut o.commands {
        command.delay_seconds = join_delay;
    }
    Ok(Command::Run(Box::new(o)))
}

/// `--cmd /afk` (einmalig) oder `--cmd 300:/afk` (alle 300 s).
fn parse_command(input: &str) -> Result<AutoCommand, String> {
    let (repeat, command) = match input.split_once(':') {
        // Nur zerlegen, wenn vorn wirklich eine Zahl steht – sonst zerschnitte man Befehle,
        // die selbst einen Doppelpunkt tragen.
        Some((head, rest)) if head.trim().parse::<u64>().is_ok() => {
            (head.trim().parse::<u64>().unwrap(), rest)
        }
        _ => (0, input),
    };
    let command = command.trim();
    if command.is_empty() {
        return Err("--cmd braucht einen Befehl, z. B. --cmd 300:/afk".to_string());
    }
    Ok(AutoCommand {
        command: command.to_string(),
        delay_seconds: 4,
        repeat_seconds: if repeat == 0 {
            0
        } else {
            repeat.max(MIN_REPEAT_SECONDS)
        },
    })
}

fn number(text: &str, name: &str) -> Result<u64, String> {
    text.trim()
        .parse::<u64>()
        .map_err(|_| format!("{} braucht eine Zahl, nicht '{}'.", name, text))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse_args(args: &[&str]) -> Result<Command, String> {
        parse(args.iter().map(|s| s.to_string()))
    }

    fn options(args: &[&str]) -> Options {
        match parse_args(args) {
            Ok(Command::Run(o)) => *o,
            Ok(_) => panic!("kein Lauf-Befehl"),
            Err(e) => panic!("{}", e),
        }
    }

    #[test]
    fn server_geht_auch_ohne_option() {
        assert_eq!(options(&["mc.example.net"]).server, "mc.example.net");
        assert_eq!(
            options(&["--server", "mc.example.net:25566"]).server,
            "mc.example.net:25566"
        );
    }

    #[test]
    fn ohne_server_gibt_es_eine_fehlermeldung() {
        assert!(parse_args(&["--quiet"]).is_err());
    }

    #[test]
    fn versionen_werden_geprueft() {
        assert_eq!(options(&["x", "--mc", "1.21.1"]).protocol.version, 767);
        assert_eq!(options(&["x", "--mc", "26.2"]).protocol.version, 776);
        assert_eq!(options(&["x"]).protocol.name, "26.1");
        assert!(parse_args(&["x", "--mc", "1.7.10"]).is_err());
    }

    #[test]
    fn befehle_mit_und_ohne_wiederholung() {
        let o = options(&["x", "-c", "/afk", "-c", "300:/lobby", "--join-delay", "9"]);
        assert_eq!(o.commands.len(), 2);
        assert_eq!(o.commands[0].command, "/afk");
        assert_eq!(o.commands[0].repeat_seconds, 0);
        assert_eq!(o.commands[1].command, "/lobby");
        assert_eq!(o.commands[1].repeat_seconds, 300);
        // Die Startverzögerung gilt für alle Befehle, egal wo --join-delay steht.
        assert!(o.commands.iter().all(|c| c.delay_seconds == 9));
    }

    /// Ein Doppelpunkt mitten im Befehl darf nicht als Intervall missverstanden werden.
    #[test]
    fn doppelpunkt_im_befehl_bleibt_erhalten() {
        let o = options(&["x", "-c", "/msg hugo: hallo"]);
        assert_eq!(o.commands[0].command, "/msg hugo: hallo");
        assert_eq!(o.commands[0].repeat_seconds, 0);
    }

    /// Zu schnelle Wiederholung fängt nur den Spam-Schutz des Servers ein.
    #[test]
    fn wiederholung_hat_eine_untergrenze() {
        assert_eq!(options(&["x", "-c", "1:/afk"]).commands[0].repeat_seconds, 5);
    }
}
