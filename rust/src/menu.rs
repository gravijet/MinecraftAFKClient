//! Menüs/Behälter anklicken – **nur im Premium-Build**.
//!
//! Bewusste Grenze: Der Client liest **nicht**, was in den Feldern liegt. Ein Gegenstand besteht
//! seit 1.20.5 aus Nummer, Anzahl und einer offenen Liste von „Komponenten", deren Aufbau sich
//! zwischen den vier unterstützten Versionen unterscheidet – das nachzubauen wäre viel Code für
//! wenig Nutzen. Gebraucht wird es auch nicht: Für den typischen AFK-Fall („Menü öffnet sich,
//! Feld 13 anklicken") genügen Fenster-Nummer, Zustandszähler und Feldnummer, und die stehen
//! alle *vor* den Gegenständen im Paket.
//!
//! Deshalb hört dieses Modul beim Lesen einfach auf, sobald es hat, was es braucht – die
//! Gegenstandsdaten laufen ungelesen ins Leere. Das ist zugleich der billigste Weg: ein volles
//! Kisten-Paket wird nie durchgeparst.
//!
//! Beim Senden werden „keine geänderten Felder" und „nichts in der Hand" mitgeschickt. Beides ist
//! in allen vier Versionen dasselbe Byte (`0`), obwohl der Gegenstandstyp dahinter ein anderer
//! ist – nachgesehen in den Codec-Jars.

use crate::buf::{Reader, Writer};
use crate::client::Shared;
use crate::console::{BOLD, GRAY};
use crate::nbt;
use crate::proto::{values, In};

use std::sync::{Arc, Mutex};

/// Ein geöffnetes Fenster.
struct Open {
    /// Fenster-Nummer des Servers. Vanilla vergibt 1..100 – klein genug, dass VarInt und
    /// vorzeichenloses Byte dasselbe Byte ergeben (1.21.1 sendet ein Byte, ab 1.21.11 VarInt).
    id: i32,
    title: String,
    /// Zustandszähler. Der Server erhöht ihn bei jeder Änderung; unser Klick muss den zuletzt
    /// gesehenen mitschicken, sonst schickt der Server das ganze Fenster erneut.
    state: i32,
    /// Anzahl der Felder – nur zur Anzeige und zur Prüfung der Feldnummer.
    slots: usize,
}

pub struct Menu {
    open: Mutex<Option<Open>>,
}

impl Menu {
    pub fn new() -> Menu {
        Menu {
            open: Mutex::new(None),
        }
    }

    pub fn clear(&self) {
        *self.open.lock().unwrap() = None;
    }
}

// ===================== Pakete =====================

pub fn incoming(shared: &Arc<Shared>, kind: In, r: &mut Reader) {
    let _ = read(shared, kind, r);
}

fn read(shared: &Arc<Shared>, kind: In, r: &mut Reader) -> std::io::Result<()> {
    let mut open = shared.premium.menu.open.lock().unwrap();
    match kind {
        // Fenster-Nummer, Typ, Überschrift.
        In::OpenScreen => {
            let id = r.var_int()?;
            r.var_int()?; // Fenstertyp – für uns nur eine Zahl ohne Aussage
            let title = nbt::render(&nbt::read_network(r)?, shared.console.is_color());
            shared
                .console
                .info(&format!("Menü geöffnet: {} (:menu, :click <feld>)", title));
            shared.console.event("menu", &format!("open id={}", id));
            *open = Some(Open {
                id,
                title,
                state: 0,
                slots: 0,
            });
        }

        // Fenster-Nummer, Zustandszähler, Feldanzahl – danach die Gegenstände, die wir nicht lesen.
        In::ContainerContent => {
            let id = r.var_int()?;
            let state = r.var_int()?;
            let slots = r.var_int()?.max(0) as usize;
            if let Some(current) = open.as_mut() {
                if current.id == id {
                    current.state = state;
                    current.slots = slots;
                }
            }
        }

        // Fenster-Nummer, Zustandszähler, Feld – Rest ungelesen.
        In::ContainerSlot => {
            let id = r.var_int()?;
            let state = r.var_int()?;
            if let Some(current) = open.as_mut() {
                if current.id == id {
                    current.state = state;
                }
            }
        }

        In::ContainerClose => {
            let id = r.var_int()?;
            if open.as_ref().is_some_and(|current| current.id == id) {
                *open = None;
                shared.console.info("Menü geschlossen.");
                shared.console.event("menu", "close");
            }
        }

        _ => {}
    }
    Ok(())
}

