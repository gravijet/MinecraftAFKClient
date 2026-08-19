//! Startargumente. Der Client hat weder Menü noch Konfigurationsdatei: alles, was er wissen
//! muss, steht im Startbefehl. Gespeichert wird nur, was gespeichert werden muss – die
//! Microsoft-Konten unter `~/.config/afksystems/accounts/`.

use crate::proto::{Protocol, DEFAULT};
use crate::proxy::Proxy;
use crate::rules::{Spec, Trigger};
use std::path::PathBuf;

/// Untergrenze für Wiederholungen: schneller löst nur der Spam-Schutz des Servers aus.
const MIN_REPEAT_SECONDS: u64 = 5;
/// Untergrenze für den Abstand zweier ausgehender Nachrichten.
const MIN_CHAT_DELAY_MS: u64 = 200;
/// Untergrenze für die Anti-AFK-Aktionen: häufiger ist kein Zappeln mehr, sondern auffällig.
const MIN_ANTIAFK_SECONDS: u64 = 15;

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

/// Datei ersetzen, ohne sie je halb zu hinterlassen: erst vollständig daneben schreiben, dann
/// an ihren Platz umbenennen.
///
/// `fs::write` kürzt die Zieldatei sofort und füllt sie erst danach. Wird der Client genau
/// dazwischen beendet (Kill, Stromausfall, volle Platte), bleibt eine halbe Datei zurück – bei
/// einem Konto heißt das: nur noch mit `--login` zu retten. Ein Umbenennen im selben Verzeichnis
/// ist dagegen unteilbar; es liegt immer entweder der alte oder der neue Stand da.
pub fn write_atomic(path: &std::path::Path, text: &str) {
    if let Some(parent) = path.parent() {
        let _ = std::fs::create_dir_all(parent);
    }
    let mut temporary = path.to_path_buf();
    // Nicht `with_extension`: die Endung muss erhalten bleiben, damit ein liegengebliebener
    // Rest niemals als Konto durchgeht (dort zählt genau die Endung `.json`).
    let mut name = path.file_name().unwrap_or_default().to_os_string();
    name.push(".neu");
    temporary.set_file_name(name);

    if std::fs::write(&temporary, text).is_err() {
        return;
    }
    // Unter Windows scheitert `rename` auf eine vorhandene Datei – dort muss die alte zuerst weg.
    // Das ist genau das Fenster, das es sonst gar nicht gäbe; es bleibt aber ungleich kleiner als
    // ein vollständiger Schreibvorgang.
    #[cfg(windows)]
    let _ = std::fs::remove_file(path);
    if std::fs::rename(&temporary, path).is_err() {
        // Ließ sich nicht umbenennen (etwa über Dateisystemgrenzen hinweg): lieber direkt
        // schreiben als gar nicht zu speichern.
        let _ = std::fs::write(path, text);
        let _ = std::fs::remove_file(&temporary);
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
    /// Name für den Offline-Modus („Cracked"). Schließt `account` aus.
    pub offline: Option<String>,
    pub protocol: &'static Protocol,
    pub commands: Vec<AutoCommand>,

    /// Proxy nur für die Spielverbindung.
    pub proxy: Option<Proxy>,
    /// Was im Handshake als Zieladresse steht, falls es nicht der echte Server sein soll.
    pub fakehost: Option<(String, u16)>,

    /// Makros: Auslöser -> Aktion.
    pub rules: Vec<Spec>,
    /// Sperrzeit je Regel, damit eine Regel sich nicht selbst nachtriggert.
    pub rule_cooldown_seconds: u64,

    pub color: bool,
    /// Keine Statusmeldungen – nur noch Chat auf der Standardausgabe.
    pub quiet: bool,
    /// Maschinenlesbare `@event`-Zeilen auf der Fehlerausgabe.
    pub events: bool,
    pub chat_min_delay_ms: u64,

    /// Sekunden zwischen zwei Anti-AFK-Aktionen; 0 = aus. Nur im Premium-Build wirksam.
    pub antiafk_seconds: u64,
    /// Dauerhaft geduckt beitreten. Nur im Premium-Build wirksam.
    pub sneak: bool,

    /// Sichtweite in Chunks, die dem Server gemeldet wird. Klein zu bleiben ist der billigste
    /// Hebel überhaupt: der Server schickt dann viel weniger Chunkdaten, die der Client sonst
    /// entschlüsseln und entpacken müsste, ohne sie je zu benutzen.
    pub view_distance: u8,
    /// Startet die Live-Ansicht von selbst? `None` = keine Angabe, dann entscheidet die
    /// Bauform: die POV-Datei fängt selbst an, Ultra wartet auf `:pov live`. `--pov an|aus`
    /// dreht das in beide Richtungen um.
    pub pov_autostart: Option<bool>,
    /// Bildgröße in Pixeln, falls auf der Kommandozeile vorgegeben.
    pub pov_size: Option<(usize, usize)>,
    /// Bilder je Sekunde.
    pub pov_fps: usize,

    /// Optionen, die dieser Client angenommen, aber nicht umgesetzt hat (weil es sie nur im
    /// Java-Client gibt). Wird beim Start einmal genannt.
    pub ignored: Vec<String>,
}

/// Sichtweite, die der jeweilige Build sinnvollerweise anfordert. Ohne Live-Ansicht liest der
/// Client keinen einzigen Chunk – dann ist das Minimum genau richtig. Mit Live-Ansicht reicht
/// etwas mehr als die 72 Blöcke, die die Ansicht überhaupt weit sieht.
pub const DEFAULT_VIEW_DISTANCE: u8 = if cfg!(feature = "pov") { 6 } else { 2 };

impl Default for Options {
    fn default() -> Self {
        Options {
            server: String::new(),
            account: None,
            offline: None,
            protocol: DEFAULT,
            commands: Vec::new(),
            proxy: None,
            fakehost: None,
            rules: Vec::new(),
            rule_cooldown_seconds: 3,
            color: true,
            quiet: false,
            events: false,
            chat_min_delay_ms: 1000,
            antiafk_seconds: 0,
            sneak: false,
            view_distance: DEFAULT_VIEW_DISTANCE,
            pov_autostart: None,
            pov_size: None,
            pov_fps: 8,
            ignored: Vec::new(),
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
            "--offline" | "--cracked" => o.offline = Some(value("--offline")?),
            "--proxy" => o.proxy = Some(Proxy::parse(&value("--proxy")?)?),
            "--fakehost" => o.fakehost = Some(parse_fakehost(&value("--fakehost")?)?),
            "--on" => o.rules.push(parse_rule(&value("--on")?)?),
            "--on-cooldown" => {
                o.rule_cooldown_seconds = number(&value("--on-cooldown")?, "--on-cooldown")?
            }
            "--events" => o.events = true,
            "--antiafk" => {
                // 0 heißt ausdrücklich „aus"; alles andere bekommt die Untergrenze.
                let seconds = number(&value("--antiafk")?, "--antiafk")?;
                o.antiafk_seconds = if seconds == 0 {
                    0
                } else {
                    seconds.max(MIN_ANTIAFK_SECONDS)
                };
            }
            "--sneak" => o.sneak = true,
            "--view-distance" | "--sichtweite" => {
                o.view_distance =
                    number(&value("--view-distance")?, "--view-distance")?.clamp(2, 32) as u8
            }
            "--pov" | "--ansicht" => {
                let mode = value("--pov")?;
                o.pov_autostart = match mode.trim().to_ascii_lowercase().as_str() {
                    "an" | "on" | "live" | "ein" | "1" => Some(true),
                    "aus" | "off" | "0" => Some(false),
                    other => {
                        return Err(format!(
                            "--pov nimmt 'an' oder 'aus', nicht '{}'.",
                            other
                        ))
                    }
                };
            }
            "--pov-size" | "--pov-groesse" => o.pov_size = Some(parse_size(&value("--pov-size")?)?),
            "--pov-fps" => o.pov_fps = number(&value("--pov-fps")?, "--pov-fps")?.clamp(1, 20) as usize,
            "-m" | "--mc" | "--version" => {
                let name = value("--mc")?;
                o.protocol = Protocol::find(&name).ok_or_else(|| {
                    format!(
                        "Unbekannte Version '{}'. Möglich: {}",
                        name,
                        Protocol::names()
                    )
                })?;
            }
            "-c" | "--cmd" => o.commands.push(parse_command(&value("--cmd")?)?),
            "--join-delay" => join_delay = number(&value("--join-delay")?, "--join-delay")?,
            "--chat-delay" => {
                o.chat_min_delay_ms =
                    number(&value("--chat-delay")?, "--chat-delay")?.max(MIN_CHAT_DELAY_MS)
            }
            "--no-color" => o.color = false,
            "-q" | "--quiet" => o.quiet = true,

            // Nur der Java-Client verbindet nach einem Abbruch neu. Ein Panel schickt allen
            // Bauformen dieselbe Befehlszeile – daran darf der Start nicht scheitern.
            "--no-reconnect" => o.ignored.push(arg.clone()),
            "--reconnect-delay" | "--max-backoff" => {
                value(&arg)?;
                o.ignored.push(arg.clone());
            }

            other if !other.starts_with('-') && o.server.is_empty() => o.server = other.to_string(),
            other => return Err(format!("Unbekannte Option '{}'. --help zeigt alle.", other)),
        }
    }

    if o.server.trim().is_empty() {
        return Err("Kein Server angegeben. Beispiel: afk --server mc.example.net".to_string());
    }
    if o.account.is_some() && o.offline.is_some() {
        return Err(
            "--account und --offline schließen sich aus: entweder Microsoft-Konto oder Offline-Name."
                .to_string(),
        );
    }
    o.server = o.server.trim().to_string();
    check_server(&o.server)?;
    for command in &mut o.commands {
        command.delay_seconds = join_delay;
    }
    Ok(Command::Run(Box::new(o)))
}

/// Serveradresse auf einen brauchbaren Port prüfen.
///
/// Zerlegt wird sie später in [`crate::client`]; dort ist ein unlesbarer Port stillschweigend
/// zu 25565 geworden. Ein Tippfehler wie `mc.example.net:2556x` führte damit zu einer
/// Verbindung auf einen ganz anderen Port – und zu einer Fehlersuche am falschen Ende.
fn check_server(server: &str) -> Result<(), String> {
    let port = if let Some(rest) = server.strip_prefix('[') {
        // `[::1]:25565`: nur was hinter der schließenden Klammer steht, kann ein Port sein.
        match rest.split_once(']') {
            Some((_, rest)) => rest.strip_prefix(':'),
            None => return Err(format!("Serveradresse ohne schließende Klammer: '{}'", server)),
        }
    } else if server.matches(':').count() > 1 {
        None // nackte IPv6-Adresse – da ist kein Port dabei
    } else {
        server.rsplit_once(':').map(|(_, port)| port)
    };

    match port {
        Some(port) if port.trim().parse::<u16>().is_err() => Err(format!(
            "Server-Port ist keine Zahl zwischen 0 und 65535: '{}'. Beispiel: mc.example.net:25565",
            port
        )),
        _ => Ok(()),
    }
}

/// Grenzen der Live-Ansicht. Dieselben Werte prüft [`crate::pov`] noch einmal; hier stehen sie,
/// damit ein unmöglicher Wunsch sofort eine klare Meldung bekommt statt still zurechtgebogen zu
/// werden.
const POV_WIDTH: std::ops::RangeInclusive<usize> = 24..=160;
const POV_HEIGHT: std::ops::RangeInclusive<usize> = 12..=80;

/// `--pov-size 160x80` (auch `160*80` oder `160 80`).
fn parse_size(input: &str) -> Result<(usize, usize), String> {
    let text = input.trim().to_ascii_lowercase();
    let (width, height) = text
        .split_once(['x', '*', ':'])
        .or_else(|| text.split_once(char::is_whitespace))
        .ok_or_else(|| format!("--pov-size braucht <breite>x<hoehe>, z. B. 160x80. Bekommen: '{}'", input))?;
    let parse = |part: &str, what: &str, range: std::ops::RangeInclusive<usize>| {
        let value = part
            .trim()
            .parse::<usize>()
            .map_err(|_| format!("--pov-size: {} ist keine Zahl: '{}'", what, part))?;
        // Nicht stillschweigend zurechtbiegen: wer 4000x3000 tippt, meint etwas anderes als
        // 160x80 und soll das erfahren.
        if !range.contains(&value) {
            return Err(format!(
                "--pov-size: {} muss zwischen {} und {} liegen, nicht {}.",
                what,
                range.start(),
                range.end(),
                value
            ));
        }
        Ok(value)
    };
    Ok((
        parse(width, "Breite", POV_WIDTH)?,
        parse(height, "Hoehe", POV_HEIGHT)?,
    ))
}

/// `--fakehost lobby.example.net` oder `--fakehost lobby.example.net:25565`.
///
/// Steht kein Port dabei, wird der des echten Servers eingesetzt – das erledigt der Client,
/// hier steht dann die 0.
fn parse_fakehost(input: &str) -> Result<(String, u16), String> {
    let text = input.trim();
    // `[::1]:25565` bzw. eine nackte IPv6-Adresse dürfen nicht am Doppelpunkt zerfallen.
    if let Some(end) = text.strip_prefix('[').and_then(|v| v.find(']')) {
        let host = text[1..end + 1].to_string();
        let rest = &text[end + 2..];
        let port = match rest.strip_prefix(':') {
            Some(port) => port
                .trim()
                .parse::<u16>()
                .map_err(|_| format!("--fakehost: Port ist keine Zahl: '{}'", port))?,
            None => 0,
        };
        // Dieselbe Prüfung wie unten: `[]` ist genauso wenig ein Name wie eine leere Angabe.
        if host.trim().is_empty() {
            return Err(
                "--fakehost braucht einen Namen, z. B. --fakehost play.example.net".to_string(),
            );
        }
        return Ok((host, port));
    }
    if text.matches(':').count() > 1 {
        return Ok((text.to_string(), 0)); // nackte IPv6-Adresse
    }
    let (host, port) = match text.rsplit_once(':') {
        Some((host, port)) => (
            host,
            port.trim()
                .parse::<u16>()
                .map_err(|_| format!("--fakehost: Port ist keine Zahl: '{}'", port))?,
        ),
        None => (text, 0),
    };
    if host.trim().is_empty() {
        return Err(
            "--fakehost braucht einen Namen, z. B. --fakehost play.example.net".to_string(),
        );
    }
    Ok((host.trim().to_string(), port))
}

/// `--on <auslöser>=<aktion>`, z. B. `--on join=/afk`, `--on tod=/spawn`,
/// `--on chat:du bist afk=/lobby`.
fn parse_rule(input: &str) -> Result<Spec, String> {
    let (head, action) = input.split_once('=').ok_or_else(|| {
        format!(
            "--on braucht <auslöser>=<aktion>, z. B. --on join=/afk. Bekommen: '{}'",
            input
        )
    })?;
    let action = action.trim();
    if action.is_empty() {
        return Err("--on braucht eine Aktion hinter dem '=', z. B. --on join=/afk".to_string());
    }
    let head = head.trim();
    let trigger = match head.to_ascii_lowercase().as_str() {
        "join" | "beitritt" => Trigger::Join,
        "world" | "welt" | "server" => Trigger::World,
        "death" | "tod" => Trigger::Death,
        _ => match head.split_once(':') {
            Some((kind, text)) if matches!(kind.trim().to_ascii_lowercase().as_str(), "chat") => {
                let text = text.trim();
                if text.is_empty() {
                    return Err("--on chat: braucht einen Text, auf den gewartet wird.".to_string());
                }
                Trigger::Chat(text.to_ascii_lowercase())
            }
            _ => {
                return Err(format!(
                    "Unbekannter Auslöser '{}'. Möglich: join, world, death, chat:<text>",
                    head
                ))
            }
        },
    };
    Ok(Spec {
        trigger,
        action: action.to_string(),
    })
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

    /// Ein vertippter Port ist stillschweigend zu 25565 geworden – der Client verband sich dann
    /// auf einen ganz anderen Port, und die Fehlersuche begann am falschen Ende.
    #[test]
    fn vertippter_port_wird_gemeldet() {
        assert!(parse_args(&["mc.example.net:2556x"]).is_err());
        assert!(parse_args(&["mc.example.net:"]).is_err());
        assert!(parse_args(&["mc.example.net:99999"]).is_err());
        assert!(parse_args(&["[::1"]).is_err());
        // Gültige Schreibweisen bleiben gültig.
        assert!(parse_args(&["mc.example.net"]).is_ok());
        assert!(parse_args(&["mc.example.net:25566"]).is_ok());
        assert!(parse_args(&["[::1]:25566"]).is_ok());
        assert!(parse_args(&["[::1]"]).is_ok());
        assert!(parse_args(&["::1"]).is_ok());
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
        assert_eq!(
            options(&["x", "-c", "1:/afk"]).commands[0].repeat_seconds,
            5
        );
    }

    #[test]
    fn offline_und_konto_zugleich_geht_nicht() {
        assert_eq!(
            options(&["x", "--offline", "Hugo"]).offline.unwrap(),
            "Hugo"
        );
        assert!(parse_args(&["x", "--offline", "Hugo", "-a", "user@example.invalid"]).is_err());
    }

    #[test]
    fn proxy_wird_uebernommen() {
        let o = options(&["x", "--proxy", "socks5://192.0.2.1:1080"]);
        assert_eq!(o.proxy.unwrap().describe(), "socks5://192.0.2.1:1080");
        assert!(parse_args(&["x", "--proxy", "ftp://192.0.2.1:21"]).is_err());
    }

    /// Ohne Port steht hier die 0 – den echten setzt der Client ein.
    #[test]
    fn fakehost_mit_und_ohne_port() {
        assert_eq!(
            options(&["x", "--fakehost", "play.example.net"]).fakehost,
            Some(("play.example.net".to_string(), 0))
        );
        assert_eq!(
            options(&["x", "--fakehost", "play.example.net:25566"]).fakehost,
            Some(("play.example.net".to_string(), 25566))
        );
        assert!(parse_args(&["x", "--fakehost", "play.example.net:xx"]).is_err());
    }

    #[test]
    fn regeln_werden_gelesen() {
        let o = options(&[
            "x",
            "--on",
            "join=/afk",
            "--on",
            "tod=/spawn",
            "--on",
            "chat:Du bist AFK=/lobby",
        ]);
        assert_eq!(o.rules.len(), 3);
        assert!(matches!(o.rules[0].trigger, Trigger::Join));
        assert_eq!(o.rules[0].action, "/afk");
        assert!(matches!(o.rules[1].trigger, Trigger::Death));
        // Chat-Auslöser werden kleingeschrieben verglichen.
        match &o.rules[2].trigger {
            Trigger::Chat(text) => assert_eq!(text, "du bist afk"),
            _ => panic!("kein Chat-Auslöser"),
        }
    }

    /// Ein '=' in der Aktion darf nicht stören – geteilt wird beim ersten.
    #[test]
    fn regel_aktion_darf_gleichheitszeichen_enthalten() {
        let o = options(&["x", "--on", "join=/setwarp name=hier"]);
        assert_eq!(o.rules[0].action, "/setwarp name=hier");
    }

    #[test]
    fn unsinnige_regeln_werden_abgelehnt() {
        assert!(parse_args(&["x", "--on", "join"]).is_err());
        assert!(parse_args(&["x", "--on", "join="]).is_err());
        assert!(parse_args(&["x", "--on", "chat:=/afk"]).is_err());
        assert!(parse_args(&["x", "--on", "sonstwas=/afk"]).is_err());
    }

    /// 0 heißt aus, alles andere bekommt die Untergrenze.
    #[test]
    fn antiafk_hat_eine_untergrenze() {
        assert_eq!(options(&["x"]).antiafk_seconds, 0);
        assert_eq!(options(&["x", "--antiafk", "0"]).antiafk_seconds, 0);
        assert_eq!(options(&["x", "--antiafk", "3"]).antiafk_seconds, 15);
        assert_eq!(options(&["x", "--antiafk", "90"]).antiafk_seconds, 90);
    }

    /// Ein Panel schickt allen Bauformen dieselbe Befehlszeile. Optionen, die es nur im
    /// Java-Client gibt, dürfen den Start deshalb nicht abbrechen.
    #[test]
    fn java_optionen_werden_angenommen_und_gemeldet() {
        let o = options(&[
            "x",
            "--no-reconnect",
            "--reconnect-delay",
            "5",
            "--max-backoff",
            "60",
        ]);
        assert_eq!(o.ignored, vec!["--no-reconnect", "--reconnect-delay", "--max-backoff"]);
        assert_eq!(o.server, "x");
        // Ein echter Tippfehler bleibt ein Fehler.
        assert!(parse_args(&["x", "--kein-schalter"]).is_err());
    }

    /// Ohne Live-Ansicht wird kein Chunk gelesen – dann muss die kleinste Sichtweite raus.
    #[test]
    fn sichtweite_hat_grenzen() {
        assert_eq!(options(&["x"]).view_distance, DEFAULT_VIEW_DISTANCE);
        assert_eq!(options(&["x", "--view-distance", "0"]).view_distance, 2);
        assert_eq!(options(&["x", "--view-distance", "99"]).view_distance, 32);
        assert_eq!(options(&["x", "--view-distance", "12"]).view_distance, 12);
    }

    #[test]
    fn pov_groesse_und_takt() {
        assert_eq!(options(&["x", "--pov-size", "160x80"]).pov_size, Some((160, 80)));
        assert_eq!(options(&["x", "--pov-size", "80*40"]).pov_size, Some((80, 40)));
        assert_eq!(options(&["x", "--pov-fps", "99"]).pov_fps, 20);
        // Ohne Angabe entscheidet die Bauform, nicht die Optionsauswertung.
        assert_eq!(options(&["x"]).pov_autostart, None);
        assert_eq!(options(&["x", "--pov", "an"]).pov_autostart, Some(true));
        assert_eq!(options(&["x", "--pov", "aus"]).pov_autostart, Some(false));
        assert!(parse_args(&["x", "--pov-size", "gross"]).is_err());
        assert!(parse_args(&["x", "--pov", "vielleicht"]).is_err());
    }

    /// Eine unmögliche Bildgröße wurde bisher stillschweigend auf das Erlaubte gestaucht – wer
    /// `4000x3000` tippt, meinte aber nicht `160x80` und soll das erfahren.
    #[test]
    fn pov_groesse_wird_gegen_die_grenzen_geprueft() {
        assert!(parse_args(&["x", "--pov-size", "4000x3000"]).is_err());
        assert!(parse_args(&["x", "--pov-size", "0x0"]).is_err());
        assert!(parse_args(&["x", "--pov-size", "23x40"]).is_err());
        assert!(parse_args(&["x", "--pov-size", "64x81"]).is_err());
        // Genau auf den Grenzen bleibt es gültig.
        assert_eq!(options(&["x", "--pov-size", "24x12"]).pov_size, Some((24, 12)));
        assert_eq!(options(&["x", "--pov-size", "160x80"]).pov_size, Some((160, 80)));
    }

    /// Der Klammer-Zweig hat einen leeren Namen bisher durchgelassen.
    #[test]
    fn fakehost_ohne_namen_wird_abgelehnt() {
        assert!(parse_args(&["x", "--fakehost", "[]"]).is_err());
        assert!(parse_args(&["x", "--fakehost", "[]:25565"]).is_err());
        assert!(parse_args(&["x", "--fakehost", "  "]).is_err());
        // Eine echte IPv6-Adresse bleibt erlaubt.
        assert_eq!(
            options(&["x", "--fakehost", "[::1]:25566"]).fakehost,
            Some(("::1".to_string(), 25566))
        );
    }

    /// Ein Abbruch mitten im Schreiben darf keine halbe Datei hinterlassen: entweder steht der
    /// alte Stand da oder der neue.
    #[test]
    fn atomares_schreiben_ersetzt_vollstaendig() {
        let dir = std::env::temp_dir().join(format!("afk-test-{}", std::process::id()));
        let file = dir.join("konto.json");
        write_atomic(&file, "{\"a\":1}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"a\":1}");
        // Ersetzen lässt eine vorhandene Datei nicht halb zurück.
        write_atomic(&file, "{\"b\":2}");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "{\"b\":2}");
        // Und die Zwischendatei ist wieder weg.
        assert!(!dir.join("konto.json.neu").exists());
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn schalter_ohne_wert() {
        let o = options(&["x", "--events", "--sneak"]);
        assert!(o.events);
        assert!(o.sneak);
    }
}
