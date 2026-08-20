# Funktionen im Detail

Diese Datei beschreibt die Rust-Bauformen und ihre Grenzen. Die konkreten Downloadnamen stehen
zusätzlich in [RELEASE.md](RELEASE.md). Die neuen Zusatzfunktionen werden ausschließlich für Rust
gebaut; der Java-Code bleibt davon unberührt.

## Die sieben Rust-Bauformen

| Bauform | Release-Dateien | Cargo-Features | Zusätze gegenüber normal |
| --- | --- | --- | --- |
| normal | `afk-windows.exe`, `afk-linux` | keine | keine |
| Bewegung | `afk-windows-move.exe`, `afk-linux-move` | `movement` | manuelle Bewegung und Routen |
| Items | `items-afk-windows.exe`, `items-afk-linux` | `items` | Menüs, Klicks, Inventar-/Menügegenstände |
| Premium | `premium-afk-windows.exe`, `premium-afk-linux` | `premium` | Bewegung, Scoreboard, Menü-Klicks, Tastenzustand, Anti-AFK |
| Premium + Items | `premium-items-afk-windows.exe`, `premium-items-afk-linux` | `premium,items` | Premium plus sichtbare Gegenstände |
| POV | `pov-afk-windows.exe`, `pov-afk-linux` | `pov-client` | automatisch gestartete Live-POV |
| Ultra | `ultra-afk-windows.exe`, `ultra-afk-linux` | `ultra` | Premium + Items + zuschaltbare POV |

Alle Varianten entstehen aus derselben Quelle. Ein nicht aktiviertes Feature wird nicht nur
versteckt: Code, Paket-IDs, Zustandstabellen und Threads dafür werden gar nicht einkompiliert.
Tablist und Playerlist sind aus sämtlichen Rust-Varianten entfernt.

Jede Bauform **nimmt trotzdem die Optionen aller anderen an** und meldet nur, dass sie sie
ignoriert – ein Panel kann also allen Dateien dieselbe Befehlszeile schicken, ohne vorher zu
wissen, welche vor ihm steht. Dasselbe gilt für den Java-Client in beide Richtungen.

## Grundfunktionen jeder Rust-Datei

### Verbindung

| Funktion | Bedienung/Verhalten |
| --- | --- |
| Vier Minecraft-Versionen | `--mc 1.21.1 \| 1.21.11 \| 26.1 \| 26.2` (Standard `26.1`) |
| SRV-Auflösung | automatisch, sofern kein Port angegeben wurde |
| IPv6 | z. B. `[::1]:25566` |
| SOCKS5-/HTTP-Proxy | `--proxy <adresse>` |
| alternativer Handshake-Host | `--fakehost <host[:port]>` |
| Kompression/Verschlüsselung | automatisch: zlib und AES-128-CFB8 |
| gemeldete Sichtweite | `--view-distance <2–32>`; Standard 2, in den POV-Bauformen 6 |
| Zeitlimit beim Verbindungsaufbau | 20 s, danach eine klare Meldung statt stiller Wartezeit |
| Server-Transfer | wird als Teil derselben Sitzung ohne Wartezeit befolgt |
| Kick/Verbindungsabbruch | kein automatischer Neuverbindungsversuch; Prozess endet mit Status 1 |

Der Transfer ist absichtlich vom Reconnect nach einem Kick getrennt: Beim Transfer weist der
bereits verbundene Server den Client ausdrücklich an, innerhalb derselben Sitzung ein neues Ziel
zu öffnen. Ein Kick oder gewöhnlicher Netzabbruch beendet dagegen den Client.

### Protokollbasierter Kick-Schutz

- `KeepAlive` sofort beantworten;
- `Ping` mit `Pong` beantworten;
- Server-Teleports bestätigen und die Position zurückspiegeln;
- erzwungene Resource-Packs bestätigen, aber nicht laden;
- Client-Information und ab 1.21.11 den Verhaltenskodex beantworten;
- Cookies ablegen und beantworten – auch über einen Server-Transfer hinweg, denn genau dafür
  gibt es sie (Server A legt eines ab, Server B fragt es beim Login ab);
