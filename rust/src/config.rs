//! `~/.config/hugoafk/config.json` – **dieselbe Datei wie beim Java-Client**, gleiche Feldnamen.
//! Unbekannte Felder werden beim Speichern nicht weggeworfen; beide Clients kommen sich nicht ins Gehege.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

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

    /// Nach einem echten Beitritt (Proxy-Login) automatisch einen Befehl senden.
    pub auto_command_enabled: bool,
    /// Automatisch gesendeter Befehl (z. B. "/afk"). Leer = aus.
    pub auto_command: String,
    /// Verzögerung in Sekunden zwischen Beitritt und Auto-Befehl.
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
            auto_command_enabled: true,
            auto_command: "/afk".to_string(),
            auto_command_delay_seconds: 4,
            file: PathBuf::new(),
        }
    }
}

impl Config {
    /// Lädt die Konfiguration; eine fehlende oder kaputte Datei führt nie zum Abbruch.
    pub fn load(file: &Path) -> Config {
        let mut config: Config = std::fs::read_to_string(file)
            .ok()
            .and_then(|text| serde_json::from_str(&text).ok())
            .unwrap_or_default();
        config.file = file.to_path_buf();
        config.normalize();
        config.save();
        config
    }

    fn normalize(&mut self) {
        self.reconnect_delay_seconds = self.reconnect_delay_seconds.max(1);
        self.max_backoff_seconds = self.max_backoff_seconds.max(1);
        self.chat_min_delay_ms = self.chat_min_delay_ms.max(200);
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
