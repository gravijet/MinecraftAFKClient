//! `~/.config/hugoafk/config.json` – **dieselbe Datei wie beim Java-Client**, gleiche Feldnamen.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// Untergrenze für Wiederholungen: schneller löst nur der Spam-Schutz des Servers aus.
const MIN_REPEAT_SECONDS: u64 = 5;

/// Ein wiederkehrender Befehl. `repeat_seconds = 0` heißt: nur einmal je Beitritt.
#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct AutoCommand {
    /// Was gesendet wird, z. B. "/afk". Ohne "/" wird es als normale Chat-Nachricht gesendet.
    pub command: String,
    /// Sekunden nach dem Beitritt bis zur ersten Ausführung.
    pub delay_seconds: u64,
    /// Wiederholung in Sekunden; 0 = nur einmal je Beitritt.
    pub repeat_seconds: u64,
    pub enabled: bool,
}

impl Default for AutoCommand {
    fn default() -> Self {
        AutoCommand {
            command: String::new(),
            delay_seconds: 4,
            repeat_seconds: 0,
            enabled: true,
        }
    }
}

impl AutoCommand {
    pub fn new(command: &str, delay_seconds: u64, repeat_seconds: u64) -> AutoCommand {
        AutoCommand {
            command: command.trim().to_string(),
            delay_seconds,
            // Wie in `Config::normalize`: schneller als alle 5 s löst nur den Spam-Schutz aus.
            repeat_seconds: if repeat_seconds == 0 {
                0
            } else {
                repeat_seconds.max(MIN_REPEAT_SECONDS)
            },
            enabled: true,
        }
    }

    /// Kurzbeschreibung für Menü und Hilfe.
    pub fn describe(&self) -> String {
        let when = if self.repeat_seconds == 0 {
            format!("einmalig, {} s nach Beitritt", self.delay_seconds)
        } else {
            format!(
                "erst nach {} s, dann alle {}",
                self.delay_seconds,
                pretty_seconds(self.repeat_seconds)
            )
        };
        format!("{}   ({})", self.command, when)
    }
}

/// 90 -> "1 min 30 s", 300 -> "5 min", 45 -> "45 s"
pub fn pretty_seconds(seconds: u64) -> String {
    if seconds < 60 {
        return format!("{} s", seconds);
    }
    let (minutes, rest) = (seconds / 60, seconds % 60);
    if rest == 0 {
        format!("{} min", minutes)
    } else {
        format!("{} min {} s", minutes, rest)
    }
}

#[derive(Serialize, Deserialize, Clone)]
#[serde(rename_all = "camelCase", default)]
pub struct Config {
    /// Zuletzt genutzte Server-Adresse (host[:port]).
    pub last_server: String,
    /// Name des aktiven Microsoft-Kontos (Datei accounts/<name>.json).
    pub active_account: String,

    /// Bei Verbindungsabbruch automatisch neu verbinden.
    pub auto_reconnect: bool,
    /// Basis-Wartezeit vor dem ersten Reconnect (Sekunden); danach exponentieller Backoff.
    pub reconnect_delay_seconds: u64,
    /// Obergrenze für den Backoff in Sekunden.
    pub max_backoff_seconds: u64,

    /// Farbige Ausgabe (ANSI).
    pub color_output: bool,
    /// Mindestabstand zwischen zwei ausgehenden Nachrichten in ms (gegen Spam-Kick).
    pub chat_min_delay_ms: u64,

    /// Befehle, die nach einem echten Beitritt (Proxy-Login) laufen – beliebig viele,
    /// jeder mit eigener Startverzögerung und eigenem Wiederholungsintervall.
    pub commands: Vec<AutoCommand>,

    // ---- Altfelder (einzelner Auto-Befehl) ----
    // Werden beim ersten Start in `commands` überführt und danach nicht mehr gelesen.
    // Sie bleiben in der Datei, damit eine ältere Client-Version nicht stolpert.
    pub auto_command_enabled: bool,
    pub auto_command: String,
    pub auto_command_delay_seconds: u64,

    #[serde(skip)]
    file: PathBuf,
}

impl Default for Config {
    fn default() -> Self {
        Config {
            last_server: String::new(),
            active_account: String::new(),
            auto_reconnect: true,
            reconnect_delay_seconds: 5,
            max_backoff_seconds: 60,
            color_output: true,
            chat_min_delay_ms: 1000,
            // Leer, damit eine bestehende Datei ohne `commands` nichts Erfundenes bekommt –
            // `load` setzt den Standard `/afk` nur für eine ganz neue Konfiguration.
            commands: Vec::new(),
            auto_command_enabled: true,
            auto_command: String::new(),
            auto_command_delay_seconds: 4,
            file: PathBuf::new(),
        }
    }
}

impl Config {
    /// Lädt die Konfiguration; eine fehlende oder kaputte Datei führt nie zum Abbruch.
    pub fn load(file: &Path) -> Config {
        let text = std::fs::read_to_string(file).ok();
        let existed = text.is_some();
        let mut config: Config = text
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        config.file = file.to_path_buf();
        if existed {
            config.migrate_legacy_command();
        } else {
            config.commands = vec![AutoCommand::new("/afk", 4, 0)];
        }
        config.normalize();
        config.save();
        config
    }

    /// Eine bestehende Datei mit einzelnem `autoCommand` in die neue Liste überführen.
    /// Das Altfeld wird dabei geleert, damit die Migration nicht bei jedem Start erneut
    /// zuschlägt und einen gelöschten Befehl wiederbelebt.
    fn migrate_legacy_command(&mut self) {
        let legacy = self.auto_command.trim().to_string();
        if legacy.is_empty() {
            return;
        }
        if !self.commands.iter().any(|c| c.command == legacy) {
            self.commands.push(AutoCommand {
                command: legacy,
                delay_seconds: self.auto_command_delay_seconds,
                repeat_seconds: 0,
                enabled: self.auto_command_enabled,
            });
        }
        self.auto_command = String::new();
    }

    fn normalize(&mut self) {
        self.reconnect_delay_seconds = self.reconnect_delay_seconds.max(1);
        self.max_backoff_seconds = self.max_backoff_seconds.max(1);
        self.chat_min_delay_ms = self.chat_min_delay_ms.max(200);
        self.commands.retain(|c| !c.command.trim().is_empty());
        for command in &mut self.commands {
            command.command = command.command.trim().to_string();
            if command.repeat_seconds > 0 {
                command.repeat_seconds = command.repeat_seconds.max(MIN_REPEAT_SECONDS);
            }
        }
    }

    /// Die Befehle, die tatsächlich laufen sollen.
    pub fn active_commands(&self) -> Vec<AutoCommand> {
        self.commands
            .iter()
            .filter(|c| c.enabled && !c.command.trim().is_empty())
            .cloned()
            .collect()
    }

    pub fn save(&self) {
        if self.file.as_os_str().is_empty() {
            return;
        }
        if let Some(parent) = self.file.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        if let Ok(text) = serde_json::to_string_pretty(self) {
            let _ = std::fs::write(&self.file, text);
        }
    }
}
