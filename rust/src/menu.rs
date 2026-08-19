//! Menüs/Behälter – **nur mit `--features menu`** (Items-, Premium-, Premium+Items- und
//! Ultra-Client).
//!
//! Ohne `--features items` merkt sich dieses Modul nur, **welches** Fenster offen ist: Nummer,
//! Überschrift, Zustandszähler und Feldanzahl. Das genügt für den typischen AFK-Fall („Menü
//! öffnet sich, Feld 13 anklicken") und ist der billigste Weg – die Gegenstandsdaten laufen
//! ungelesen ins Leere.
//!
//! Mit `--features items` kommt der Inhalt dazu: je Feld Anzahl, Name mit Farbformatierung und
//! Lore, gelesen von [`crate::items`]. Dazu gehört auch das **eigene Inventar** (Fenster 0),
//! das der Server über denselben Weg schickt.
//!
//! Beim Senden werden „keine geänderten Felder" und „nichts in der Hand" mitgeschickt. Beides ist
//! in allen vier Versionen dasselbe Byte (`0`), obwohl der Gegenstandstyp dahinter ein anderer
//! ist – nachgesehen in den Codec-Jars.

use crate::buf::{Reader, Writer};
use crate::client::Shared;
use crate::console::{BOLD, GRAY};
use crate::nbt::{self, Fmt};
use crate::proto::{values, In};

use std::sync::{Arc, Mutex};

#[cfg(feature = "items")]
use crate::items::{self, Item};

/// Mehr Felder hat kein Fenster (Doppelkiste 90, Spielerinventar 46).
#[cfg(feature = "items")]
const MAX_SLOTS: usize = 200;

/// Ein geöffnetes Fenster.
struct Open {
    /// Fenster-Nummer des Servers. Vanilla vergibt 1..100 – klein genug, dass VarInt und
    /// vorzeichenloses Byte dasselbe Byte ergeben (1.21.1 sendet ein Byte, ab 1.21.11 VarInt).
    id: i32,
    /// Überschrift als `§`-Text (siehe [`crate::nbt::Fmt::Legacy`]).
    title: String,
    /// Zustandszähler. Der Server erhöht ihn bei jeder Änderung; unser Klick muss den zuletzt
    /// gesehenen mitschicken, sonst schickt der Server das ganze Fenster erneut.
    state: i32,
    /// Anzahl der Felder – zur Anzeige und zur Prüfung der Feldnummer.
    slots: usize,
    /// Inhalt je Feld.
    #[cfg(feature = "items")]
    items: Vec<Option<Item>>,
    /// Ab diesem Feld war der Inhalt nicht mehr lesbar (siehe [`crate::items`]).
    #[cfg(feature = "items")]
    unknown_from: usize,
}

pub struct Menu {
    open: Mutex<Option<Open>>,
    /// Eigenes Inventar (Fenster 0) – 9 Rüstungs-/Handwerksfelder, 27 Taschen, 9 Schnellleiste.
    #[cfg(feature = "items")]
    inventory: Mutex<Vec<Option<Item>>>,
}

impl Menu {
    pub fn new() -> Menu {
        Menu {
            open: Mutex::new(None),
            #[cfg(feature = "items")]
            inventory: Mutex::new(Vec::new()),
        }
    }

    pub fn clear(&self) {
        *self.open.lock().unwrap() = None;
        #[cfg(feature = "items")]
        self.inventory.lock().unwrap().clear();
    }
}

// ===================== Pakete =====================

pub fn incoming(shared: &Arc<Shared>, kind: In, r: &mut Reader) {
    let _ = read(shared, kind, r);
}

