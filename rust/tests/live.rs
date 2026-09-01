//! Ablauftests gegen einen echten (winzigen) Minecraft-Server – siehe `tests/common/mod.rs`.
//!
//! Sie starten das gebaute Binary als eigenen Prozess. Damit wird genau das geprüft, was ein
//! Panel oder ein Mensch tatsächlich zu sehen bekommt: die Standardausgabe (nur Chat), die
//! Fehlerausgabe (Zustand, `@event`, POV-Bilder) und die Pakete, die beim Server ankommen.

mod common;

use common::{Note, Plan};
#[cfg(feature = "pov")]
use std::io::Read;
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
        before_login_ms: 0,
        before_position_ms: 0,
        chunks,
        // Abschnitt 4 deckt bei min_y = -64 die Höhen y = 0..15 ab; der Spieler steht darauf.
        solid_section: 4,
        mixed_palette: false,
        filled_sections: 1,
        position: (8.0, 16.0, 8.0),
        broken_chat: false,
        chat: vec!["Willkommen auf dem Testserver".to_string()],
        player_chat: Vec::new(),
        player_chat_filter: 0,
        close_after_chat: false,
        chat_flood: 0,
        kill: false,
        scoreboard: false,
        menu: false,
        store_cookie: None,
        request_cookie: None,
        transfer_to: None,
        hold_secs: 20,
        compression: None,
        encrypt: false,
        respawn_after_death: false,
        connections: 1,
        // Wie ein echter Server: voller Himmel über dem Boden.
        sky_light: Some(15),
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

    let got_command = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Command(c) if c == "afk"),
    );
    assert!(got_command, "--cmd hat den Befehl nicht geschickt");

    let _ = child.kill();
}

/// Die Client-Brand wird über Vanillas `minecraft:brand`-Payload unmittelbar beim
/// Spielbeitritt gesendet und enthält immer die tatsächlich gewählte Minecraft-Version.
#[test]
fn client_brand_ist_versionsgenau_und_vollstaendig() {
    for ids in [&common::MC_1_21_1, &common::MC_26_1, &common::MC_26_2] {
        let server = common::start(ids, plan_with_ground());
        let mut child = common::spawn_client(server.port, ids.name, &["--no-color"]);
        let expected = format!("example.invalid {}", ids.name);
        let branded = common::wait_note(
            &server.notes,
            TIMEOUT,
            |note| matches!(note, Note::Brand(brand) if brand == &expected),
        );
        assert!(branded, "{}: Client-Brand fehlte oder war falsch", ids.name);
        let _ = child.kill();
    }
}

/// Ein sofort fälliger `--cmd`-Eintrag wartet auf den ersten autoritativen Teleport. Dadurch
/// landet kein Spielerkommando zwischen Spiel-Login und dem vollständigen Weltzustand.
#[test]
fn serverbefehl_wartet_auf_startposition() {
    let mut plan = plan_with_ground();
    plan.before_position_ms = 400;
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(
        server.port,
        "26.1",
        &["--no-color", "-c", "/erst-nach-position"],
    );

    let joined = common::wait_note(&server.notes, TIMEOUT, |note| matches!(note, Note::Joined));
    assert!(joined, "Spiel-Login kam nicht an");

    let premature = common::wait_note(
        &server.notes,
        Duration::from_millis(200),
        |note| matches!(note, Note::Command(command) if command == "erst-nach-position"),
    );
    assert!(!premature, "Befehl wurde vor der Startposition gesendet");

    let sent = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Command(command) if command == "erst-nach-position"),
    );
    assert!(sent, "Befehl wurde nach der Startposition nicht gesendet");
    let _ = child.kill();
}

/// Moderne Vanilla-Clients markieren das Ende jedes 50-ms-Ticks mit einem leeren Paket. Ohne
/// diese Grenze verarbeitet der Server Eingabeereignisse anders als bei einem normalen Client.
#[test]
fn moderne_client_ticks_werden_abgeschlossen() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);

    let tick = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Packet(id, 0) if *id == common::MC_26_1.sb_client_tick_end),
    );
    assert!(tick, "kein ClientTickEnd-Paket empfangen");
    let _ = child.kill();
}

/// Lokale Aktionen dürfen während Login/Konfiguration keine Spielpakete senden. Die künstliche
/// Pause hält den Server sicher vor der Spielphase, während die Eingabe bereits verarbeitet wird.
#[test]
#[cfg(feature = "state")]
fn spielaktion_vor_dem_join_wird_nicht_gesendet() {
    let mut plan = plan_with_ground();
    plan.before_login_ms = 500;
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    writeln!(child.stdin.as_mut().unwrap(), ":use").unwrap();

    let err = common::collect(child.stderr.take().unwrap());
    let (rejected, output) = common::wait_for(
        &err,
        TIMEOUT,
        "Diese Aktion ist erst nach dem Beitritt und der Startposition möglich",
    );
    assert!(rejected, "Aktion wurde nicht lokal abgelehnt:\n{}", output);

    let sent = common::wait_note(
        &server.notes,
        Duration::from_secs(1),
        |note| matches!(note, Note::Packet(id, _) if *id == common::MC_26_1.sb_use_item),
    );
    assert!(!sent, "UseItem wurde trotz fehlender Spielphase gesendet");
    let _ = child.kill();
}

/// `--sneak` wird erst nach Bestätigung der autoritativen Startposition gesendet. Damit steht
/// kein Spieler-Eingabepaket zwischen Teleport und dessen verpflichtender Antwort.
#[test]
#[cfg(feature = "state")]
fn schleichen_startet_erst_nach_der_position() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "--sneak"]);

    let positioned = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Packet(id, _) if *id == common::MC_26_1.sb_move),
    );
    assert!(positioned, "Startposition wurde nicht zurückgespiegelt");
    let sneaking = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::PlayerInput(bits) if *bits == 0x20),
    );
    assert!(
        sneaking,
        "Schleichzustand kam nicht nach der Startposition an"
    );
    let _ = child.kill();
}

/// Reines Drehen verwendet Vanillas kurzes Rotationspaket. Ein PosRot-Paket würde unnötig alte
/// Koordinaten wiederholen und könnte mit einer gleichzeitig laufenden Bewegung kollidieren.
#[test]
#[cfg(feature = "movement")]
fn blickbewegung_sendet_keine_position() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);

    let positioned = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Packet(id, _) if *id == common::MC_26_1.sb_accept_teleportation),
    );
    assert!(positioned, "Startposition wurde nicht bestätigt");
    writeln!(child.stdin.as_mut().unwrap(), ":look rechts 7").unwrap();

    let rotated = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Packet(id, 9) if *id == common::MC_26_1.sb_move_rot),
    );
    assert!(rotated, "kein korrektes Rotationspaket empfangen");
    let _ = child.kill();
}

/// Ändert sich nur die Position, verwendet Vanilla `MovePlayerPos` (3 Double + Flag) und nicht
/// in jedem Tick das längere Paket mit unveränderten Blickwinkeln.
#[test]
#[cfg(feature = "movement")]
fn laufbewegung_sendet_nur_die_position() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);

    let positioned = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Packet(id, _) if *id == common::MC_26_1.sb_accept_teleportation),
    );
    assert!(positioned, "Startposition wurde nicht bestätigt");
    writeln!(child.stdin.as_mut().unwrap(), ":go vor 0.1").unwrap();

    // Auf die Koordinaten prüfen, nicht nur auf die Paketlänge: Der Erinnerungstakt eines
    // stillstehenden Clients schickt dasselbe Paket, nur mit unveränderter Position. Der Start
    // liegt bei z = 8.0, gelaufen wird nach vorn (Blick nach Süden, also +z).
    let moved = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::MovePos(_, _, z) if *z > 8.0),
    );
    assert!(moved, "kein korrektes Positionspaket empfangen");
    let _ = child.kill();
}

/// Auch wer stillsteht, meldet dem Server alle 20 Ticks erneut seine Position.
///
/// `LocalPlayer.sendPosition()` sendet spätestens jeden 20. Tick, selbst wenn sich nichts geändert
/// hat. Ein Client, der nach dem Beitritt überhaupt kein Positionspaket mehr schickt, sieht auf
/// dem Server anders aus als jeder echte Spieler.
#[test]
fn stillstehender_client_meldet_seine_position_weiter() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "-q"]);

    // Der erste Teleport wird mit einem PosRot beantwortet; erst danach greift der Takt. Deshalb
    // zweimal warten: das erste Pos-Paket kann noch nichts beweisen, das zweite schon.
    let (x, z) = (8.0, 8.0);
    for round in 1..=2 {
        let ticked = common::wait_note(&server.notes, TIMEOUT, |note| {
            matches!(note, Note::MovePos(px, _, pz)
                if (*px - x).abs() < 1e-9 && (*pz - z).abs() < 1e-9)
        });
        assert!(
            ticked,
            "Positionserinnerung {} kam nicht (Takt steht still)",
            round
        );
    }
    let _ = child.kill();
}

