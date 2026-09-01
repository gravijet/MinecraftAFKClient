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
| POV | `pov-afk-windows.exe`, `pov-afk-linux` | `pov-client` | Live-POV, Browser-HUD, Menüs und Gegenstände |
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
| Kick/Verbindungsabbruch | Neuverbindung mit wachsender Wartezeit; `--no-reconnect` beendet stattdessen mit Status 1 |

Der Transfer ist absichtlich vom Reconnect nach einem Kick getrennt: Beim Transfer weist der
bereits verbundene Server den Client ausdrücklich an, innerhalb derselben Sitzung ein neues Ziel
zu öffnen; die Sitzung geht mitsamt ihren Cookies weiter. Ein Kick oder gewöhnlicher Netzabbruch
beginnt dagegen eine neue Sitzung – die abgelegten Cookies werden dabei verworfen, weil ein
frischer Login sie beim Server neu abfragt.

#### Neuverbinden

| Schalter | Wirkung |
| --- | --- |
| *(ohne Angabe)* | an: erster Versuch nach 5 s, Verdopplung bis 60 s, unbegrenzt viele Versuche |
| `--no-reconnect` | aus; der Prozess endet nach dem Abbruch mit Status 1 |
| `--reconnect-delay <s>` | Wartezeit vor dem ersten Versuch (Standard 5) |
| `--max-backoff <s>` | Obergrenze der Wartezeit (Standard 60) |
| `--reconnect-tries <n>` | nach `n` erfolglosen Versuchen aufgeben; `0` = unbegrenzt |

Die Wartezeit verdoppelt sich mit jedem erfolglosen Versuch und wird bei der Obergrenze gekappt –
ein Server, der gerade neu startet, wird also nicht im Sekundentakt angeklopft, und einer, der
nur kurz gestolpert ist, ist nach fünf Sekunden wieder da. Der Zähler springt auf null zurück,
sobald der Client die Spielphase erreicht hat; eine Sitzung, die nach zwei Stunden abbricht,
beginnt wieder bei 5 s statt bei der zuletzt erreichten Obergrenze.

`--no-reconnect` gewinnt unabhängig von der Reihenfolge auf der Kommandozeile: Wer
`--reconnect-delay 10 --no-reconnect` schreibt, bekommt keinen Reconnect. Umgekehrt schaltet jede
der drei Feineinstellungen den Reconnect ein, falls er nicht ausdrücklich abgeschaltet wurde.

Jeder Versuch meldet sich als `@event reconnect versuch=<n> in=<sekunden>`, damit ein Panel den
Zustand mitbekommt, ohne die Fehlerausgabe mitzulesen.

### Protokollbasierter Kick-Schutz

- `KeepAlive` sofort beantworten;
- `Ping` mit `Pong` beantworten;
- Server-Teleports bestätigen und die Position zurückspiegeln;
- **jeden Chunk-Stapel bestätigen** (`ChunkBatchReceived`) mit dem Durchsatz, den Vanillas
  `ChunkBatchSizeCalculator` errechnet. Ohne diese Antwort zählt der `PlayerChunkSender` des
  Servers die offenen Stapel hoch und **stellt bei zehn die Chunk-Auslieferung ein**;
- **das Ende der Ladephase melden** (`PlayerLoaded`, ab 1.21.4), sobald Startposition und erster
  Chunk-Stapel da sind. Ohne diese Meldung hält der Server den Spieler bis zu 30 Sekunden in
  einem Schwebezustand;
- **die eigene Position alle 20 Ticks erneut melden**, auch im Stillstand – genau das tut
  `LocalPlayer.sendPosition()`. Ein Client, der nach dem Beitritt gar kein Positionspaket mehr
  schickt, sieht auf dem Server anders aus als jeder echte Spieler;
