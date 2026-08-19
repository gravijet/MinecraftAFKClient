//! Ablauftests gegen einen echten (winzigen) Minecraft-Server – siehe `tests/common/mod.rs`.
//!
//! Sie starten das gebaute Binary als eigenen Prozess. Damit wird genau das geprüft, was ein
//! Panel oder ein Mensch tatsächlich zu sehen bekommt: die Standardausgabe (nur Chat), die
//! Fehlerausgabe (Zustand, `@event`, POV-Bilder) und die Pakete, die beim Server ankommen.

mod common;

use common::{Note, Plan};
use std::io::Write;
use std::time::Duration;

const TIMEOUT: Duration = Duration::from_secs(25);

fn plan_with_ground() -> Plan {
    let mut chunks = Vec::new();
    for x in -2..=2 {
        for z in -2..=2 {
            chunks.push((x, z));
        }
    }
    Plan {
        chunks,
        // Abschnitt 4 deckt bei min_y = -64 die Höhen y = 0..15 ab; der Spieler steht darauf.
        solid_section: 4,
        mixed_palette: false,
        filled_sections: 1,
        position: (8.0, 16.0, 8.0),
        chat: vec!["Willkommen auf dem Testserver".to_string()],
        hold_secs: 20,
    }
}

/// Der Grundablauf: verbinden, beitreten, Chat lesen, Befehl senden.
#[test]
fn beitritt_chat_und_befehl() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(
        server.port,
        "26.1",
        &["--no-color", "--events", "-c", "/afk", "--join-delay", "0"],
    );
    let out = common::collect(child.stdout.take().unwrap());
    let err = common::collect(child.stderr.take().unwrap());

    let (joined, log) = common::wait_for(&err, TIMEOUT, "@event join");
    assert!(joined, "kein Beitritt gemeldet. Ausgabe:\n{}", log);

    let (chat, seen) = common::wait_for(&out, TIMEOUT, "Willkommen auf dem Testserver");
    assert!(chat, "Chat kam nicht an. Ausgabe:\n{}", seen);

    let got_command = common::wait_note(&server.notes, TIMEOUT, |note| {
        matches!(note, Note::Command(c) if c == "afk")
    });
    assert!(got_command, "--cmd hat den Befehl nicht geschickt");

    let _ = child.kill();
}

/// Derselbe Ablauf auf dem älteren Protokoll – dort sind mehrere Paketformate anders.
#[test]
fn beitritt_auch_auf_1_21_1() {
    let server = common::start(&common::MC_1_21_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "1.21.1", &["--no-color", "--events"]);
    let out = common::collect(child.stdout.take().unwrap());
    let err = common::collect(child.stderr.take().unwrap());

    let (joined, log) = common::wait_for(&err, TIMEOUT, "@event join");
    assert!(joined, "kein Beitritt gemeldet. Ausgabe:\n{}", log);
    let (chat, seen) = common::wait_for(&out, TIMEOUT, "Willkommen auf dem Testserver");
    assert!(chat, "Chat kam nicht an. Ausgabe:\n{}", seen);

    let _ = child.kill();
}

/// Der Teleport muss bestätigt werden, sonst holt der Server uns per Rubberband zurück.
#[test]
fn teleport_wird_bestaetigt() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "-q"]);
    let accepted = common::wait_note(&server.notes, TIMEOUT, |note| {
        matches!(note, Note::Packet(id) if *id == common::MC_26_1.sb_accept_teleportation)
    });
    assert!(accepted, "Teleport wurde nicht bestätigt");
    let _ = child.kill();
}

/// Eingabezeilen gehen als Chat raus.
#[test]
fn eingabe_geht_in_den_chat() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "--chat-delay", "200"]);
    let mut stdin = child.stdin.take().unwrap();
    let err = common::collect(child.stderr.take().unwrap());
    let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);

    let _ = writeln!(stdin, "hallo welt");
    let arrived = common::wait_note(&server.notes, TIMEOUT, |note| {
        matches!(note, Note::Chat(text) if text == "hallo welt")
    });
    assert!(arrived, "die Chatzeile kam nicht beim Server an");
    let _ = child.kill();
}

// ===================== Live-POV =====================