- signierte Chat-Nachrichten quittieren;
- nach Tod automatisch respawnen.

Das ist vom optionalen Anti-AFK getrennt. Der normale Client bewegt sich nicht von selbst.

### Konto, Chat und Automatisierung

| Funktion | Bedienung |
| --- | --- |
| Microsoft-Konto hinzufügen | `--login` |
| Konten auflisten/auswählen | `--accounts`, `--account <name>` |
| Offline-/Cracked-Modus | `--offline <name>` |
| Chat lesen/schreiben | stdout bzw. stdin; `/` am Anfang sendet einen Serverbefehl |
| Befehl nach Beitritt | `-c /afk` |
| Wiederholter Befehl | `-c 300:/afk` |
| Ereignismakro | `--on join=/afk`, `--on death=/spawn`, `--on chat:text=/befehl` |
| Ausgabefarben | `--no-color` schaltet Textfarben ab |
| maschinenlesbare Ereignisse | `--events` |

Nachrichten und Befehle werden bereinigt, auf die Protokollgrenzen gekürzt und über dieselbe
rate-limitierte Warteschlange gesendet. Chat-Regeln werden nur ausgewertet, wenn mindestens eine
`chat:`-Regel existiert.

Eingehender Chat kommt als Netzwerk-NBT, und dessen Zeichenketten stehen in Javas
**modifiziertem** UTF-8: Zeichen über U+FFFF – also jedes Emoji – als zwei Drei-Byte-Folgen, das
Nullzeichen als Überlänge. Der Client dekodiert genau dieses Format; Emoji, Umlaute und
Sonderzeichen kommen deshalb unverändert auf der Standardausgabe an. Dasselbe gilt für alles
andere, was als NBT ankommt: Kick-Gründe, Scoreboard-Zeilen, Gegenstandsnamen und Lore.

Geschrieben wird **von keinem der beiden Ströme im Netz-Thread**: Standardausgabe und
Fehlerausgabe haben je einen eigenen Schreib-Thread. Der Netz-Thread, der KeepAlive beantwortet,
darf nicht an einer vollen Pipe hängen bleiben – sonst kostet ausgerechnet ein gerade
beschäftigtes Panel die Verbindung. Holt niemand die Ausgabe ab, fällt nach 256 wartenden Zeilen
die älteste heraus (mit einer einmaligen Meldung); die Verbindung zu halten hat Vorrang vor
Zeilen, die ohnehin niemand liest.

Getrennte Threads je Strom, nicht einer für beides: Sonst hielte eine volle Standardausgabe auch
die Fehlerausgabe an – und ausgerechnet `@event disconnect`, mit dem ein Panel erfährt, dass der
Client weg ist, käme dann nie an.

Ausgehender Chat wird auf **256 UTF-16-Einheiten** gekürzt, nicht auf 256 Zeichen: Der Server
zählt mit Javas `String.length()` (jedes Emoji zählt dort doppelt) und deckelt zusätzlich die
Bytezahl auf das Dreifache. Nach Zeichen gezählt passte eine Zeile aus 200 Emoji scheinbar, kam
aber mit 800 statt höchstens 768 Byte an – und der Server brach die Verbindung schon beim
Dekodieren ab.

## Bewegung

In Bewegung, Premium, Premium + Items und Ultra:

| Befehl | Wirkung |
| --- | --- |
| `:pos` | aktuelle Position/Blickrichtung |
| `:go <nord\|süd\|ost\|west> [blöcke]` | in Blick-/Himmelsrichtung laufen |
| `:look <nord\|süd\|ost\|west\|grad>` | Blick drehen |
| `:home [set\|go]` | Heimatpunkt speichern/erlaufen |
| `:route …` | Routen verwalten/ablaufen |
| `:jump`, `:fall`, `:stop` | Sprung, Fallschritt, Bewegung stoppen |

Die Einstellungen liegen in `movement.json` beim Kontenverzeichnis. Ein Bewegungs-Thread existiert
nur, solange tatsächlich eine Bewegung läuft.

