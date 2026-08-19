# Welche Release-Datei ist welche?

Alle neuen Zusatzvarianten in dieser Tabelle sind **ausschließlich Rust-Clients**. Jede Rust-Datei
spricht Minecraft `1.21.1`, `1.21.11`, `26.1` und `26.2`; die gewünschte Version wird beim Start
mit `--mc <version>` gewählt. Windows-Dateien enden auf `.exe`, Linux-Dateien haben keine Endung.

## Rust-Dateien

| Windows | Linux | Enthaltene Funktionen | Gedacht für |
| --- | --- | --- | --- |
| `afk-windows.exe` | `afk-linux` | Verbindung, Chat, Befehle/Makros, Kick-Schutz | möglichst kleiner Client zum bloßen AFK-Stehen |
| `afk-windows-move.exe` | `afk-linux-move` | normal plus `:go`, `:look`, `:home`, `:route`, Sprung/Fall | AFK mit manuell gesteuerter Bewegung |
| `items-afk-windows.exe` | `items-afk-linux` | normal plus Menü-Klicks sowie Menü- und Inventargegenstände mit Anzahl, Name, Farbcodes und Lore | schlanker Client, bei dem Inventarinhalte sichtbar sein müssen |
| `premium-afk-windows.exe` | `premium-afk-linux` | Bewegung, farbiges Scoreboard, Menü-Klicks, Schleichen/Sprinten, Benutzen/Handwechsel, Anti-AFK | vollständige AFK-Steuerung ohne Gegenstandsdaten und POV-Weltspeicher |
| `premium-items-afk-windows.exe` | `premium-items-afk-linux` | Premium plus Menü- und Inventargegenstände mit Namen, Farben und Lore | Premium-Steuerung mit sichtbaren Gegenständen |
| `pov-afk-windows.exe` | `pov-afk-linux` | eigene Live-POV-Datei; lädt Chunk-, Block- und Entity-Daten und startet nach dem Beitritt automatisch die First-Person-Terminalansicht | beobachten, was der angemeldete Spieler aktuell sieht |
| `ultra-afk-windows.exe` | `ultra-afk-linux` | alles aus Premium + Gegenstände + POV; POV wird mit `:pov live` zugeschaltet | eine Datei mit allen Rust-Funktionen |

Keine dieser Rust-Dateien enthält eine Tablist oder Playerlist. Nach einem Kick wird **nicht**
automatisch neu verbunden; der Prozess beendet sich mit Fehlerstatus. Ein vom Server ausdrücklich
angeordneter Transfer auf einen Unterserver wird weiterhin befolgt, weil er Teil derselben
Spielsitzung ist.

## POV bedienen

Der POV-Client beginnt automatisch (mit 64x32), der Ultra-Client erst auf Befehl. Beide Vorgaben
lassen sich beim Start umstellen:

| Startargument | Wirkung |
| --- | --- |
| `--pov an` / `--pov aus` | Ansicht gleich nach dem Beitritt starten bzw. eben nicht |
| `--pov-size 160x80` | Bildgröße von Anfang an, ohne den Umweg über `:pov size` |
| `--pov-fps 4` | Bilder je Sekunde (1–20, Standard 8) |

Örtliche Befehle im laufenden Client:

| Befehl | Wirkung |
| --- | --- |
| `:pov live` | laufende Ansicht starten |
| `:pov stop` | laufende Ansicht stoppen |
| `:pov frame` | genau ein aktuelles Bild zeichnen |
| `:pov size 80 40` | interne Bildgröße setzen (24–160 × 12–80 Pixel) |
| `:pov fps 4` | Takt ändern |
| `:pov info` | Dimension, Welthöhe, Chunk-/Entity-Zahl und Zustand anzeigen |

Die Bilder gehen auf die **Standardfehlerausgabe**; das genaue Format steht in
[FEATURES.md](FEATURES.md#bildformat-der-live-pov) und ist als Schnittstelle zugesagt.

Die Ansicht ist kein Textdump von Koordinaten: Der Client decodiert die tatsächlich geladenen
Chunk-Paletten, hält Blockänderungen und Entities live nach und raycastet das Bild aus der aktuellen
Kameraposition und Blickrichtung. Minecraft überträgt dabei keine fertigen Bildschirmbilder oder
Blocktexturen; deshalb rendert diese headless Datei eine farbige Voxelansicht im Terminal.

## Gegenstände und Scoreboard

`items-afk-*`, `premium-items-afk-*` und `ultra-afk-*` bieten:

- `:menu` für das offene Menü samt belegten Feldern;
- `:slot <nummer>` für Name, Anzahl, Registry-ID und Lore eines Menüfelds;
- `:inv` für das eigene Inventar;
- `@event slot ...` und `@event lore ...` mit unveränderten `§`-Farbcodes bei `--events`.

Unveränderte Vanilla-Items erhalten ihren echten Ressourcenname aus der zur gewählten
Minecraft-Version gehörenden Item-ID-Liste; sie fallen daher nicht bloß auf eine Nummer zurück.

Premium und Ultra bieten `:board`. Titel, fertige Anzeigenamen, Team-Präfix/-Farbe/-Suffix sowie
die Zahlenformate der Zeilen bleiben erhalten. Dazu gehören farbige Punktwerte, fester Text und
ausgeblendete Zahlen. Mit `--events` werden sie zusätzlich als `@event board ...` mit `§`-Codes
ausgegeben.

## Java-Dateien

`afk-1.21.1.jar`, `afk-1.21.11.jar`, `afk-26.1.jar` und `afk-26.2.jar` sind die bisherigen
Java-Clients, jeweils für genau die im Dateinamen genannte Version. Die neuen Items-, POV-,
Premium-Items- und Ultra-Varianten gibt es bewusst nur für Rust.

## Startbeispiele

```powershell
.\items-afk-windows.exe mc.example.net --mc 26.1 --offline Testkonto
.\pov-afk-windows.exe mc.example.net --mc 26.2 --account MeinKonto
.\ultra-afk-windows.exe mc.example.net --mc 1.21.11 --account MeinKonto --antiafk 60
```

```bash
./items-afk-linux mc.example.net --mc 26.1 --offline Testkonto
./pov-afk-linux mc.example.net --mc 26.2 --account MeinKonto
./ultra-afk-linux mc.example.net --mc 1.21.11 --account MeinKonto --antiafk 60
```

Alle Kommandozeilenoptionen zeigt `<datei> --help`; alle in der jeweiligen Bauform enthaltenen
örtlichen `:`-Befehle zeigt während des Betriebs `:help`.