/// Das Bildformat ist Schnittstelle nach außen: Kopfzeile `POV …`, danach je Terminalzeile eine
/// Reihe Halbblöcke mit Vorder- und Hintergrundfarbe. Ändert sich das, muss jedes Panel nach.
#[cfg(feature = "pov")]
#[test]
fn pov_bild_hat_das_vereinbarte_format() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--pov-size", "40x20"]);
    let mut stdin = child.stdin.take().unwrap();
    let err = common::collect(child.stderr.take().unwrap());

    let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);
    let _ = writeln!(stdin, ":pov live");

    // 20 Bildzeilen ergeben 10 Zeichenzeilen à 40 Zellen. Auf alle zehn **vollständig gelesenen**
    // warten: sonst prüft der Test eine Zeile, von der erst ein Teil aus der Pipe da ist.
    let (rows, log) = common::wait_for_rows(&err, TIMEOUT, 10, |line| {
        line.matches('\u{2580}').count() > 1
    });
    assert!(
        rows.len() >= 10,
        "kein vollständiges POV-Bild ({} von 10 Zeilen). Ausgabe:\n{}",
        rows.len(),
        log.escape_debug()
    );

    assert!(log.contains("POV  x="), "Kopfzeile fehlt");
    assert!(
        log.contains("\u{1b}[38;2;") && log.contains("\u{1b}[48;2;"),
        "Vorder-/Hintergrundfarbe fehlen"
    );
    for (index, row) in rows.iter().take(10).enumerate() {
        assert_eq!(
            row.matches('\u{2580}').count(),
            40,
            "Zeile {} passt nicht zu --pov-size 40x20",
            index
        );
    }

    let _ = child.kill();
}

/// Ohne Farbe kommt die Helligkeitsrampe – und darin muss der Boden zu sehen sein.
#[cfg(feature = "pov")]
#[test]
fn pov_zeigt_den_boden() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child =
        common::spawn_client(server.port, "26.1", &["--no-color", "--pov-size", "40x20"]);
    let mut stdin = child.stdin.take().unwrap();
    let err = common::collect(child.stderr.take().unwrap());

    let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);
    let _ = writeln!(stdin, ":pov live");

    // Ohne Farbe ist jede Bildzeile eine Zeichenzeile: 20 Stück. Auf alle **vollständig
    // gelesenen** warten – sonst wäre die „unterste" Zeile in Wahrheit die Bildmitte.
    let (rows, log) = common::wait_for_rows(&err, TIMEOUT, 20, |line| {
        line.len() == 40 && line.chars().all(|c| " .:-=+*#%@".contains(c))
    });
    assert!(
        rows.len() >= 20,
        "kein vollständiges POV-Bild ({} von 20 Zeilen). Ausgabe:\n{}",
        rows.len(),
        log
    );
    assert!(!log.contains('\u{1b}'), "ohne Farbe darf kein ANSI kommen");

    // Die unterste Zeile zeigt den Boden – dort darf nicht nur Himmel stehen.
    let ground = &rows[19];
    assert!(
        ground.chars().any(|c| c != ' ' && c != '.'),
        "der Boden fehlt im Bild:\n{}",
        rows[..20].join("\n")
    );

    let _ = child.kill();
}

/// Der Chunk-Stapel muss bestätigt werden, sonst hört der Server nach dem ersten Stapel auf.
#[cfg(feature = "pov")]
#[test]
fn chunk_stapel_wird_bestaetigt() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "-q"]);
    let ok = common::wait_note(&server.notes, TIMEOUT, |note| {
        matches!(note, Note::Packet(id) if *id == common::MC_26_1.sb_chunk_batch_received)
    });
    assert!(ok, "ServerboundChunkBatchReceived kam nie an");
    let _ = child.kill();
}

/// Alle 25 Chunks müssen ankommen **und** lesbar sein – auf beiden Protokollformaten.
#[cfg(feature = "pov")]
#[test]
fn pov_liest_alle_chunks() {
    for ids in [&common::MC_26_1, &common::MC_1_21_1] {
        let server = common::start(ids, plan_with_ground());
        let mut child = common::spawn_client(server.port, ids.name, &["--no-color"]);
        let mut stdin = child.stdin.take().unwrap();
        let err = common::collect(child.stderr.take().unwrap());

        let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
        assert!(joined, "{}: kein Beitritt. Ausgabe:\n{}", ids.name, log);
        std::thread::sleep(Duration::from_millis(600));
        let _ = writeln!(stdin, ":pov info");

        let (found, log) = common::wait_for(&err, TIMEOUT, "25 Chunks");
        assert!(
            found,
            "{}: die 25 Chunks wurden nicht gelesen. Ausgabe:\n{}",
            ids.name, log
        );
        assert!(
            !log.contains("POV-Paket verworfen"),
            "{}: Chunks wurden verworfen. Ausgabe:\n{}",
            ids.name,
            log
        );
        // Ein solider Abschnitt je Chunk – reine Luft belegt keinen Speicher.
        assert!(
            log.contains("25 Abschnitte"),
            "{}: falsche Abschnittszahl. Ausgabe:\n{}",
            ids.name,
            log
        );
        let _ = child.kill();
    }
}

