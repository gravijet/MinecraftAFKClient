//! Automatisches Anti-AFK – **nur im Premium-Build**.
//!
//! Der schlanke Client rührt sich grundsätzlich nicht: gegen den *Server*-Timeout hilft schon
//! das Beantworten von `KeepAlive`, und das kostet nichts. Wogegen dieses Modul hilft, ist etwas
//! anderes – die AFK-Erkennung mancher *Plugins*, die nach echter Spielerbewegung schaut.
//!
//! Kosten: **ein** Thread, der blockierend bis zum nächsten Termin schläft (kein Polling), und
//! alle paar Minuten drei winzige Pakete. Endet die Verbindung, endet der Thread – erkannt an der
//! Verbindungs-Generation, ohne eigene Signalisierung.
//!
//! Bewusst zurückhaltend: ein kleiner Blickschwenk und ein Armschwung. Wer sich im Sekundentakt
//! dreht, fällt mehr auf als jemand, der stillsteht – deshalb die Untergrenze in
//! [`crate::options`].

use crate::buf::Writer;
use crate::client::Shared;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::thread;
use std::time::Duration;

/// Wie weit der Kopf je Aktion schwenkt. Groß genug, dass es als Bewegung zählt, klein genug,
/// dass die Blickrichtung praktisch erhalten bleibt.
const TURN_DEGREES: f32 = 7.0;
/// Ohne Angabe verwendetes Intervall bei `:antiafk on`.
const DEFAULT_SECONDS: u64 = 60;
/// Untergrenze auch für den Befehl zur Laufzeit (dieselbe wie für `--antiafk`).
const MIN_SECONDS: u64 = 15;

/// Nach dem Beitritt: Thread starten, falls Anti-AFK an ist.
pub fn on_join(shared: &Arc<Shared>) {
    if shared.extras.antiafk.load(Ordering::Relaxed) > 0 {
        start(shared);
    }
}

/// `:antiafk` · `:antiafk on|off` · `:antiafk <sekunden>`
pub fn command(shared: &Arc<Shared>, arg: &str) {
    let console = &shared.console;
    let current = shared.extras.antiafk.load(Ordering::Relaxed);

    let seconds = match arg.trim().to_lowercase().as_str() {
        "" | "status" => {
            return match current {
                0 => console.info("Anti-AFK: aus. Einschalten mit  :antiafk on  oder  :antiafk 90"),
                s => console.info(&format!("Anti-AFK: alle {} s eine kleine Bewegung.", s)),
            }
        }
        "on" | "an" | "ein" => {
            if current > 0 {
                current
            } else {
                DEFAULT_SECONDS
            }
        }
        "off" | "aus" | "0" => 0,
        text => match text.parse::<u64>() {
            Ok(value) => value.max(MIN_SECONDS),
            Err(_) => {
                return console
                    .error("Nutzung: :antiafk   ·   :antiafk on|off   ·   :antiafk <sekunden>")
            }
        },
    };

    shared.extras.antiafk.store(seconds, Ordering::Relaxed);
    if seconds == 0 {
        // Der laufende Thread sieht die 0 beim nächsten Aufwachen und beendet sich selbst.
        return console.info("Anti-AFK aus.");
    }
    console.ok(&format!(
        "Anti-AFK: alle {} s eine kleine Bewegung.",
        seconds
    ));
    start(shared);
}

/// Thread starten – höchstens einer, und nur solange wir im Spiel sind.
fn start(shared: &Arc<Shared>) {
    if !shared.in_game.load(Ordering::Relaxed) {
        return; // beim nächsten Beitritt startet on_join ihn
    }
    // Wer den Schalter von false auf true dreht, ist der eine Thread. Ist gerade noch einer am
    // Aufräumen (`:antiafk off` direkt gefolgt von `:antiafk on`), wird kurz auf ihn gewartet –
    // sonst stünde am Ende gar keiner mehr da.
    let mut tries = 0;
    while shared
        .extras
        .antiafk_running
        .compare_exchange(false, true, Ordering::SeqCst, Ordering::SeqCst)
        .is_err()
    {
        tries += 1;
        if tries > 20 {
            return; // es läuft wirklich noch einer – der liest das neue Intervall selbst
        }
        thread::sleep(Duration::from_millis(10));
    }

    let generation = shared.generation.load(Ordering::SeqCst);
    let owned = Arc::clone(shared);
    let started = thread::Builder::new()
        .name("afk-antiafk".into())
        .spawn(move || {
            run(&owned, generation);
            // Erst hier wieder freigeben – sonst könnten zwei Threads nebeneinander laufen.
            owned.extras.antiafk_running.store(false, Ordering::SeqCst);
        });
    if started.is_err() {
        shared.extras.antiafk_running.store(false, Ordering::SeqCst);
    }
}

fn run(shared: &Arc<Shared>, generation: u32) {
    loop {
        let seconds = shared.extras.antiafk.load(Ordering::Relaxed);
        if seconds == 0 {
            return;
        }
        // Blockierendes Warten mit Weckruf bei Verbindungsende – kein Timer, kein Polling.
        if !shared.wait(Duration::from_secs(seconds), generation) {
            return;
        }
        if shared.in_game.load(Ordering::Relaxed) {
            act(shared);
        }
    }
}

/// Eine Runde „Lebenszeichen": Arm schwingen, Kopf ein Stück drehen, Kopf zurückdrehen.
///
/// Die Bewegung geht über [`crate::movement::send_move`], damit der gemerkte Zustand mitwandert –
/// sonst würde ein späteres `:go` von der falschen Blickrichtung aus rechnen.
fn act(shared: &Arc<Shared>) {
    let mut swing = Writer::packet(shared.proto.extra.sb_swing);
    swing.var_int(0); // Haupthand
    shared.send(swing);

    let Some((x, y, z, yaw, pitch)) = shared.position() else {
        return; // noch keine Position bekannt: der Armschwung muss reichen
    };
    crate::movement::send_move(shared, (x, y, z, yaw + TURN_DEGREES, pitch));
    thread::sleep(Duration::from_millis(150));
    crate::movement::send_move(shared, (x, y, z, yaw, pitch));
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zu häufiges Zappeln fällt mehr auf als Stillstehen – die Untergrenze muss greifen.
    #[test]
    fn untergrenze_gilt_auch_zur_laufzeit() {
        let parse = |text: &str| text.parse::<u64>().map(|v| v.max(MIN_SECONDS));
        assert_eq!(parse("1").unwrap(), MIN_SECONDS);
        assert_eq!(parse("120").unwrap(), 120);
        assert!(parse("x").is_err());
    }

    /// Der Schwenk muss klein bleiben: er soll als Bewegung zählen, nicht als Herumfahren.
    #[test]
    fn schwenk_bleibt_klein() {
        assert!(TURN_DEGREES > 0.0 && TURN_DEGREES < 15.0);
    }
}