- ab 1.21.2 jeden 50-ms-Tick mit `ClientTickEnd` abschließen;
- erzwungene Resource-Packs bestätigen, aber nicht laden;
- Client-Information und ab 1.21.11 den Verhaltenskodex beantworten;
- Cookies ablegen und beantworten – auch über einen Server-Transfer hinweg, denn genau dafür
  gibt es sie (Server A legt eines ab, Server B fragt es beim Login ab);
- signierte Chat-Nachrichten quittieren;
- nach Tod automatisch respawnen.

Die ausgehende zlib-Kompression läuft auf derselben Stufe wie in Vanilla (`new Deflater()`, also
die zlib-Vorgabe). Das ist keine Geschmacksfrage: Die Stufe steht in den FLEVEL-Bits jedes
gesendeten zlib-Kopfes.

Das ist vom optionalen Anti-AFK getrennt. Der normale Client läuft nicht umher und dreht sich
nicht; er meldet nur, wie jeder Client, weiterhin dieselbe Position.

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

Menü-Klicks sind in Items, Premium, Premium + Items, POV und Ultra enthalten. Premium ohne Items kennt
Fensternummer, Titel, Zustandszähler und Feldanzahl, hält aber bewusst keine Gegenstandsdaten.

| Befehl | Items | Premium | Premium + Items | POV | Ultra |
| --- | ---: | ---: | ---: | ---: | ---: |
| `:menu` | Inhalt | Metadaten | Inhalt | Inhalt | Inhalt |
| `:click <feld> [rechts\|shift]` | ja | ja | ja | ja | ja |
| `:close` | ja | ja | ja | ja | ja |
| `:slot <feld>` | ja | nein | ja | ja | ja |
| `:inv` | ja | nein | ja | ja | ja |

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

### Texturierte Browser-POV

Mit `--pov-web <port>` startet zusätzlich ein lokaler Viewer mit **echten Minecraft-Texturen**.
Die Client-JAR dazu sucht sich der Client selbst; `--pov-resources` braucht es nur, wenn eine
bestimmte Datei benutzt werden soll (siehe unten). Gelesen wird aus der zur gewählten
`--mc`-Version passenden Original-Client-JAR:

- Blockstates und vererbte Blockmodelle, einschließlich Varianten, Multipart-Teilen,
  Modellelementen und deren Drehungen;
- echte Block-PNGs samt Alphakanal und `tintindex`;
- Crosshair, Hotbar, Auswahlrahmen und Container-Hintergründe aus `textures/gui`;
- Item-/Blockicons für Inventar und Menüfelder.

Der Browser zeigt die POV pixelgenau skaliert, das eigene Inventar in der Hotbar und ein geöffnetes
Vanilla-Menü über der Welt. Links-, Rechts- und Shift-Klick werden als echte Containerklicks an den
Server gesendet; ein Klick außerhalb schließt das Fenster. Der Viewer benutzt kein CDN und keine
kopierten Texturdateien. Die beim Start ausgegebene URL enthält einen zufälligen Zugriffstoken.

```bash
pov-afk-linux mc.example.net --mc 26.2 --pov-web 8765
```

#### Woher die Texturen kommen

Die Binary enthält **keine** Minecraft-PNGs und wird auch keine enthalten. Sie liest sie aus einer
originalen Client-JAR und sucht die in dieser Reihenfolge:

1. den Pfad aus `--pov-resources <datei>`, falls angegeben;
2. die eigene Ablage `<konfigverzeichnis>/assets/<version>.jar`;
3. eine vorhandene Minecraft-Installation auf dem Rechner – der offizielle Launcher unter
   `.minecraft/versions/<version>/<version>.jar` (Windows, Linux, macOS, Flatpak) sowie die
   Bibliotheksablage von Prism/MultiMC;
4. der Download von Mojang über dasselbe öffentliche Versionsmanifest, aus dem sich auch der
   Launcher bedient. Geprüft wird die dort genannte SHA-1-Summe; die Datei landet unter (2) und
   wird ab dann wiederverwendet.