/// Ein Respawn setzt die Ladephase zurück – aber der Client darf daran nicht hängenbleiben.
///
/// Ein Vanilla-Server schickt nach dem Respawn Position und Chunks hinterher; ein Plugin muss das
/// nicht. Käme der Client nur mit dem Vanilla-Ablauf zurecht, wäre er nach dem ersten Tod
/// dauerhaft stumm: kein Chat, kein `--cmd`, keine Regel. Geprüft wird deshalb der karge Fall –
/// nacktes Respawn-Paket, sonst nichts – und dass danach trotzdem alles weiterläuft.
#[test]
fn nackter_respawn_laesst_den_client_nicht_haengen() {
    let mut plan = plan_with_ground();
    plan.kill = true;
    plan.respawn_after_death = true;
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "--events"]);
    let mut stdin = child.stdin.take().unwrap();
    let err = common::collect(child.stderr.take().unwrap());

    let (died, log) = common::wait_for(&err, TIMEOUT, "@event death");
    assert!(died, "kein Tod gemeldet. Ausgabe:\n{}", log);

    // Nach dem Respawn muss die Ladephase erneut abgeschlossen werden – notfalls über die
    // Notbremse, denn Chunks kommen hier keine mehr.
    let loaded = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Packet(id, 0) if *id == common::MC_26_1.sb_player_loaded),
    );
    assert!(loaded, "nach dem Respawn kam kein zweites PlayerLoaded");

    // Und der Client ist wieder ansprechbar.
    let _ = writeln!(stdin, "wieder da");
    let back = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Chat(text) if text == "wieder da"),
    );
    assert!(back, "der Client blieb nach dem Respawn stumm");
    let _ = child.kill();
}

/// Ab 1.21.4 meldet der Client das Ende seiner Ladephase; vorher gibt es das Paket nicht.
///
/// Ohne diese Meldung hält der Server den Spieler bis zu 30 Sekunden in einem Schwebezustand –
/// und der Client selbst käme nie in die Spielphase.
#[test]
fn welt_geladen_wird_gemeldet_sobald_es_das_paket_gibt() {
    for ids in [&common::MC_26_1, &common::MC_1_21_1] {
        let server = common::start(ids, plan_with_ground());
        let mut child = common::spawn_client(server.port, ids.name, &["--no-color", "-q"]);
        let wanted = ids.sb_player_loaded;
        let seen = common::wait_note(
            &server.notes,
            Duration::from_secs(10),
            |note| matches!(note, Note::Packet(id, 0) if wanted >= 0 && *id == wanted),
        );
        assert_eq!(
            seen,
            ids.modern,
            "{}: PlayerLoaded {}",
            ids.name,
            if ids.modern {
                "kam nicht an"
            } else {
                "gibt es hier noch gar nicht"
            }
        );
        let _ = child.kill();
    }
}

/// Derselbe Ablauf mit dem Verschlüsselungs-Handshake eines Online-Mode-Servers.
///
/// Jeder Server mit Kontoprüfung verlangt es, und danach läuft **jedes** Byte in beiden
/// Richtungen durch AES-128-CFB8 – auch die Rahmenlänge vor jedem Paket, die deshalb byteweise
/// entschlüsselt werden muss. Bisher lief im Ablauftest ausschließlich der unverschlüsselte
/// Zweig; geprüft war die Chiffre nur gegen sich selbst, nicht über einen echten Socket.
///
/// Mit Kompression zusammen, weil ein echter Server beides gleichzeitig macht: erst
/// verschlüsseln, dann komprimieren.
#[test]
fn beitritt_und_chat_mit_verschluesselung() {
    for compression in [None, Some(256)] {
        let mut plan = plan_with_ground();
        plan.encrypt = true;
        plan.compression = compression;
        plan.chat = vec!["Willkommen auf dem Testserver".to_string()];
        plan.chat.push("V".repeat(600));
        let server = common::start(&common::MC_26_1, plan);
        let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
        let mut stdin = child.stdin.take().unwrap();
        let out = common::collect(child.stdout.take().unwrap());

        // Ein Warten auf die zuletzt geschickte Zeile – siehe `beitritt_und_chat_mit_kompression`.
        let (lang, log) = common::wait_for(&out, TIMEOUT, &"V".repeat(600));
        assert!(
            lang,
            "{:?}: die lange Zeile fehlt. Ausgabe:\n{}",
            compression, log
        );
        assert!(
            log.contains("Willkommen auf dem Testserver"),
            "{:?}: die kurze Zeile fehlt. Ausgabe:\n{}",
            compression,
            log
        );

        // Und in die Gegenrichtung – dort verschlüsselt der Client selbst.
        let _ = writeln!(stdin, "/{}", "y".repeat(600));
        let _ = stdin.flush();
        let angekommen = common::wait_note(
            &server.notes,
            TIMEOUT,
            |note| matches!(note, Note::Command(text) if text.len() == 600),
        );
        assert!(angekommen, "{:?}: der Befehl kam nicht an", compression);
        let _ = child.kill();
    }
}

