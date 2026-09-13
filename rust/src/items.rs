//! Gegenstände in Behältern lesen – **nur mit `--features items`** (Items-, Premium+Items- und
//! Ultra-Client).
//!
//! Ein Gegenstand besteht seit 1.20.5 aus Anzahl, Nummer und einer offenen Liste von
//! „Komponenten". Die Komponenten stehen **ohne Längenangabe** hintereinander: wer eine davon
//! nicht kennt, kann auch die nächste nicht finden. Deshalb gibt es hier eine Tabelle, die je
//! Komponente sagt, wie ihre Daten aufgebaut sind ([`crate::proto::Components::shapes`]) – sie
//! ist aus denselben Codec-Jars abgelesen wie die Paket-IDs, nicht geraten.
//!
//! Was nicht in der Tabelle steht, beendet das Lesen **dieses** Pakets: der Gegenstand wird mit
//! dem gezeigt, was bis dahin gelesen wurde (Name und Lore stehen fast immer vorn), und die
//! restlichen Felder gelten als unbekannt. Das ist die ehrliche Variante – lieber „weiß ich
//! nicht" als geratene Inhalte. Ein Fehlschlag kostet nie die Verbindung: Pakete sind
//! längenbegrenzt, ein falsch gelesenes Paket verschiebt den Strom also nicht.
//!
//! Namen und Lore werden als `§`-Text gehalten (siehe [`crate::nbt::Fmt::Legacy`]), damit die
//! Farbformatierung des Servers erhalten bleibt – im Terminal wie auch über `--events`.

use crate::buf::Reader;
use crate::nbt::{self, Fmt};
use crate::proto::Protocol;

use std::io;

/// Obergrenze für Lore-Zeilen und Listenlängen beim Überspringen.
const MAX_LIST: i32 = 1024;

/// Ein Gegenstand in einem Feld.
///
/// `Clone`, weil `:menu`, `:slot` und `:inv` einen Abzug ziehen und die Menü-Sperre danach
/// sofort wieder loslassen – ausgegeben wird ohne sie (siehe [`crate::menu`]).
#[derive(Clone)]
pub struct Item {
    /// Registrierungsnummer des Gegenstands. Ohne Registerdaten des Servers lässt sich daraus
    /// kein Name ableiten – angezeigt wird sie deshalb als `#nummer`.
    pub id: i32,
    /// Schadenswert/Metadatum des alten 1.8.9-Slots; moderne Komponenten-Slots setzen ihn auf 0.
    pub legacy_damage: i16,
    pub count: i32,
    /// Anzeigename (`custom_name`, sonst `item_name`) als `§`-Text.
    pub name: Option<String>,
    /// Lore-Zeilen als `§`-Text.
    pub lore: Vec<String>,
    /// Wurden alle Komponenten verstanden? `false` = es kam eine unbekannte Komponente.
    pub complete: bool,
}

impl Item {
    /// Kurzbeschreibung als `§`-Text: Name, sonst die Nummer.
    pub fn label(&self) -> String {
        match &self.name {
            Some(name) if !nbt::strip_legacy(name).trim().is_empty() => name.clone(),
            _ => format!("#{}", self.id),
        }
    }
}

/// Ergebnis eines Feldes.
pub struct Slot {
    /// `None` = leeres Feld.
    pub item: Option<Item>,
    /// `false` heißt: ab hier steht der Lesezeiger nicht mehr sicher, der Rest des Pakets darf
    /// nicht mehr gelesen werden.
    pub synced: bool,
}

/// Ein Feld lesen: Anzahl, Nummer, Komponenten. Anzahl `<= 0` heißt „leer" – dann steht auch
/// nichts weiter im Paket (in allen vier Versionen ein einzelnes Null-Byte).
pub fn read_slot(proto: &'static Protocol, r: &mut Reader) -> io::Result<Slot> {
    if proto.legacy {
        return read_legacy_slot(r);
    }
    let count = r.var_int()?;
    if count <= 0 {
        return Ok(Slot {
            item: None,
            synced: true,
        });
    }
    let mut item = Item {
        id: r.var_int()?,
        legacy_damage: 0,
        count,
        name: None,
        lore: Vec::new(),
        complete: true,
    };
    let synced = read_components(proto, r, &mut item)?;
    item.complete = synced;
    Ok(Slot {
        item: Some(item),
        synced,
    })
}

