//! Makros: „wenn X passiert, sende Y". Aus `--on <auslöser>=<aktion>`.
//!
//! Bewusst winzig gehalten: keine Skriptsprache, kein Zustand außer einem Zeitstempel je Regel.
//! Ohne `--on` kostet das Ganze zur Laufzeit einen einzigen `is_empty()`-Test je Ereignis; auch
//! die Chat-Prüfung wird erst dann angefasst, wenn wirklich eine Chat-Regel existiert – so bleibt
//! die Zeile bei reinem Mitlesen unangetastet.

use std::sync::Mutex;
use std::time::{Duration, Instant};

/// Worauf eine Regel wartet.
#[derive(Clone)]
pub enum Trigger {
    /// Echter Beitritt (erstes Login-Paket einer Verbindung).
    Join,
    /// Weltwechsel: Unterserver-Wechsel oder Respawn in einer anderen Dimension.
    World,
    /// Eigener Tod.
    Death,
    /// Eine Chat-Zeile enthält diesen Text (bereits kleingeschrieben).
    Chat(String),
}

/// Eine Regel, wie sie auf der Kommandozeile stand.
#[derive(Clone)]
pub struct Spec {
    pub trigger: Trigger,
    /// Was gesendet wird – mit `/` ein Serverbefehl, sonst eine Chat-Nachricht.
    pub action: String,
}

/// Was gerade passiert ist.
pub enum Event<'a> {
    Join,
    World,
    Death,
    Chat(&'a str),
}

impl Event<'_> {
    /// Kurzname für die `@event`-Ausgabe und für Meldungen.
    pub fn name(&self) -> &'static str {
        match self {
            Event::Join => "join",
            Event::World => "world",
            Event::Death => "death",
            Event::Chat(_) => "chat",
        }
    }
}

/// Alle Regeln plus Sperrzeit. Der Zeitstempel je Regel verhindert, dass eine Regel sich über
/// die eigene Antwort des Servers endlos selbst nachtriggert.
pub struct Rules {
    specs: Vec<Spec>,
    cooldown: Duration,
    /// Wann jede Regel zuletzt ausgelöst hat.
    last: Mutex<Vec<Option<Instant>>>,
    /// Gibt es überhaupt eine Chat-Regel? Wenn nicht, wird keine Chat-Zeile angefasst.
    watches_chat: bool,
}

impl Rules {
    pub fn new(specs: Vec<Spec>, cooldown_seconds: u64) -> Rules {
        let watches_chat = specs.iter().any(|s| matches!(s.trigger, Trigger::Chat(_)));
        Rules {
            last: Mutex::new(vec![None; specs.len()]),
            cooldown: Duration::from_secs(cooldown_seconds),
            watches_chat,
            specs,
        }
    }

    /// Reagiert überhaupt eine Regel auf Chat? Der Aufrufer spart sich sonst das Aufbereiten
    /// der Zeile (Farbcodes entfernen, kleinschreiben) – und das bei jeder einzelnen Nachricht.
    pub fn watches_chat(&self) -> bool {
        self.watches_chat
    }

    /// Passende Aktionen zu einem Ereignis, Sperrzeit bereits berücksichtigt.
    ///
    /// Die Chat-Zeile wird nur einmal aufbereitet, egal wie viele Chat-Regeln es gibt.
    pub fn fire(&self, event: &Event) -> Vec<String> {
        if self.specs.is_empty() {
            return Vec::new();
        }
        let haystack = match event {
            Event::Chat(line) if self.watches_chat => Some(prepare(line)),
            _ => None,
        };

        let now = Instant::now();
        let mut last = self.last.lock().unwrap();
        let mut actions = Vec::new();
        for (index, spec) in self.specs.iter().enumerate() {
            let hit = match (&spec.trigger, event) {
                (Trigger::Join, Event::Join) => true,
                // Ein Beitritt ist immer auch ein Weltwechsel – wer auf `world` wartet, will
                // beim ersten Betreten nicht leer ausgehen.
                (Trigger::World, Event::World | Event::Join) => true,
                (Trigger::Death, Event::Death) => true,
                (Trigger::Chat(needle), Event::Chat(_)) => {
                    haystack.as_deref().is_some_and(|text| text.contains(needle))
                }
                _ => false,
            };
            if !hit {
                continue;
            }
            if let Some(previous) = last[index] {
                if now.duration_since(previous) < self.cooldown {
                    continue;
                }
            }
            last[index] = Some(now);
            actions.push(spec.action.clone());
        }
        actions
    }