fn read(shared: &Arc<Shared>, kind: In, r: &mut Reader) -> std::io::Result<()> {
    match kind {
        // Fenster-Nummer, Typ, Überschrift.
        In::OpenScreen => {
            let id = r.var_int()?;
            r.var_int()?; // Fenstertyp – für uns nur eine Zahl ohne Aussage
            let title = nbt::render(&nbt::read_network(r)?, Fmt::Legacy);
            shared.console.info(&format!(
                "Menü geöffnet: {} (:menu, :click <feld>)",
                shared.console.text(&title)
            ));
            shared
                .console
                .event("menu", &format!("open id={} {}", id, title));
            *shared.extras.menu.open.lock().unwrap() = Some(Open {
                id,
                title,
                state: 0,
                slots: 0,
                #[cfg(feature = "items")]
                items: Vec::new(),
                #[cfg(feature = "items")]
                unknown_from: usize::MAX,
            });
        }

        // Fenster-Nummer, Zustandszähler, Feldanzahl – danach die Gegenstände.
        In::ContainerContent => {
            let id = read_container_id(shared, r)?;
            let state = r.var_int()?;
            let slots = r.var_int()?.max(0) as usize;
            #[cfg(feature = "items")]
            let content = read_content(shared, r, slots);

            #[cfg(feature = "items")]
            if id == 0 {
                // Fenster 0 ist immer das eigene Inventar – auch wenn gerade ein Menü offen ist.
                *shared.extras.menu.inventory.lock().unwrap() = content.0;
                return Ok(());
            }

            let mut open = shared.extras.menu.open.lock().unwrap();
            if let Some(current) = open.as_mut() {
                if current.id == id {
                    current.state = state;
                    current.slots = slots;
                    #[cfg(feature = "items")]
                    {
                        current.items = content.0;
                        current.unknown_from = content.1;
                    }
                }
            }
        }

        // Fenster-Nummer, Zustandszähler, Feld, Gegenstand.
        In::ContainerSlot => {
            let id = read_container_id(shared, r)?;
            let state = r.var_int()?;
            #[cfg(feature = "items")]
            let slot = r.i16()?;
            #[cfg(feature = "items")]
            let item = read_item_slot(shared, r).ok().and_then(|s| s.item);

            #[cfg(feature = "items")]
            if id == 0 {
                let mut inventory = shared.extras.menu.inventory.lock().unwrap();
                if slot >= 0 && (slot as usize) < MAX_SLOTS {
                    let slot = slot as usize;
                    if inventory.len() <= slot {
                        inventory.resize_with(slot + 1, || None);
                    }
                    inventory[slot] = item;
                }
                return Ok(());
            }

            let mut open = shared.extras.menu.open.lock().unwrap();
            if let Some(current) = open.as_mut() {
                if current.id == id {
                    current.state = state;
                    #[cfg(feature = "items")]
                    if slot >= 0 && (slot as usize) < current.items.len() {
                        let slot = slot as usize;
                        current.items[slot] = item;
                        if current.unknown_from == slot {
                            current.unknown_from = slot + 1;
                        }
                    }
                }
            }
        }

        // Einzelnes Feld des eigenen Inventars (ab 1.21.11): Feldnummer, Gegenstand.
        #[cfg(feature = "items")]
        In::PlayerInventory => {
            let slot = r.var_int()?.max(0) as usize;
            let item = read_item_slot(shared, r).ok().and_then(|s| s.item);
            let mut inventory = shared.extras.menu.inventory.lock().unwrap();
            if slot < MAX_SLOTS {
                if inventory.len() <= slot {
                    inventory.resize_with(slot + 1, || None);
                }
                inventory[slot] = item;
            }
        }

        In::ContainerClose => {
            let id = read_container_id(shared, r)?;
            let mut open = shared.extras.menu.open.lock().unwrap();
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

/// 1.21.1 codiert die Fenster-ID in Containerpaketen als unsigned Byte, die drei neueren
/// Protokolle als VarInt. `OpenScreen` ist davon ausgenommen und verwendet immer VarInt.
fn read_container_id(shared: &Shared, r: &mut Reader) -> std::io::Result<i32> {
    if shared.proto.modern {
        r.var_int()
    } else {
        Ok(r.u8()? as i32)
    }
}

fn write_container_id(shared: &Shared, w: &mut Writer, id: i32) {
    if shared.proto.modern {
        w.var_int(id);
    } else {
        w.u8(id.clamp(0, u8::MAX as i32) as u8);
    }
}

/// Alle Felder eines Fensters lesen. Zurück kommt der Inhalt und die Nummer des ersten Feldes,
/// ab dem nichts mehr gelesen werden konnte (`usize::MAX` = alles gelesen).
#[cfg(feature = "items")]
fn read_content(shared: &Arc<Shared>, r: &mut Reader, slots: usize) -> (Vec<Option<Item>>, usize) {
    let slots = slots.min(MAX_SLOTS);
    let mut items = Vec::with_capacity(slots);
    for index in 0..slots {
        match read_item_slot(shared, r) {
            Ok(slot) => {
                let synced = slot.synced;
                items.push(slot.item);
                if !synced {
                    // Ohne bekannte Komponentenlänge lässt sich das nächste Feld nicht finden.
                    items.resize_with(slots, || None);
                    return (items, index + 1);
                }
            }
            Err(_) => {
                items.resize_with(slots, || None);
                return (items, index);
            }
        }
    }
    (items, usize::MAX)
}

/// Slot lesen und für unveränderte Vanilla-Gegenstände den Registry-Namen ergänzen. Vorrang hat
/// eine eventuell vom Server synchronisierte Registry; der verlässliche Vanilla-Fallback kommt
/// aus der zur Protokollversion gehörenden Mojang-Liste. Ein `custom_name`/`item_name` aus dem
/// Paket hat immer Vorrang und behält seine Farbcodes.
#[cfg(feature = "items")]
fn read_item_slot(shared: &Arc<Shared>, r: &mut Reader) -> std::io::Result<items::Slot> {
    let mut slot = items::read_slot(shared.proto, r)?;
    if let Some(item) = slot.item.as_mut() {
        if item.name.is_none() {
            let live_name = usize::try_from(item.id)
                .ok()
                .and_then(|id| shared.extras.item_names.lock().unwrap().get(id).cloned());
            item.name = live_name
                .or_else(|| crate::item_names::get(shared.proto.name, item.id).map(str::to_string));
        }
    }
    Ok(slot)
}

// ===================== Befehle =====================

/// `:menu` – was gerade offen ist (mit `items` samt Inhalt).
pub fn print(shared: &Arc<Shared>) {
    let open = shared.extras.menu.open.lock().unwrap();
    let console = &shared.console;
    let Some(current) = open.as_ref() else {
        return console.error("Gerade ist kein Menü offen.");
    };
    console.print("");
    console.print(&format!(
        "  {}",
        console.paint(BOLD, &console.text(&current.title))
    ));
    console.print(&console.paint(
        GRAY,
        &format!(
            "    Fenster {}  ·  {} Felder (0 bis {})",
            current.id,
            current.slots,
            current.slots.saturating_sub(1)
        ),
    ));

    #[cfg(feature = "items")]
    {
        print_items(shared, &current.items, current.unknown_from);
        console.print(&console.paint(
            GRAY,
            "    :slot <feld> zeigt Lore   ·   :click <feld> [rechts|shift]   ·   :close",
        ));
    }
    #[cfg(not(feature = "items"))]
    {
        console.print(&console.paint(GRAY, "    :click <feld> [rechts|shift]   ·   :close"));
        console.print(&console.paint(
            GRAY,
            "    Feldinhalte liest nur eine Bauform mit Gegenstandslesung (siehe FEATURES.md).",
        ));
    }
}

/// Belegte Felder auflisten – und dieselbe Liste als `@event slot`-Zeilen mit `§`-Farbcodes.
#[cfg(feature = "items")]
fn print_items(shared: &Arc<Shared>, items: &[Option<Item>], unknown_from: usize) {
    let console = &shared.console;
    let mut shown = 0;
    for (index, slot) in items.iter().enumerate() {
        let Some(item) = slot else { continue };
        shown += 1;
        let label = item.label();
        console.print(&format!(
            "    {:>3}  {}{}{}",
            index,
            if item.count > 1 {
                format!("{}x ", item.count)
            } else {
                String::new()
            },
            console.text(&label),
            if item.lore.is_empty() {
                String::new()
            } else {
                console.paint(GRAY, &format!("  ({} Zeilen Lore)", item.lore.len()))
            }
        ));
        console.event("slot", &format!("{} {} {}", index, item.count, label));
    }
    if shown == 0 {
        console.print(&console.paint(GRAY, "    (alle Felder leer)"));
    }
    if unknown_from != usize::MAX {
        console.warn(&format!(
            "    Ab Feld {} unbekannt: eine Gegenstands-Komponente ist nicht hinterlegt.",
            unknown_from
        ));
    }
}

/// `:slot <feld>` – ein Feld ausführlich, mit Lore.
#[cfg(feature = "items")]
pub fn print_slot(shared: &Arc<Shared>, arg: &str) {
    let console = &shared.console;
    let Some(index) = arg
        .split_whitespace()
        .next()
        .and_then(|t| t.parse::<usize>().ok())
    else {
        return console.error("Nutzung: :slot <feld>     z. B.  :slot 13");
    };
    let open = shared.extras.menu.open.lock().unwrap();
    let Some(current) = open.as_ref() else {
        return console.error("Gerade ist kein Menü offen (:menu).");
    };
    match current.items.get(index) {
        Some(Some(item)) => print_item(shared, index, item),
        Some(None) => console.info(&format!("Feld {} ist leer.", index)),
        None => console.error(&format!(
            "Feld {} gibt es nicht – das Menü hat {} Felder.",
            index, current.slots
        )),
    }
}

/// `:inv` – das eigene Inventar.
#[cfg(feature = "items")]
pub fn print_inventory(shared: &Arc<Shared>) {
    let inventory = shared.extras.menu.inventory.lock().unwrap();
    let console = &shared.console;
    console.print("");
    console.print(&format!("  {}", console.paint(BOLD, "Eigenes Inventar")));
    if inventory.is_empty() {
        return console.print(&console.paint(
            GRAY,
            "    (noch nichts empfangen – kommt kurz nach dem Beitritt)",
        ));
    }
    console.print(&console.paint(
        GRAY,
        "    0 Ergebnis · 1-4 Werkbank · 5-8 Rüstung · 9-35 Tasche · 36-44 Schnellleiste · 45 Nebenhand",
    ));
    print_items(shared, &inventory, usize::MAX);
}

#[cfg(feature = "items")]
fn print_item(shared: &Arc<Shared>, index: usize, item: &Item) {
    let console = &shared.console;
    console.print("");
    console.print(&format!(
        "  {}  {}",
        console.paint(BOLD, &console.text(&item.label())),
        console.paint(
            GRAY,
            &format!("Feld {} · {}x · Nummer {}", index, item.count, item.id)
        )
    ));
    for line in &item.lore {
        console.print(&format!("    {}", console.text(line)));
    }
    if !item.complete {
        console.warn("    Unvollständig: eine Gegenstands-Komponente ist nicht hinterlegt.");
    }
    console.event(
        "slot",
        &format!("{} {} {}", index, item.count, item.label()),
    );
    for line in &item.lore {
        console.event("lore", &format!("{} {}", index, line));
    }
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

    // Übertragen wird die Feldnummer als Short. Alles außerhalb passt schon aufs Kabel nicht und
    // käme beim Server als eine ganz andere Zahl an – dann lieber hier ablehnen.
    if i16::try_from(slot).is_err() {
        return shared
            .console
            .error(&format!("Feld {} gibt es in keinem Menü.", slot));
    }

    let open = shared.extras.menu.open.lock().unwrap();
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
    write_container_id(shared, &mut w, current.id);
    w.var_int(current.state);
    w.u16(slot as u16); // Feldnummer ist ein Short
    w.u8(button);
    w.u8(mode);
    w.var_int(0); // keine geänderten Felder – die rechnet der Server ohnehin selbst nach
    w.u8(0); // nichts in der Hand
    shared.send(w);
    shared.console.info(&format!("Feld {} angeklickt.", slot));
}

/// `:close` – Fenster schließen (der Server erwartet das, sonst bleibt es für ihn offen).
pub fn close_command(shared: &Arc<Shared>) {
    let mut open = shared.extras.menu.open.lock().unwrap();
    let Some(current) = open.take() else {
        return shared.console.error("Gerade ist kein Menü offen.");
    };
    let mut w = Writer::packet(shared.proto.extra.sb_container_close);
    write_container_id(shared, &mut w, current.id);
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

    /// Die 1.21.1- und moderne Kodierung sind für kleine Nummern bytegleich, aber ab 128 nicht.
    /// Der Parser darf sich deshalb nicht zufällig auf die üblichen IDs 1..100 verlassen.
    #[test]
    fn fenster_nummer_hat_zwei_kodierungen() {
        let mut byte = crate::buf::Writer::default();
        byte.u8(200);
        let mut varint = crate::buf::Writer::default();
        varint.var_int(200);
        assert_eq!(byte.data, vec![200]);
        assert_eq!(varint.data, vec![200, 1]);
    }
}