## Menüs und sichtbare Gegenstände

Menü-Klicks sind in Items, Premium, Premium + Items und Ultra enthalten. Premium ohne Items kennt
Fensternummer, Titel, Zustandszähler und Feldanzahl, hält aber bewusst keine Gegenstandsdaten.

| Befehl | Items | Premium | Premium + Items | Ultra |
| --- | ---: | ---: | ---: | ---: |
| `:menu` | Inhalt | Metadaten | Inhalt | Inhalt |
| `:click <feld> [rechts\|shift]` | ja | ja | ja | ja |
| `:close` | ja | ja | ja | ja |
| `:slot <feld>` | ja | nein | ja | ja |
| `:inv` | ja | nein | ja | ja |

Die Gegenstandsvarianten lesen pro Slot:

- Anzahl und Registry-ID;
- `custom_name`, sonst `item_name`, sonst den Ressourcenname aus der zur Version gehörenden
  offiziellen Vanilla-Item-Registry;
- sämtliche Minecraft-Farb-/Formatcodes im Namen;
- Lore-Zeilen einschließlich Farb-/Formatcodes;
- leere und serverseitig aktualisierte Felder;
- das eigene Inventar (Fenster 0 und ab 1.21.11 Einzelupdates).

Die Vanilla-ID-Namenslisten stammen aus den offiziellen Mojang-Server-JARs und sind je Version
getrennt; Herkunft, SHA-1 und Eintragszahl stehen in `rust/data/README.md`. Die Komponententabelle
ist ebenfalls versionsabhängig und aus den Codec-Jars der vier Protokolle abgelesen.
Da Item-Komponenten im Paket keine eigene Längenangabe haben, stoppt ein nicht hinterlegter Typ das
Lesen des restlichen Pakets mit einer sichtbaren Warnung. Er wird nicht geraten und trennt die
Verbindung nicht.

## Farbiges Scoreboard

Premium, Premium + Items und Ultra führen die Seitenleiste mit. `:board` zeigt höchstens die 15
Minecraft-Zeilen, nach Punktwert sortiert. Erhalten bleiben:

- Titel des Objectives;
- fertiger Anzeige-Text eines Scores;
- Team-Präfix, Teamfarbe des Eintrags und Team-Suffix;
- das Zahlenformat des Objectives oder der einzelnen Zeile (farbiger Wert, fester Text oder
  ausgeblendete Zahl);
- Standardfarben, echte RGB-Farben sowie Fett/Kursiv/Unterstrichen/Durchgestrichen.

Im Terminal werden daraus ANSI-Farben. Mit `--events` bleiben sie als `§`-Codes maschinenlesbar:

```text
@event board titel §r§6Meine Tafel
@event board zeile wert=10 zahl=§c10 text=§r§aRang: §6Spieler§r
```

## Live-POV

POV und Ultra enthalten einen echten Weltzustand. Der POV-Client startet das Rendering nach dem
Beitritt automatisch; Ultra startet erst nach `:pov live`, damit ein normaler Ultra-Prozess nicht
ungefragt das Terminal übernimmt. Beides lässt sich mit `--pov an|aus` umdrehen.

| Befehl | Wirkung |
| --- | --- |
| `:pov live` | laufendes Bild starten |
| `:pov stop` | Rendering stoppen |
| `:pov frame` | ein einzelnes aktuelles Bild |
| `:pov size <breite> <höhe>` | interne Auflösung 24–160 × 12–80 |
| `:pov fps <n>` | Bilder je Sekunde, 1–20 |
| `:pov info` | Dimension, Welthöhe, Chunk-/Entity-Anzahl |

Dieselben Einstellungen gibt es als Startargument, damit ein Panel sie setzen kann, **bevor** das
erste Bild rausgeht: `--pov an|aus`, `--pov-size <breite>x<höhe>`, `--pov-fps <n>`. Ohne Angabe
zeichnet der POV-Client mit 64x32 – wer eine andere Größe will, muss sie beim Start mitgeben oder
vor `:pov live` setzen.