/// Derselbe Ablauf mit eingeschalteter Paket-Kompression.
///
/// Fast jeder echte Server schaltet sie ein (Vanilla ab 256 Byte), und ab dann sieht **jedes**
/// Paket in beiden Richtungen anders aus: erst die entpackte Länge als VarInt, dann ein
/// zlib-Strom – oder eine 0 und die Nutzdaten roh, wenn das Paket unter der Schwelle bleibt.
/// Bisher lief im Ablauftest ausschließlich der unkomprimierte Zweig, also genau der, den
/// draußen kaum jemand benutzt.
#[test]
fn beitritt_und_chat_mit_kompression() {
    for threshold in [0, 8, 256] {
        let mut plan = plan_with_ground();
        plan.compression = Some(threshold);
        plan.chat = vec!["Willkommen auf dem Testserver".to_string()];
        // Eine Zeile deutlich über jeder Schwelle, damit der zlib-Zweig sicher drankommt.
        plan.chat.push("L".repeat(600));
        let server = common::start(&common::MC_26_1, plan);
        let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
        let mut stdin = child.stdin.take().unwrap();
        let out = common::collect(child.stdout.take().unwrap());

        // **Ein** Warten auf die zuletzt geschickte Zeile: `wait_for` sammelt alles davor mit und
        // gibt es zurück. Zwei Aufrufe nacheinander gingen schief – der erste nimmt beide Zeilen
        // mit und der zweite fängt mit leerem Puffer an.
        let (lang, log) = common::wait_for(&out, TIMEOUT, &"L".repeat(600));
        assert!(
            lang,
            "Schwelle {}: die lange Zeile fehlt. Ausgabe:\n{}",
            threshold, log
        );
        assert!(
            log.contains("Willkommen auf dem Testserver"),
            "Schwelle {}: die kurze Zeile fehlt. Ausgabe:\n{}",
            threshold,
            log
        );

        // Und in die Gegenrichtung: ein langer Befehl muss beim Server heil ankommen.
        let befehl = format!("/{}", "x".repeat(600));
        let _ = writeln!(stdin, "{}", befehl);
        let _ = stdin.flush();
        let angekommen = common::wait_note(
            &server.notes,
            TIMEOUT,
            |note| matches!(note, Note::Command(text) if text.len() == 600),
        );
        assert!(
            angekommen,
            "Schwelle {}: der lange Befehl kam nicht an",
            threshold
        );
        let _ = child.kill();
    }
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

/// Eine unlesbare Chat-Komponente darf die Zeile kosten, nicht die Verbindung.
///
/// Pakete sind einzeln gerahmt – das nächste beginnt ohnehin an einer bekannten Stelle. Vorher
/// wurde ein Lesefehler nach oben gereicht und beendete die ganze Verbindung: Eine einzige
/// seltsame Zeile eines Plugins meldete den Client ab.
#[test]
fn kaputte_chatzeile_kostet_nicht_die_verbindung() {
    let mut plan = plan_with_ground();
    plan.broken_chat = true;
    plan.chat = vec!["Danach geht es weiter".to_string()];
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    let out = common::collect(child.stdout.take().unwrap());

    let (found, log) = common::wait_for(&out, TIMEOUT, "Danach geht es weiter");
    assert!(
        found,
        "die kaputte Zeile hat die Verbindung mitgenommen. Ausgabe:\n{}",
        log
    );
    assert!(
        child.try_wait().expect("Status").is_none(),
        "der Client hat sich beendet"
    );
    let _ = child.kill();
}

/// Emoji und Sonderzeichen im Chat müssen heil ankommen.
///
/// Minecraft schickt Chat als NBT, und NBT-Zeichenketten stehen in Javas modifiziertem UTF-8:
/// Zeichen über U+FFFF – also jedes Emoji – als **zwei** Drei-Byte-Folgen. Der Client las das als
/// gewöhnliches UTF-8 und machte daraus zwei Ersatzzeichen; auf einem Server, der Emoji im Chat
/// benutzt, war damit fast jede zweite Zeile verstümmelt.
#[test]
fn emoji_im_chat_kommen_heil_an() {
    const LINE: &str = "Hallo \u{1F389} Welt \u{1F60A} – Gruesse \u{20AC}";
    let mut plan = plan_with_ground();
    plan.chat = vec![LINE.to_string()];
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    let out = common::collect(child.stdout.take().unwrap());

    let (found, seen) = common::wait_for(&out, TIMEOUT, LINE);
    assert!(
        found,
        "Emoji kamen nicht heil an. Ausgabe:\n{}",
        seen.escape_debug()
    );
    assert!(
        !seen.contains('\u{FFFD}'),
        "Ersatzzeichen in der Ausgabe:\n{}",
        seen.escape_debug()
    );
    let _ = child.kill();
}

/// Echter Spielerchat kommt über ein ganz anderes Paket als Systemmeldungen – mit
/// Quittungsliste, Signaturfeld und Filterangabe. Meldet der Server „teilweise gefiltert",
/// folgt hinter der Angabe noch ein Bitfeld; der Client hat es nicht gelesen, stand danach
/// mitten im Paket und ließ die Zeile wortlos fallen. Auf einem Server mit eingeschaltetem
/// Chatfilter war damit **kein** Spielerchat mehr zu sehen.
#[test]
fn spieler_chat_kommt_an_auch_mit_filter() {
    for ids in [&common::MC_26_1, &common::MC_1_21_1] {
        for filter in [0, 2] {
            let mut plan = plan_with_ground();
            plan.player_chat = vec![format!("Filter {} sagt hallo", filter)];
            plan.player_chat_filter = filter;
            let server = common::start(ids, plan);
            let mut child = common::spawn_client(server.port, ids.name, &["--no-color"]);
            let out = common::collect(child.stdout.take().unwrap());
            let (found, log) = common::wait_for(
                &out,
                TIMEOUT,
                &format!("<Hugo> Filter {} sagt hallo", filter),
            );
            assert!(
                found,
                "Spielerchat fehlt ({}, Filter {}). Ausgabe:\n{}",
                ids.name, filter, log
            );
            let _ = child.kill();
        }
    }
}

/// Legt der Server auf, muss der Client die schon gelesenen Zeilen noch hinausschreiben, bevor
/// er sich beendet. Der Chat geht über einen eigenen Thread, und `exit` wartet auf keinen –
/// ausgerechnet die letzten Zeilen vor einem Kick sind aber die mit dem Grund.
#[test]
fn letzte_chatzeilen_gehen_vor_dem_beenden_noch_raus() {
    let mut plan = plan_with_ground();
    plan.chat = (1..=40).map(|n| format!("Zeile {}", n)).collect();
    plan.close_after_chat = true;

    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    let out = common::collect(child.stdout.take().unwrap());
    let (found, log) = common::wait_for(&out, TIMEOUT, "Zeile 40");
    assert!(found, "die letzte Chatzeile fehlt. Ausgabe:\n{}", log);
    let _ = child.kill();
}

/// Das Licht steht am Ende jedes Chunk-Pakets, hinter den Blockentitäten. Es zu lesen heißt
/// also, den Rest des Pakets **exakt** zu überspringen – ein Byte daneben, und es kommt Unsinn
/// heraus statt gar nichts.
///
/// Geprüft wird auf beiden Chunk-Formaten, die dieser Testserver nachbauen kann: dem alten
/// (1.21.1, ohne Fluidzähler) und dem modernen (26.1 und 26.2). Genau daran fällt ein falscher
/// Lesezeiger auf, denn Höhenkarten und Palettenformat davor sind unterschiedlich lang.
///
/// **Nicht** abgedeckt ist 1.21.11: Der Testserver hat für dieses Protokoll keine ID-Tabelle, und
/// eine mit geratenen Nummern wäre schlimmer als keine – sie sähe aus wie Abdeckung. Dass die
/// Paket-ID dort stimmt, sichert stattdessen der Einzeltest `licht_id_liegt_vor_dem_login`, der
/// die Beziehung zu den Nachbarpaketen in **allen vier** Tabellen festhält.
#[cfg(feature = "pov")]
#[test]
fn licht_wird_aus_dem_chunk_paket_gelesen() {
    for ids in [&common::MC_26_1, &common::MC_1_21_1, &common::MC_26_2] {
        let server = common::start(ids, plan_with_ground());
        let mut child = common::spawn_client(server.port, ids.name, &["--no-color"]);
        let mut stdin = child.stdin.take().unwrap();
        let err = common::collect(child.stderr.take().unwrap());

        let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
        assert!(joined, "{}: kein Beitritt. Ausgabe:\n{}", ids.name, log);

        let (found, log) =
            common::poll_command(&mut stdin, &err, TIMEOUT, ":pov info", "25 mit Licht");
        assert!(
            found,
            "{}: nicht alle 25 Chunks brachten Licht mit. Ausgabe:\n{}",
            ids.name, log
        );
    }
}

/// Schickt ein Server kein Licht, darf die Ansicht daran nicht zerbrechen: Sie rechnet dann wie
/// früher mit geschätzter Flächenhelligkeit weiter. Erfunden wird kein Licht.
#[cfg(feature = "pov")]
#[test]
fn ohne_licht_bleibt_die_ansicht_benutzbar() {
    let mut plan = plan_with_ground();
    plan.sky_light = None;
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    let mut stdin = child.stdin.take().unwrap();
    let err = common::collect(child.stderr.take().unwrap());

    let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);
    let (found, log) = common::poll_command(&mut stdin, &err, TIMEOUT, ":pov info", "25 Chunks");
    assert!(found, "die Chunks fehlen. Ausgabe:\n{}", log);
    assert!(
        log.contains("0 mit Licht"),
        "ohne Lichtdaten darf keins gemeldet werden. Ausgabe:\n{}",
        log
    );
}

/// Mit `--reconnect` muss ein Verbindungsabbruch ein zweiter **vollständiger** Beitritt werden –
/// nicht nur ein neuer Socket.
///
/// Geprüft wird deshalb am Server (`Note::Joined` zweimal auf demselben Port) und nicht an einer
/// Meldung des Clients: Dass er es *vorhat*, sagt die Meldung; dass er wirklich wieder im Spiel
/// ankommt, sagt nur der Server. Zusätzlich muss `--cmd` in der zweiten Sitzung erneut laufen –
/// genau dafür ist der Reconnect da, und die Befehlsplanung hängt an der Verbindungsgeneration.
#[test]
fn nach_einem_abbruch_wird_neu_verbunden() {
    let mut plan = plan_with_ground();
    // Zwei Sekunden bedienen, dann von Serverseite auflegen – der Client muss von selbst
    // wiederkommen. So lange, dass der `--cmd`-Befehl der ersten Sitzung noch ankommt.
    plan.hold_secs = 2;
    plan.connections = 2;

    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(
        server.port,
        "26.1",
        &[
            "--no-color",
            "--events",
            "--reconnect",
            // Ohne die kurze Grundzeit wartete der Test fünf Sekunden.
            "--reconnect-delay",
            "1",
            "-c",
            "/afk",
            "--join-delay",
            "0",
        ],
    );
    let err = common::collect(child.stderr.take().unwrap());

    let mut joins = 0;
    let mut commands = 0;
    let deadline = std::time::Instant::now() + TIMEOUT;
    while std::time::Instant::now() < deadline && (joins < 2 || commands < 2) {
        match server.notes.recv_timeout(Duration::from_millis(250)) {
            Ok(Note::Joined) => joins += 1,
            Ok(Note::Command(command)) if command == "afk" => commands += 1,
            Ok(_) => {}
            Err(_) => {}
        }
    }
    let (_, log) = common::wait_for(&err, Duration::from_millis(200), "@event reconnect");
    assert_eq!(
        joins, 2,
        "der Client ist nach dem Abbruch nicht wieder beigetreten. Ausgabe:\n{}",
        log
    );
    assert_eq!(
        commands, 2,
        "die --cmd-Befehle liefen in der zweiten Sitzung nicht erneut. Ausgabe:\n{}",
        log
    );
    assert!(
        log.contains("@event reconnect"),
        "ein Panel bekommt den Reconnect nicht zu sehen. Ausgabe:\n{}",
        log
    );
    let _ = child.kill();
}

