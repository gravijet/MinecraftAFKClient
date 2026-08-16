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
- Client-Information, Cookies und ab 1.21.11 den Verhaltenskodex beantworten;
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
ungefragt das Terminal übernimmt.

| Befehl | Wirkung |
| --- | --- |
| `:pov live` | laufendes Bild starten |
| `:pov stop` | Rendering stoppen |
| `:pov frame` | ein einzelnes aktuelles Bild |
| `:pov size <breite> <höhe>` | interne Auflösung 24–160 × 12–80 |
| `:pov info` | Dimension, Welthöhe, Chunk-/Entity-Anzahl |

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

## Woher Paket-IDs und Feldreihenfolgen kommen

Alle Paket-IDs und versionsabhängigen Feldreihenfolgen stammen aus der Registrierungsreihenfolge
und den Lese-/Schreibcodecs der jeweils gepinnten MCProtocolLib-Fassung. Unterschiede liegen in
`rust/src/proto.rs` und an den dokumentierten Parserzweigen. Tests prüfen unter anderem eindeutige
Paket-IDs, Textformatierung, Item-Komponenten, Chunk-Paletten und Koordinaten-Packing.