Die POV liest tatsächlich die Serverpakete:

- Dimension-Registry für `min_y` und Welthöhe;
- vollständige Chunk-Abschnitte und deren kompakte Block-Paletten;
- einzelne und abschnittsweise Blockänderungen sowie Chunk-Unloads;
- Spawn, Bewegung, Teleport und Entfernen von Entities;
- aktuelle eigene Kameraposition, Yaw und Pitch.

Das Bild entsteht über Voxel-Raycasts aus der First-Person-Kamera, mit Tiefenverdeckung,
Entitäts-Overlays, Flächenlicht und Distanznebel. Es folgt Server-Teleports sowie `:look` und
`:go` live. Minecraft schickt einem headless Protokollclient keine fertigen Frames und keine
Blocktexturen; die POV ist deshalb eine farbige Terminal-Voxelansicht der wirklichen Geometrie,
kein abgegriffenes Bild aus dem offiziellen Spielrenderer.

### Bildformat der Live-POV

Das Format ist eine **zugesagte Schnittstelle**: ein Panel darf es fest einlesen. Alles geht auf
die **Standardfehlerausgabe** (nicht auf die Standardausgabe – dort steht weiterhin nur Chat) und
wird nach jedem Bild geleert.

```text
[H                                   <- nur im Dauerbetrieb, vor jedem Bild
POV  x=9.5 y=-60.0 z=-8.5  Blick 0/0  Chunks 213  (:pov stop)

[38;2;R;G;Bm[48;2;R;G;Bm▀  … je Spalte einmal …  [0m

… Höhe/2 solcher Zeilen …
```

* Eine Terminalzeile trägt **zwei** Bildzeilen: `▀` mit Vordergrundfarbe = oberes Pixel,
  Hintergrundfarbe = unteres. Bei 160x80 sind das 40 Zeilen à 160 Zeichen.
* Ein Bild beginnt immer an der Kopfzeile `POV  x=…`; sie ist zugleich das Ende des vorherigen.
* `▀` ist UTF-8 (`E2 96 80`). Wer den Datenstrom stückweise liest, muss die Bytes über einen
  Dekodierer laufen lassen – ein `toString('utf8')` je Datenstück zerreißt das Zeichen an der
  Stückgrenze und kappt ab dort jedes Bild.
* Mit `--no-color` kommt statt der Farbzeilen eine Helligkeitsrampe ` .:-=+*#%@`, eine
  Terminalzeile je Bildzeile. Ein Panel, das Farbe erwartet, sollte `--no-color` also **nicht**
  setzen.
* **Unveränderte Bilder werden ausgelassen.** Steht der Bot still, kommt trotzdem mindestens alle
  2 Sekunden ein Bild – Stille heißt also nicht, dass die Ansicht tot ist.

### Speicherbedarf

Die POV-Bauformen sind die einzigen, die Chunks überhaupt auswerten; alle anderen werfen die Pakete
weg. Gemessen an einem identischen Arbeitspunkt (441 gesendete Chunks, Live-Ansicht an):

| | Resident |
| --- | --- |
| vorherige Fassung | 42 940 K |
| jetzt | 10 132 K |

Der Messlauf steht als Test im Baum und lässt sich nachfahren:

```bash
cd rust && cargo test --release --features pov-client -- --ignored --nocapture speicher
```

Der Unterschied kommt aus vier Stellen: Palettenindizes als `u8`/`u16` statt `u32`, reine
Luft-Abschnitte werden gar nicht erst behalten, Abschnitte hängen einzeln an einem `Arc` (ein
Blockwechsel kopiert nicht mehr den ganzen Chunk), und Chunks weiter als 6 Chunks von der Kamera
fallen wieder raus.

Die Obergrenze für gehaltene Chunks leitet sich aus genau diesem Radius ab (17×17 = 289 statt
vormals einer glatten 1024). Sie greift nur in dem kurzen Fenster, in dem noch nicht aufgeräumt
werden kann – der Server schickt Chunks, bevor er die erste Position schickt. Im Regelbetrieb
bleiben es die 169 Chunks aus der Tabelle oben.

