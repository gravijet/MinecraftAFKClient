//! Anzeigetafel (Seitenleiste) und Tab-Liste – **nur im Premium-Build**.
//!
//! Beides ist reiner Zustand: der Server schickt Ziele, Punkte, Teams und Spieler einzeln, und
//! wer sie anzeigen will, muss sie mitführen. Genau das ist der Grund, warum es das im schlanken
//! Client nicht gibt – der wirft jedes Paket weg, das er nicht sofort beantworten muss.
//!
//! Der Speicherbedarf ist trotzdem gedeckelt (siehe [`MAX_ENTRIES`]): ein Server, der Tausende
//! Einträge schickt, soll den Client nicht wachsen lassen. Skin-Texturen in der Tab-Liste werden
//! übersprungen statt gelesen – sie sind je Spieler ein paar Kilobyte und für uns wertlos.
//!
//! Alle Feldreihenfolgen sind aus den Codec-Jars von MCProtocolLib abgelesen (siehe
//! [`crate::proto`]); die einzige Stelle, die sich zwischen den Versionen unterscheidet, ist das
//! Team-Paket – dafür gibt es [`crate::proto::TeamLayout`].

use crate::buf::Reader;
use crate::client::Shared;
use crate::console::{BOLD, GRAY};
use crate::nbt;
use crate::proto::{values, In, TeamLayout};

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

/// Obergrenze je Tabelle. Reicht für jede echte Seitenleiste (15 Zeilen) und jeden vollen Server;
/// alles darüber ist Unsinn und wird verworfen.
const MAX_ENTRIES: usize = 512;
/// So viele Zeilen zeigt `:board` höchstens an – die Vanilla-Seitenleiste kann nicht mehr.
const MAX_LINES: usize = 15;

#[derive(Default)]
struct Team {
    prefix: String,
    suffix: String,
}

/// Ein Eintrag der Seitenleiste.
struct Score {
    value: i32,
    /// Ab 1.20.3 darf der Server je Zeile einen fertigen Anzeigetext mitschicken. Ist er da,
    /// gilt er – sonst wird die Zeile aus Team-Präfix + Name + Suffix zusammengesetzt.
    display: Option<String>,
}

#[derive(Default)]
struct Inner {
    /// Zielname -> Überschrift.
    objectives: HashMap<String, String>,
    /// Welches Ziel steht gerade in der Seitenleiste?
    sidebar: Option<String>,
    /// Zielname -> (Eintrag -> Punktzahl).
    scores: HashMap<String, HashMap<String, Score>>,
    teams: HashMap<String, Team>,
    /// Eintrag/Spielername -> Teamname.
    membership: HashMap<String, String>,
    /// Tab-Liste: UUID -> (Name, Anzeigename).
    players: HashMap<[u8; 16], (String, Option<String>)>,
}

pub struct Board {
    inner: Mutex<Inner>,
}

impl Board {
    pub fn new() -> Board {
        Board {
            inner: Mutex::new(Inner::default()),
        }
    }

    /// Nach Beitritt und Trennung: der neue Server hat seine eigene Tafel.
    pub fn clear(&self) {
        *self.inner.lock().unwrap() = Inner::default();
    }
}

// ===================== Pakete =====================

pub fn incoming(shared: &Arc<Shared>, kind: In, r: &mut Reader) {
    // Ein unlesbares Paket ist kein Grund, die Verbindung zu beenden: die Anzeigetafel ist
    // Beiwerk. Deshalb wird hier still abgebrochen statt einen Fehler nach oben zu reichen.
    let _ = read(shared, kind, r);
}