/// `--reconnect-tries` muss wirklich aufgeben – sonst wäre es ein Schalter, der nur so aussieht.
///
/// Der Ablauf: Der Server bedient genau eine Verbindung und legt dann auf. Der erste Fehlversuch
/// zählt als Versuch 1 und ist damit noch erlaubt; er trifft auf einen Port, an dem niemand mehr
/// horcht, und scheitert sofort. Versuch 2 überschreitet die Grenze, und der Prozess endet mit
/// Status 1 – genau wie ohne Reconnect. Ein endloser Client bliebe hier bis zum Zeitlimit hängen.
#[test]
fn nach_der_versuchsgrenze_gibt_der_client_auf() {
    let mut plan = plan_with_ground();
    plan.close_after_chat = true;

    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(
        server.port,
        "26.1",
        &[
            "--no-color",
            "--events",
            "--reconnect-tries",
            "1",
            "--reconnect-delay",
            "1",
        ],
    );
    let status = child
        .wait_within(TIMEOUT)
        .expect("nach der Versuchsgrenze muss der Client sich beenden");
    assert_eq!(
        status.code(),
        Some(1),
        "das Aufgeben muss denselben Status ergeben wie --no-reconnect"
    );
}

/// Mit `--no-reconnect` bleibt es beim bisherigen, zugesagten Verhalten: Der Prozess endet nach
/// einem Abbruch mit Status 1, damit ein Panel das überhaupt bemerkt. Wer eine Aufsicht davor
/// gesetzt hat, die den Client neu startet, verlässt sich genau darauf.
#[test]
fn mit_no_reconnect_endet_der_prozess_nach_einem_abbruch() {
    let mut plan = plan_with_ground();
    plan.close_after_chat = true;

    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(
        server.port,
        "26.1",
        &["--no-color", "--events", "--no-reconnect"],
    );
    let status = child
        .wait_within(TIMEOUT)
        .expect("der Client muss sich nach einem Abbruch beenden");
    assert_eq!(
        status.code(),
        Some(1),
        "ein Abbruch mit --no-reconnect muss Status 1 ergeben"
    );
}

/// Stirbt der Spieler, muss der Client von selbst wieder einsteigen – sonst steht er bis in alle
/// Ewigkeit auf dem Todesbildschirm, ohne dass jemand etwas davon merkt. Zugleich die Probe auf
/// `--on death=`: die Regel muss genau dann feuern.
#[test]
fn tod_loest_respawn_und_regel_aus() {
    let mut plan = plan_with_ground();
    plan.kill = true;
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(
        server.port,
        "26.1",
        &["--no-color", "--events", "--on", "death=/spawn"],
    );
    let err = common::collect(child.stderr.take().unwrap());

    let respawned = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Packet(id, _) if *id == common::MC_26_1.sb_client_command),
    );
    assert!(respawned, "der Client hat nicht von selbst respawnt");

    let fired = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Command(c) if c == "spawn"),
    );
    let (_, log) = common::wait_for(&err, Duration::from_millis(300), "@event death");
    assert!(fired, "--on death hat nicht ausgeloest. Ausgabe:\n{}", log);
    let _ = child.kill();
}

/// Ein vom Server angeordneter Transfer ist kein Kick: der Client muss die neue Adresse
/// übernehmen und dort neu beitreten. Genau das ist der Unterschied zum Verbindungsabbruch, nach
/// dem sich der Rust-Client bewusst beendet.
#[test]
fn server_transfer_wird_befolgt() {
    // Ziel zuerst starten, damit seine Adresse feststeht.
    let ziel = common::start(&common::MC_26_1, plan_with_ground());
    let mut plan = plan_with_ground();
    plan.chat.clear();
    plan.transfer_to = Some(("127.0.0.1".to_string(), ziel.port));
    let start = common::start(&common::MC_26_1, plan);

    let mut child = common::spawn_client(start.port, "26.1", &["--no-color", "--events"]);
    let err = common::collect(child.stderr.take().unwrap());

    let angekommen = common::wait_note(&ziel.notes, TIMEOUT, |note| matches!(note, Note::Joined));
    let (_, log) = common::wait_for(&err, Duration::from_millis(300), "Server-Transfer");
    assert!(
        angekommen,
        "der Client ist dem Transfer nicht gefolgt. Ausgabe:\n{}",
        log
    );
    let _ = child.kill();
}

/// Die Spielverbindung muss auch über einen Proxy zustande kommen.
///
/// SOCKS5 mit und ohne Anmeldung sowie HTTP-CONNECT sind von Hand umgesetzt – ein paar Dutzend
/// Zeilen, die genau einmal je Verbindungsaufbau laufen und deshalb nie jemandem auffallen, wenn
/// sie falsch sind. Geprüft wird gegen zwei winzige echte Proxys, nicht gegen Attrappen.
#[test]
fn verbindung_ueber_proxy() {
    for (name, login) in [
        ("socks5 ohne Anmeldung", None),
        (
            "socks5 mit Anmeldung",
            Some(("hugo".to_string(), "geheim".to_string())),
        ),
        ("http-connect", None),
    ] {
        let server = common::start(&common::MC_26_1, plan_with_ground());
        let (proxy_port, url) = if name == "http-connect" {
            let port = common::start_http_proxy(server.port);
            (port, format!("http://127.0.0.1:{}", port))
        } else {
            let port = common::start_socks5(server.port, login.clone());
            match &login {
                Some((user, pass)) => (
                    port,
                    format!("socks5://{}:{}@127.0.0.1:{}", user, pass, port),
                ),
                None => (port, format!("socks5://127.0.0.1:{}", port)),
            }
        };
        assert_ne!(proxy_port, server.port);

        // Die Zieladresse löst der Proxy auf – deshalb ein Name statt 127.0.0.1.
        let mut child = common::spawn_client_at(
            "localhost",
            server.port,
            "26.1",
            &["--no-color", "--proxy", &url],
        );
        let out = common::collect(child.stdout.take().unwrap());
        let (found, log) = common::wait_for(&out, TIMEOUT, "Willkommen auf dem Testserver");
        assert!(
            found,
            "{}: kein Beitritt über den Proxy. Ausgabe:\n{}",
            name, log
        );
        let _ = child.kill();
    }
}

/// Cookies müssen den Transfer überleben – genau dafür gibt es sie.
///
/// Server A legt eines ab und schickt einen Transfer, Server B fragt es beim Login ab: So laufen
/// Anmeldung und Warteschlange auf großen Netzwerken. Der Client hat seine Ablage aber bei
/// **jedem** Verbindungsaufbau geleert und antwortete deshalb immer „habe ich nicht" – womit
/// Server B den Spieler zurückschickte oder gleich hinauswarf.
#[test]
fn cookies_ueberleben_den_transfer() {
    let mut ziel_plan = plan_with_ground();
    ziel_plan.request_cookie = Some("afk:test".to_string());
    let ziel = common::start(&common::MC_26_1, ziel_plan);

    let mut plan = plan_with_ground();
    plan.chat.clear();
    plan.store_cookie = Some(("afk:test".to_string(), b"geheim".to_vec()));
    plan.transfer_to = Some(("127.0.0.1".to_string(), ziel.port));
    let start = common::start(&common::MC_26_1, plan);

    let mut child = common::spawn_client(start.port, "26.1", &["--no-color"]);
    let angekommen = common::wait_note(
        &ziel.notes,
        TIMEOUT,
        |note| matches!(note, Note::Cookie(key, Some(value)) if key == "afk:test" && value == b"geheim"),
    );
    assert!(angekommen, "das Cookie kam beim Transferziel nicht an");
    let _ = child.kill();
}

/// Die Seitenleiste muss auf allen drei Feldreihenfolgen des Team-Pakets herauskommen.
///
/// Auf fast jedem Server ist der Eintrag selbst ein unsichtbarer Platzhalter; der sichtbare Text
/// steckt in Präfix und Suffix des Teams. Wird davon auch nur ein Feld falsch gelesen, steht in
/// der Anzeige Müll – und zwar genau auf einer der vier Versionen.
#[cfg(feature = "board")]
#[test]
fn seitenleiste_auf_allen_feldreihenfolgen() {
    for ids in [&common::MC_1_21_1, &common::MC_26_1, &common::MC_26_2] {
        let mut plan = plan_with_ground();
        plan.scoreboard = true;
        let server = common::start(ids, plan);
        let mut child = common::spawn_client(server.port, ids.name, &["--no-color"]);
        let mut stdin = child.stdin.take().unwrap();
        let err = common::collect(child.stderr.take().unwrap());

        let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
        assert!(joined, "{}: kein Beitritt. Ausgabe:\n{}", ids.name, log);

        let (found, log) =
            common::poll_command(&mut stdin, &err, TIMEOUT, ":board", "Rang: hugo *");
        assert!(
            found,
            "{}: die Seitenleiste kam nicht richtig heraus. Ausgabe:\n{}",
            ids.name, log
        );
        assert!(
            log.contains("Testserver"),
            "{}: die Überschrift fehlt. Ausgabe:\n{}",
            ids.name,
            log
        );
        let _ = child.kill();
    }
}