    /// Nach einer Trennung beginnt alles von vorn: die Sperrzeiten der alten Verbindung
    /// dürfen den neuen Beitritt nicht verschlucken.
    pub fn reset(&self) {
        for entry in self.last.lock().unwrap().iter_mut() {
            *entry = None;
        }
    }
}

/// Chat-Zeile vergleichbar machen: Farbcodes raus, alles klein.
fn prepare(line: &str) -> String {
    strip_ansi(line).to_lowercase()
}

/// ANSI-Farbcodes entfernen. Die Anzeige ist bereits eingefärbt, wenn `--no-color` fehlt; ein
/// `--on chat:...` soll aber unabhängig davon greifen.
///
/// Ohne Escape-Zeichen wird nichts kopiert, was nicht ohnehin kopiert würde.
fn strip_ansi(line: &str) -> String {
    if !line.contains('\x1b') {
        return line.to_string();
    }
    let mut out = String::with_capacity(line.len());
    let mut chars = line.chars();
    while let Some(c) = chars.next() {
        if c != '\x1b' {
            out.push(c);
            continue;
        }
        // Nur die üblichen CSI-Folgen: ESC [ … <Buchstabe>.
        if chars.next() != Some('[') {
            continue;
        }
        for c in chars.by_ref() {
            if c.is_ascii_alphabetic() {
                break;
            }
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rules(specs: Vec<(Trigger, &str)>, cooldown: u64) -> Rules {
        Rules::new(
            specs
                .into_iter()
                .map(|(trigger, action)| Spec {
                    trigger,
                    action: action.to_string(),
                })
                .collect(),
            cooldown,
        )
    }

    #[test]
    fn ereignisse_treffen_die_richtigen_regeln() {
        let r = rules(
            vec![
                (Trigger::Join, "/afk"),
                (Trigger::Death, "/spawn"),
                (Trigger::Chat("du bist afk".into()), "/lobby"),
            ],
            0,
        );
        assert_eq!(r.fire(&Event::Join), vec!["/afk".to_string()]);
        assert_eq!(r.fire(&Event::Death), vec!["/spawn".to_string()]);
        assert_eq!(r.fire(&Event::Chat("Hey, Du bist AFK!")), vec!["/lobby"]);
        assert!(r.fire(&Event::Chat("nichts davon")).is_empty());
    }

    /// Ein Beitritt ist auch ein Weltwechsel – sonst liefe `--on world=` beim Start ins Leere.
    #[test]
    fn beitritt_zaehlt_auch_als_weltwechsel() {
        let r = rules(vec![(Trigger::World, "/warp afk")], 0);
        assert_eq!(r.fire(&Event::Join), vec!["/warp afk".to_string()]);
        assert_eq!(r.fire(&Event::World), vec!["/warp afk".to_string()]);
        assert!(r.fire(&Event::Death).is_empty());
    }

    /// Die Sperrzeit verhindert, dass die eigene Antwort des Servers die Regel erneut auslöst.
    #[test]
    fn sperrzeit_bremst_wiederholtes_ausloesen() {
        let r = rules(vec![(Trigger::Chat("afk".into()), "/lobby")], 60);
        assert_eq!(r.fire(&Event::Chat("du bist afk")).len(), 1);
        assert!(r.fire(&Event::Chat("du bist afk")).is_empty());
        // Nach einer Trennung beginnt die Zählung neu.
        r.reset();
        assert_eq!(r.fire(&Event::Chat("du bist afk")).len(), 1);
    }

    #[test]
    fn ohne_chat_regel_wird_kein_chat_angefasst() {
        let r = rules(vec![(Trigger::Join, "/afk")], 0);
        assert!(!r.watches_chat());
        assert!(r.fire(&Event::Chat("egal was")).is_empty());
    }

    #[test]
    fn farbcodes_stoeren_den_vergleich_nicht() {
        assert_eq!(strip_ansi("\x1b[91mrot\x1b[0m"), "rot");
        assert_eq!(strip_ansi("ohne codes"), "ohne codes");
        let r = rules(vec![(Trigger::Chat("bist afk".into()), "/lobby")], 0);
        assert_eq!(r.fire(&Event::Chat("\x1b[93mDu \x1b[1mbist AFK\x1b[0m")).len(), 1);
    }

    #[test]
    fn ohne_regeln_passiert_nichts() {
        let r = rules(vec![], 0);
        assert!(!r.watches_chat());
        assert!(r.fire(&Event::Join).is_empty());
    }
}