fn read(shared: &Arc<Shared>, kind: In, r: &mut Reader) -> std::io::Result<()> {
    let color = shared.console.is_color();
    let mut inner = shared.premium.board.inner.lock().unwrap();

    match kind {
        // name, action, [Anzeigename, Punktart, optionales Zahlenformat]
        In::Objective => {
            let name = r.string()?;
            match r.u8()? {
                values::objective::REMOVE => {
                    inner.objectives.remove(&name);
                    inner.scores.remove(&name);
                    if inner.sidebar.as_deref() == Some(name.as_str()) {
                        inner.sidebar = None;
                    }
                }
                values::objective::ADD | values::objective::UPDATE => {
                    let title = nbt::render(&nbt::read_network(r)?, color);
                    if inner.objectives.len() < MAX_ENTRIES || inner.objectives.contains_key(&name) {
                        inner.objectives.insert(name, title);
                    }
                }
                _ => {}
            }
        }

        // Eintrag, Ziel, Punktzahl, optionaler Anzeigename (danach kommt nur noch das
        // Zahlenformat, das uns nicht interessiert – wir hören einfach auf zu lesen).
        In::Score => {
            let owner = r.string()?;
            let objective = r.string()?;
            let value = r.var_int()?;
            let display = match r.bool() {
                Ok(true) => Some(nbt::render(&nbt::read_network(r)?, color)),
                _ => None,
            };
            let entries = inner.scores.entry(objective).or_default();
            if entries.len() < MAX_ENTRIES || entries.contains_key(&owner) {
                entries.insert(owner, Score { value, display });
            }
        }

        // Eintrag, optionales Ziel (fehlt es, gilt der Eintrag in allen Zielen).
        In::ResetScore => {
            let owner = r.string()?;
            match r.bool() {
                Ok(true) => {
                    let objective = r.string()?;
                    if let Some(entries) = inner.scores.get_mut(&objective) {
                        entries.remove(&owner);
                    }
                }
                _ => {
                    for entries in inner.scores.values_mut() {
                        entries.remove(&owner);
                    }
                }
            }
        }

        In::DisplayObjective => {
            let position = r.var_int()?;
            let objective = r.string()?;
            if position == values::SIDEBAR {
                inner.sidebar = (!objective.is_empty()).then_some(objective);
            }
        }

        In::Team => read_team(shared, &mut inner, r, color)?,

        In::PlayerInfoUpdate => read_player_info(shared, &mut inner, r, color)?,

        In::PlayerInfoRemove => {
            let count = r.var_int()?.clamp(0, MAX_ENTRIES as i32);
            for _ in 0..count {
                inner.players.remove(&r.uuid()?);
            }
        }

        _ => {}
    }
    Ok(())
}

/// Team-Paket. Aufbau nach `name` und `action` je nach Version verschieden – siehe
/// [`TeamLayout`]. Uns interessieren nur Präfix und Suffix (daraus bestehen auf fast jedem
/// Server die Zeilen der Seitenleiste) und die Mitgliederliste.
fn read_team(
    shared: &Arc<Shared>,
    inner: &mut Inner,
    r: &mut Reader,
    color: bool,
) -> std::io::Result<()> {
    let name = r.string()?;
    let action = r.u8()?;

    if action == values::team::REMOVE {
        inner.teams.remove(&name);
        inner.membership.retain(|_, team| team != &name);
        return Ok(());
    }

    if action == values::team::CREATE || action == values::team::UPDATE {
        let team = match shared.proto.extra.team_layout {
            TeamLayout::Legacy => {
                nbt::read_network(r)?; // Anzeigename
                r.u8()?; // Flags
                r.skip_string()?; // Sichtbarkeit der Namensschilder
                r.skip_string()?; // Kollisionsregel
                r.var_int()?; // Farbe
                let prefix = nbt::render(&nbt::read_network(r)?, color);
                let suffix = nbt::render(&nbt::read_network(r)?, color);
                Team { prefix, suffix }
            }
            TeamLayout::VarIntRules => {
                nbt::read_network(r)?; // Anzeigename
                r.u8()?; // Flags
                r.var_int()?; // Sichtbarkeit
                r.var_int()?; // Kollisionsregel
                r.var_int()?; // Farbe
                let prefix = nbt::render(&nbt::read_network(r)?, color);
                let suffix = nbt::render(&nbt::read_network(r)?, color);
                Team { prefix, suffix }
            }
            TeamLayout::Reordered => {
                nbt::read_network(r)?; // Anzeigename
                let prefix = nbt::render(&nbt::read_network(r)?, color);
                let suffix = nbt::render(&nbt::read_network(r)?, color);
                // Sichtbarkeit, Kollision, optionale Farbe, Flags – alles nach Präfix/Suffix,
                // also für uns nur noch Überspringen.
                r.var_int()?;
                r.var_int()?;
                if r.bool()? {
                    r.var_int()?;
                }
                r.u8()?;
                Team { prefix, suffix }
            }
        };
        if inner.teams.len() < MAX_ENTRIES || inner.teams.contains_key(&name) {
            inner.teams.insert(name.clone(), team);
        }
    }

    // Mitglieder stehen bei „anlegen", „hinzufügen" und „entfernen" am Ende.
    if action == values::team::CREATE
        || action == values::team::ADD_PLAYER
        || action == values::team::REMOVE_PLAYER
    {
        let count = r.var_int()?.clamp(0, MAX_ENTRIES as i32);
        for _ in 0..count {
            let entry = r.string()?;
            if action == values::team::REMOVE_PLAYER {
                inner.membership.remove(&entry);
            } else if inner.membership.len() < MAX_ENTRIES {
                inner.membership.insert(entry, name.clone());
            }
        }
    }
    Ok(())
}

