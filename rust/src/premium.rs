//! Premium-Zusätze: Anzeigetafel, Tab-Liste, Menüs, Anti-AFK, Schleichen/Sprinten.
//!
//! **Nur im Build mit `--features premium`.** Der schlanke Client enthält davon kein einziges
//! Byte – weder den Code noch die zusätzlichen Paket-IDs, weder die Zustandstabellen noch die
//! Threads. Genau deshalb liegt das hier und nicht in [`crate::client`].
//!
//! Warum „teuer"? Alles hier merkt sich etwas: die Anzeigetafel hält Ziele, Punkte, Teams und
//! die Tab-Liste im Speicher, das Menü den offenen Behälter, Anti-AFK braucht einen Zeitgeber.
//! Der schlanke Client wirft dagegen jedes Paket weg, das er nicht sofort beantworten muss, und
//! kommt damit ohne Zustand und ohne Timer aus.
//!
//! Diese Datei ist die Verteilerstelle: Zustand anlegen, Pakete an [`crate::board`] bzw.
//! [`crate::menu`] weiterreichen, die kurzen Befehle (`:sneak`, `:swing`, `:hand` ...) selbst
//! erledigen.

use crate::buf::{Reader, Writer};
use crate::client::Shared;
use crate::options::Options;
use crate::proto::{values, In};

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::Arc;

/// Zustand aller Premium-Teile. Hängt in `Shared` und lebt damit genauso lange wie der Client.
pub struct Premium {
    pub(crate) board: crate::board::Board,
    pub(crate) menu: crate::menu::Menu,
    /// Sekunden zwischen zwei Anti-AFK-Aktionen; 0 = aus.
    pub(crate) antiafk: AtomicU64,
    /// Läuft der Anti-AFK-Thread gerade? Verhindert, dass `:antiafk on` einen zweiten startet.
    pub(crate) antiafk_running: AtomicBool,
    /// Beim Beitritt automatisch losschleichen (`--sneak`).
    sneak_on_join: bool,
    /// Aktueller Tastenzustand – ab 1.21.11 wird er als ganzes Bitfeld gesendet, deshalb müssen
    /// wir ihn kennen und nicht nur die Änderung.
    sneaking: AtomicBool,
    sprinting: AtomicBool,
}

impl Premium {
    pub fn new(options: &Options) -> Premium {
        Premium {
            board: crate::board::Board::new(),
            menu: crate::menu::Menu::new(),
            antiafk: AtomicU64::new(options.antiafk_seconds),
            antiafk_running: AtomicBool::new(false),
            sneak_on_join: options.sneak,
            sneaking: AtomicBool::new(false),
            sprinting: AtomicBool::new(false),
        }
    }

}

// ===================== Haken aus dem Client =====================

/// Premium-Pakete auswerten. Alles, was hier ankommt, hat [`crate::proto::Protocol::incoming`]
/// bereits als „nur Premium" eingeordnet.
pub fn incoming(shared: &Arc<Shared>, kind: In, r: &mut Reader) {
    match kind {
        In::Objective
        | In::Score
        | In::ResetScore
        | In::DisplayObjective
        | In::Team
        | In::PlayerInfoUpdate
        | In::PlayerInfoRemove => crate::board::incoming(shared, kind, r),

        In::OpenScreen | In::ContainerContent | In::ContainerSlot | In::ContainerClose => {
            crate::menu::incoming(shared, kind, r)
        }

        _ => {}
    }
}

pub fn on_join(shared: &Arc<Shared>) {
    // Die alte Anzeigetafel gilt nicht mehr – der neue Server schickt seine eigene.
    shared.premium.board.clear();
    shared.premium.menu.clear();
    shared.premium.sneaking.store(false, Ordering::Relaxed);
    shared.premium.sprinting.store(false, Ordering::Relaxed);

    if shared.premium.sneak_on_join {
        set_sneak(shared, true);
    }
    crate::antiafk::on_join(shared);
}

pub fn on_disconnect(shared: &Shared) {
    shared.premium.board.clear();
    shared.premium.menu.clear();
}

// ===================== Befehle =====================

/// Premium-Befehl aus der Eingabeschleife. `true` = erledigt, `false` = nicht meiner (dann
/// bekommt ihn die Bewegung).
pub fn command(shared: &Arc<Shared>, verb: &str, arg: &str) -> bool {
    match verb {
        "board" | "tafel" | "scoreboard" => crate::board::print_sidebar(shared),
        "tab" | "liste" | "spieler" => crate::board::print_tab(shared),

        "menu" | "menü" | "container" | "kiste" => crate::menu::print(shared),
        "click" | "klick" | "klicke" => crate::menu::click_command(shared, arg),
        "close" | "schliessen" | "schließen" | "zu" => crate::menu::close_command(shared),

        "sneak" | "schleich" | "schleichen" | "ducken" => {
            toggle(shared, arg, &shared.premium.sneaking, "Schleichen", set_sneak)
        }
        "sprint" | "rennen" | "sprinten" => {
            toggle(shared, arg, &shared.premium.sprinting, "Sprinten", set_sprint)
        }

        "swing" | "schlag" | "schlage" | "arm" => {
            let mut w = Writer::packet(shared.proto.extra.sb_swing);
            w.var_int(0); // Haupthand
            shared.send(w);
            shared.console.info("Arm geschwungen.");
        }
        "use" | "benutze" | "rechtsklick" => use_item(shared),
        "hand" | "slot" | "hotbar" => hotbar(shared, arg),

        "antiafk" | "anti-afk" | "zappeln" => crate::antiafk::command(shared, arg),

        _ => return false,
    }
    true
}

