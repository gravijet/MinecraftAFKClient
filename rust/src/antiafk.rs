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
/// Obergrenze (30 Tage), dieselbe wie in [`crate::options`]. Ohne sie machte `:antiafk
/// 000000000000000000` aus dem Wartezeitraum eine Zahl, mit der keine Uhr mehr rechnen kann –
/// und der Thread wäre bis zum Verbindungsende nie wieder aufgewacht.
const MAX_SECONDS: u64 = 30 * 24 * 60 * 60;

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
            Ok(value) => value.clamp(MIN_SECONDS, MAX_SECONDS),
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
    let generation = shared.generation.load(Ordering::SeqCst);
    let owned = Arc::clone(shared);
    let _ = thread::Builder::new()
        .name("afk-antiafk".into())
        .spawn(move || {
            // Wer den Schalter von false auf true dreht, ist der eine Thread. Ist gerade noch
            // einer am Aufräumen (`:antiafk off` direkt gefolgt von `:antiafk on`, oder ein
            // Unterserver-Wechsel), wird kurz auf ihn gewartet – sonst stünde am Ende gar keiner
            // mehr da.
            //
            // Gewartet wird im **eigenen** Thread. Vorher lief diese Schleife im Aufrufer, und
            // der ist beim Beitritt der Netz-Thread: der hätte in dieser Zeit keine
            // KeepAlive-Pakete beantwortet – ausgerechnet die Aufgabe, für die es den ganzen
            // Client gibt.
            let mut tries = 0;
            while owned
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
            run(&owned, generation);
            // Erst hier wieder freigeben – sonst könnten zwei Threads nebeneinander laufen.
            owned.extras.antiafk_running.store(false, Ordering::SeqCst);
        });
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
/// Die Bewegung geht über das reine Rotationspaket, damit keine alten Koordinaten erneut gesendet
/// werden. Das ist besonders wichtig, wenn zeitgleich ein ausdrücklicher `:go`-Befehl läuft.
fn act(shared: &Arc<Shared>) {
    // Eine ausdrücklich angeforderte Bewegung hat Vorrang. Zwei unabhängige Taktgeber würden
    // sonst abwechselnd verschiedene Blickrichtungen senden.
    if shared.mover.is_active() {
        return;
    }
    let mut swing = Writer::packet(shared.proto.extra.sb_swing);
    if !shared.proto.legacy {
        swing.var_int(0); // Haupthand; 1.8.9 hat keine Nutzlast
    }
    shared.send(swing);

    let Some((_, _, _, yaw, pitch)) = shared.position() else {
        return; // noch keine Position bekannt: der Armschwung muss reichen
    };
    crate::movement::send_rotation(shared, yaw + TURN_DEGREES, pitch);
    thread::sleep(Duration::from_millis(150));
    crate::movement::send_rotation(shared, yaw, pitch);
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Zu häufiges Zappeln fällt mehr auf als Stillstehen – die Untergrenze muss greifen.
    #[test]
    fn untergrenze_gilt_auch_zur_laufzeit() {
        let parse = |text: &str| {
            text.parse::<u64>()
                .map(|value| value.clamp(MIN_SECONDS, MAX_SECONDS))
        };
        assert_eq!(parse("1").unwrap(), MIN_SECONDS);
        assert_eq!(parse("120").unwrap(), 120);
        assert!(parse("x").is_err());
        // Auch nach oben: aus einer unmöglichen Zahl darf keine unmögliche Wartezeit werden.
        assert_eq!(parse("000000000000000000").unwrap(), MAX_SECONDS);
        assert!(std::time::Duration::from_secs(MAX_SECONDS)
            .checked_add(std::time::Duration::from_secs(MAX_SECONDS))
            .is_some());
    }

    /// Der Schwenk muss klein bleiben: er soll als Bewegung zählen, nicht als Herumfahren.
    #[test]
    fn schwenk_bleibt_klein() {
        const _: () = assert!(TURN_DEGREES > 0.0 && TURN_DEGREES < 15.0);
    }
}