/// Slot aus Protokoll 47: Short-ID, Anzahl, Metadatum und optionales benanntes NBT.
fn read_legacy_slot(r: &mut Reader) -> io::Result<Slot> {
    let id = r.i16()?;
    if id < 0 {
        return Ok(Slot {
            item: None,
            synced: true,
        });
    }
    let count = r.i8()? as i32;
    let damage = r.i16()?;
    let tag = nbt::read_named(r)?;
    let display = tag.get("display");
    let name = display
        .and_then(|value| value.get_str("Name"))
        .map(str::to_string);
    let lore = display
        .and_then(|value| value.get("Lore"))
        .and_then(nbt::Nbt::as_list)
        .map(|values| {
            values
                .iter()
                .filter_map(nbt::Nbt::as_str)
                .map(str::to_string)
                .collect()
        })
        .unwrap_or_default();
    Ok(Slot {
        item: Some(Item {
            id: id as i32,
            legacy_damage: damage,
            count,
            name,
            lore,
            complete: true,
        }),
        synced: true,
    })
}

/// Komponenten lesen. Rückgabe `false`: unbekannte Komponente, ab hier ist der Lesezeiger
/// unbrauchbar.
fn read_components(proto: &'static Protocol, r: &mut Reader, item: &mut Item) -> io::Result<bool> {
    let components = &proto.extra.components;
    let added = r.var_int()?;
    let removed = r.var_int()?;
    if !(0..=MAX_LIST).contains(&added) || !(0..=MAX_LIST).contains(&removed) {
        return Ok(false);
    }

    for _ in 0..added {
        let id = r.var_int()?;
        let shape = usize::try_from(id)
            .ok()
            .and_then(|index| components.shapes.as_bytes().get(index).copied())
            .unwrap_or(b'.');

        // Name und Lore werden gelesen, alles andere nur übersprungen.
        if id == components.custom_name as i32 {
            item.name = Some(nbt::render(&nbt::read_network(r)?, Fmt::Legacy));
            continue;
        }
        if id == components.item_name as i32 {
            let value = nbt::render(&nbt::read_network(r)?, Fmt::Legacy);
            // Der Anzeigename des Servers hat Vorrang vor dem Namen des Gegenstands selbst.
            item.name.get_or_insert(value);
            continue;
        }
        if id == components.lore as i32 {
            let count = list_len(r)?;
            // Platz auf Verdacht, aber nur für eine plausible Lore: Die Zahl kommt vom Server,
            // und dahinter muss noch nichts stehen. Was wirklich kommt, lässt den Vektor ohnehin
            // wachsen.
            item.lore.reserve(count.min(64) as usize);
            for _ in 0..count {
                item.lore
                    .push(nbt::render(&nbt::read_network(r)?, Fmt::Legacy));
            }
            continue;
        }

        if !skip_shape(r, shape)? {
            return Ok(false);
        }
    }

    // Entfernte Komponenten sind nur Nummern ohne Daten.
    for _ in 0..removed {
        r.var_int()?;
    }
    Ok(true)
}

/// Eine Komponente überspringen. `false` = Aufbau nicht hinterlegt.
///
/// Die Buchstaben stammen aus [`crate::proto`]; dort steht auch, woher die Tabelle kommt.
fn skip_shape(r: &mut Reader, shape: u8) -> io::Result<bool> {
    match shape {
        b'U' => {}
        b'V' => {
            r.var_int()?;
        }
        b'B' => {
            r.u8()?;
        }
        b'F' | b'I' => r.skip(4)?,
        b'S' => r.skip_string()?,
        b'N' | b'C' => {
            nbt::read_network(r)?;
        }
        b'L' => {
            for _ in 0..list_len(r)? {
                nbt::read_network(r)?;
            }
        }
        b'v' => {
            for _ in 0..list_len(r)? {
                r.var_int()?;
            }
        }
        // HolderSet: 0 = ein Tag-Name, sonst n-1 Einträge.
        b'H' => {
            let n = r.var_int()?;
            if n == 0 {
                r.skip_string()?;
            } else if (1..=MAX_LIST).contains(&n) {
                for _ in 0..n - 1 {
                    r.var_int()?;
                }
            } else {
                return Ok(false);
            }
        }
        // Verzauberungen: Anzahl, je (Nummer, Stufe); in 1.21.1 folgt ein Anzeige-Flag.
        b'E' | b'e' => {
            for _ in 0..list_len(r)? {
                r.var_int()?;
                r.var_int()?;
            }
            if shape == b'E' {
                r.u8()?;
            }
        }
        // Anzeigeregeln (ab 1.21.5): Flag + Liste der ausgeblendeten Komponenten.
        b'T' => {
            r.u8()?;
            for _ in 0..list_len(r)? {
                r.var_int()?;
            }
        }
        // Modelldaten (ab 1.21.5): Listen aus Fließkommazahlen, Flags, Zeichenketten, Farben.
        b'M' => {
            for _ in 0..list_len(r)? {
                r.skip(4)?;
            }
            for _ in 0..list_len(r)? {
                r.u8()?;
            }
            for _ in 0..list_len(r)? {
                r.skip_string()?;
            }
            for _ in 0..list_len(r)? {
                r.skip(4)?;
            }
        }
        // Spielerprofil, 1.21.1: Name?, UUID?, Eigenschaften.
        b'P' => {
            if r.bool()? {
                r.skip_string()?;
            }
            if r.bool()? {
                r.skip(16)?;
            }
            skip_properties(r)?;
        }
        // Spielerprofil ab 1.21.11: fest (UUID + Name) oder aufzulösen (Name?, UUID?),
        // danach Eigenschaften, drei optionale Kennungen und das optionale Hautmodell.
        b'p' => {
            if r.bool()? {
                r.skip(16)?;
                r.skip_string()?;
            } else {
                if r.bool()? {
                    r.skip_string()?;
                }
                if r.bool()? {
                    r.skip(16)?;
                }
            }
            skip_properties(r)?;
            for _ in 0..3 {
                if r.bool()? {
                    r.skip_string()?;
                }
            }
            if r.bool()? {
                r.u8()?;
            }
        }
        _ => return Ok(false),
    }
    Ok(true)
}