/// Tab-Listen-Paket. Vorn steht ein Bitfeld, welche Angaben je Spieler folgen; danach die
/// Spieler. Alles, was uns nicht interessiert, wird übersprungen statt kopiert – vor allem die
/// Skin-Textur, die je Spieler mehrere Kilobyte groß ist.
fn read_player_info(
    shared: &Arc<Shared>,
    inner: &mut Inner,
    r: &mut Reader,
    color: bool,
) -> std::io::Result<()> {
    let actions = shared.proto.extra.player_info_actions;
    let mut mask: u32 = 0;
    for byte in 0..actions.div_ceil(8) {
        mask |= (r.u8()? as u32) << (8 * byte);
    }
    let has = |bit: u32| mask & bit != 0;

    let count = r.var_int()?.clamp(0, MAX_ENTRIES as i32);
    for _ in 0..count {
        let id = r.uuid()?;
        let mut name = None;
        let mut display = None;

        if has(values::info::ADD_PLAYER) {
            name = Some(r.string()?);
            // Eigenschaften (Skin): je Eintrag Name, Wert und optionale Signatur.
            let properties = r.var_int()?.clamp(0, 32);
            for _ in 0..properties {
                r.skip_string()?;
                r.skip_string()?;
                if r.bool()? {
                    r.skip_string()?;
                }
            }
        }
        if has(values::info::INITIALIZE_CHAT) && r.bool()? {
            r.skip(16)?; // Sitzungs-ID
            r.skip(8)?; // Gültig bis
            let key = r.var_int()?.max(0) as usize;
            r.skip(key)?;
            let signature = r.var_int()?.max(0) as usize;
            r.skip(signature)?;
        }
        if has(values::info::UPDATE_GAME_MODE) {
            r.var_int()?;
        }
        if has(values::info::UPDATE_LISTED) {
            r.bool()?;
        }
        if has(values::info::UPDATE_LATENCY) {
            r.var_int()?;
        }
        if has(values::info::UPDATE_DISPLAY_NAME) && r.bool()? {
            display = Some(nbt::render(&nbt::read_network(r)?, color));
        }
        if has(values::info::UPDATE_LIST_ORDER) {
            r.var_int()?;
        }
        if has(values::info::UPDATE_HAT) {
            r.bool()?;
        }

        let room = inner.players.len() < MAX_ENTRIES;
        match inner.players.get_mut(&id) {
            Some(entry) => {
                if let Some(name) = name {
                    entry.0 = name;
                }
                if display.is_some() {
                    entry.1 = display;
                }
            }
            None if room => {
                inner.players.insert(id, (name.unwrap_or_default(), display));
            }
            None => {}
        }
    }
    Ok(())
}

// ===================== Anzeige =====================