/// Menüinhalte müssen auf beiden Komponenten-Tabellen herauskommen.
///
/// Ein Gegenstand besteht seit 1.20.5 aus einer Liste von Komponenten **ohne Längenangabe**: Wer
/// eine davon nicht kennt, findet auch die nächste nicht mehr. Die Tabelle dazu unterscheidet
/// sich zwischen 1.21.1 und den neueren Versionen erheblich – ein Fehler darin fällt nur genau
/// hier auf.
#[cfg(feature = "items")]
#[test]
fn menue_inhalt_auf_beiden_komponententabellen() {
    for ids in [&common::MC_1_21_1, &common::MC_26_1] {
        let mut plan = plan_with_ground();
        plan.menu = true;
        let server = common::start(ids, plan);
        let mut child = common::spawn_client(server.port, ids.name, &["--no-color"]);
        let mut stdin = child.stdin.take().unwrap();
        let err = common::collect(child.stderr.take().unwrap());

        let (opened, log) = common::wait_for(&err, TIMEOUT, "Menü geöffnet: Warp-Menü");
        assert!(
            opened,
            "{}: kein Menü gemeldet. Ausgabe:\n{}",
            ids.name, log
        );

        let (found, log) = common::poll_command(&mut stdin, &err, TIMEOUT, ":menu", "Zum Spawn");
        assert!(
            found,
            "{}: der Gegenstandsname fehlt. Ausgabe:\n{}",
            ids.name, log
        );
        assert!(
            !log.contains("Ab Feld"),
            "{}: der Inhalt war nicht vollständig lesbar. Ausgabe:\n{}",
            ids.name,
            log
        );

        let (lore, log) =
            common::poll_command(&mut stdin, &err, TIMEOUT, ":slot 4", "kostet nichts");
        assert!(lore, "{}: die Lore fehlt. Ausgabe:\n{}", ids.name, log);
        let _ = child.kill();
    }
}

/// Liest niemand die Standardausgabe mit, darf der Client trotzdem nicht stehen bleiben.
///
/// Der Netz-Thread liest den Chat aus dem Paket **und** beantwortet KeepAlive. Schrieb er die
/// Zeile selbst und war die Pipe voll (Panel gerade beschäftigt), blockierte er darin – und flog
/// mit `disconnect.timeout` heraus, obwohl die Verbindung völlig in Ordnung war. Der Test füllt
/// genau diese Pipe: Die Standardausgabe wird bewusst **nicht** gelesen.
#[test]
fn voller_ausgabepuffer_blockiert_den_netz_thread_nicht() {
    let mut plan = plan_with_ground();
    // Reichlich mehr, als in eine Pipe passt (die fasst je nach System 4 bis 64 KB).
    plan.chat_flood = 4000;
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "--events"]);
    // stdout bleibt absichtlich ungelesen – genau das ist der Fall, um den es geht.
    let err = common::collect(child.stderr.take().unwrap());

    let (joined, log) = common::wait_for(&err, TIMEOUT, "@event join");
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);

    // Der Testserver schickt alle 500 ms ein KeepAlive. Antwortet der Client noch, während die
    // Pipe längst voll ist, hängt er nicht im Schreiben fest.
    let ids = &common::MC_26_1;
    let mut antworten = 0;
    while antworten < 3 {
        let ok = common::wait_note(
            &server.notes,
            TIMEOUT,
            |note| matches!(note, Note::Packet(id, len) if *id == ids.sb_keep_alive && *len == 8),
        );
        assert!(
            ok,
            "der Client hat aufgehört, KeepAlive zu beantworten – er steckt im Schreiben fest"
        );
        antworten += 1;
    }
    assert!(
        child.try_wait().expect("Status").is_none(),
        "der Client ist beendet, statt weiterzulaufen"
    );
    let _ = child.kill();
}

/// Dasselbe für die **Fehlerausgabe**: Auch über sie schreibt der Netz-Thread.
///
/// Beitritts-, Regel- und Ereignismeldungen gehen dorthin. Ein Panel, das nur die Standardausgabe
/// mitliest, füllte damit die zweite Pipe – und der Netz-Thread blieb genauso darin stecken wie
/// früher in der ersten. Der Test lässt stderr absichtlich ungelesen und sorgt mit einer
/// Chat-Regel ohne Sperrzeit dafür, dass jede eingehende Zeile dort eine Meldung erzeugt.
#[test]
fn volle_fehlerausgabe_blockiert_den_netz_thread_nicht() {
    let mut plan = plan_with_ground();
    plan.chat_flood = 4000; // jede Zeile trifft die Regel und erzeugt eine Meldung auf stderr
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(
        server.port,
        "26.1",
        &[
            "--no-color",
            "--events",
            "--on",
            "chat:flut=/nichts",
            "--on-cooldown",
            "0",
        ],
    );
    // stderr bleibt absichtlich ungelesen – genau das ist der Fall, um den es geht.
    // Die Standardausgabe wird zwar gelesen, taugt hier aber nicht als Beitrittsmerkmal: Sie
    // läuft gleich mit tausenden Zeilen voll, und bei Überlauf fällt die älteste heraus – auch
    // die erste. Gefragt wird deshalb der Testserver selbst.
    let _out = common::collect(child.stdout.take().unwrap());
    assert!(
        common::wait_note(&server.notes, TIMEOUT, |note| matches!(note, Note::Joined)),
        "kein Beitritt"
    );

    let ids = &common::MC_26_1;
    for _ in 0..3 {
        let ok = common::wait_note(
            &server.notes,
            TIMEOUT,
            |note| matches!(note, Note::Packet(id, len) if *id == ids.sb_keep_alive && *len == 8),
        );
        assert!(
            ok,
            "der Client hat aufgehört, KeepAlive zu beantworten – er steckt im Schreiben auf die \
             Fehlerausgabe fest"
        );
    }
    assert!(
        child.try_wait().expect("Status").is_none(),
        "der Client ist beendet, statt weiterzulaufen"
    );
    let _ = child.kill();
}

/// Der Teleport muss bestätigt werden, sonst holt der Server uns per Rubberband zurück.
#[test]
fn teleport_wird_bestaetigt() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "-q"]);
    let accepted = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Packet(id, _) if *id == common::MC_26_1.sb_accept_teleportation),
    );
    assert!(accepted, "Teleport wurde nicht bestätigt");
    let _ = child.kill();
}

/// Eingabezeilen gehen als Chat raus.
#[test]
fn eingabe_geht_in_den_chat() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child =
        common::spawn_client(server.port, "26.1", &["--no-color", "--chat-delay", "200"]);
    let mut stdin = child.stdin.take().unwrap();
    let err = common::collect(child.stderr.take().unwrap());
    let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);

    let _ = writeln!(stdin, "hallo welt");
    let arrived = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Chat(text) if text == "hallo welt"),
    );
    assert!(arrived, "die Chatzeile kam nicht beim Server an");
    let _ = child.kill();
}

/// Regressionstest: `:use` hat in 1.21.1 acht Byte zu viel geschickt.
///
/// Die Blickrichtung steht erst ab 1.21.2 mit im `ServerboundUseItemPacket`; in 1.21.1 besteht
/// es nur aus Hand und Sequenznummer. Die zwei überzähligen Fließkommazahlen brachten dort den
/// Paket-Decoder des Servers aus dem Tritt – und damit die ganze Verbindung. Geprüft wird
/// deshalb am Socket, wie lang das Paket wirklich ankommt: zwei Byte gegen zehn.
#[cfg(feature = "state")]
#[test]
fn use_item_paket_passt_zur_version() {
    for (ids, expected) in [(&common::MC_1_21_1, 2usize), (&common::MC_26_1, 10usize)] {
        let server = common::start(ids, plan_with_ground());
        let mut child = common::spawn_client(server.port, ids.name, &["--no-color"]);
        let mut stdin = child.stdin.take().unwrap();
        let err = common::collect(child.stderr.take().unwrap());

        let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
        assert!(joined, "{}: kein Beitritt. Ausgabe:\n{}", ids.name, log);
        let positioned = common::wait_note(
            &server.notes,
            TIMEOUT,
            |note| matches!(note, Note::Packet(id, _) if *id == ids.sb_accept_teleportation),
        );
        assert!(positioned, "{}: Startposition nicht bestätigt", ids.name);
        let _ = writeln!(stdin, ":use");

        let id = ids.sb_use_item;
        let ok = common::wait_note(
            &server.notes,
            TIMEOUT,
            |note| matches!(note, Note::Packet(got, len) if *got == id && *len == expected),
        );
        assert!(
            ok,
            "{}: ServerboundUseItem kam nicht mit {} Byte Nutzdaten an",
            ids.name, expected
        );
        let _ = child.kill();
    }
}

