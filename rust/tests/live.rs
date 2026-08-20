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

        let (kurz, log) = common::wait_for(&out, TIMEOUT, "Willkommen auf dem Testserver");
        assert!(kurz, "{:?}: kein Chat. Ausgabe:\n{}", compression, log);
        let (lang, log) = common::wait_for(&out, TIMEOUT, &"V".repeat(600));
        assert!(lang, "{:?}: die lange Zeile fehlt. Ausgabe:\n{}", compression, log);

        // Und in die Gegenrichtung – dort verschlüsselt der Client selbst.
        let _ = writeln!(stdin, "/{}", "y".repeat(600));
        let _ = stdin.flush();
        let angekommen = common::wait_note(&server.notes, TIMEOUT, |note| {
            matches!(note, Note::Command(text) if text.len() == 600)
        });
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

        let (kurz, log) = common::wait_for(&out, TIMEOUT, "Willkommen auf dem Testserver");
        assert!(kurz, "Schwelle {}: kein Chat. Ausgabe:\n{}", threshold, log);
        let (lang, log) = common::wait_for(&out, TIMEOUT, &"L".repeat(600));
        assert!(lang, "Schwelle {}: die lange Zeile fehlt. Ausgabe:\n{}", threshold, log);

        // Und in die Gegenrichtung: ein langer Befehl muss beim Server heil ankommen.
        let befehl = format!("/{}", "x".repeat(600));
        let _ = writeln!(stdin, "{}", befehl);
        let _ = stdin.flush();
        let angekommen = common::wait_note(&server.notes, TIMEOUT, |note| {
            matches!(note, Note::Command(text) if text.len() == 600)
        });
        assert!(angekommen, "Schwelle {}: der lange Befehl kam nicht an", threshold);
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

    let respawned = common::wait_note(&server.notes, TIMEOUT, |note| {
        matches!(note, Note::Packet(id, _) if *id == common::MC_26_1.sb_client_command)
    });
    assert!(respawned, "der Client hat nicht von selbst respawnt");

    let fired = common::wait_note(&server.notes, TIMEOUT, |note| {
        matches!(note, Note::Command(c) if c == "spawn")
    });
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

    let angekommen = common::wait_note(&ziel.notes, TIMEOUT, |note| {
        matches!(note, Note::Joined)
    });
    let (_, log) = common::wait_for(&err, Duration::from_millis(300), "Server-Transfer");
    assert!(
        angekommen,
        "der Client ist dem Transfer nicht gefolgt. Ausgabe:\n{}",
        log
    );
    let _ = child.kill();
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
    let angekommen = common::wait_note(&ziel.notes, TIMEOUT, |note| {
        matches!(note, Note::Cookie(key, Some(value)) if key == "afk:test" && value == b"geheim")
    });
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
        assert!(opened, "{}: kein Menü gemeldet. Ausgabe:\n{}", ids.name, log);

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

        let (lore, log) = common::poll_command(&mut stdin, &err, TIMEOUT, ":slot 4", "kostet nichts");
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
        let ok = common::wait_note(&server.notes, TIMEOUT, |note| {
            matches!(note, Note::Packet(id, len) if *id == ids.sb_keep_alive && *len == 8)
        });
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
    let out = common::collect(child.stdout.take().unwrap());
    let (joined, log) = common::wait_for(&out, TIMEOUT, "Willkommen auf dem Testserver");
    assert!(joined, "kein Beitritt. Ausgabe:\n{}", log);

    let ids = &common::MC_26_1;
    for _ in 0..3 {
        let ok = common::wait_note(&server.notes, TIMEOUT, |note| {
            matches!(note, Note::Packet(id, len) if *id == ids.sb_keep_alive && *len == 8)
        });
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
    let accepted = common::wait_note(&server.notes, TIMEOUT, |note| {
        matches!(note, Note::Packet(id, _) if *id == common::MC_26_1.sb_accept_teleportation)
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
        let _ = writeln!(stdin, ":use");

        let id = ids.sb_use_item;
        let ok = common::wait_note(&server.notes, TIMEOUT, |note| {
            matches!(note, Note::Packet(got, len) if *got == id && *len == expected)
        });
        assert!(
            ok,
            "{}: ServerboundUseItem kam nicht mit {} Byte Nutzdaten an",
            ids.name, expected
        );
        let _ = child.kill();
    }
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
#[cfg(feature = "pov")]
#[test]
fn chunk_stapel_wird_bestaetigt() {
    let server = common::start(&common::MC_26_1, plan_with_ground());
    let mut child = common::spawn_client(server.port, "26.1", &["--no-color", "-q"]);
    let ok = common::wait_note(&server.notes, TIMEOUT, |note| {
        matches!(note, Note::Packet(id, _) if *id == common::MC_26_1.sb_chunk_batch_received)
    });
    assert!(ok, "ServerboundChunkBatchReceived kam nie an");
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
        let (found, log) = common::poll_command(&mut stdin, &err, TIMEOUT, ":pov info", "25 Chunks");
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
        ":help", ":pos", ":board", ":menu", ":inv", ":slot 4", ":click 4", ":click 4 rechts",
        ":click 4 shift", ":close", ":pov info", ":pov frame", ":pov size 24 12", ":pov fps 3",
        ":pov live", ":pov stop", ":sneak on", ":sneak off", ":sneak", ":sprint an", ":swing",
        ":use", ":hand 3", ":antiafk 20", ":antiafk off", ":look nord", ":look 90 -10",
        ":look links 30", ":go vor 1", ":stop", ":fall on", ":fall 2", ":home", ":home set",
        ":home delay 1", ":home speed 4", ":home off", ":home clear", ":route", ":route rec",
        ":route add", ":route stop", ":route del", ":route clear",
        // ... und alles, womit niemand rechnet.
        ":", ":go", ":go rueckwaerts abc", ":go vor -5", ":go vor 99999", ":look", ":look xyz",
        ":click", ":click abc", ":click -1", ":click 999999999999999999999", ":slot",
        ":slot xyz", ":slot 99999", ":hand 0", ":hand 99", ":hand x", ":pov size 9999 9999",
        ":pov fps 0", ":pov unsinn", ":antiafk 000000000000000000", ":home delay -3",
        ":home speed x", ":route del 99", ":sneak vielleicht", ":unbekannt", ":HELP", ":Pos",
    ] {
        assert!(writeln!(stdin, "{}", line).is_ok(), "Eingabe '{}' abgewiesen", line);
    }
    let _ = stdin.flush();

    // Der Client muss danach noch da sein **und** weiter antworten.
    let ids = &common::MC_26_1;
    for _ in 0..2 {
        let ok = common::wait_note(&server.notes, TIMEOUT, |note| {
            matches!(note, Note::Packet(id, len) if *id == ids.sb_keep_alive && *len == 8)
        });
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