Das sind einmalig rund 30 MB. Wer das nicht will, schaltet es mit `--pov-resources aus` ab – dann
läuft der Viewer ohne Texturen weiter. Das Suchen und Einlesen läuft in einem eigenen Thread: der
Client verbindet sich sofort, und der Browser zeigt so lange an, was gerade passiert.

Nur eine Portnummer bindet an `127.0.0.1`; für einen bewusst extern erreichbaren Viewer kann eine
IP mitgegeben werden. Ist `--pov-web` gesetzt, bleibt der alte ANSI-Dauerstrom standardmäßig aus,
damit nicht zwei Renderer parallel arbeiten. `--pov an` bzw. `:pov live` schaltet ihn bei Bedarf
zusätzlich ein. Die komprimierte State-ID-Zuordnung stammt aus Mojangs offiziellen
Server-Reports; Herkunft und Regeneration stehen in `rust/data/README.md`.

#### Einfärbung

Modelle sagen über `tintindex`, **dass** eine Fläche eingefärbt wird; **womit**, steht in Vanilla
im Code (`BlockColors`) und hängt am Biom. Genau dieses Biom liest die Ansicht jetzt mit: Jeder
Chunk-Abschnitt bringt neben der Block- auch eine **Biom-Palette** (ein Wert je 4×4×4-Zelle), und
der Strahl schlägt am Treffer nach, in welchem Biom er steht.

Die Farbe zu einem Biom entsteht so, wie Vanilla sie bildet:

1. Nennt die Biom-Registry des Servers eine feste Farbe (`effects.grass_color`,
   `foliage_color`, `water_color`), gilt die. Datenpakete setzen das häufig.
2. Sonst wird `colormap/grass.png` bzw. `foliage.png` aus der Client-JAR an der Stelle abgetastet,
   die sich aus `temperature` und `downfall` des Bioms ergibt – dieselbe Rechnung wie in
   `ColorMapColorUtil.get`, samt der Multiplikation von `downfall` mit `temperature`.
3. Danach greift `grass_color_modifier`: `swamp` setzt einen Festwert, `dark_forest` mischt den
   Grundton mit `#28340A`.

Blöcke ohne bekannten Einfärber bleiben ungefärbt; die Festwerte, die Vanilla unabhängig vom Biom
setzt (Fichten- und Birkenlaub, Seerosen), bleiben fest.

Was **nicht** nachgebaut wird, ist Vanillas Mittelung über die Nachbarschaft: Gezeichnet wird die
Farbe des getroffenen Bioms, nicht der Durchschnitt eines 3×3-Chunk-Fensters. An einer Biomgrenze
gibt es also eine harte Kante statt eines weichen Übergangs. Der Unterschied ist eine Kante
gegenüber einem Verlauf – vorher war es der falsche Farbton in jedem Biom außer der Ebene.

Der Aufwand dafür ist klein: Die Farben aller Biome werden einmal beim Empfang der Registry
ausgerechnet und liegen als flache Tabelle bereit; je getroffenem Bildpunkt bleibt ein
Feldzugriff. Ein Abschnitt, der nur ein Biom enthält – der Regelfall – behält genau einen Wert
statt 64.

Flüssigkeiten haben in Vanilla ein Modell **ohne Flächen** – `block/water.json` nennt nur die
Partikeltextur, weil das Spiel Wasser und Lava eigens zeichnet. Genau diese Partikeltextur wird
hier als Fläche benutzt.

#### Licht

Vorher waren alle Höhlen genauso hell wie die Oberfläche, und ein Fackelschein war nirgends zu
sehen: Die Helligkeit einer Fläche hing allein an ihrer Ausrichtung. Jetzt kommt sie aus dem
Licht, das der Server ohnehin mitschickt – im Chunk-Paket direkt hinter den Blockdaten und danach
in `LightUpdate`, wenn sich etwas ändert.