/// Mehrere Weltinteraktionen brauchen aufsteigende Sequenznummern. Mit dauerhaft `0` könnten
/// Server-Acknowledgements nicht mehr eindeutig der auslösenden Aktion zugeordnet werden.
#[cfg(feature = "state")]
#[test]
fn use_item_sequenz_steigt() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    let mut stdin = child.stdin.take().unwrap();

    let positioned = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::Packet(id, _) if *id == common::MC_26_1.sb_accept_teleportation),
    );
    assert!(positioned, "Startposition wurde nicht bestätigt");
    writeln!(stdin, ":use\n:use").unwrap();

    for expected in [0, 1] {
        let received = common::wait_note(
            &server.notes,
            TIMEOUT,
            |note| matches!(note, Note::UseSequence(sequence) if *sequence == expected),
        );
        assert!(received, "UseItem-Sequenz {} kam nicht an", expected);
    }
    let _ = child.kill();
}

// ===================== Live-POV =====================

/// Das Chat-Paket hat ab 1.21.11 ein Byte mehr als in 1.21.1: die Prüfsumme am Ende.
///
/// Genau wie bei `:use` ist das der Fehler, den man am Client nicht sieht: Ein Feld zu viel oder
/// zu wenig bringt den Paket-Decoder des Servers aus dem Tritt, und die Verbindung ist weg.
/// Geprüft wird deshalb am Socket, wie lang das Paket wirklich ankommt.
#[test]
fn chat_paket_passt_zur_version() {
    // "test" = 1 Byte Länge + 4 Byte Text, dazu Zeitstempel und Salt (je 8), das Signaturflag,
    // der Quittungs-Offset und das 20-Bit-Bitfeld (3 Byte) – ab 1.21.11 plus Prüfsumme.
    for (ids, expected) in [(&common::MC_1_21_1, 26usize), (&common::MC_26_1, 27usize)] {
        let mut plan = plan_with_ground();
        plan.chat.clear();
        let server = common::start(ids, plan);
        let mut child = common::spawn_client(server.port, ids.name, &["--no-color", "-q"]);
        let mut stdin = child.stdin.take().unwrap();
        let err = common::collect(child.stderr.take().unwrap());
        // Erst wenn der Server uns im Spiel hat, nimmt der Sender überhaupt etwas an. Als
        // Merkmal dient die Teleport-Bestätigung: Sie kommt nur aus der Spielphase, also erst,
        // nachdem der Client das Login-Paket verarbeitet hat. Ein fester Schlaf wäre auf einem
        // ausgelasteten Rechner mal zu kurz und sonst immer zu lang.
        let accept = ids.sb_accept_teleportation;
        assert!(
            common::wait_note(&server.notes, TIMEOUT, |note| {
                matches!(note, Note::Packet(id, _) if *id == accept)
            }),
            "{}: der Client kam nicht in die Spielphase",
            ids.name
        );
        let _ = writeln!(stdin, "test");
        let _ = stdin.flush();

        let sb_chat = ids.sb_chat;
        let ok = common::wait_note(
            &server.notes,
            TIMEOUT,
            |note| matches!(note, Note::Packet(id, len) if *id == sb_chat && *len == expected),
        );
        if !ok {
            // Nur im Fehlerfall noch einmal mitlesen, um die Meldung des Clients zu zeigen.
            let (_, log) = common::wait_for(&err, Duration::from_millis(200), "kommt nie");
            panic!(
                "{}: das Chat-Paket hat nicht {} Byte. Ausgabe:\n{}",
                ids.name, expected, log
            );
        }
        let _ = child.kill();
    }
}

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
    let (rows, log) = common::wait_for_frame(&err, TIMEOUT, 10, "POV  x=", |line| {
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
    let (rows, log) = common::wait_for_frame(&err, TIMEOUT, 20, "POV  x=", |line| {
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
///
/// Das ist Flusskontrolle des Servers und keine POV-Funktion: Der `PlayerChunkSender` zählt die
/// unbeantworteten Stapel und stellt bei zehn offenen die Auslieferung ein. Deshalb gilt der Test
/// für **jede** Bauform, auch für die schlanke, die keinen einzigen Chunk liest.
#[test]
fn chunk_stapel_wird_bestaetigt() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "-q"]);
    // Der gemeldete Wert muss in den Grenzen liegen, die der Server ohnehin anlegt – ein `inf`
    // oder eine Null hieße, dass der Server gar keine Chunks mehr schickt.
    let ok = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::ChunkBatchReceived(rate) if (0.01..=64.0).contains(rate)),
    );
    assert!(ok, "ServerboundChunkBatchReceived kam nie an");
    let _ = child.kill();
}

/// Der Browser-Viewer: erreichbar, aber nur mit dem Zugriffstoken aus dem Terminal.
///
/// Er lief bisher ganz ohne Ablauftest. Geprüft wird deshalb genau das, was ein Browser sieht:
/// die Seite selbst, der Zustand als JSON – und dass ohne Token nichts davon herausgeht.
/// Ausdrücklich ohne Ressourcen (`--pov-resources aus`), damit der Test weder Netz braucht noch
/// 30 MB lädt.
#[cfg(feature = "pov")]
#[test]
fn browser_viewer_antwortet_nur_mit_token() {
    let server = common::start(&common::MC_26_1, plan_with_ground());

    // Der freie Port wird ermittelt, indem er kurz belegt und sofort wieder freigegeben wird –
    // und genau in dieser Lücke kann ihn ein anderer, gleichzeitig laufender Test erwischen.
    // Dann startet der Viewer nicht. Statt daran hin und wieder zu scheitern, wird es einfach
    // noch einmal versucht.
    let mut attempt = 0;
    let (port, mut child, token) = loop {
        attempt += 1;
        let port = common::free_port();
        let mut child = common::spawn_client(
            server.port,
            "26.1",
            &[
                "--no-color",
                "--pov-web",
                &port.to_string(),
                "--pov-resources",
                "aus",
            ],
        );
        let err = common::collect(child.stderr.take().unwrap());
        // Der Client nennt die vollständige Adresse samt Token auf der Fehlerausgabe.
        let (found, log) = common::wait_for(&err, Duration::from_secs(10), "Browser-POV: http://");
        if found {
            let token = log
                .split("?token=")
                .nth(1)
                .and_then(|rest| rest.split_whitespace().next())
                .expect("Token in der Adresse")
                .to_string();
            break (port, child, token);
        }
        let _ = child.kill();
        assert!(
            attempt < 4,
            "Viewer kam auch nach {} Versuchen nicht hoch. Letzte Ausgabe:\n{}",
            attempt,
            log
        );
    };
    assert_eq!(token.len(), 32, "Token hat nicht 128 Bit: {}", token);

    // Ohne Token: nichts.
    let (status, _, _) = common::http_get(port, "/");
    assert_eq!(status, 403, "die Seite ging ohne Token heraus");
    let (status, _, _) = common::http_get(port, "/api/state.json");
    assert_eq!(status, 403, "der Zustand ging ohne Token heraus");
    let (status, _, _) = common::http_get(port, &format!("/?token={}x", token));
    assert_eq!(status, 403, "ein falscher Token wurde angenommen");

    // Mit Token: die Seite, und sie bringt alles selbst mit (kein CDN, kein fremder Server).
    let (status, head, body) = common::http_get(port, &format!("/?token={}", token));
    assert_eq!(status, 200);
    assert!(head.contains("text/html"), "{}", head);
    let page = String::from_utf8_lossy(&body);
    assert!(page.contains("Live-POV"), "unerwartete Seite");
    assert!(
        !page.contains("https://"),
        "die Seite laedt von aussen nach"
    );

    // Und der Zustand ist gültiges JSON mit den Feldern, an denen die Seite hängt.
    let (status, head, body) = common::http_get(port, &format!("/api/state.json?token={}", token));
    assert_eq!(status, 200);
    assert!(head.contains("application/json"), "{}", head);
    let state: serde_json::Value = serde_json::from_slice(&body).expect("gueltiges JSON");
    assert_eq!(state["textures"], serde_json::Value::Bool(false));
    assert!(
        state["texture_error"]
            .as_str()
            .unwrap_or("")
            .contains("aus"),
        "der abgeschaltete Zustand steht nicht im JSON: {}",
        state["texture_error"]
    );
    // Ohne Ressourcen gibt es kein texturiertes Bild – und das sagt der Viewer auch, statt ein
    // leeres PNG zu liefern.
    let (status, _, _) = common::http_get(port, &format!("/api/frame.png?token={}", token));
    assert_eq!(status, 503);

    let _ = child.kill();
}

