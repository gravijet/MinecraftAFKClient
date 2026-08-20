//! Verteilerstelle der Ausbaustufen: Anzeigetafel, Menüs, Gegenstände, Live-Ansicht,
//! Tastenzustand, Anti-AFK.
//!
//! **Nur mit `--features extras`**, und darin noch einmal je Funktion getrennt. Der schlanke
//! Client enthält davon kein einziges Byte – weder den Code noch die Zustandstabellen, weder die
//! zusätzlichen Paket-IDs noch die Threads. Genau deshalb liegt das hier und nicht in
//! [`crate::client`].
//!
//! Warum „teuer"? Alles hier merkt sich etwas: die Anzeigetafel hält Ziele, Punkte und Teams im
//! Speicher, das Menü den offenen Behälter samt Inhalt, die Live-Ansicht die Chunks der
//! Umgebung, Anti-AFK braucht einen Zeitgeber. Der schlanke Client wirft dagegen jedes Paket
//! weg, das er nicht sofort beantworten muss, und kommt damit ohne Zustand und ohne Timer aus.
//!
//! Diese Datei legt den Zustand an, reicht Pakete an das zuständige Modul weiter und erledigt
//! die kurzen Befehle selbst.

use crate::buf::Reader;
use crate::client::Shared;
use crate::options::Options;
use crate::proto::In;

use std::sync::Arc;
#[cfg(feature = "items")]
use std::sync::Mutex;

#[cfg(feature = "state")]
use crate::buf::Writer;
#[cfg(feature = "state")]
use crate::proto::values;
#[cfg(any(feature = "state", feature = "antiafk"))]
use std::sync::atomic::{AtomicBool, Ordering};

/// Zustand aller Zusatzteile. Hängt in `Shared` und lebt damit genauso lange wie der Client.
pub struct Extras {
    #[cfg(feature = "board")]
    pub(crate) board: crate::board::Board,
    #[cfg(feature = "menu")]
    pub(crate) menu: crate::menu::Menu,
    #[cfg(feature = "pov")]
    pub(crate) pov: crate::pov::Pov,
    /// Optional vom Server synchronisierte Registry-ID -> Ressourcenname. Vanilla-Namen kommen
    /// zuverlässig aus [`crate::item_names`]; diese Liste darf sie bei einem Server ersetzen,
    /// der `minecraft:item` tatsächlich synchronisiert.
    #[cfg(feature = "items")]
    pub(crate) item_names: Mutex<Vec<String>>,

    /// Sekunden zwischen zwei Anti-AFK-Aktionen; 0 = aus.
    #[cfg(feature = "antiafk")]
    pub(crate) antiafk: std::sync::atomic::AtomicU64,
    /// Läuft der Anti-AFK-Thread gerade? Verhindert, dass `:antiafk on` einen zweiten startet.
    #[cfg(feature = "antiafk")]
    pub(crate) antiafk_running: AtomicBool,

    /// Beim Beitritt automatisch losschleichen (`--sneak`).
    #[cfg(feature = "state")]
    sneak_on_join: bool,
    /// Aktueller Tastenzustand – ab 1.21.11 wird er als ganzes Bitfeld gesendet, deshalb müssen
    /// wir ihn kennen und nicht nur die Änderung.
    #[cfg(feature = "state")]
    sneaking: AtomicBool,
    #[cfg(feature = "state")]
    sprinting: AtomicBool,
}

impl Extras {
    pub fn new(options: &Options) -> Extras {
        let _ = options;
        Extras {
            #[cfg(feature = "board")]
            board: crate::board::Board::new(),
            #[cfg(feature = "menu")]
            menu: crate::menu::Menu::new(),
            #[cfg(feature = "pov")]
            pov: crate::pov::Pov::new(options),
            #[cfg(feature = "items")]
            item_names: Mutex::new(Vec::new()),
            #[cfg(feature = "antiafk")]
            antiafk: std::sync::atomic::AtomicU64::new(options.antiafk_seconds),
            #[cfg(feature = "antiafk")]
            antiafk_running: AtomicBool::new(false),
            #[cfg(feature = "state")]
            sneak_on_join: options.sneak,
            #[cfg(feature = "state")]
            sneaking: AtomicBool::new(false),
            #[cfg(feature = "state")]
            sprinting: AtomicBool::new(false),
        }
    }
}