Gelesen werden Himmels- und Blocklicht getrennt, als je ein Nibble pro Block. Für einen Treffer
wird das Licht **des Nachbarblocks vor der getroffenen Fläche** genommen – so, wie das Spiel es
auch tut; das Licht im Block selbst ist bei einem festen Block null. Aus dem größeren der beiden
Werte wird die Vanilla-Kurve `f / (4 - 3f)` gebildet und mit der Flächenausrichtung multipliziert.
Ein Restwert von 0,06 bleibt stehen, damit ein unbeleuchteter Block noch als Umriss erkennbar ist
statt als schwarze Fläche.

Der Speicher dafür wird nicht einfach hingenommen: Roh sind das 2 × 2048 Byte je Abschnitt, bei
24 Abschnitten also 96 KB je Chunk. Fast alle davon sind aber gleichförmig – tief unter Tage
überall 0, hoch über dem Boden überall 15. Ein Abschnitt mit nur einem Wert wird deshalb als
dieser eine Wert abgelegt. Nachgemessen am selben Arbeitspunkt wie oben:

| | Resident |
| --- | --- |
| ohne Licht | 7 984 K |
| mit Licht, verdichtet | 8 256 K |
| mit Licht, ohne Verdichtung | 15 208 K |

Die Verdichtung ist also nicht Kosmetik, sondern der Grund, warum das Feature überhaupt
vertretbar ist: 272 KB statt 7,2 MB.

Licht ist strikt additiv. Ein Server, der keins schickt, ein Paket in unerwarteter Form oder ein
Abschnitt ohne Lichtdaten kosten weder den Chunk noch die Verbindung – die Ansicht fällt dann
genau auf die frühere Flächenschattierung zurück. `:pov info` nennt deshalb neben der Chunk-Zahl,
wie viele davon Licht mitbringen.

### Terminal-Fallback

| Befehl | Wirkung |
| --- | --- |
| `:pov live` | laufendes Bild starten |
| `:pov stop` | Rendering stoppen |
| `:pov frame` | ein einzelnes aktuelles Bild |
| `:pov size <breite> <höhe>` | interne Auflösung 24–160 × 12–80 |
| `:pov fps <n>` | Bilder je Sekunde, 1–20 |
| `:pov info` | Dimension, Welthöhe, Chunk-/Entity-Anzahl, wie viele Chunks Licht mitbringen |

Dieselben Einstellungen gibt es als Startargument, damit ein Panel sie setzen kann, **bevor** das
erste Bild rausgeht: `--pov an|aus`, `--pov-size <breite>x<höhe>`, `--pov-fps <n>`. Ohne Angabe
zeichnet der POV-Client mit 64x32 – wer eine andere Größe will, muss sie beim Start mitgeben oder
vor `:pov live` setzen.

Die POV liest tatsächlich die Serverpakete:

- Dimension-Registry für `min_y` und Welthöhe;
- Biom-Registry für Temperatur, Niederschlag und feste Farbwerte;
- vollständige Chunk-Abschnitte und deren kompakte Block- und Biom-Paletten;
- **Himmels- und Blocklicht** aus dem Chunk-Paket und aus `LightUpdate`;
- einzelne und abschnittsweise Blockänderungen sowie Chunk-Unloads;
- Spawn, Bewegung, Teleport und Entfernen von Entities;
- aktuelle eigene Kameraposition, Yaw und Pitch.

Das Bild entsteht über Raycasts aus der First-Person-Kamera, mit Tiefenverdeckung, echtem Licht und
Distanznebel. Es folgt Server-Teleports sowie `:look` und `:go` live. Minecraft schickt einem
headless Protokollclient keine fertigen Frames. Der Terminal-Fallback bleibt deshalb eine farbige
Voxelansicht; der Browser ergänzt die fehlenden Modelle und Texturen aus der Original-Client-JAR.

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
| jetzt | 8 256 K |

