//! Anzeigetafel (Seitenleiste) – **nur mit `--features board`** (also im Premium- und im
//! Ultra-Client).
//!
//! Das ist reiner Zustand: der Server schickt Ziele, Punkte und Teams einzeln, und wer sie
//! anzeigen will, muss sie mitführen. Genau das ist der Grund, warum es das im schlanken Client
//! nicht gibt – der wirft jedes Paket weg, das er nicht sofort beantworten muss.
//!
//! **Farben bleiben erhalten.** Alles, was hier landet, wird als `§`-Text gespeichert (siehe
//! [`crate::nbt::Fmt::Legacy`]) statt als fertige ANSI-Zeile. Damit lässt sich dieselbe Zeile
//! sowohl eingefärbt im Terminal anzeigen als auch über `--events` unverändert an ein Programm
//! davor weiterreichen – mit Farbcodes, Fettschrift und allem, was der Server geschickt hat.
//!
//! Der Speicherbedarf ist gedeckelt (siehe [`MAX_ENTRIES`]): ein Server, der Tausende Einträge
//! schickt, soll den Client nicht wachsen lassen.
//!
//! Alle Feldreihenfolgen sind aus den Codec-Jars von MCProtocolLib abgelesen (siehe
//! [`crate::proto`]); die einzige Stelle, die sich zwischen den Versionen unterscheidet, ist das
//! Team-Paket – dafür gibt es [`crate::proto::TeamLayout`].

use crate::buf::Reader;
use crate::client::Shared;
use crate::console::{BOLD, GRAY};
use crate::nbt::{self, Fmt};
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
    /// `§`-Text, kein ANSI – siehe Modulkopf.
    prefix: String,
    suffix: String,
    /// Stil des eigentlichen Eintrags. Vanilla setzt ihn zwischen Präfix und Suffix separat.
    color: Option<char>,
}

struct Objective {
    title: String,
    number: Option<NumberFormat>,
}

#[derive(Clone)]
enum NumberFormat {
    Blank,
    /// Bereits fertiges `§`-Präfix; dahinter kommt der tatsächliche Punktwert.
    Styled(String),
    /// Vollständig vom Server vorgegebener Text, unabhängig vom Punktwert.
    Fixed(String),
}

/// Ein Eintrag der Seitenleiste.
struct Score {
    value: i32,
    /// Ab 1.20.3 darf der Server je Zeile einen fertigen Anzeigetext mitschicken. Ist er da,
    /// gilt er – sonst wird die Zeile aus Team-Präfix + Name + Suffix zusammengesetzt.
    display: Option<String>,
    /// `None` übernimmt das Zahlenformat des Objectives bzw. Vanilla-Rot.
    number: Option<NumberFormat>,
}

#[derive(Default)]
struct Inner {
    /// Zielname -> Überschrift und Standard-Zahlenformat.
    objectives: HashMap<String, Objective>,
    /// Welches Ziel steht gerade in der Seitenleiste?
    sidebar: Option<String>,
    /// Zielname -> (Eintrag -> Punktzahl).
    scores: HashMap<String, HashMap<String, Score>>,
    teams: HashMap<String, Team>,
    /// Eintrag/Spielername -> Teamname.
    membership: HashMap<String, String>,
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
    let mut inner = shared.extras.board.inner.lock().unwrap();

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
                    let title = nbt::render(&nbt::read_network(r)?, Fmt::Legacy);
                    r.var_int()?; // ScoreType (Integer/Hearts)
                    let number = read_optional_number_format(r)?;
                    if inner.objectives.len() < MAX_ENTRIES || inner.objectives.contains_key(&name)
                    {
                        inner.objectives.insert(name, Objective { title, number });
                    }
                }
                _ => {}
            }
        }

        // Eintrag, Ziel, Punktzahl, optionaler Anzeigename und optionales Zahlenformat.
        In::Score => {
            let owner = r.string()?;
            let objective = r.string()?;
            let value = r.var_int()?;
            let display = if r.bool()? {
                Some(nbt::render(&nbt::read_network(r)?, Fmt::Legacy))
            } else {
                None
            };
            let number = read_optional_number_format(r)?;
            // Auch die Zahl der *Ziele* deckeln: ohne das legte ein Server, der Punkte für immer
            // neue Zielnamen schickt, mit jedem Paket eine weitere Tabelle an.
            if inner.scores.len() >= MAX_ENTRIES && !inner.scores.contains_key(&objective) {
                return Ok(());
            }
            let entries = inner.scores.entry(objective).or_default();
            if entries.len() < MAX_ENTRIES || entries.contains_key(&owner) {
                entries.insert(
                    owner,
                    Score {
                        value,
                        display,
                        number,
                    },
                );
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

        In::Team => read_team(shared, &mut inner, r)?,

        _ => {}
    }
    Ok(())
}

