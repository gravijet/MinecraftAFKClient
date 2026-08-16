//! Vanilla-Item-ID -> Ressourcenname – nur in Bauformen mit `items`.
//!
//! Ein Slot enthält nur die numerische Item-ID und einen Patch gegenüber den bekannten
//! Standard-Komponenten. Der Server schickt die eingebaute Item-Registry normalerweise nicht
//! in der Konfigurationsphase; ein Vanilla-Client kennt sie bereits aus seiner Version. Diese
//! vier Listen übernehmen genau diese Aufgabe. Ihre Herkunft und Prüfsummen stehen in
//! `rust/data/README.md`.

const V1_21_1: &str = include_str!("../data/items-1.21.1.txt");
const V1_21_11: &str = include_str!("../data/items-1.21.11.txt");
const V26_1: &str = include_str!("../data/items-26.1.txt");
const V26_2: &str = include_str!("../data/items-26.2.txt");

/// Ressourcenname für die Protokoll-ID. Die Listen sind nach `protocol_id` sortiert und
/// lückenlos, deshalb ist die Zeilennummer direkt die Netzwerk-ID.
pub fn get(protocol: &str, id: i32) -> Option<&'static str> {
    let table = match protocol {
        "1.21.1" => V1_21_1,
        "1.21.11" => V1_21_11,
        "26.1" => V26_1,
        "26.2" => V26_2,
        _ => return None,
    };
    table.lines().nth(usize::try_from(id).ok()?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn listen_sind_lueckenlos_und_versionsgenau() {
        for (version, table, count) in [
            ("1.21.1", V1_21_1, 1333),
            ("1.21.11", V1_21_11, 1505),
            ("26.1", V26_1, 1506),
            ("26.2", V26_2, 1537),
        ] {
            assert_eq!(table.lines().count(), count, "{}", version);
            assert_eq!(get(version, 0), Some("minecraft:air"), "{}", version);
            assert_eq!(get(version, 1), Some("minecraft:stone"), "{}", version);
            assert_eq!(get(version, count as i32), None, "{}", version);
        }
    }

    #[test]
    fn neue_versionen_haben_eigene_ids() {
        assert_eq!(get("1.21.1", 1332), Some("minecraft:breeze_rod"));
        assert_eq!(get("1.21.11", 1504), Some("minecraft:ominous_bottle"));
        assert_eq!(get("26.1", 1505), Some("minecraft:ominous_bottle"));
        assert_eq!(get("26.2", 1536), Some("minecraft:ominous_bottle"));
    }
}