/// Der ganze Weg am Stück, durch das echte Binary: verbinden, Chunks lesen, Original-Texturen
/// laden, ein Bild rendern und es über HTTP ausliefern.
///
/// Läuft nicht im normalen Testlauf mit – er braucht eine echte Client-JAR. Aufruf:
/// `AFK_POV_RESOURCES=~/.minecraft/versions/1.21.1/1.21.1.jar cargo test --features ultra -- --ignored --nocapture browser_viewer_liefert`
#[cfg(feature = "pov")]
#[test]
#[ignore]
fn browser_viewer_liefert_ein_texturiertes_bild() {
    let Ok(jar) = std::env::var("AFK_POV_RESOURCES") else {
        println!("AFK_POV_RESOURCES nicht gesetzt – nichts zu tun");
        return;
    };
    let server = common::start(&common::MC_1_21_1, plan_with_ground());
    let port = common::free_port();
    let mut child = common::spawn_client(
        server.port,
        "1.21.1",
        &[
            "--no-color",
            "--pov-web",
            &port.to_string(),
            "--pov-resources",
            &jar,
        ],
    );
    let err = common::collect(child.stderr.take().unwrap());

    let (loaded, log) = common::wait_for(&err, Duration::from_secs(120), "Texturen sind geladen");
    assert!(loaded, "die Texturen kamen nicht. Ausgabe:\n{}", log);
    let token = log
        .split("?token=")
        .nth(1)
        .and_then(|rest| rest.split_whitespace().next())
        .expect("Token in der Adresse")
        .to_string();

    // Der Zustand meldet die Texturen als vorhanden und ohne Hinweistext.
    let (status, _, body) = common::http_get(port, &format!("/api/state.json?token={}", token));
    assert_eq!(status, 200);
    let state: serde_json::Value = serde_json::from_slice(&body).expect("gueltiges JSON");
    assert_eq!(state["textures"], serde_json::Value::Bool(true));
    assert_eq!(state["texture_error"], serde_json::Value::Null);

    // Und das Bild kommt als PNG heraus – mit Inhalt, nicht als leere Fläche.
    let (status, head, png) =
        common::http_get(port, &format!("/api/frame.png?token={}&w=320&h=180", token));
    assert_eq!(status, 200, "kein Bild: {}", head);
    assert!(head.contains("image/png"), "{}", head);
    assert_eq!(&png[..8], b"\x89PNG\r\n\x1a\n", "das ist kein PNG");
    // Ein einfarbiges 320x180-PNG bliebe winzig; ein texturiertes Bild nicht.
    assert!(
        png.len() > 10_000,
        "das Bild ist mit {} Byte verdaechtig klein",
        png.len()
    );
    // Zum Ansehen ablegen, wenn gewünscht – geprüft wird schließlich ein Bild.
    if let Ok(target) = std::env::var("AFK_POV_PNG") {
        std::fs::write(&target, &png).expect("Bild schreiben");
        println!("Bild geschrieben: {}", target);
    }
    let _ = child.kill();
}

/// Alle 25 Chunks müssen ankommen **und** lesbar sein – auf beiden Protokollformaten.
#[cfg(feature = "pov")]
#[test]
fn pov_liest_alle_chunks() {
    for (ids, compression) in [
        (&common::MC_26_1, None),
        (&common::MC_1_21_1, None),
        // Chunk-Pakete sind die groessten, die je kommen – und auf einem echten Server sind sie
        // immer komprimiert. Genau daran haengt der Entpackpfad mitsamt seinen Puffern.
        (&common::MC_26_1, Some(256)),
    ] {
        let mut plan = plan_with_ground();
        plan.compression = compression;
        let server = common::start(ids, plan);
        let mut child = common::spawn_client(server.port, ids.name, &["--no-color"]);
        let mut stdin = child.stdin.take().unwrap();
        let err = common::collect(child.stderr.take().unwrap());

        let (joined, log) = common::wait_for(&err, TIMEOUT, "im Spiel");
        assert!(joined, "{}: kein Beitritt. Ausgabe:\n{}", ids.name, log);

        // Nachfragen statt einmal raten: `:pov info` beantwortet den Stand von jetzt, und der
        // Client kann die Chunks noch einlesen.
        let (found, log) =
            common::poll_command(&mut stdin, &err, TIMEOUT, ":pov info", "25 Chunks");
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
    let (found, log) = common::poll_command(&mut stdin, &err, TIMEOUT, ":pov info", "25 Chunks");
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

    // Erst fragen, wenn alle 441 Chunks durch sind – sonst zählt die Auskunft einen Zwischenstand.
    // Nach dem Aufräumen bleiben genau 169 übrig, also darauf warten.
    let (found, log) = common::poll_command(&mut stdin, &err, TIMEOUT, ":pov info", "169 Chunks");
    assert!(found, "keine Auskunft über 169 Chunks. Ausgabe:\n{}", log);
    // Die *letzte* Auskunft zählt: durch das Nachfragen stehen frühere Zwischenstände mit im Log.
    let line = log
        .lines()
        .rfind(|line| line.contains("Chunks ·"))
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
    assert!(
        !found,
        "--pov aus hat die Ansicht nicht verhindert:\n{}",
        log
    );
}

/// Der neue Viewer ist eine echte HTTP-Schnittstelle, nicht nur eine im Unit-Test gerenderte
/// Zeichenkette. Token, HTML und Live-Zustand werden gegen das gebaute Binary am Socket geprueft.
#[cfg(feature = "pov")]
#[test]
fn browser_pov_liefert_seite_und_live_zustand() {
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").expect("freien Port suchen");
    let web_port = reserved.local_addr().unwrap().port();
    drop(reserved);
    let web_port_text = web_port.to_string();

    let server = common::start(&common::MC_26_2, plan_with_ground());
    let mut extra = vec![
        "--no-color".to_string(),
        "--pov-web".to_string(),
        web_port_text,
        "--pov".to_string(),
        "aus".to_string(),
    ];
    let original = std::env::var("AFK_POV_RESOURCES").ok();
    if let Some(path) = &original {
        extra.extend(["--pov-resources".to_string(), path.clone()]);
    }
    let references: Vec<&str> = extra.iter().map(String::as_str).collect();
    let mut child = common::spawn_client(server.port, "26.2", &references);
    let err = common::collect(child.stderr.take().unwrap());
    let (found, log) = common::wait_for(&err, TIMEOUT, "Browser-POV: http://");
    assert!(found, "Viewer-URL fehlt. Ausgabe:\n{}", log);
    let token = log
        .lines()
        .find_map(|line| line.split("?token=").nth(1))
        .and_then(|value| value.split_whitespace().next())
        .expect("Token in Viewer-URL");
    assert_eq!(token.len(), 32, "Viewer-Token hat nicht 128 Bit");
    assert!(token.bytes().all(|byte| byte.is_ascii_hexdigit()));

    fn get(port: u16, path: &str) -> Vec<u8> {
        let mut stream = std::net::TcpStream::connect(("127.0.0.1", port)).expect("Viewer offen");
        write!(
            stream,
            "GET {} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n",
            path
        )
        .unwrap();
        let mut answer = Vec::new();
        stream.read_to_end(&mut answer).unwrap();
        answer
    }

    let page = String::from_utf8(get(web_port, &format!("/?token={}", token))).unwrap();
    assert!(page.starts_with("HTTP/1.1 200"), "{}", page);
    assert!(page.contains("AFKSystems – Live-POV"), "HTML fehlt");

    let state =
        String::from_utf8(get(web_port, &format!("/api/state.json?token={}", token))).unwrap();
    assert!(state.starts_with("HTTP/1.1 200"), "{}", state);
    assert!(
        state.contains("\"dimension\":\"minecraft:overworld\""),
        "{}",
        state
    );

    let forbidden = String::from_utf8(get(web_port, "/api/state.json")).unwrap();
    assert!(forbidden.starts_with("HTTP/1.1 403"), "{}", forbidden);
    let asset_forbidden = String::from_utf8(get(
        web_port,
        "/assets/minecraft/textures/gui/sprites/hud/crosshair.png",
    ))
    .unwrap();
    assert!(
        asset_forbidden.starts_with("HTTP/1.1 403"),
        "JAR-Ressource war ohne Token erreichbar: {}",
        asset_forbidden
    );

    let asset = get(
        web_port,
        &format!(
            "/assets/minecraft/textures/gui/sprites/hud/crosshair.png?token={}",
            token
        ),
    );
    let asset_header_end = asset
        .windows(4)
        .position(|part| part == b"\r\n\r\n")
        .expect("Header der JAR-Ressource")
        + 4;
    let asset_header = String::from_utf8_lossy(&asset[..asset_header_end]);
    if original.is_some() {
        assert!(asset.starts_with(b"HTTP/1.1 200"), "{}", asset_header);
        assert!(
            asset_header.contains("Cache-Control: private, max-age=31536000, immutable"),
            "JAR-Ressource wurde nicht dauerhaft gecacht: {}",
            asset_header
        );
    } else {
        assert!(asset.starts_with(b"HTTP/1.1 503"), "{}", asset_header);
    }

    if original.is_some() {
        let (joined, output) = common::wait_for(&err, TIMEOUT, "im Spiel");
        assert!(joined, "Texturtest trat nicht bei: {}", output);
        let frame = get(
            web_port,
            &format!("/api/frame.png?token={}&w=426&h=240", token),
        );
        let split = frame
            .windows(4)
            .position(|part| part == b"\r\n\r\n")
            .unwrap()
            + 4;
        assert!(
            frame.starts_with(b"HTTP/1.1 200"),
            "PNG-Antwort war kein 200"
        );
        assert_eq!(&frame[split..split + 8], b"\x89PNG\r\n\x1a\n");
        let decoder = png::Decoder::new(std::io::Cursor::new(&frame[split..]));
        let mut reader = decoder.read_info().expect("Browser-PNG lesbar");
        let mut rgba = vec![0; reader.output_buffer_size()];
        let info = reader
            .next_frame(&mut rgba)
            .expect("Browser-PNG decodieren");
        let rgba = &rgba[..info.buffer_size()];
        assert!(
            rgba.as_chunks::<4>()
                .0
                .iter()
                .any(|pixel| pixel[..3] != rgba[..3]),
            "Texturframe bestand nur aus einer einzigen Farbe"
        );
        let missing = rgba
            .as_chunks::<4>()
            .0
            .iter()
            .filter(|pixel| pixel[0] > 220 && pixel[1] < 30 && pixel[2] > 220)
            .count();
        assert_eq!(missing, 0, "Stone wurde als lila Fehlertextur gerendert");
    }
    let _ = child.kill();
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
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);
    let (found, log) = common::wait_for(&err, Duration::from_secs(3), "(:pov stop)");
    assert!(!found, "Ultra startete die Ansicht ungefragt:\n{}", log);
    drop(child);

    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "--pov", "an"]);
    let err = common::collect(child.stderr.take().unwrap());
    let (found, log) = common::wait_for(&err, TIMEOUT, "(:pov stop)");
    assert!(found, "--pov an blieb bei Ultra wirkungslos:\n{}", log);
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