/// Nullable `NumberFormat`: 0 = keine Zahl, 1 = Stil für den echten Wert, 2 = fester Text.
fn read_optional_number_format(r: &mut Reader) -> std::io::Result<Option<NumberFormat>> {
    if !r.bool()? {
        return Ok(None);
    }
    Ok(Some(match r.var_int()? {
        0 => NumberFormat::Blank,
        1 => NumberFormat::Styled(nbt::style_legacy(&nbt::read_network(r)?)),
        2 => NumberFormat::Fixed(nbt::render(&nbt::read_network(r)?, Fmt::Legacy)),
        _ => return Err(crate::buf::err("Unbekanntes Scoreboard-Zahlenformat")),
    }))
}

/// Team-Paket. Aufbau nach `name` und `action` je nach Version verschieden – siehe
/// [`TeamLayout`]. Uns interessieren nur Präfix und Suffix (daraus bestehen auf fast jedem
/// Server die Zeilen der Seitenleiste) und die Mitgliederliste.
fn read_team(shared: &Arc<Shared>, inner: &mut Inner, r: &mut Reader) -> std::io::Result<()> {
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
                let color = team_color(r.var_int()?);
                let prefix = nbt::render(&nbt::read_network(r)?, Fmt::Legacy);
                let suffix = nbt::render(&nbt::read_network(r)?, Fmt::Legacy);
                Team {
                    prefix,
                    suffix,
                    color,
                }
            }
            TeamLayout::VarIntRules => {
                nbt::read_network(r)?; // Anzeigename
                r.u8()?; // Flags
                r.var_int()?; // Sichtbarkeit
                r.var_int()?; // Kollisionsregel
                let color = team_color(r.var_int()?);
                let prefix = nbt::render(&nbt::read_network(r)?, Fmt::Legacy);
                let suffix = nbt::render(&nbt::read_network(r)?, Fmt::Legacy);
                Team {
                    prefix,
                    suffix,
                    color,
                }
            }
            TeamLayout::Reordered => {
                nbt::read_network(r)?; // Anzeigename
                let prefix = nbt::render(&nbt::read_network(r)?, Fmt::Legacy);
                let suffix = nbt::render(&nbt::read_network(r)?, Fmt::Legacy);
                // Sichtbarkeit, Kollision, optionale Farbe, Flags – alles nach Präfix/Suffix,
                // also für uns nur noch Überspringen.
                r.var_int()?;
                r.var_int()?;
                let color = r
                    .bool()?
                    .then(|| r.var_int())
                    .transpose()?
                    .and_then(team_color);
                r.u8()?;
                Team {
                    prefix,
                    suffix,
                    color,
                }
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

// ===================== Anzeige =====================

/// `:board` – die Seitenleiste so, wie sie im Spiel rechts stünde.
///
/// Zusätzlich geht mit `--events` dieselbe Tafel als `@event board`-Zeilen raus, dort aber mit
/// `§`-Farbcodes statt ANSI: ein Panel kann sie damit genauso einfärben wie im Spiel.
pub fn print_sidebar(shared: &Arc<Shared>) {
    let inner = shared.extras.board.inner.lock().unwrap();
    let console = &shared.console;

    let Some(objective) = inner.sidebar.as_ref() else {
        return console.error("Der Server zeigt gerade keine Seitenleiste an.");
    };
    let objective_data = inner.objectives.get(objective);
    let title = objective_data
        .map(|data| data.title.as_str())
        .unwrap_or(objective);

    // Die Seitenleiste ist nach Punktzahl absteigend sortiert – genau wie im Spiel. Bei gleicher
    // Punktzahl entscheidet der Eintragsname (auch das macht Vanilla so). Ohne dieses zweite
    // Kriterium käme die Reihenfolge aus der Hashtabelle und die Zeilen sprängen bei jedem
    // `:board` neu durcheinander.
    let mut lines: Vec<(i32, &str, String, Option<String>)> = match inner.scores.get(objective) {
        Some(entries) => entries
            .iter()
            .map(|(entry, score)| {
                (
                    score.value,
                    entry.as_str(),
                    line_for(&inner, entry, score),
                    number_for(score, objective_data.and_then(|data| data.number.as_ref())),
                )
            })
            .collect(),
        None => Vec::new(),
    };
    lines.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
    lines.truncate(MAX_LINES);

    console.print("");
    console.print(&format!("  {}", console.paint(BOLD, &console.text(&title))));
    if lines.is_empty() {
        console.print(&console.paint(GRAY, "    (keine Zeilen)"));
    }
    for (_, _, text, number) in &lines {
        let number = number.as_deref().map(|text| console.text(text));
        console.print(&match number {
            Some(number) => format!("    {}  {}", console.text(text), number),
            None => format!("    {}", console.text(text)),
        });
    }

    console.event("board", &format!("titel {}", title));
    for (value, _, text, number) in &lines {
        console.event(
            "board",
            &format!(
                "zeile wert={} zahl={} text={}",
                value,
                number.as_deref().unwrap_or(""),
                text
            ),
        );
    }
}

fn number_for(score: &Score, objective: Option<&NumberFormat>) -> Option<String> {
    match score.number.as_ref().or(objective) {
        Some(NumberFormat::Blank) => None,
        Some(NumberFormat::Styled(style)) => Some(format!("{}{}", style, score.value)),
        Some(NumberFormat::Fixed(text)) => Some(text.clone()),
        None => Some(format!("§c{}", score.value)),
    }
}

/// Eine Zeile der Seitenleiste als `§`-Text. Vorrang hat der fertige Anzeigetext des Servers;
/// sonst wird sie aus Team-Präfix + Eintrag + Suffix gebaut – so machen es fast alle Server,
/// weil der Eintrag selbst nur ein unsichtbarer Platzhalter ist.
fn line_for(inner: &Inner, entry: &str, score: &Score) -> String {
    if let Some(display) = &score.display {
        return display.clone();
    }
    match inner.membership.get(entry).and_then(|t| inner.teams.get(t)) {
        Some(team) => match team.color {
            Some(color) => format!("{}§{}{}{}", team.prefix, color, entry, team.suffix),
            None => format!("{}{}{}", team.prefix, entry, team.suffix),
        },
        None => entry.to_string(),
    }
}

/// `TeamColor` ist in den Codec-Jars genau in dieser Reihenfolge registriert. Neben den 16
/// Farben sind die sechs alten Formatierungen zulässig; unbekannte Werte werden nicht erfunden.
fn team_color(id: i32) -> Option<char> {
    const LEGACY: &[u8; 22] = b"0123456789abcdefklmnor";
    usize::try_from(id)
        .ok()
        .and_then(|index| LEGACY.get(index).copied())
        .map(char::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn inner_with_team() -> Inner {
        let mut inner = Inner::default();
        inner.teams.insert(
            "t1".to_string(),
            Team {
                prefix: "\u{a7}aRang: ".to_string(),
                suffix: " \u{a7}6*".to_string(),
                color: None,
            },
        );
        inner
            .membership
            .insert("hugo".to_string(), "t1".to_string());
        inner
    }

    /// Auf den meisten Servern ist der Eintrag selbst ein unsichtbarer Platzhalter; der Text
    /// steckt in Präfix und Suffix des Teams – **mit** den Farbcodes des Servers.
    #[test]
    fn zeile_kommt_aus_praefix_und_suffix() {
        let inner = inner_with_team();
        let score = Score {
            value: 3,
            display: None,
            number: None,
        };
        assert_eq!(line_for(&inner, "hugo", &score), "§aRang: hugo §6*");
        // Ohne Team bleibt der Eintrag stehen.
        assert_eq!(line_for(&inner, "ohne", &score), "ohne");
    }

    /// Die gespeicherte Zeile muss sich sowohl einfärben als auch entfärben lassen.
    #[test]
    fn zeile_laesst_sich_einfaerben_und_entfaerben() {
        let inner = inner_with_team();
        let line = line_for(
            &inner,
            "hugo",
            &Score {
                value: 1,
                display: None,
                number: None,
            },
        );
        assert_eq!(nbt::strip_legacy(&line), "Rang: hugo *");
        assert!(nbt::legacy_to_ansi(&line).contains("\x1b[92m"));
    }

    /// Schickt der Server einen fertigen Anzeigetext, gilt der – Team hin oder her.
    #[test]
    fn anzeigetext_hat_vorrang() {
        let inner = inner_with_team();
        let score = Score {
            value: 3,
            display: Some("fertige Zeile".to_string()),
            number: None,
        };
        assert_eq!(line_for(&inner, "hugo", &score), "fertige Zeile");
    }

    /// Gleiche Punktzahl kam bisher in der Reihenfolge der Hashtabelle heraus – die Zeilen
    /// sprangen dadurch bei jedem `:board` neu durcheinander. Entscheiden muss der Eintragsname.
    #[test]
    fn gleiche_punktzahl_bleibt_in_fester_reihenfolge() {
        fn sort(mut lines: Vec<(i32, &str)>) -> Vec<(i32, &str)> {
            lines.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(b.1)));
            lines
        }
        let expected = vec![(9, "alpha"), (3, "aaa"), (3, "bbb"), (3, "ccc"), (1, "zzz")];
        assert_eq!(
            sort(vec![(3, "ccc"), (1, "zzz"), (3, "aaa"), (9, "alpha"), (3, "bbb")]),
            expected
        );
        // Dieselbe Menge in anderer Ausgangsreihenfolge muss dasselbe Bild ergeben.
        assert_eq!(
            sort(vec![(3, "bbb"), (9, "alpha"), (3, "aaa"), (3, "ccc"), (1, "zzz")]),
            expected
        );
    }

    #[test]
    fn zahlenformat_kann_faerben_ersetzen_oder_ausblenden() {
        let score = Score {
            value: 12,
            display: None,
            number: None,
        };
        assert_eq!(number_for(&score, None).as_deref(), Some("§c12"));
        assert_eq!(
            number_for(&score, Some(&NumberFormat::Styled("§6§l".into()))).as_deref(),
            Some("§6§l12")
        );
        assert_eq!(
            number_for(&score, Some(&NumberFormat::Fixed("§bX".into()))).as_deref(),
            Some("§bX")
        );
        assert!(number_for(&score, Some(&NumberFormat::Blank)).is_none());
    }
}