Wie lange das Einlesen dauert, misst ein zweiter Test am fertigen Chunk (24 Abschnitte, acht davon
gefüllt, gemischte Palette):

```bash
cd rust && cargo test --release --features pov -- --ignored --nocapture chunk_einlesen
```

## Tastenzustand und Anti-AFK

Premium, Premium + Items und Ultra:

| Befehl/Option | Wirkung |
| --- | --- |
| `--sneak` | nach dem Beitritt geduckt bleiben |
| `:sneak [on\|off]` | Schleichen ändern |
| `:sprint [on\|off]` | Sprinten ändern |
| `:swing` | Arm/Haupthand schwingen |
| `:use` | Gegenstand in der Haupthand benutzen |
| `:hand <1-9>` | Schnellleistenfeld wechseln |
| `--antiafk <sek>` / `:antiafk …` | kleine automatische Bewegung (mindestens 15 s) |

## Ereignisse für Panels

`--events` schreibt ungefärbte Zustandszeilen auf stderr, auch zusammen mit `--quiet`:

| Ereignis | Bedeutung |
| --- | --- |
| `@event connecting host=… port=… mc=…` | genau der gestartete Verbindungsversuch |
| `@event join name=…` | echter Beitritt |
| `@event world …` | Unterserver-/Weltwechsel |
| `@event death` | Tod |
| `@event disconnect <grund>` | endgültige Trennung |
| `@event menu open …` / `close` | Menüstatus |
| `@event slot …` / `lore …` | Gegenstände samt `§`-Formatierung |
| `@event board …` | Scoreboard samt `§`-Formatierung |
| `@event output ausgelassen` | die Standardausgabe wird nicht abgeholt, Chatzeilen fallen heraus |

Es gibt kein Reconnect-Ereignis mehr, weil der Rust-Client nach einem Kick nicht erneut verbindet.

## Feature-Abhängigkeiten im Quellcode

```text
local
├─ movement
└─ extras
   ├─ board
   ├─ menu ── items
   ├─ state
   └─ pov ── pov-client

antiafk = movement + state
premium = movement + board + menu + state + antiafk
ultra   = premium + items + pov
```

Die maßgeblichen Module sind getrennt:

| Datei | Verantwortung |
| --- | --- |
| `rust/src/extras.rs` | feature-gesteuerte Verteilerstelle und gemeinsamer Zustand |
| `rust/src/board.rs` | Scoreboard |
| `rust/src/menu.rs` | Menüs, Klicks, Inventarzustand |
| `rust/src/items.rs` | Slot-Komponenten, Name, Farbe, Lore |
| `rust/src/pov.rs` | Chunks, Blocks, Entities und Renderer |
| `rust/src/antiafk.rs` | Anti-AFK |
| `rust/src/movement.rs` | manuelle Bewegung/Routen |
| `rust/tests/common/mod.rs` | Nachbau eines Minecraft-Servers für die Tests |
| `rust/tests/live.rs` | Ende-zu-Ende-Tests gegen diesen Server, mit dem echten Programm |

## Woher Paket-IDs und Feldreihenfolgen kommen

Alle Paket-IDs und versionsabhängigen Feldreihenfolgen stammen aus der Registrierungsreihenfolge
und den Lese-/Schreibcodecs der jeweils gepinnten MCProtocolLib-Fassung. Unterschiede liegen in
`rust/src/proto.rs` und an den dokumentierten Parserzweigen. Tests prüfen unter anderem eindeutige
Paket-IDs, Textformatierung, Item-Komponenten, Chunk-Paletten und Koordinaten-Packing.

Dazu kommen Ende-zu-Ende-Tests: `rust/tests/` startet die **wirklich gebaute Datei** gegen einen
nachgebauten Server und prüft Beitritt, Chat, Befehle, Teleportbestätigung, gemeldete Sichtweite,
das Bildformat der Live-POV und das Verhalten bei bösartigen Chunk-Daten. `cargo test --features
ultra` deckt damit alles ab, was ein Server tatsächlich schickt.