/// Eigenschaften eines Profils: je Name, Wert und optionale Signatur.
fn skip_properties(r: &mut Reader) -> io::Result<()> {
    for _ in 0..list_len(r)? {
        r.skip_string()?;
        r.skip_string()?;
        if r.bool()? {
            r.skip_string()?;
        }
    }
    Ok(())
}

/// Länge einer Liste innerhalb einer Komponente.
///
/// Bewusst ein **Fehler** statt einer stillen Deckelung: Eine Zahl außerhalb des Erwartbaren
/// heißt, dass der Lesezeiger schon falsch steht. Vorher wurde sie auf [`MAX_LIST`] gestaucht –
/// der Leser lief dann mit einer erfundenen Länge weiter und meldete den Gegenstand am Ende als
/// „vollständig gelesen", obwohl ab dort nur noch Zufall herauskam.
fn list_len(r: &mut Reader) -> io::Result<i32> {
    let len = r.var_int()?;
    if !(0..=MAX_LIST).contains(&len) {
        return Err(crate::buf::err("Gegenstands-Liste unplausibel"));
    }
    Ok(len)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::buf::Writer;
    use crate::proto::PROTOCOLS;

    fn protocol(name: &str) -> &'static Protocol {
        Protocol::find(name).expect("Version")
    }

    /// Ein NBT-Textbaustein mit Farbe, so wie ihn der Server schickt.
    fn text_component(w: &mut Writer, text: &str, color: &str) {
        w.u8(10); // Compound
        w.u8(8); // String
        w.u16(4);
        w.raw(b"text");
        w.u16(text.len() as u16);
        w.raw(text.as_bytes());
        w.u8(8);
        w.u16(5);
        w.raw(b"color");
        w.u16(color.len() as u16);
        w.raw(color.as_bytes());
        w.u8(0); // Ende des Compounds
    }

    /// Der Regelfall aus einem Menü: Anzahl, Nummer, Anzeigename und zwei Lore-Zeilen.
    #[test]
    fn name_und_lore_werden_gelesen() {
        let proto = protocol("26.1");
        let c = &proto.extra.components;
        let mut w = Writer::default();
        w.var_int(1); // Anzahl
        w.var_int(848); // Nummer
        w.var_int(2); // zwei Komponenten
        w.var_int(0); // keine entfernten
        w.var_int(c.custom_name as i32);
        text_component(&mut w, "Kompass", "gold");
        w.var_int(c.lore as i32);
        w.var_int(2);
        text_component(&mut w, "Zeile 1", "gray");
        text_component(&mut w, "Zeile 2", "#ff8800");

        let mut r = Reader::new(&w.data);
        let slot = read_slot(proto, &mut r).expect("lesbar");
        assert!(slot.synced);
        let item = slot.item.expect("Gegenstand");
        assert!(item.complete);
        assert_eq!(item.count, 1);
        assert_eq!(item.id, 848);
        assert_eq!(item.name.as_deref(), Some("§r§6Kompass"));
        assert_eq!(item.lore.len(), 2);
        assert_eq!(item.lore[1], "§r§x§f§f§8§8§0§0Zeile 2");
        assert_eq!(r.remaining(), 0, "das Feld muss restlos gelesen sein");
    }

    /// Ein modernes leeres Feld ist ein Null-Byte; 1.8.9 benutzt die Short-ID -1.
    #[test]
    fn leeres_feld_ist_ein_byte() {
        for p in PROTOCOLS {
            let bytes = if p.legacy {
                &[0xff, 0xff][..]
            } else {
                &[0u8][..]
            };
            let mut r = Reader::new(bytes);
            let slot = read_slot(p, &mut r).expect("lesbar");
            assert!(slot.item.is_none() && slot.synced, "{}", p.name);
            assert_eq!(r.remaining(), 0);
        }
    }

    /// Bekannte Komponenten vor dem Namen dürfen ihn nicht verdecken – genau dafür gibt es die
    /// Formtabelle.
    #[test]
    fn bekannte_komponenten_werden_uebersprungen() {
        let proto = protocol("26.1");
        let c = &proto.extra.components;
        let mut w = Writer::default();
        w.var_int(1);
        w.var_int(1);
        w.var_int(3);
        w.var_int(0);
        w.var_int(1); // max_stack_size = VarInt
        w.var_int(64);
        w.var_int(21); // enchantment_glint_override = Bool
        w.bool(true);
        w.var_int(c.custom_name as i32);
        text_component(&mut w, "Glanz", "aqua");

        let mut r = Reader::new(&w.data);
        let item = read_slot(proto, &mut r).unwrap().item.expect("Gegenstand");
        assert_eq!(item.name.as_deref(), Some("§r§bGlanz"));
        assert!(item.complete);
    }

    /// Eine unbekannte Komponente beendet das Lesen – gemeldet wird das ehrlich, statt zu raten.
    #[test]
    fn unbekannte_komponente_stoppt_sauber() {
        let proto = protocol("26.1");
        let c = &proto.extra.components;
        let mut w = Writer::default();
        w.var_int(1);
        w.var_int(1);
        w.var_int(2);
        w.var_int(0);
        w.var_int(c.custom_name as i32);
        text_component(&mut w, "Name", "white");
        w.var_int(16); // attribute_modifiers: Aufbau nicht hinterlegt
        w.raw(&[1, 2, 3]);

        let mut r = Reader::new(&w.data);
        let slot = read_slot(proto, &mut r).expect("lesbar");
        assert!(!slot.synced);
        let item = slot.item.expect("Gegenstand");
        assert!(!item.complete);
        assert_eq!(item.name.as_deref(), Some("§r§fName"));
    }

    /// Eine unplausible Listenlänge wurde bisher stillschweigend auf 1024 gestaucht. Der Leser
    /// lief danach mit einer erfundenen Länge weiter und meldete den Gegenstand trotzdem als
    /// vollständig – aus einem falsch gelesenen Feld wurden so erfundene Inhalte.
    #[test]
    fn unplausible_listenlaenge_gilt_nicht_als_gelesen() {
        let proto = protocol("26.1");
        let c = &proto.extra.components;
        let mut w = Writer::default();
        w.var_int(1); // Anzahl
        w.var_int(1); // Nummer
        w.var_int(1); // eine Komponente
        w.var_int(0); // keine entfernten
        w.var_int(c.lore as i32);
        w.var_int(500_000); // so viele Lore-Zeilen gibt es nicht
        w.raw(&[0; 32]);

        let mut r = Reader::new(&w.data);
        assert!(read_slot(proto, &mut r).is_err());
    }

    /// Beliebige Bytes dürfen den Leser weder abstürzen lassen noch aufhängen: Der Inhalt eines
    /// Menüs kommt vom Server, gelesen wird im Netz-Thread, und der Client läuft mit
    /// `panic = "abort"` – ein Absturz dort reißt den ganzen Prozess mit.
    #[test]
    fn beliebige_bytes_stuerzen_nicht_ab() {
        let mut seed = 0xC3D2_E1F0_A458_36B9u64;
        let mut random = move || {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            seed
        };
        for _ in 0..4000 {
            let len = (random() % 512) as usize;
            let data: Vec<u8> = (0..len).map(|_| random() as u8).collect();
            for p in PROTOCOLS {
                let mut r = Reader::new(&data);
                if let Ok(slot) = read_slot(p, &mut r) {
                    if let Some(item) = slot.item {
                        let _ = item.label();
                    }
                }
            }
        }
    }

    /// Die Tabelle muss zu den Nummern passen, die wir gezielt lesen.
    #[test]
    fn tabelle_passt_zu_den_gelesenen_nummern() {
        for p in PROTOCOLS {
            if p.legacy {
                continue;
            }
            let c = &p.extra.components;
            let shapes = c.shapes.as_bytes();
            assert!(
                shapes.len() > c.lore as usize,
                "Tabelle zu kurz: {}",
                p.name
            );
            assert_eq!(shapes[c.custom_name as usize], b'C', "{}", p.name);
            assert_eq!(shapes[c.item_name as usize], b'C', "{}", p.name);
            assert_eq!(shapes[c.lore as usize], b'L', "{}", p.name);
            assert!(c.shapes.is_ascii(), "{}", p.name);
        }
    }
}