/// `:board` – die Seitenleiste so, wie sie im Spiel rechts stünde.
pub fn print_sidebar(shared: &Arc<Shared>) {
    let inner = shared.premium.board.inner.lock().unwrap();
    let console = &shared.console;

    let Some(objective) = inner.sidebar.as_ref() else {
        return console.error("Der Server zeigt gerade keine Seitenleiste an.");
    };
    let title = inner
        .objectives
        .get(objective)
        .cloned()
        .unwrap_or_else(|| objective.clone());

    // Die Seitenleiste ist nach Punktzahl absteigend sortiert – genau wie im Spiel.
    let mut lines: Vec<(i32, String)> = match inner.scores.get(objective) {
        Some(entries) => entries
            .iter()
            .map(|(entry, score)| (score.value, line_for(&inner, entry, score)))
            .collect(),
        None => Vec::new(),
    };
    lines.sort_by(|a, b| b.0.cmp(&a.0));

    console.print("");
    console.print(&format!("  {}", console.paint(BOLD, &title)));
    if lines.is_empty() {
        console.print(&console.paint(GRAY, "    (keine Zeilen)"));
    }
    for (value, text) in lines.iter().take(MAX_LINES) {
        console.print(&format!("    {}  {}", text, console.paint(GRAY, &value.to_string())));
    }
}

/// Eine Zeile der Seitenleiste. Vorrang hat der fertige Anzeigetext des Servers; sonst wird sie
/// aus Team-Präfix + Eintrag + Suffix gebaut – so machen es fast alle Server, weil der Eintrag
/// selbst nur ein unsichtbarer Platzhalter ist.
fn line_for(inner: &Inner, entry: &str, score: &Score) -> String {
    if let Some(display) = &score.display {
        return display.clone();
    }
    match inner.membership.get(entry).and_then(|t| inner.teams.get(t)) {
        Some(team) => format!("{}{}{}", team.prefix, entry, team.suffix),
        None => entry.to_string(),
    }
}

/// `:tab` – die Spielerliste.
pub fn print_tab(shared: &Arc<Shared>) {
    let inner = shared.premium.board.inner.lock().unwrap();
    let console = &shared.console;

    let mut names: Vec<String> = inner
        .players
        .values()
        .map(|(name, display)| display.clone().unwrap_or_else(|| name.clone()))
        .filter(|name| !name.trim().is_empty())
        .collect();
    names.sort_by_key(|name| name.to_lowercase());

    console.print("");
    console.print(&format!(
        "  {}",
        console.paint(BOLD, &format!("Spieler ({})", names.len()))
    ));
    if names.is_empty() {
        return console.print(&console.paint(GRAY, "    (noch keine – kurz nach dem Beitritt)"));
    }
    // Drei je Zeile: eine Liste mit 200 Namen soll das Terminal nicht fluten.
    for chunk in names.chunks(3) {
        console.print(&format!("    {}", chunk.join("   ")));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inner_with_team() -> Inner {
        let mut inner = Inner::default();
        inner.teams.insert(
            "t1".to_string(),
            Team {
                prefix: "Rang: ".to_string(),
                suffix: " ★".to_string(),
            },
        );
        inner.membership.insert("§a".to_string(), "t1".to_string());
        inner
    }

    /// Auf den meisten Servern ist der Eintrag selbst ein unsichtbarer Platzhalter; der Text
    /// steckt in Präfix und Suffix des Teams.
    #[test]
    fn zeile_kommt_aus_praefix_und_suffix() {
        let inner = inner_with_team();
        let score = Score {
            value: 3,
            display: None,
        };
        assert_eq!(line_for(&inner, "§a", &score), "Rang: §a ★");
        // Ohne Team bleibt der Eintrag stehen.
        assert_eq!(line_for(&inner, "ohne", &score), "ohne");
    }

    /// Schickt der Server einen fertigen Anzeigetext, gilt der – Team hin oder her.
    #[test]
    fn anzeigetext_hat_vorrang() {
        let inner = inner_with_team();
        let score = Score {
            value: 3,
            display: Some("fertige Zeile".to_string()),
        };
        assert_eq!(line_for(&inner, "§a", &score), "fertige Zeile");
    }

    /// Das Bitfeld der Tab-Liste ist in allen unterstützten Versionen genau ein Byte breit.
    #[test]
    fn bitfeld_der_tabliste_ist_ein_byte() {
        for p in crate::proto::PROTOCOLS {
            assert_eq!(
                p.extra.player_info_actions.div_ceil(8),
                1,
                "unerwartete Bitfeldbreite in {}",
                p.name
            );
        }
    }
}