Im heutigen Wert sind Licht und Biome bereits enthalten; die Aufschlüsselung dazu steht oben unter
[Licht](#licht). Der Messlauf steht als Test im Baum und lässt sich nachfahren:

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

### Rechenzeit je Bild

Das Zeichnen selbst ist der teuerste Teil der Live-Ansicht – vor allem in der Browser-Auflösung,
wo je Bild rund hunderttausend Strahlen laufen. Zwei Änderungen daran:

* **Abschnitte werden einmal nachgeschlagen, nicht je Block.** Der Strahl merkt sich, wie viele
  Blockschritte je Achse noch im aktuellen 16er-Abschnitt bleiben, und läuft ihn dann ohne jede
  weitere Suche ab. In reiner Luft – bei einem Blick in den Himmel also fast überall – fällt damit
  die gesamte Nachschlagearbeit weg. Die Schrittfolge bleibt dabei Byte für Byte dieselbe; ein
  Test vergleicht über zehntausend Strahlen gegen die Lehrbuchfassung.
* **Große Bilder laufen über mehrere Kerne**, zeilenweise auf Zuruf verteilt. Feste Bänder wären
  hier falsch: Ein Strahl in den Himmel läuft die vollen 72 Blöcke ab, einer auf den Boden vor den
  Füßen ist nach drei Schritten fertig – die Wanduhr richtet sich sonst nach dem langsamsten Band.
  Terminalbilder (höchstens 160×80) bleiben einthreadig; dort kostet das Aufteilen mehr, als es
  spart.

Gemessen auf einem stark gedrosselten Testrechner, der selbst mit vier Prozessen nur den
1,37-fachen Durchsatz eines einzelnen erreicht – auf einem echten Vierkerner fällt der zweite
Schritt entsprechend deutlicher aus:

| Bildgröße | vorher | nur Abschnittssprung | dazu mehrkernig |
| --- | --- | --- | --- |
| 64×32 (Terminal) | 3,1 ms | 2,8 ms | 2,8 ms (bewusst einthreadig) |
| 160×80 (Terminal) | 18 ms | 15 ms | 15 ms (bewusst einthreadig) |
| 426×240 (Browser) | 141 ms | 110 ms | 77 ms |

Licht und Biomfarben kamen danach dazu und kosten je Treffer einen Nachschlag mehr. Derselbe
Messlauf auf einem *nicht* gedrosselten Rechner ergibt damit 2,2 ms, 12,7 ms und 35,4 ms – die
Zahlen der Tabelle sind also nicht direkt vergleichbar, aber die Reihenfolge stimmt weiter. Der
Grund, dass es nicht teurer wurde: Die Biomfarben liegen als fertige Tabelle bereit (eine
Nachschlagestelle je Bildpunkt, nicht eine Rechnung), und ein gleichförmiger Lichtabschnitt
antwortet ohne Speicherzugriff auf das Nibble-Feld.

```bash
cd rust && cargo test --release --features ultra -- --ignored --nocapture messlauf_bildrate
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
   └─ pov ── pov-client (= pov + items + state)

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

Geraten wird dabei nicht. Beispiel `LightUpdate`: Die Serverpakete werden in
`MinecraftCodec.CODEC` alphabetisch nach Registrierungsnamen eingetragen, und `light_update` steht
zwischen `level_particles` und `login`. Also gilt in **jeder** der vier Tabellen
`cb_light_update == cb_login - 1 == cb_level_chunk + 3` – nachgerechnet ergibt das 42, 47 und 48.
Ein Test hält diese Beziehung fest, damit sie beim nächsten Protokoll auffällt, statt still falsch
zu werden.

Dazu kommen Ende-zu-Ende-Tests: `rust/tests/` startet die **wirklich gebaute Datei** gegen einen
nachgebauten Server und prüft Beitritt, Chat, Befehle, Teleportbestätigung, gemeldete Sichtweite,
das Bildformat der Live-POV und das Verhalten bei bösartigen Chunk-Daten. `cargo test --features
ultra` deckt damit alles ab, was ein Server tatsächlich schickt.