/// Jeder örtliche Befehl muss antworten – auch mit Unsinn als Argument.
///
/// Die `:`-Befehle laufen im Eingabe-Thread, greifen aber auf denselben Zustand zu wie der
/// Netz-Thread (Menü, Anzeigetafel, Weltdaten, Bewegungseinstellungen). Ein Absturz dort reißt
/// mit `panic = "abort"` den ganzen Prozess mit, und ein hängengebliebener Sperrvorgang kostet
/// das nächste KeepAlive. Der Test schickt deshalb alles durch, inklusive der Eingaben, mit
/// denen niemand rechnet.
#[cfg(feature = "ultra")]
#[test]
fn alle_oertlichen_befehle_ueberstehen_auch_unsinn() {
    let mut plan = plan_with_ground();
    plan.scoreboard = true;
    plan.menu = true;
    plan.compression = Some(256);
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "--events"]);
    let mut stdin = child.stdin.take().unwrap();
    let err = common::collect(child.stderr.take().unwrap());
    let (joined, log) = common::wait_for(&err, TIMEOUT, "@event join");
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);

    for line in [
        // Alles, was es gibt ...
        ":help",
        ":pos",
        ":board",
        ":menu",
        ":inv",
        ":slot 4",
        ":click 4",
        ":click 4 rechts",
        ":click 4 shift",
        ":close",
        ":pov info",
        ":pov frame",
        ":pov size 24 12",
        ":pov fps 3",
        ":pov live",
        ":pov stop",
        ":sneak on",
        ":sneak off",
        ":sneak",
        ":sprint an",
        ":swing",
        ":use",
        ":hand 3",
        ":antiafk 20",
        ":antiafk off",
        ":look nord",
        ":look 90 -10",
        ":look links 30",
        ":go vor 1",
        ":stop",
        ":fall on",
        ":fall 2",
        ":home",
        ":home set",
        ":home delay 1",
        ":home speed 4",
        ":home off",
        ":home clear",
        ":route",
        ":route rec",
        ":route add",
        ":route stop",
        ":route del",
        ":route clear",
        // ... und alles, womit niemand rechnet.
        ":",
        ":go",
        ":go rueckwaerts abc",
        ":go vor -5",
        ":go vor 99999",
        ":look",
        ":look xyz",
        ":click",
        ":click abc",
        ":click -1",
        ":click 999999999999999999999",
        ":slot",
        ":slot xyz",
        ":slot 99999",
        ":hand 0",
        ":hand 99",
        ":hand x",
        ":pov size 9999 9999",
        ":pov fps 0",
        ":pov unsinn",
        ":antiafk 000000000000000000",
        ":home delay -3",
        ":home speed x",
        ":route del 99",
        ":sneak vielleicht",
        ":unbekannt",
        ":HELP",
        ":Pos",
    ] {
        assert!(
            writeln!(stdin, "{}", line).is_ok(),
            "Eingabe '{}' abgewiesen",
            line
        );
    }
    let _ = stdin.flush();

    // Der Client muss danach noch da sein **und** weiter antworten.
    let ids = &common::MC_26_1;
    for _ in 0..2 {
        let ok = common::wait_note(
            &server.notes,
            TIMEOUT,
            |note| matches!(note, Note::Packet(id, len) if *id == ids.sb_keep_alive && *len == 8),
        );
        assert!(ok, "nach den Befehlen kommt kein KeepAlive mehr");
    }
    assert!(
        child.try_wait().expect("Status").is_none(),
        "ein örtlicher Befehl hat den Client beendet"
    );
    let _ = child.kill();
}

/// Die gemeldete Sichtweite entscheidet, wie viele Chunkdaten der Server überhaupt schickt.
/// Ohne Live-Ansicht liest der Client keinen einzigen Chunk – dann muss dort das Minimum stehen.
#[test]
fn sichtweite_passt_zur_bauform() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "-q"]);
    let expected: u8 = if cfg!(feature = "pov") { 6 } else { 2 };
    let ok = common::wait_note(
        &server.notes,
        TIMEOUT,
        |note| matches!(note, Note::ViewDistance(v) if *v == expected),
    );
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

/// Messlauf statt Behauptung: Wie viele Zeilen eines Schwalls kommen wirklich an?
///
/// Die Warteschlange der Ausgabe ist gedeckelt (256 Zeilen); läuft sie über, fällt die älteste
/// heraus. Ob das passiert, hängt allein davon ab, ob der Schreib-Thread mit dem Netz-Thread
/// mithält – und genau daran ändert es etwas, mehrere wartende Zeilen zu **einem**
/// Schreibvorgang zusammenzufassen statt je Zeile einen Systemaufruf zu machen. Auf einem
/// Rechner, der ohnehin mithält, kommt mit und ohne dasselbe heraus; auf einem ausgelasteten
/// fielen vorher die ersten Zeilen heraus, obwohl mitgelesen wurde.
///
/// Läuft nicht im normalen Testlauf mit – die Zahl hängt vom Rechner ab. Aufruf:
/// `cargo test --release --features ultra --test live -- --ignored --nocapture chatzeilen`
#[test]
#[ignore]
fn messlauf_wie_viele_chatzeilen_ankommen() {
    const ZEILEN: usize = 4000;
    let mut plan = plan_with_ground();
    plan.chat = (1..=ZEILEN).map(|n| format!("Zeile {}", n)).collect();
    plan.close_after_chat = true;
    let server = common::start(&common::MC_26_1, plan);
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color"]);
    let out = common::collect(child.stdout.take().unwrap());
    // Bis zum Ende mitlesen: Der Server legt nach dem Chat auf, der Client schreibt noch aus.
    let (_, log) = common::wait_for(&out, Duration::from_secs(10), "kommt nie");
    let angekommen = log.lines().filter(|l| l.starts_with("Zeile ")).count();
    println!(
        "\nvon {} Chatzeilen angekommen: {} ({} herausgefallen)\n",
        ZEILEN,
        angekommen,
        ZEILEN - angekommen
    );
    let _ = child.kill();
}