/// Regressionstest zum echten Absturz: auf normal erzeugten Welten kam eine Palette, deren
/// häufigster Zustand mehr Blöcke belegte, als der Abschnitt Luft hatte. Der Client ist daran
/// im Netz-Thread abgestürzt – mit `panic = "abort"` also der ganze Prozess, Sekunden nach dem
/// Beitritt und ohne dass jemand `:pov live` geschickt hätte.
#[cfg(feature = "pov")]
#[test]
fn gemischte_palette_stuerzt_nicht_ab() {
    let mut plan = plan_with_ground();
    plan.mixed_palette = true;
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    let mut stdin = child.stdin.take().unwrap();
    let err = common::collect(child.stderr.take().unwrap());

    let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);
    std::thread::sleep(Duration::from_secs(3));

    // Läuft der Prozess noch? (Beim Absturz war er längst weg.)
    assert!(
        child.try_wait().expect("Status").is_none(),
        "der Client ist abgestürzt"
    );
    let _ = writeln!(stdin, ":pov info");
    let (found, log) = common::wait_for(&err, TIMEOUT, "25 Chunks");
    assert!(found, "die Chunks fehlen. Ausgabe:\n{}", log);
    let _ = child.kill();
}

/// Weit entfernte Chunks kann die Ansicht nie sehen (sie reicht 72 Blöcke = 4,5 Chunks weit).
/// Sie müssen deshalb wieder wegfallen – sonst wächst der Speicher auf Servern, die nie ein
/// `ForgetChunk` schicken, unbegrenzt.
#[cfg(feature = "pov")]
#[test]
fn weit_entfernte_chunks_fallen_weg() {
    let mut plan = plan_with_ground();
    plan.chunks.clear();
    for x in -10..=10 {
        for z in -10..=10 {
            plan.chunks.push((x, z)); // 441 Chunks
        }
    }
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    let mut stdin = child.stdin.take().unwrap();
    let err = common::collect(child.stderr.take().unwrap());

    let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);
    std::thread::sleep(Duration::from_secs(1));
    let _ = writeln!(stdin, ":pov info");

    let (found, log) = common::wait_for(&err, TIMEOUT, "Chunks ·");
    assert!(found, "keine Auskunft. Ausgabe:\n{}", log);
    let line = log
        .lines()
        .find(|line| line.contains("Chunks ·"))
        .expect("Auskunftszeile");
    let chunks: usize = line
        .split_whitespace()
        .zip(line.split_whitespace().skip(1))
        .find(|(_, next)| *next == "Chunks")
        .and_then(|(value, _)| value.parse().ok())
        .expect("Chunkzahl");
    // 13x13 um den eigenen Chunk herum bleiben übrig, nicht alle 441.
    assert_eq!(chunks, 169, "Aufräumen greift nicht: {}", line);
    let _ = child.kill();
}

/// Die eigene POV-Datei startet die Ansicht ohne Zutun – und `--pov aus` schaltet genau das ab.
#[cfg(feature = "pov-client")]
#[test]
fn povdatei_startet_von_selbst_und_laesst_sich_abschalten() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    let err = common::collect(child.stderr.take().unwrap());
    let (found, log) = common::wait_for(&err, TIMEOUT, "(:pov stop)");
    assert!(found, "die Ansicht startete nicht von selbst:\n{}", log);
    drop(child);

    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "--pov", "aus"]);
    let err = common::collect(child.stderr.take().unwrap());
    let (joined, _) = common::wait_for(&err, TIMEOUT, "im Spiel");
    assert!(joined, "kein Beitritt");
    let (found, log) = common::wait_for(&err, Duration::from_secs(3), "(:pov stop)");
    assert!(!found, "--pov aus hat die Ansicht nicht verhindert:\n{}", log);
}