/// `on|off|an|aus` oder ohne Angabe umschalten – für `:sneak` und `:sprint` gleich.
fn toggle(
    shared: &Arc<Shared>,
    arg: &str,
    state: &AtomicBool,
    label: &str,
    apply: fn(&Arc<Shared>, bool),
) {
    let want = match arg.trim().to_lowercase().as_str() {
        "" | "toggle" | "um" => !state.load(Ordering::Relaxed),
        "on" | "an" | "ein" | "1" => true,
        "off" | "aus" | "0" => false,
        other => {
            return shared
                .console
                .error(&format!("Unbekannt: '{}'. Nutzung: on | off", other))
        }
    };
    apply(shared, want);
    shared
        .console
        .info(&format!("{}: {}", label, if want { "an" } else { "aus" }));
}

// ===================== Tastenzustand =====================

pub(crate) fn set_sneak(shared: &Arc<Shared>, on: bool) {
    shared.premium.sneaking.store(on, Ordering::Relaxed);
    send_state(shared, on, values::player_state::START_SNEAKING, values::player_state::STOP_SNEAKING);
}

pub(crate) fn set_sprint(shared: &Arc<Shared>, on: bool) {
    shared.premium.sprinting.store(on, Ordering::Relaxed);
    send_state(
        shared,
        on,
        values::player_state::START_SPRINTING,
        values::player_state::STOP_SPRINTING,
    );
}

/// Der Weg zum Server ist versionsabhängig:
///
/// * **1.21.1** kennt für Schleichen und Sprinten je ein eigenes „Spielerbefehl"-Paket; dort wird
///   nur die *Änderung* gemeldet.
/// * **ab 1.21.11** gibt es stattdessen ein Eingabepaket mit einem Bitfeld aller Tasten. Es
///   beschreibt den *Gesamtzustand*, deshalb müssen beide Bits jedes Mal mit hinein.
fn send_state(shared: &Arc<Shared>, on: bool, start: i32, stop: i32) {
    if shared.proto.modern {
        let mut bits = 0u8;
        if shared.premium.sneaking.load(Ordering::Relaxed) {
            bits |= values::input::SNEAK;
        }
        if shared.premium.sprinting.load(Ordering::Relaxed) {
            bits |= values::input::SPRINT;
        }
        let mut w = Writer::packet(shared.proto.extra.sb_player_input);
        w.u8(bits);
        return shared.send(w);
    }

    let mut w = Writer::packet(shared.proto.extra.sb_player_command);
    w.var_int(shared.entity_id.load(Ordering::Relaxed));
    w.var_int(if on { start } else { stop });
    w.var_int(0); // Zusatzwert (nur beim Reiten benutzt)
    shared.send(w);
}

/// Rechtsklick mit dem Gegenstand in der Hand („Gegenstand benutzen").
fn use_item(shared: &Arc<Shared>) {
    let (yaw, pitch) = shared
        .position()
        .map(|(_, _, _, yaw, pitch)| (yaw, pitch))
        .unwrap_or((0.0, 0.0));
    let mut w = Writer::packet(shared.proto.extra.sb_use_item);
    w.var_int(0); // Haupthand
    w.var_int(0); // Sequenznummer – der Server nutzt sie nur zum Zurückrollen von Blockänderungen
    w.f32(yaw);
    w.f32(pitch);
    shared.send(w);
    shared.console.info("Gegenstand benutzt (Rechtsklick).");
}

/// `:hand 1..9` – das Feld in der Schnellleiste wechseln.
fn hotbar(shared: &Arc<Shared>, arg: &str) {
    match arg.trim().parse::<u8>() {
        Ok(slot @ 1..=9) => {
            let mut w = Writer::packet(shared.proto.extra.sb_set_carried_item);
            w.u16(slot as u16 - 1); // übertragen wird 0..8
            shared.send(w);
            shared.console.info(&format!("Schnellleiste: Feld {}.", slot));
        }
        _ => shared.console.error("Nutzung: :hand <1-9>"),
    }
}

#[cfg(test)]
mod tests {
    use crate::proto::{values, PROTOCOLS};

    /// Das Bitfeld ab 1.21.11 beschreibt den Gesamtzustand: beide Tasten müssen sich unabhängig
    /// setzen lassen, ohne einander zu überschreiben.
    #[test]
    fn eingabebits_stoeren_sich_nicht() {
        let bits = values::input::SNEAK | values::input::SPRINT;
        assert_eq!(bits & values::input::SNEAK, values::input::SNEAK);
        assert_eq!(bits & values::input::SPRINT, values::input::SPRINT);
        assert_ne!(values::input::SNEAK, values::input::SPRINT);
    }

    /// Die Zusatz-IDs müssen in jeder Version gesetzt sein – eine vergessene 0 würde ein
    /// völlig anderes Paket senden.
    #[test]
    fn zusatz_ids_sind_gefuellt() {
        for p in PROTOCOLS {
            let e = &p.extra;
            for (name, id) in [
                ("sb_container_click", e.sb_container_click),
                ("sb_container_close", e.sb_container_close),
                ("sb_player_command", e.sb_player_command),
                ("sb_player_input", e.sb_player_input),
                ("sb_set_carried_item", e.sb_set_carried_item),
                ("sb_swing", e.sb_swing),
                ("sb_use_item", e.sb_use_item),
            ] {
                assert!(id > 0, "{} fehlt in {}", name, p.name);
            }
        }
    }
}