/// Registry-Paket der Konfigurationsphase. Nur die beiden Register werden vollständig gelesen,
/// die eine aktive Bauform wirklich benötigt; ein Paket ist bereits längenbegrenzt, deshalb darf
/// der Rest aller anderen Register ungelesen verworfen werden.
pub fn registry(shared: &Arc<Shared>, r: &mut Reader) {
    #[cfg(any(feature = "items", feature = "pov"))]
    registry_used(shared, r);
    // Bauformen ohne Gegenstände und ohne Live-Ansicht brauchen aus der Registry nichts.
    #[cfg(not(any(feature = "items", feature = "pov")))]
    let _ = (shared, r);
}

#[cfg(any(feature = "items", feature = "pov"))]
fn registry_used(shared: &Arc<Shared>, r: &mut Reader) {
    let Ok(registry) = r.string() else { return };
    let wants_items = cfg!(feature = "items") && registry == "minecraft:item";
    let wants_dimensions = cfg!(feature = "pov") && registry == "minecraft:dimension_type";
    if !wants_items && !wants_dimensions {
        return;
    }

    let Ok(count) = r.var_int() else { return };
    if !(0..=65_536).contains(&count) {
        return;
    }

    // Nur der wirklich gesuchte Vektor bekommt Platz. `minecraft:item` hat gut anderthalbtausend
    // Einträge – dafür in einer POV-Bauform, die die Liste gar nicht will, Speicher anzufordern,
    // wäre reine Verschwendung.
    #[cfg(feature = "items")]
    let mut item_names = match wants_items {
        true => Vec::with_capacity(count as usize),
        false => Vec::new(),
    };
    #[cfg(feature = "pov")]
    let mut dimensions = match wants_dimensions {
        true => Vec::with_capacity(count as usize),
        false => Vec::new(),
    };

    for _ in 0..count {
        let Ok(name) = r.string() else { return };
        let Ok(has_data) = r.bool() else { return };
        let data = if has_data {
            match crate::nbt::read_network(r) {
                Ok(data) => Some(data),
                Err(_) => return,
            }
        } else {
            None
        };
        #[cfg(not(feature = "pov"))]
        let _ = &data;

        #[cfg(feature = "items")]
        if wants_items {
            item_names.push(name.clone());
        }
        #[cfg(feature = "pov")]
        if wants_dimensions {
            dimensions.push(crate::pov::Dimension::from_registry(name, data.as_ref()));
        }
    }

    #[cfg(feature = "items")]
    if wants_items {
        *shared.extras.item_names.lock().unwrap() = item_names;
    }
    #[cfg(feature = "pov")]
    if wants_dimensions {
        shared.extras.pov.set_dimensions(dimensions);
    }
}

// ===================== Haken aus dem Client =====================

/// Zusatzpakete auswerten. Alles, was hier ankommt, hat [`crate::proto::Protocol::incoming`]
/// bereits einer Ausbaustufe zugeordnet.
pub fn incoming(shared: &Arc<Shared>, kind: In, r: &mut Reader) {
    match kind {
        #[cfg(feature = "board")]
        In::Objective | In::Score | In::ResetScore | In::DisplayObjective | In::Team => {
            crate::board::incoming(shared, kind, r)
        }

        #[cfg(feature = "menu")]
        In::OpenScreen | In::ContainerContent | In::ContainerSlot | In::ContainerClose => {
            crate::menu::incoming(shared, kind, r)
        }
        #[cfg(feature = "items")]
        In::PlayerInventory => crate::menu::incoming(shared, kind, r),

        #[cfg(feature = "pov")]
        In::LevelChunk
        | In::ForgetChunk
        | In::BlockUpdate
        | In::SectionBlocks
        | In::ChunkBatchFinished
        | In::AddEntity
        | In::RemoveEntities
        | In::MoveEntityPos
        | In::MoveEntityPosRot
        | In::TeleportEntity
        | In::EntityPositionSync => crate::pov::incoming(shared, kind, r),

        _ => {}
    }
}

pub fn on_join(shared: &Arc<Shared>) {
    // Der alte Stand gilt nicht mehr – der neue Server schickt seinen eigenen.
    #[cfg(feature = "board")]
    shared.extras.board.clear();
    #[cfg(feature = "menu")]
    shared.extras.menu.clear();
    #[cfg(feature = "pov")]
    crate::pov::on_join(shared);

    #[cfg(feature = "state")]
    {
        shared.extras.sneaking.store(false, Ordering::Relaxed);
        shared.extras.sprinting.store(false, Ordering::Relaxed);
        if shared.extras.sneak_on_join {
            set_sneak(shared, true);
        }
    }
    #[cfg(feature = "antiafk")]
    crate::antiafk::on_join(shared);
}