/// Ultra wartet umgekehrt auf `:pov live` – `--pov an` muss die Ansicht trotzdem von Anfang an
/// starten. Sonst hätte die Option in der Datei, die sie am ehesten braucht, keine Wirkung.
#[cfg(all(feature = "pov", not(feature = "pov-client")))]
#[test]
fn ultra_wartet_ab_startet_aber_auf_wunsch() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    let err = common::collect(child.stderr.take().unwrap());
    let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
    assert!(joined, "kein Beitritt. Ausgabe:
{}", log);
    let (found, log) = common::wait_for(&err, Duration::from_secs(3), "(:pov stop)");
    assert!(!found, "Ultra startete die Ansicht ungefragt:
{}", log);
    drop(child);

    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "--pov", "an"]);
    let err = common::collect(child.stderr.take().unwrap());
    let (found, log) = common::wait_for(&err, TIMEOUT, "(:pov stop)");
    assert!(found, "--pov an blieb bei Ultra wirkungslos:
{}", log);
}

/// Messlauf statt Behauptung: 441 Chunks mit je acht gefüllten Abschnitten (so sieht eine
/// gewachsene Überwelt aus) und danach der tatsächliche Speicherbedarf des Prozesses.
///
/// Läuft nicht im normalen Testlauf mit – die Zahl hängt vom Rechner ab. Aufruf:
/// `cargo test --features pov-client -- --ignored --nocapture speicher`
#[cfg(feature = "pov")]
#[test]
#[ignore]
fn speicherbedarf_der_live_ansicht() {
    let mut plan = plan_with_ground();
    plan.chunks.clear();
    for x in -10..=10 {
        for z in -10..=10 {
            plan.chunks.push((x, z));
        }
    }
    // Ohne gemischte Palette, damit sich derselbe Messlauf auch gegen eine ältere Fassung
    // fahren lässt (die stürzt an gemischten Paletten ab – genau darum geht es ja).
    plan.mixed_palette = std::env::var("AFK_TEST_BIN").is_err();
    plan.filled_sections = 8;
    plan.hold_secs = 30;

    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    let mut stdin = child.stdin.take().unwrap();
    let err = common::collect(child.stderr.take().unwrap());
    let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);
    std::thread::sleep(Duration::from_secs(3));
    let _ = writeln!(stdin, ":pov info");
    let (_, log) = common::wait_for(&err, TIMEOUT, "Chunks ·");
    let info = log
        .lines()
        .find(|line| line.contains("Chunks ·"))
        .unwrap_or("(keine Auskunft)");

    println!("\n{}", info.trim());
    println!("Arbeitsspeicher: {}\n", resident_memory(child.id()));
    let _ = child.kill();
}

/// Arbeitsspeicher eines Prozesses als lesbarer Text – nur für den Messlauf oben.
#[cfg(feature = "pov")]
fn resident_memory(pid: u32) -> String {
    #[cfg(windows)]
    {
        let out = std::process::Command::new("tasklist")
            .args(["/FI", &format!("PID eq {}", pid), "/NH", "/FO", "CSV"])
            .output();
        if let Ok(out) = out {
            let text = String::from_utf8_lossy(&out.stdout).into_owned();
            if let Some(field) = text.split('"').nth(9) {
                return field.to_string();
            }
        }
    }
    #[cfg(not(windows))]
    {
        if let Ok(status) = std::fs::read_to_string(format!("/proc/{}/status", pid)) {
            if let Some(line) = status.lines().find(|l| l.starts_with("VmRSS:")) {
                return line.trim_start_matches("VmRSS:").trim().to_string();
            }
        }
    }
    "unbekannt".to_string()
}

/// Die gemeldete Sichtweite entscheidet, wie viele Chunkdaten der Server überhaupt schickt.
/// Ohne Live-Ansicht liest der Client keinen einzigen Chunk – dann muss dort das Minimum stehen.
#[test]
fn sichtweite_passt_zur_bauform() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "-q"]);
    let expected: u8 = if cfg!(feature = "pov") { 6 } else { 2 };
    let ok = common::wait_note(&server.notes, TIMEOUT, |note| {
        matches!(note, Note::ViewDistance(v) if *v == expected)
    });
    assert!(ok, "Sichtweite {} kam nicht an", expected);
    drop(child.kill());

    // Und wer sie ausdrücklich vorgibt, bekommt genau die.
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(
        server.port,
        "26.1",
        &["--no-color", "-q", "--view-distance", "12"],
    );
    let ok = common::wait_note(&server.notes, TIMEOUT, |note| {
        matches!(note, Note::ViewDistance(12))
    });
    assert!(ok, "--view-distance 12 kam nicht an");
    let _ = child.kill();
}