// ===================== Befehle =====================

/// `:menu` – was gerade offen ist.
pub fn print(shared: &Arc<Shared>) {
    let open = shared.premium.menu.open.lock().unwrap();
    let console = &shared.console;
    let Some(current) = open.as_ref() else {
        return console.error("Gerade ist kein Menü offen.");
    };
    console.print("");
    console.print(&format!("  {}", console.paint(BOLD, &current.title)));
    console.print(&console.paint(
        GRAY,
        &format!(
            "    Fenster {}  ·  {} Felder (0 bis {})",
            current.id,
            current.slots,
            current.slots.saturating_sub(1)
        ),
    ));
    console.print(&console.paint(
        GRAY,
        "    :click <feld> [rechts|shift]   ·   :close",
    ));
    console.print(&console.paint(
        GRAY,
        "    Der Inhalt der Felder wird bewusst nicht gelesen (siehe FEATURES.md).",
    ));
}

/// `:click <feld> [rechts|shift]`
pub fn click_command(shared: &Arc<Shared>, arg: &str) {
    let mut parts = arg.split_whitespace();
    let Some(slot) = parts.next().and_then(|text| text.parse::<i32>().ok()) else {
        return shared
            .console
            .error("Nutzung: :click <feld> [rechts|shift]   z. B.  :click 13");
    };
    let (button, mode) = match parts.next().unwrap_or("").to_lowercase().as_str() {
        "" | "links" | "left" | "l" => (0u8, values::click::NORMAL),
        "rechts" | "right" | "r" => (1u8, values::click::NORMAL),
        "shift" | "umschalt" | "s" => (0u8, values::click::SHIFT),
        other => {
            return shared
                .console
                .error(&format!("Unbekannt: '{}'. Möglich: rechts, shift", other))
        }
    };

    let open = shared.premium.menu.open.lock().unwrap();
    let Some(current) = open.as_ref() else {
        return shared.console.error("Gerade ist kein Menü offen (:menu).");
    };
    // Feldanzahl kennen wir erst, wenn der Server den Inhalt geschickt hat – vorher wird nicht
    // geprüft, sondern dem Nutzer geglaubt.
    if current.slots > 0 && !(0..current.slots as i32).contains(&slot) {
        return shared.console.error(&format!(
            "Feld {} gibt es nicht – das Menü hat {} Felder (0 bis {}).",
            slot,
            current.slots,
            current.slots - 1
        ));
    }

    let mut w = Writer::packet(shared.proto.extra.sb_container_click);
    w.var_int(current.id);
    w.var_int(current.state);
    w.u16(slot as u16); // Feldnummer ist ein Short
    w.u8(button);
    w.u8(mode);
    w.var_int(0); // keine geänderten Felder – die rechnet der Server ohnehin selbst nach
    w.u8(0); // nichts in der Hand
    shared.send(w);
    shared
        .console
        .info(&format!("Feld {} angeklickt.", slot));
}

/// `:close` – Fenster schließen (der Server erwartet das, sonst bleibt es für ihn offen).
pub fn close_command(shared: &Arc<Shared>) {
    let mut open = shared.premium.menu.open.lock().unwrap();
    let Some(current) = open.take() else {
        return shared.console.error("Gerade ist kein Menü offen.");
    };
    let mut w = Writer::packet(shared.proto.extra.sb_container_close);
    w.var_int(current.id);
    shared.send(w);
    shared.console.info("Menü geschlossen.");
}

#[cfg(test)]
mod tests {
    use crate::proto::values;

    /// Klickarten dürfen sich nicht überschneiden – sonst würde ein Linksklick als
    /// Umschalt-Klick beim Server ankommen und den halben Behälter leerräumen.
    #[test]
    fn klickarten_sind_verschieden() {
        assert_ne!(values::click::NORMAL, values::click::SHIFT);
        assert_eq!(values::click::NORMAL, 0);
    }

    /// Fenster-Nummern bleiben klein genug, dass VarInt und Byte dasselbe Byte ergeben – darauf
    /// beruht, dass ein Klick-Paket für alle vier Versionen gleich gebaut wird.
    #[test]
    fn fenster_nummern_passen_in_ein_byte() {
        for id in [0i32, 1, 100, 127] {
            let mut w = crate::buf::Writer::default();
            w.var_int(id);
            assert_eq!(w.data, vec![id as u8], "VarInt {} ist kein einzelnes Byte", id);
        }
    }
}