pub fn on_disconnect(shared: &Shared) {
    let _ = shared;
    #[cfg(feature = "board")]
    shared.extras.board.clear();
    #[cfg(feature = "menu")]
    shared.extras.menu.clear();
    #[cfg(feature = "pov")]
    shared.extras.pov.clear();
}

// ===================== Befehle =====================

/// Befehl aus der Eingabeschleife. `true` = erledigt, `false` = nicht meiner (dann bekommt ihn
/// die Bewegung).
pub fn command(shared: &Arc<Shared>, verb: &str, arg: &str) -> bool {
    let _ = arg;
    match verb {
        #[cfg(feature = "board")]
        "board" | "tafel" | "scoreboard" => crate::board::print_sidebar(shared),

        #[cfg(feature = "menu")]
        "menu" | "menü" | "container" | "kiste" => crate::menu::print(shared),
        #[cfg(feature = "menu")]
        "click" | "klick" | "klicke" => crate::menu::click_command(shared, arg),
        #[cfg(feature = "menu")]
        "close" | "schliessen" | "schließen" | "zu" => crate::menu::close_command(shared),
        #[cfg(feature = "items")]
        "slot" | "feld" => crate::menu::print_slot(shared, arg),
        #[cfg(feature = "items")]
        "inv" | "inventar" | "inventory" => crate::menu::print_inventory(shared),

        #[cfg(feature = "pov")]
        "pov" | "sicht" | "ansicht" => crate::pov::command(shared, arg),

        #[cfg(feature = "state")]
        "sneak" | "schleich" | "schleichen" | "ducken" => toggle(
            shared,
            arg,
            &shared.extras.sneaking,
            "Schleichen",
            set_sneak,
        ),
        #[cfg(feature = "state")]
        "sprint" | "rennen" | "sprinten" => toggle(
            shared,
            arg,
            &shared.extras.sprinting,
            "Sprinten",
            set_sprint,
        ),
        #[cfg(feature = "state")]
        "swing" | "schlag" | "schlage" | "arm" => {
            let mut w = Writer::packet(shared.proto.extra.sb_swing);
            w.var_int(0); // Haupthand
            shared.send(w);
            shared.console.info("Arm geschwungen.");
        }
        #[cfg(feature = "state")]
        "use" | "benutze" | "rechtsklick" => use_item(shared),
        #[cfg(feature = "state")]
        "hand" | "slot-hotbar" | "hotbar" => hotbar(shared, arg),

        #[cfg(feature = "antiafk")]
        "antiafk" | "anti-afk" | "zappeln" => crate::antiafk::command(shared, arg),

        _ => return false,
    }
    true
}

/// Zeilen für `:help` – nur die, die dieser Build wirklich hat.
pub fn help_lines() -> &'static [&'static str] {
    &[
        #[cfg(feature = "board")]
        ":board                                 Seitenleiste (mit Farbcodes)",
        #[cfg(feature = "menu")]
        ":menu  ·  :click <feld> [rechts|shift] ·  :close",
        #[cfg(feature = "items")]
        ":slot <feld>  ·  :inv                  Gegenstand mit Lore · eigenes Inventar",
        #[cfg(feature = "pov")]
        ":pov [live|stop|frame|size|info]       Live-Ansicht dessen, was der Spieler sieht",
        #[cfg(feature = "state")]
        ":sneak [on|off]  ·  :sprint [on|off]   Schleichen · Sprinten",
        #[cfg(feature = "state")]
        ":swing  ·  :use  ·  :hand <1-9>        Arm · Rechtsklick · Schnellleiste",
        #[cfg(feature = "antiafk")]
        ":antiafk [on|off|<sek>]                automatische kleine Bewegung",
    ]
}

/// `on|off|an|aus` oder ohne Angabe umschalten – für `:sneak` und `:sprint` gleich.
#[cfg(feature = "state")]
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

#[cfg(feature = "state")]
pub(crate) fn set_sneak(shared: &Arc<Shared>, on: bool) {
    shared.extras.sneaking.store(on, Ordering::Relaxed);
    send_state(
        shared,
        on,
        values::player_state::START_SNEAKING,
        values::player_state::STOP_SNEAKING,
    );
}

#[cfg(feature = "state")]
pub(crate) fn set_sprint(shared: &Arc<Shared>, on: bool) {
    shared.extras.sprinting.store(on, Ordering::Relaxed);
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
#[cfg(feature = "state")]
fn send_state(shared: &Arc<Shared>, on: bool, start: i32, stop: i32) {
    if shared.proto.modern {
        let mut bits = 0u8;
        if shared.extras.sneaking.load(Ordering::Relaxed) {
            bits |= values::input::SNEAK;
        }
        if shared.extras.sprinting.load(Ordering::Relaxed) {
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
///
/// Die Blickrichtung steht erst ab 1.21.2 mit im Paket. In 1.21.1 besteht
/// `ServerboundUseItemPacket` nur aus Hand und Sequenznummer – die acht zusätzlichen Bytes
/// haben dort den Paket-Decoder des Servers aus dem Tritt gebracht und die Verbindung gekostet.
#[cfg(feature = "state")]
fn use_item(shared: &Arc<Shared>) {
    let mut w = Writer::packet(shared.proto.extra.sb_use_item);
    w.var_int(0); // Haupthand
    w.var_int(0); // Sequenznummer – der Server nutzt sie nur zum Zurückrollen von Blockänderungen
    if shared.proto.modern {
        let (yaw, pitch) = shared
            .position()
            .map(|(_, _, _, yaw, pitch)| (yaw, pitch))
            .unwrap_or((0.0, 0.0));
        w.f32(yaw);
        w.f32(pitch);
    }
    shared.send(w);
    shared.console.info("Gegenstand benutzt (Rechtsklick).");
}

/// `:hand 1..9` – das Feld in der Schnellleiste wechseln.
#[cfg(feature = "state")]
fn hotbar(shared: &Arc<Shared>, arg: &str) {
    match arg.trim().parse::<u8>() {
        Ok(slot @ 1..=9) => {
            let mut w = Writer::packet(shared.proto.extra.sb_set_carried_item);
            w.u16(slot as u16 - 1); // übertragen wird 0..8
            shared.send(w);
            shared
                .console
                .info(&format!("Schnellleiste: Feld {}.", slot));
        }
        _ => shared.console.error("Nutzung: :hand <1-9>"),
    }
}

#[cfg(test)]
mod tests {
    use crate::proto::PROTOCOLS;

    /// Das Bitfeld ab 1.21.11 beschreibt den Gesamtzustand: beide Tasten müssen sich unabhängig
    /// setzen lassen, ohne einander zu überschreiben.
    #[cfg(feature = "state")]
    #[test]
    fn eingabebits_stoeren_sich_nicht() {
        use crate::proto::values;
        let bits = values::input::SNEAK | values::input::SPRINT;
        assert_eq!(bits & values::input::SNEAK, values::input::SNEAK);
        assert_eq!(bits & values::input::SPRINT, values::input::SPRINT);
        assert_ne!(values::input::SNEAK, values::input::SPRINT);
    }

    /// Die Zusatz-IDs müssen in jeder Version gesetzt sein – eine vergessene 0 würde ein
    /// völlig anderes Paket senden. Ausnahme sind die beiden Pakete, die es in 1.21.1 noch
    /// nicht gibt; die stehen dort ausdrücklich auf -1.
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
                ("cb_level_chunk", e.cb_level_chunk),
                ("cb_forget_level_chunk", e.cb_forget_level_chunk),
                ("cb_block_update", e.cb_block_update),
                ("cb_section_blocks_update", e.cb_section_blocks_update),
                ("cb_chunk_batch_finished", e.cb_chunk_batch_finished),
                ("sb_chunk_batch_received", e.sb_chunk_batch_received),
                ("cb_add_entity", e.cb_add_entity),
                ("cb_remove_entities", e.cb_remove_entities),
                ("cb_move_entity_pos", e.cb_move_entity_pos),
                ("cb_move_entity_pos_rot", e.cb_move_entity_pos_rot),
                ("cb_teleport_entity", e.cb_teleport_entity),
                ("player_entity_type", e.player_entity_type),
            ] {
                assert!(id > 0, "{} fehlt in {}", name, p.name);
            }
            let only_modern = [
                ("cb_entity_position_sync", e.cb_entity_position_sync),
                ("cb_set_player_inventory", e.cb_set_player_inventory),
            ];
            for (name, id) in only_modern {
                if p.modern {
                    assert!(id > 0, "{} fehlt in {}", name, p.name);
                } else {
                    assert_eq!(id, -1, "{} gibt es in {} noch nicht", name, p.name);
                }
            }
        }
    }
}
