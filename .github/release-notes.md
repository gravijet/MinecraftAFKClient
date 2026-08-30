## Welche Datei ist welche?

Die Zusatzvarianten sind ausschließlich Rust-Clients. Jede Rust-Datei unterstützt Minecraft
`1.21.1`, `1.21.11`, `26.1` und `26.2`; ausgewählt wird mit `--mc <version>`.

| Windows | Linux | Enthaltene Funktionen |
| --- | --- | --- |
| `afk-windows.exe` | `afk-linux` | kleiner Basisclient: Verbindung, Chat, Befehle und Makros |
| `afk-windows-move.exe` | `afk-linux-move` | Basisclient plus manuelle Bewegung, Blickrichtung, Routen und Sprung/Fall |
| `items-afk-windows.exe` | `items-afk-linux` | Basisclient plus Menü-Klicks und sichtbare Menü-/Inventargegenstände mit Anzahl, Name, Farbcodes und Lore |
| `premium-afk-windows.exe` | `premium-afk-linux` | Bewegung, formatiertes Scoreboard, Menü-Klicks, Tastenzustand und Anti-AFK; ohne Gegenstandsdaten und POV-Weltspeicher |
| `premium-items-afk-windows.exe` | `premium-items-afk-linux` | Premium plus sichtbare Menü- und Inventargegenstände mit Namen, Farben und Lore |
| `pov-afk-windows.exe` | `pov-afk-linux` | Live-POV mit Terminal-Fallback, texturiertem Browser-HUD, Menüs und Gegenständen |
| `ultra-afk-windows.exe` | `ultra-afk-linux` | alle Rust-Funktionen in einer Datei: Premium, Items und mit `:pov live` zuschaltbare POV |

Die vier Java-Dateien bleiben getrennt nach Protokollversion: `afk-1.21.1.jar`,
`afk-1.21.11.jar`, `afk-26.1.jar` und `afk-26.2.jar`. Items-, POV-, Premium-Items- und
Ultra-Varianten gibt es bewusst nur für Rust.

---

# Rust-Client 2.5.0 – texturierte Live-POV und Projekt-Audit

Stand: 30. August 2026

## Ergebnis

Die Live-POV hat jetzt zusätzlich zur bisherigen Terminalausgabe einen eigenen lokalen
Browser-Viewer. Dieser Viewer verwendet keine Hash-/Kontrastfarben und keine generierten
Ersatzbilder. Blocktexturen, Blockmodelle, HUD-Elemente, Container-Hintergründe und direkte
Itemtexturen werden unverändert aus einer echten Minecraft-Client-JAR gelesen.

Die alte ANSI-POV bleibt absichtlich erhalten, weil bestehende Panels ihr festes Ausgabeformat
einlesen. Sobald `--pov-web` verwendet wird, läuft sie nicht mehr automatisch parallel. Der
Browser holt nur dann Frames, wenn er geöffnet ist.

## Schnellstart

Voraussetzungen:

1. `pov-afk-*` oder `ultra-afk-*` aus diesem Stand verwenden.
2. Im offiziellen Minecraft-Launcher genau die mit `--mc` gewählte Version einmal installieren.
3. Den Pfad zu deren Client-JAR an `--pov-resources` übergeben.

Linux:

```bash
./pov-afk-linux mc.example.net --mc 26.2 \
  --pov-web 8765 \
  --pov-resources "$HOME/.minecraft/versions/26.2/26.2.jar"
```

Windows PowerShell:

```powershell
.\pov-afk-windows.exe mc.example.net --mc 26.2 `
  --pov-web 8765 `
  --pov-resources "$env:APPDATA\.minecraft\versions\26.2\26.2.jar"
```

Beim Start erscheint zum Beispiel:

```text
Browser-POV: http://127.0.0.1:8765/?token=4f4d9b2a6c6e6f18a71c0841346ea6c0
```

Diese vollständige URL im Browser öffnen. Der Token wird bei jedem Start zufällig erzeugt. Ohne
ihn können andere lokale Webseiten weder den Weltzustand lesen noch Menü-/Hotbar-Klicks senden.

Für `1.21.1`, `1.21.11` oder `26.1` müssen `--mc`, Versionsordner und JAR-Dateiname entsprechend
zusammenpassen. Eine 26.2-JAR zu `--mc 1.21.1` ist keine unterstützte Mischung: Modelle können
sich geändert haben, auch wenn einzelne Texturen zufällig gleich heißen. Bei einer offiziellen
Client-JAR wird die ID aus `version.json` geprüft und eine Abweichung mit einer klaren Meldung
abgelehnt.

## Neue Startoptionen

| Option | Bedeutung |
| --- | --- |
| `--pov-web 8765` | bindet den Viewer an `127.0.0.1:8765` |
| `--pov-web 127.0.0.1:8765` | dasselbe mit vollständiger Adresse |
| `--pov-web 0.0.0.0:8765` | macht den Viewer bewusst im Netzwerk erreichbar; Token geheim halten |
| `--pov-resources <client.jar>` | passende Original-Client-JAR mit Modellen, Texturen und GUI |
| `--pov an` | startet zusätzlich den alten ANSI-Dauerstrom |
| `--pov aus` | lässt den ANSI-Dauerstrom aus; der Browser funktioniert trotzdem |

`--pov-size` und `--pov-fps` betreffen weiterhin die Terminal-POV. Die mitgelieferte Browserseite
fordert 426×240 Pixel an und skaliert sie mit `image-rendering: pixelated` auf die Fenstergröße.
Die HTTP-Schnittstelle akzeptiert 160–640 × 90–360 Pixel, höchstens 230.400 Pixel je Frame.

## Bedienung im Browser

- Das Fadenkreuz, die Hotbar, der Auswahlrahmen und die Menü-Hintergründe kommen aus der
  angegebenen Minecraft-JAR.
- Die Hotbar zeigt die Felder 36–44 des eigenen Inventars. Ein Klick wählt das Feld beim Server.
- Öffnet der Server ein Menü, erscheint der zur `minecraft:menu`-ID passende Vanilla-Hintergrund.
- Linksklick auf einen Slot sendet einen normalen Containerklick.
- Rechtsklick sendet einen rechten Containerklick; das Browser-Kontextmenü wird dabei unterdrückt.
- Umschalt+Linksklick sendet einen Shift-Klick.
- Ein Klick in den abgedunkelten Bereich außerhalb des Fensters sendet `ContainerClose`.
- Namen, Anzahl und Lore kommen aus den echten Container-/Item-Paketen. Tooltips werden als Text
  gezeigt; Serverfarben bleiben in der Terminal-/Event-Schnittstelle weiterhin vollständig
  erhalten.

Unterstützte Vanilla-Hintergründe bzw. Layouts sind generische 9×N-Container, 3×3-Container,
Crafter, Amboss, Leuchtfeuer, Ofen, Schmelzofen, Räucherofen, Braustand, Werkbank,
Verzauberungstisch, Schleifstein, Trichter, Webstuhl, Händler, Shulkerkiste, Schmiedetisch,
Kartentisch und Steinsäge. Die Zuordnung verwendet die versionsgeprüfte `minecraft:menu`-Registry,
nicht die Anzahl der Slots als geratenen Fenstertyp.

## Was am Weltbild geändert wurde

### Versionsgenaue Block-State-Zuordnung

Chunk-Pakete enthalten nur globale numerische State-IDs. Die Client-JAR enthält dagegen JSONs und
PNGs unter Ressourcennamen. Neu eingebunden sind deshalb vier komprimierte Tabellen:

| Version | State-IDs |
| --- | ---: |
| 1.21.1 | 26.684 |
| 1.21.11 | 29.671 |
| 26.1 | 29.873 |
| 26.2 | 32.366 |

Sie wurden mit `rust/data/generate-block-states.sh` aus `generated/reports/blocks.json` der
offiziellen Mojang-Server-JAR der jeweiligen Version erzeugt. IDs sind lückenlos sortiert; jede
Zeile enthält außerdem Mojangs `default`-Markierung. Dadurch verwenden Blockitems den wirklichen
Default-State und nicht fälschlich die kleinste State-ID – bei 544 bis 661 Blocks je nach Version
ist das nicht dasselbe. Es gibt keine von Hand gepflegte oder geschätzte ID-Liste. Die vier
komprimierten Dateien benötigen zusammen rund 640 KB statt etwa 10 MB Rohtext.

### Originale Resource-Dateien

Der neue Lader verarbeitet:

- `assets/<namespace>/blockstates/*.json`;
- `variants` und bedingte `multipart`-Modelle;
- Model-Arrays deterministisch mit der ersten Vanilla-Variante;
- rekursive `parent`-Vererbung und Texture-Variablen wie `#all`, `#side` oder `#cross`;
- `elements`, `from`/`to`, Elementrotationen sowie Blockstate-X/Y-Rotationen;
- explizite und automatisch abgeleitete UV-Koordinaten;
- Face-UV-Rotationen;
- PNGs als RGB, RGBA, Graustufen oder Palette;
- Alphatest: vollständig transparente Texel blockieren den Strahl nicht;
- `tintindex` mit einem neutralen Overworld-Grün;
- den ersten Frame senkrecht gestapelter animierter Texturen.

Damit belegen Slabs, Treppen, Zaunteile und gedrehte Pflanzenkreuze im Browser nicht mehr pauschal
einen ganzen Farbwürfel. Der Raycast trifft die tatsächlichen Modellelemente und tastet am
Trefferpunkt die echte Textur ab.

### Itemicons

Direkte `textures/item/*.png` werden unverändert ausgeliefert. Blockitems ohne flache Item-PNG
werden aus den echten Modellelementen und Blocktexturen als transparentes 16×16-GUI-Modell
gerastert. Fertige Icons liegen in einem kleinen Laufzeitcache; ein unverändertes Menü rendert sie
nicht bei jeder Zustandsabfrage neu.

### Himmel, Nebel und Flächenlicht

Die Zentralprojektion, Tiefenverdeckung und der vorhandene Distanznebel bleiben erhalten. Das
Flächenlicht wird nicht mehr auf Hashfarben, sondern auf die abgetasteten Texturpixel angewendet.
Die Browseransicht zeichnet absichtlich keine pinken oder türkisen Entity-Rechtecke mehr.

## Interne HTTP-Schnittstelle

Die mitgelieferte Seite verwendet folgende Endpunkte. Ausnahmslos jeder Aufruf – einschließlich
der PNGs unter `/assets/` – braucht den Token aus der Start-URL.

| Endpunkt | Zweck |
| --- | --- |
| `GET /?token=…` | vollständige lokale Viewer-Seite |
| `GET /api/state.json?token=…` | Position, Dimension, Chunkzahl, Menü, Inventar, Hotbar |
| `GET /api/frame.png?token=…&w=426&h=240` | aktueller texturierter POV-Frame |
| `GET /api/item.png?token=…&id=N` | Item-/Blockicon zur Registry-ID |
| `POST /api/click?token=…&slot=N&action=left\|right\|shift` | Menüklick |
| `POST /api/close?token=…` | offenes Menü schließen |
| `POST /api/hotbar?token=…&slot=0..8` | Hotbarfeld wählen |
| `GET /assets/...?token=…` | unveränderte PNG aus der angegebenen Client-JAR |

Es gibt kein CDN, keine Webfonts, keine Analytics und keine Verbindung zu einem fremden
Webdienst. HTML, CSS und JavaScript sind in der Binary enthalten. HTTP-Antworten verwenden unter
anderem eine enge `Content-Security-Policy`, `nosniff`, `DENY`, `no-referrer` und
`Connection: close`. Seite, Zustand und Frames sind `no-store`; unveränderliche JAR-PNGs und
gerenderte Itemicons dürfen der Browser dagegen privat und dauerhaft cachen. Ressourcenpfade mit
`..` oder Backslashes werden abgelehnt.

## Korrekturen aus dem Veröffentlichungs-Audit

Die neue POV wurde nicht isoliert veröffentlicht. Der gesamte Rust- und Java-Bestand, sämtliche
Feature-Kombinationen, Abhängigkeiten, Generatoren und Release-Schritte wurden erneut geprüft.

### Sicherheit und Robustheit

- JAR-Ressourcen waren in einem frühen Stand ohne Token abrufbar. Die Prüfung liegt jetzt vor
  sämtlichen Routen; der zufällige Token umfasst 128 statt 64 Bit.
- Der kleine HTTP-Server verarbeitet Anfragen in vier festen Arbeitern mit einer begrenzten
  Warteschlange. Langsame oder absichtlich offene Verbindungen können weder den Viewer seriell
  blockieren noch unbegrenzt neue Threads erzeugen. Lese- und Schreibzeitlimits, maximal 16 KiB
  Anfragekopf und HTTP-1.0/1.1-Prüfung begrenzen den Eingang; Überlast antwortet mit HTTP 503.
- JAR-Einträge sind beim Lesen auf 4 MiB für JSON und 32 MiB für Assets begrenzt. PNGs dürfen
  höchstens 4096×4096 bzw. 16.777.216 Pixel und 64 MiB decodierte Daten belegen; Größenrechnungen
  werden vor der Allokation auf Überlauf geprüft.
- Eine vorhandene `version.json` in der Client-JAR muss zu `--mc` passen. Damit wird eine falsche
  offizielle Versions-JAR nicht mehr mit zufällig passenden, aber semantisch anderen Modellen
  weiterverwendet.
- Ungültige Menüklicks, Hotbarfelder und Schließversuche liefern nicht mehr irreführend HTTP 204,
  sondern HTTP 400. Ohne geladene JAR liefern Asset-/Icon-Endpunkte HTTP 503 statt eines leeren
  Bildes.
- Serveradressen mit Port 0, leerem Host, unlesbarem Port oder unerlaubtem Text hinter einer
  geklammerten IPv6-Adresse werden in Rust und Java beim Start klar abgelehnt.

### Tempo und Leerlauflast

- Unveränderte Hotbar- und Menüzustände bauen ihre DOM-Knoten nicht mehr viermal pro Sekunde neu
  auf. JAR-PNGs und Itemicons werden nicht bei jedem Poll erneut übertragen.
- Der Browser fragt Frames mit fünf statt acht Bildern pro Sekunde ab; versteckte Tabs fallen auf
  einen Abruf pro Sekunde zurück. Nahezu gleichzeitige Anfragen mehrerer Tabs teilen sich für
  150 ms denselben Raycast und PNG-Encode. Der RGBA-Puffer wird zwischen Frames wiederverwendet.
- `--pov-resources` ohne `--pov-web` lädt die große JAR nicht mehr nutzlos und meldet die ignorierte
  Angabe. Browser und ANSI-Renderer teilen ihre vorhandenen Szenen-/Pixelpuffer und blockieren den
  Netzwerkthread beim Zeichnen nicht.
- Die sieben veröffentlichten Rust-Bauformen teilen in lokalen und GitHub-Builds nun einen
  Cargo-Zielcache. Jede fertige Datei wird vor dem nächsten Featureprofil gesichert; gemeinsame
  Abhängigkeiten werden nicht mehr siebenmal in getrennten Zielverzeichnissen kompiliert.

### Java-Client und Abhängigkeiten

- Der Java-Client startete Senderthread und Bewegungsadapter aus seinem Konstruktor; damit konnte
  die noch nicht fertig initialisierte Instanz entkommen. Beides beginnt jetzt beim ersten
  `connect`, genau einmal.
- Java löschte Transfer-Cookies unmittelbar vor dem Zielserver. Normale Reconnects beginnen
  weiterhin sauber, ein vom Server angeordneter Transfer und dessen Wiederholungen bewahren die
  Cookies jetzt wie der Rust-Client.
- Der Java-Chatfilter akzeptierte einen hohen UTF-16-Surrogate vor einem beliebigen Zeichen als
  Paar. Kaputte Hälften werden verworfen; Emoji bleiben innerhalb der Servergrenze vollständig.
- Der Java-Parser akzeptiert die neuen Rust-Schalter `--pov-web` und `--pov-resources` für
  gemeinsame Panel-Befehlszeilen. Neue JUnit-Tests decken Optionen, Ports und Chat-Sanitizing ab;
  `javac -Xlint:all -Werror` macht neue Compilerwarnungen zum Buildfehler.
- Direkte Java-Abhängigkeiten wurden auf MinecraftAuth 5.0.2, Adventure 4.26.1, Gson 2.14.0 und
  SLF4J 2.0.18 aktualisiert. Der Rust-Lockstand wurde auf alle kompatiblen aktuellen Versionen
  gebracht; `cargo machete` meldet keine unbenutzte Abhängigkeit.
- Die Release-CI verwendet die aktuellen offiziellen Hauptversionen `actions/checkout@v7` und
  `actions/setup-java@v6`. Lokale Buildskripte entfernen vor dem Java-Build gezielt alte
  `afk-*.jar`, damit ein früherer Bewegungs-Build keine veraltete JAR ins neue Paket schmuggelt.
- Die vier Java-JARs werden trotz sauberem Einzelbuild in einem eigenen Ausgabeordner gesammelt.
  Vor dem Ersetzen von `latest` vergleicht die CI alle 19 erwarteten Assetnamen exakt; ein fehlendes
  oder zusätzliches Paket lässt das alte vollständige Release unangetastet.
- `cargo audit` ist bis auf `RUSTSEC-2023-0071` sauber. Dieses Advisory betrifft zeitabhängige
  private RSA-PKCS#1-v1.5-Entschlüsselung und hat upstream für `rsa 0.9` keinen Fix. Das
  ausgelieferte Programm besitzt keinen privaten RSA-Schlüssel und verwendet RSA ausschließlich
  zur Verschlüsselung mit dem öffentlichen Serverschlüssel; private RSA-Operationen existieren nur
  im an localhost gebundenen Testserver. Genau dieser Fall ist mit Begründung ausgenommen, damit
  alle künftigen Advisories weiterhin fehlschlagen.

## Kompatibilität

- Das bisher dokumentierte ANSI-Frameformat auf stderr wurde nicht geändert.
- `:pov live|stop|frame|size|fps|info` funktioniert weiter.
- Ohne `--pov-web` startet die POV-Datei den Terminalrenderer wie bisher automatisch.
- Mit `--pov-web` bleibt der Terminalrenderer standardmäßig aus. Das verhindert doppelte CPU- und
  Ausgabelast; `--pov an` schaltet ihn ausdrücklich wieder dazu.
- Ultra startet die Terminal-POV weiterhin nur ausdrücklich. Der Browser startet, sobald
  `--pov-web` angegeben wurde.
- Die POV-Bauform enthält jetzt `items`, `menu` und `state`, damit Hotbar und echte Menüs nicht nur
  gemalt, sondern auch mit dem Server synchronisiert und bedient werden können.
- Schlanke, Movement-, Items- und Premium-Binaries ziehen `png`/`zip`, Assettabellen und
  HTTP-Viewer nicht mit hinein. Die Abhängigkeiten hängen am Cargo-Feature `pov`.

## Fehlerbilder

### `Browser-POV startet ohne Texturen`

Der Pfad fehlt, zeigt nicht auf eine ZIP/JAR oder die Datei ist nicht lesbar. `--pov-resources`
auf die echte Client-JAR der gewählten Version setzen. Der Viewer bleibt erreichbar und zeigt den
genauen Fehler in Rot; `/api/frame.png` antwortet bis zur Korrektur mit HTTP 503.

### Lila-schwarzes Icon oder Modell

Die referenzierte Texture/Modelldatei fehlt in der JAR. Zuerst prüfen, ob JAR und `--mc` exakt
zusammenpassen. Die Fehlertextur ist absichtlich deutlich: Fehlende Ressourcen werden nicht durch
eine zufällige Farbe kaschiert.

### Port kann nicht geöffnet werden

Ein anderer Prozess verwendet ihn bereits oder die Adresse ist auf dem Rechner nicht vorhanden.
Einen anderen Port wählen, zum Beispiel `--pov-web 8766`. Port 0 wird bereits beim Parsen
abgelehnt.

### Von einem anderen Rechner nicht erreichbar

Eine nackte Portnummer bindet aus Sicherheitsgründen nur an localhost. Für LAN-Zugriff bewusst
zum Beispiel `--pov-web 0.0.0.0:8765` verwenden und die ausgegebene URL auf die Server-IP ändern.
Der Token bleibt Pflicht. Für öffentliches Internet gehört weiterhin ein TLS-Reverse-Proxy mit
zusätzlicher Anmeldung davor; der eingebaute Server spricht absichtlich nur einfaches HTTP.

## Ehrliche Grenzen

Die verwendeten Block-/GUI-PNGs und JSON-Modelle sind die Originaldateien. Das Ergebnis ist
trotzdem kein abgegriffener Frame des offiziellen Minecraft-Renderers:

- Der Protokollclient behält derzeit keine Blocklicht-, Himmelslicht- und Biome-Paletten. Licht,
  Gras-/Laubtönung, Himmel und Nebel sind deshalb angenähert.
- Animierte Blocktexturen zeigen derzeit ihren ersten Frame.
- Zufallsarrays in Blockstates verwenden deterministisch die erste Variante.
- Flüssigkeitsoberflächen, Transparenzsortierung und Minecrafts Render-Layer sind vereinfacht.
- Entity-Modelle, Entity-Metadaten, Ausrüstung und Spielerskins werden noch nicht geführt. Der
  Browser lässt Entities deshalb weg, statt wieder auffällige Ersatzrechtecke zu zeichnen.
- Direkte Item-PNGs und Blockitem-Modelle sind vorhanden; komplexe bedingte Itemmodelle (z. B.
  zustandsabhängige Bögen/Uhren) werden nicht vollständig ausgewertet.
- Menühintergründe und Slots sind original. Dynamische Balken, Rezeptlisten, Händlerangebote und
  Spezialbuttons brauchen zusätzliche Container-Property-/Recipe-Pakete und sind noch statisch.
- Die Minecraft-Bitmap-Schrift wird nicht in einen Webfont umgewandelt; Titel/Tooltips verwenden
  die lokale Monospace-Schrift des Browsers.
- Vom Server angebotene Resourcepacks bestätigt der AFK-Client weiterhin nur. Sie werden nicht
  automatisch heruntergeladen oder über die Client-JAR gelegt.

Diese Grenzen sind bewusst dokumentiert. Es werden keine AI-Bilder, keine erfundenen Texturen und
keine scheinbar „passenden“ Zufallsfarben benutzt, um fehlende Protokolldaten zu verstecken.

## Bauen und prüfen

Alle Features bauen:

```bash
cd rust
cargo build --release --features pov-client --target-dir target/pov
cargo build --release --features ultra --target-dir target/ultra
```

Gesamter Testlauf:

```bash
cd rust
cargo test --locked --release --all-features
```

Zusätzlicher Asset-Test gegen eine echte 26.2-Client-JAR:

```bash
cd rust
AFK_POV_RESOURCES="$HOME/.minecraft/versions/26.2/26.2.jar" \
  cargo test --release --features pov-client originale_client_jar -- --ignored --nocapture
```

Der HTTP-Ablauftest `browser_pov_liefert_seite_und_live_zustand` startet das gebaute Binary gegen
den eingebauten Minecraft-Testserver und prüft Viewer-URL, Tokenpflicht, HTML und Live-JSON. Wenn
`AFK_POV_RESOURCES` gesetzt ist, prüft er zusätzlich einen echten texturierten PNG-Frame, decodiert
ihn und stellt sicher, dass Stone nicht als Fehlertextur erscheint.

Im finalen Stand bestehen 134 Rust-Modultests (vier explizite Mess-/Originaldatenläufe bleiben
standardmäßig ignoriert) und 30 Socket-Ablauftests (zwei Messläufe ignoriert). Die 14 sinnvollen
Rust-Feature-Zusammenstellungen bauen warnungsfrei; Clippy läuft für alle Targets und Features mit
`-D warnings`. Der Original-JAR-Test läuft zusätzlich gegen die SHA-1-geprüfte offizielle
26.2-Client-JAR. Java besteht Tests, Warnungsprüfung und Kompilierung jeweils für 1.21.1, 1.21.11,
26.1 und 26.2, sowohl mit als auch ohne Bewegungsquellen.

## Geänderte und neue Dateien

| Datei | Änderung |
| --- | --- |
| `rust/src/pov_assets.rs` | State-Tabelle, JAR-/PNG-/Model-Lader, Model-Raycasts, Itemicon-Renderer |
| `rust/src/pov_web.rs` | token-geschützter HTTP-Server und lokale Viewer-Oberfläche |
| `rust/src/pov.rs` | texturierter PNG-Pfad, Browser-Lebenszyklus, Asset-Zugriffe |
| `rust/src/menu.rs` | Fenstertyp behalten, Browser-Snapshot für Menü und Inventar |
| `rust/src/extras.rs` | ausgewähltes Hotbarfeld und Browser-Hotbaraktion |
| `rust/src/options.rs` | `--pov-web`, `--pov-resources`, Validierung und Tests |
| `rust/src/client.rs` | Browser-Viewer beim Clientstart initialisieren |
| `rust/Cargo.toml` / `Cargo.lock` | `png` und `zip` nur im POV-Feature; POV-Bauform enthält Menü/Items/State |
| `rust/data/block-states-*.txt.gz` | vier offizielle, komprimierte State-ID-Tabellen |
| `rust/data/generate-block-states.sh` | reproduzierbarer Maintainer-Generator |
| `rust/tests/live.rs` | HTTP-, Token- und optionaler Originaltextur-PNG-Ablauftest |
| `java/src/main/.../Options.java`, `AfkClient.java` | gemeinsame POV-Schalter, Transfer-Cookies, Lifecycle, Unicode und Adressprüfung |
| `java/src/test/...` | JUnit-Regressionstests für Optionen und Chat-Sanitizing |
| `gradle.properties`, `java/build.gradle.kts` | aktualisierte Abhängigkeiten, JUnit und warnungsfreier Compilerlauf |
| `rust/.cargo/audit.toml` | eng begründete Ausnahme für das nicht einschlägige, upstream ungefixte RSA-Advisory |
| `build-all.sh`, `build-all.ps1`, `.github/workflows/release.yml` | ausführbare, reproduzierbare Locked-Builds ohne Alt-JARs, aktuelle CI-Actions/-Tests und bytegenauer Release-Changelog |
| `README.md`, `FEATURES.md` | neue Optionen, Bauform und Browser-Nutzung |

---

# Frühere Änderungen: Rust-Client 2.1.0 bis 2.4.0

Stand davor war der Commit, mit dem POV-, Items- und Ultra-Variante dazukamen. Seitdem gab es
darauf vier Runden: **2.1.0**, **2.2.0**, **2.3.0** und **2.4.0**. Der Java-Client bekam in 2.1.0
die einstellbare Sichtweite und in 2.3.0 eine Reihe echter Fehlerbehebungen; 2.4.0 betrifft
ausschließlich den Rust-Client.

## Rust-Client 2.4.0

Diese Runde ist reine Fehlersuche. Vier der behobenen Fehler kosteten unter den richtigen
Umständen die Verbindung, zwei weitere ließen Chat still verschwinden – und keiner davon wäre am
Client selbst aufgefallen, sondern nur an einem Server, der sich völlig normal verhält.

Fünf davon sind mit einem Test belegt, der ohne die Änderung wirklich fehlschlägt – nachgeprüft,
nicht behauptet. Zwei hängen an der Zeitabfolge und lassen sich nicht festnageln: die Lücke vor
dem Umschalten auf Verschlüsselung und die zuletzt geschriebene Zeile beim Beenden. Für sie hält
je ein Test das richtige Verhalten fest.

### Behobene Fehler, die die Verbindung kosteten

- **Eine Zeile mit Emoji beendete die Verbindung.** Gekürzt wurde auf 256 *Zeichen*, der Server
  zählt aber mit Javas `String.length()` – dort zählt jedes Zeichen über U+FFFF doppelt – und
  deckelt zusätzlich die Bytezahl auf das Dreifache der erlaubten Länge. Eine Zeile aus 200 Emoji
  ging deshalb glatt durch die eigene Prüfung und kam mit 400 Java-Zeichen und 800 statt höchstens
  768 Byte an: Der Server brach schon beim Dekodieren ab. Gekürzt wird jetzt in UTF-16-Einheiten,
  womit beide Grenzen zugleich eingehalten sind.
- **Eine nicht gelesene Fehlerausgabe warf den Client aus dem Spiel.** Für die Standardausgabe war
  das seit 2.3.0 behoben, für die Fehlerausgabe nicht – und über sie gehen Beitritts-, Regel- und
  Ereignismeldungen, geschrieben vom Netz-Thread. Ein Panel, das nur stdout mitliest, füllte damit
  die zweite Pipe, der Netz-Thread blieb darin stecken und der Server trennte mit
  `disconnect.timeout`. Beide Ströme haben jetzt je einen eigenen Schreib-Thread mit gedeckelter
  Warteschlange. Getrennt und nicht einer für beides: Sonst hielte eine volle Standardausgabe auch
  `@event disconnect` auf – ausgerechnet die Zeile, an der ein Panel merkt, dass der Client weg
  ist. Der neue Ablauftest lässt stderr absichtlich ungelesen volllaufen; ohne die Änderung
  schlägt er fehl.
- **Eine einzige unlesbare Chat-Komponente meldete den Client ab.** Beim Systemchat wurde ein
  Lesefehler nach oben gereicht und beendete die ganze Verbindung. Pakete sind einzeln gerahmt –
  das nächste beginnt ohnehin an einer bekannten Stelle, die Zeile wird jetzt einfach übersprungen.
- **Zwischen dem letzten Klartextpaket und dem Umschalten auf Verschlüsselung war eine Lücke.**
  Gesendet und umgeschaltet wurde unter zwei getrennten Sperren; ein anderer Thread konnte in
  diesem Fenster ein Paket dazwischenschieben, das dann unverschlüsselt hinausging, während der
  Server bereits entschlüsselte. Beides läuft jetzt unter einer Sperre.

### Behobene Fehler, die still etwas verschluckten

- **Spielerchat verschwand auf jedem Server mit eingeschaltetem Chatfilter.** Meldet der Server
  „teilweise gefiltert", folgt hinter der Filterangabe noch ein Bitfeld. Es wurde nicht gelesen,
  der Lesezeiger stand danach mitten im Paket, und die Zeile fiel wortlos weg – bei jeder
  Nachricht.
- **Cookies überlebten den Server-Transfer nicht.** Der Client leerte seine Ablage bei *jedem*
  Verbindungsaufbau. Genau dafür gibt es Cookies aber: Server A legt eines ab und schickt einen
  Transfer, Server B fragt es beim Login ab – so laufen Anmeldung und Warteschlange auf großen
  Netzwerken. Der Client antwortete stattdessen immer „habe ich nicht", und Server B schickte den
  Spieler zurück oder gleich hinaus. Der Vanilla-Client reicht sie aus demselben Grund an die neue
  Verbindung weiter.
- **Die letzte Chatzeile vor einem Kick ging verloren.** Vor dem Beenden wurde gewartet, bis die
  Warteschlange leer ist – die zuletzt herausgenommene Zeile war da aber noch gar nicht
  geschrieben. `exit` wartet auf keinen Thread, und ausgerechnet die letzten Zeilen sind die mit
  dem Grund.
- **Der Kick-Grund der Login-Phase stand als roher JSON-Text auf dem Bildschirm.** Dort kommt er
  noch als JSON und nicht als NBT; aus `{"text":"Du bist gesperrt.","color":"red"}` wird jetzt
  wieder ein lesbarer, eingefärbter Satz.
- **Eine NBT-Liste ohne Elementtyp, aber mit Länge, ergab still eine leere Liste.** Der Lesezeiger
  stand danach falsch, und der Rest des Pakets wurde aus zufälligen Bytes zusammengesetzt.
- **Eine unplausible Parameterzahl in einer Chat-Verzierung wurde auf 16 gestaucht** und
  weitergelesen – dieselbe Sorte Fehler, die 2.3.0 schon für die Quittungsliste und für
  Gegenstandslisten behoben hat.

### Weitere Behebungen

- **Örtliche Befehle hielten Sperren, die der Netz-Thread braucht.** `:click` und `:close`
  sendeten über den Socket, während sie das Menü gesperrt hielten; `:menu`, `:board`, `:inv` und
  `:slot` gaben unter derselben Sperre aus. Beides kann bis zum Schreib-Zeitablauf dauern, und so
  lange käme der Netz-Thread nicht dazu, ein KeepAlive zu beantworten. Jetzt wird der Stand
  abgeschrieben, die Sperre losgelassen und erst danach gesendet bzw. ausgegeben. Dasselbe gilt
  für das Speichern der Bewegungseinstellungen: geschrieben wird ohne die Sperre.
- **Eine Adresse ohne Namen wurde angenommen.** `[]`, `[]:25565` und `:25565` scheiterten erst
  beim Verbinden – mit einer Meldung des Netzstapels statt eines Hinweises auf den Tippfehler.
- **Die Übernahme einer alten `auth.json` schrieb die Kontodatei mit 0644 und nicht unteilbar.**
  In ihr stehen Microsoft-Token; jedes andere Speichern eines Kontos ging diesen Weg längst.
- **Der Microsoft-Gerätecode lief nach fünf Minuten ab**, obwohl Microsoft in derselben Antwort
  eine Viertelstunde nennt – wer den Browser erst suchen musste, kam zu spät. Auch der Abstand
  zwischen zwei Abfragen kommt jetzt aus der Antwort statt aus einer festen Zahl.
- **`:antiafk <riesige Zahl>` ergab eine Wartezeit, mit der keine Uhr mehr rechnet.** Dieselbe
  Obergrenze wie auf der Kommandozeile (30 Tage) gilt jetzt auch zur Laufzeit.
- **Unmögliche Zahlen in `movement.json` liefen ungeprüft in jede Rechnung.** Aus `1e400` macht
  JSON eine Unendlichkeit; der Abstand zum Ziel war danach unendlich, die Schrittweite null, und
  `:home go` lief bis ins Zeitlimit, ohne sich einen Block zu bewegen.
- **`--help` und `--accounts` stellten die Windows-Konsole nicht auf UTF-8.** Beide legen keine
  Konsole an, geben aber einen Pfad aus – steht im Benutzernamen ein Umlaut, kam er als
  Zeichensalat heraus.
- Die Feldanzahl eines Menüs wird gedeckelt übernommen; die SOCKS5-Anmeldung prüft die Version
  der Teilverhandlung; ein fehlgeschlagener Thread-Start meldet den Grund, statt nur abzubrechen.

### Tempo und Verbrauch

- **Der Rahmenpuffer wird nicht mehr bei jedem Paket genullt.** `resize` füllte ihn erst
  vollständig mit Nullen, die der Lesevorgang unmittelbar danach überschrieb – bei einem Megabyte
  Chunkdaten also ein Megabyte reines Nullenschreiben je Paket. Er wächst jetzt nur noch und wird
  weiterhin eingezogen, sobald ein Ausreißer vorbei ist.
- **Reine Luft-Abschnitte werden übersprungen statt ausgepackt.** In einer gewachsenen Überwelt
  sind das zwei Drittel aller Chunk-Abschnitte, und jeder von ihnen hat bisher 4096 Blockindizes
  aufgebaut, um sie sofort wieder wegzuwerfen.
- **Chunkdaten werden ohne Kopie aus dem Paketpuffer gelesen.** Beim Beitritt kommen gut 170
  solche Pakete mit je einigen zehn Kilobyte; jede dieser Kopien lebte nur bis zum Ende einer
  Funktion.
- **Der POV-Zeichner schläft zwischen zwei Verbindungen, statt im Bildtakt aufzuwachen.**
- **Eine Statuszeile ist ein Systemaufruf statt zwei** (die Fehlerausgabe ist ungepuffert, und
  `writeln!` schreibt Text und Zeilenumbruch getrennt), und die Schreib-Threads geben alles auf
  einmal hinaus, was gerade wartet, statt einen Systemaufruf je Zeile zu machen. Im Regelfall ist
  das dieselbe eine Zeile wie vorher; bei einem Schwall Chat war der Systemaufruf je Zeile aber
  der Engpass – der Netz-Thread füllte die Warteschlange schneller, als sie geleert wurde, und
  die ältesten Zeilen fielen heraus, obwohl das Programm davor durchaus mitlas.
- **Der Sendeabstand wird vor dem Senden abgewartet statt blind danach.** Derselbe Abstand – aber
  wer eine Minute lang nichts schickt, ist seine nächste Zeile ohne Wartezeit los, und beim
  Beenden hängt der Sender nicht in einem Schlaf fest, den niemand mehr braucht.

Der Messlauf für das Chunk-Einlesen bleibt bei rund 0,06 ms je Chunk; die Arbeit steckt dort in
den 4096 Blöcken der *gefüllten* Abschnitte, nicht in den leeren. Neu dazu kommt ein Messlauf für
den Durchsatz der Verschlüsselung (`cfb8_durchsatz`): rund 68 MB/s, und mehr ist bauartbedingt
nicht drin – CFB8 ist seriell, der Durchsatz ist genau die Latenz einer AES-Blockverschlüsselung
je Byte. Gespart werden kann nur an der Datenmenge, und dafür gibt es `--view-distance`.

### Tests

Drei ganze Codepfade, die an einem echten Server laufen, hatten bisher keinen einzigen
Ablauftest – geprüft wurde immer nur der Zweig, den draußen kaum jemand benutzt:

- **Verschlüsselung.** Der Testserver macht jetzt das vollständige Handshake eines
  Online-Mode-Servers: RSA-Schlüsselpaar, Prüffolge, und danach läuft jedes Byte in beiden
  Richtungen durch AES-128-CFB8 – auch die Rahmenlänge vor jedem Paket, die deshalb byteweise
  entschlüsselt werden muss. Die Chiffre war vorher nur gegen sich selbst geprüft, nicht über
  einen echten Socket.
- **Kompression.** Drei Schwellen, in beide Richtungen, mit Paketen ober- und unterhalb der
  Schwelle – und zusammen mit Verschlüsselung, weil ein echter Server beides gleichzeitig macht.
- **`--proxy`.** Zwei winzige echte Proxys im Testbaum: SOCKS5 mit und ohne Anmeldung sowie
  HTTP-CONNECT. Beides ist im Client von Hand umgesetzt, läuft genau einmal je Verbindungsaufbau
  und fällt deshalb niemandem auf, wenn es falsch ist.

Dazu neu: Spielerchat mit und ohne Filterangabe (auf beiden Protokollformaten), Cookies über einen
Transfer hinweg, eine Systemmeldung mit unlesbarer Komponente (die Zeile darf fallen, die
Verbindung nicht), die volle Fehlerausgabe, die letzten Chatzeilen vor dem Beenden, die genaue
Bytelänge des Chat-Pakets je Protokollversion (ab 1.21.11 ist es ein Byte länger – die Prüfsumme),
und ein Durchlauf **aller** örtlichen `:`-Befehle einschließlich der Eingaben, mit denen niemand
rechnet (`:click -1`, `:pov size 9999 9999`, `:antiafk 000000000000000000`, `:hand x`).

Die Ablauftests laufen jetzt außerdem in einem eigenen Konfigurationsverzeichnis je Client. Vorher
schrieb ein Test mit `:home set` in die echte `movement.json` – ein Test darf weder etwas
hinterlassen noch davon abhängen, was er vorfindet.

Stand: **124 Modultests und 29 Ablauftests**, alle sieben Bauformen bauen und testen ohne eine
einzige Warnung.

## Rust-Client 2.3.0 (die Runde davor)

### Behobene Fehler

- **Emoji und Sonderzeichen im Chat kamen als Kästchen an.** Minecraft schickt Chat als
  Netzwerk-NBT, und dessen Zeichenketten stehen in Javas *modifiziertem* UTF-8: Zeichen über
  U+FFFF – also jedes Emoji – als **zwei** Drei-Byte-Folgen. Der Client las das als gewöhnliches
  UTF-8 und machte aus jedem Emoji zwei Ersatzzeichen. Auf einem Server, der Emoji im Chat, in
  Gegenstandsnamen oder in der Seitenleiste benutzt, war damit fast jede zweite Zeile verstümmelt.
  Betroffen war alles, was als NBT ankommt: Chat, Kick-Gründe, Scoreboard, Lore. Ein neuer
  Ablauftest schickt Emoji über den echten Socket und prüft die Standardausgabe.
- **Ein vertippter `--join-delay` beendete den Client.** Aus den Sekundenangaben werden
  `Duration`-Werte, die der Befehls-Planer addiert – und diese Addition bricht bei einem Überlauf
  das Programm ab. Aus `--join-delay 000000000000000000` wurde so ein stiller Abbruch beim
  ersten Befehl statt einer Meldung. Alle Zeitangaben sind jetzt auf 30 Tage gedeckelt.
- **Unbrauchbare Positionen blieben dauerhaft hängen.** Ein `NaN` aus einem kaputten Plugin oder
  einem übergelaufenen relativen Teleport steckte danach im Zustand: `:go` rechnete nur noch mit
  `NaN`, die Live-Ansicht zeichnete nichts mehr, und das zurückgeschickte Bewegungspaket kostete
  die Verbindung. Werte außerhalb der Weltgrenze zählen ebenfalls als unbrauchbar.
- **Der Verbindungsaufbau hatte kein Zeitlimit.** Ein Server, der das SYN verschluckt (Firewall,
  falscher Port), hing je nach Betriebssystem bis zu zwei Minuten am Netz-Thread, ohne dass der
  Client etwas gemeldet hätte. Jetzt bricht er nach 20 Sekunden mit einer klaren Meldung ab –
  dasselbe Zeitlimit, das der Proxy-Weg längst hatte.
- **Kontodateien waren für jeden lesbar.** Unter Linux und macOS legte `fs::write` sie mit 0644
  an; darin stehen Microsoft-Token. Konto- und Bewegungsdateien bekommen jetzt 0600, das
  Verzeichnis 0700.
- **Unter Windows gab es beim Speichern ein Fenster ganz ohne Datei.** Das Ersetzen löschte die
  alte Datei erst und benannte dann um – ein Abbruch dazwischen ließ gar nichts zurück. Dabei
  ersetzt `rename` dort längst unteilbar. Zusätzlich wird die neue Datei vor dem Umbenennen auf
  die Platte gezwungen, damit nach einem Stromausfall nicht eine leere am Platz der gültigen steht.
- **`--features menu` ließ sich gar nicht bauen.** Eine reine Menü-Bauform verwendete eine
  Funktion, die nur mit Scoreboard oder Gegenständen einkompiliert wurde.
- **Eine unplausible Listenlänge in einem Gegenstand galt trotzdem als gelesen.** Sie wurde
  stillschweigend auf 1024 gestaucht; der Leser lief danach mit einer erfundenen Länge weiter und
  meldete den Gegenstand am Ende als „vollständig", obwohl ab dort nur noch Zufall herauskam.
  Jetzt gilt dieselbe Regel wie für unbekannte Komponenten: lieber „weiß ich nicht" als geraten.
- **`:click` mit negativer Feldnummer warf den Gegenstand weg.** `-1` kam beim Server als 65535 an
  und heißt dort „außerhalb des Fensters". Auffallen konnte das nur, solange der Server den
  Fensterinhalt noch nicht geschickt hatte – vorher greift die Feldanzahl-Prüfung gar nicht.
- **Ein Chat-Paket mit unplausibler Quittungsliste ergab eine Zeile aus zufälligen Bytes.** Die
  Zahl wurde auf 20 gestaucht und einfach weitergelesen; jetzt wird die Zeile verworfen.
- **`:pov frame` konnte die Live-Ansicht bis zu zwei Sekunden einfrieren.** Das erzwungene Bild
  ging an der Merkstelle für „unverändert" vorbei, das nächste Bild galt deshalb fälschlich als
  identisch.
- **`:pov size 400 300` wurde stillschweigend auf 160x80 gestaucht** – anders als `--pov-size`
  auf der Kommandozeile, das schon immer eine Meldung gab.
- **`--pov-fps` verpuffte im schlanken Client wortlos.** Jetzt gibt es dieselbe Meldung wie für
  `--pov` und `--pov-size`.
- **HTTP-Proxy: der Antwortkopf hatte keine Längengrenze.** Ein Proxy, der endlos Kopfzeilen (oder
  eine einzige endlose Zeile) schickt, ließ den Speicher volllaufen, ohne dass je ein Zeitablauf
  gegriffen hätte – es kamen ja laufend Daten.
- **Der Microsoft-Login ignorierte `slow_down`.** Microsoft beantwortet zu häufiges Nachfragen
  irgendwann gar nicht mehr; der Login lief dann in den Zeitablauf, statt zustande zu kommen.
- **Eine nicht gelesene Standardausgabe hat den Client aus dem Spiel geworfen.** Der Netz-Thread
  liest den Chat aus dem Paket *und* beantwortet KeepAlive. Er schrieb die Chatzeile bisher selbst
  – war die Pipe voll, weil das Panel gerade nicht mitlas, blieb er darin stecken und flog mit
  `disconnect.timeout` heraus, obwohl die Verbindung völlig in Ordnung war. Chat geht jetzt über
  einen eigenen Thread mit gedeckelter Warteschlange; läuft die über, fällt die älteste Zeile
  heraus (dieselbe Regel wie beim Senden) und es gibt **eine** Meldung dazu. Der neue Ablauftest
  lässt die Standardausgabe absichtlich ungelesen volllaufen und prüft, dass weiter geantwortet
  wird – ohne die Änderung schlägt er fehl.

### Tempo und Verbrauch

- **Chunks einlesen: gut ein Drittel schneller.** Gemessen an einem Chunk, wie ihn eine gewachsene
  Überwelt liefert (24 Abschnitte, acht davon gefüllt, gemischte Palette): **0,10 → 0,06 ms je
  Chunk**, Median aus sieben Läufen auf demselben Rechner. Zwei Stellen: Die gepackten
  Palettendaten werden jetzt Long für Long ausgeschoben statt je Block einmal dividiert und
  geprüft (4096 Divisionen je Abschnitt weniger), und der 8-KB-Zwischenvektor je Abschnitt
  entfällt – beim Beitritt mit 169 Chunks waren das mehrere Dutzend Megabyte reine
  Verwaltungsarbeit. Der Messlauf steht als Test im Baum.
- **Die Chunk-Tabelle wird nur noch kopiert, wenn sich wirklich etwas geändert hat.** Ein
  stillstehender Bot bekommt minutenlang keinen neuen Chunk; die Tabelle mit ihren 169 Einträgen
  wurde trotzdem achtmal je Sekunde neu aufgebaut. Das Zeichnen selbst ist dadurch nur wenige
  Prozent schneller (1,20 → 1,14 ms je Bild bei 64x32) – die Arbeit steckt im Strahl, nicht in der
  Tabelle.
- **Die Bildpunkte werden einmal geschrieben statt zweimal.** Der Puffer wurde erst gefüllt und
  dann Punkt für Punkt überschrieben.
- **Obergrenze für gehaltene Chunks aus dem Aufräumradius abgeleitet** (17×17 statt einer glatten
  1024). Sie greift nur in dem kurzen Fenster, in dem noch nicht aufgeräumt werden kann; im
  ungünstigsten Fall sind das jetzt rund 11 statt rund 40 MB. Im Regelbetrieb ändert sich nichts.
- **Große Netzpuffer werden wieder eingezogen.** Sie bleiben absichtlich zwischen den Paketen
  bestehen, wuchsen dabei aber auf das größte je gesehene Paket und gaben den Platz nie wieder
  her: Ein einzelnes Riesenpaket beim Beitritt hielt so für den Rest der Laufzeit mehrere Megabyte
  belegt.
- **Gegenstandsnamen: ein Sperrvorgang je Paket statt je Feld** (eine Doppelkiste hat 90 Felder,
  und ihr Inhalt kommt bei jeder Änderung neu), und die Registry-Namen werden beim Empfangen nicht
  mehr doppelt angelegt.

### Tests

Neu und alle gegen einen echten Socket, nicht gegen Attrappen: Emoji im Chat, Tod mit
Selbst-Respawn samt `--on death=`, befolgter Server-Transfer, Seitenleiste auf **allen drei**
Feldreihenfolgen des Team-Pakets (1.21.1, 26.1, 26.2) und Menüinhalte auf **beiden**
Komponenten-Tabellen (1.21.1 und 26.1). Dazu Zufallsdaten-Tests für den NBT- und den
Gegenstandsleser – beide laufen im Netz-Thread, und der Client läuft mit `panic = "abort"`.

Der Testserver schreibt NBT-Zeichenketten jetzt so, wie ein echter Server sie schreibt; vorher
hätte der Emoji-Fehler dort gar nicht auftauchen können. Der Messlauf für die Bildrate misst
außerdem den Weg, den der Client wirklich geht (wiederverwendete Puffer) statt eines
Testkomforts, den es seit 2.2.0 nicht mehr gibt.

Stand: 113 Modultests und 19 Ablauftests, alle sieben Bauformen bauen ohne eine einzige Warnung.

## Rust-Client 2.1.0 und 2.2.0 (die Runden davor)

- Absturz an gemischten Block-Paletten behoben – der traf jeden Beitritt auf einer normal
  erzeugten Welt, Sekunden nach dem Verbinden und ohne Zutun.
- `:use` kostete auf 1.21.1 die Verbindung (acht Byte zu viel im Paket).
- `--on chat:` traf Umlaute nicht; `@event`-Zeilen zerbrachen an mehrzeiligen Servermeldungen.
- Die SRV-Auflösung fragte auch nach einer verbindlichen Antwort noch alle drei Resolver – bei
  blockiertem UDP 4,5 Sekunden vor **jedem** Verbindungsversuch.
- Eine einzige unbrauchbare Eingabezeile machte den Client dauerhaft taub; `:look` konnte sich
  endlos drehen; Anti-AFK bremste beim Beitritt den Netz-Thread; die Live-Ansicht konnte nach
  einem Neustart stumm bleiben.
- Grenzen gegen bösartige oder kaputte Server (NBT-Knotenzahl, Cookies), Port 0 wird abgewiesen,
  eine negative Paket-ID traf nicht mehr auf die `-1`-Einträge der Versionstabelle.
- Live-POV 12–14 % schneller, wiederverwendete Bildpuffer, Chat-Ausgabe ohne
  Zwischenzeichenketten, eine 16-Byte-Kopie je Byte weniger in der Verschlüsselung.
- **Beide** Clients melden die Sichtweite jetzt einstellbar (`--view-distance`, Standard 2, in den
  POV-Bauformen 6) und nehmen die Optionen des jeweils anderen an, statt am Start abzubrechen.
  Das ist die einzige Änderung, die in diesen beiden Runden auch den Java-Client betraf.

## Java-Client (Stand bis Rust 2.4.0, `afk-*.jar`)

Im damaligen Rust-Stand 2.4.0 unverändert seit 2.3.0 – diese Runde betraf nur den Rust-Client. In 2.1.0 kam hier nur
`--view-distance` dazu (und die Duldung der Rust-Optionen); 2.3.0 war die erste Runde, die im
Java-Client wieder Fehler behebt. Die Protokollarbeit erledigt weiterhin
MCProtocolLib; alles hier betrifft das Drumherum.

- **Die Kontodatei wurde nicht unteilbar geschrieben.** Ein Abbruch mitten im Speichern hinterließ
  eine halbe Datei – und damit ein unbrauchbares Refresh-Token, das sich nur mit `--login`
  ersetzen lässt. Dasselbe galt für `movement.json`. Beide gehen jetzt denselben Weg wie im
  Rust-Client: daneben schreiben, dann umbenennen. Unter Linux und macOS zusätzlich mit 0600.
- **Chatzeilen gingen beim Beenden verloren.** Die Ausgabe ist gepuffert und der Schreib-Thread
  ein Daemon; ein Kick ohne Reconnect beendet den Prozess – ausgerechnet die letzten Zeilen mit
  dem Grund kamen dadurch nie an.
- **Cookies wuchsen unbegrenzt und wurden nie geleert.** Ein Server konnte unter immer neuen Namen
  beliebig viele ablegen, und der nächste Server bekam obendrein die des vorigen zurückgereicht.
  Jetzt gelten dieselben Grenzen wie im Vanilla-Client, und beim Verbindungsaufbau ist die Ablage
  leer.
- **Ein vertippter Port führte stillschweigend auf 25565.** `mc.example.net:2556x` verband sich
  also auf einen ganz anderen Port, und die Fehlersuche begann am falschen Ende. Port 0 wurde
  ebenfalls angenommen. Beides wird jetzt wie im Rust-Client mit einer klaren Meldung abgelehnt.
- **`:help` gab es im Bewegungs-Jar gar nicht**, obwohl die Befehlsliste im Code stand – sie wurde
  nur nie aufgerufen.
- **`:route del <nr>` konnte mit einer Ausnahme abbrechen**, wenn die Route zwischen dem Zählen
  und dem Ändern kürzer geworden war.
- **Die Bewegungseinstellungen wurden an Ort und Stelle geändert, während ein Bewegungs-Thread sie
  las.** Ein `:route clear` während eines Heimlaufs veränderte dessen Liste mitten im Durchlaufen.
  Geändert wird jetzt eine Kopie, die erst fertig eingehängt wird.
- Der Reconnect-Zähler war nicht atomar, obwohl zwei Threads ihn hochzählen – ein verlorenes
  Inkrement setzte den Backoff zurück und ließ den Client im Sekundentakt gegen einen Server
  laufen, der ihn gerade nicht will.
- Unbrauchbare Positionen vom Server (`NaN`, unendlich, außerhalb der Welt) werden wie im
  Rust-Client abgefangen.
- Die Sendewarteschlange ist gedeckelt; das Kürzen einer Nachricht zerschneidet kein Emoji mehr in
  der Mitte; `--ansicht` wird als Rust-Option angenommen statt abgelehnt.
- Die Chat-Warteschlange war unbegrenzt: Liest niemand die Standardausgabe, bleibt der
  Schreib-Thread in der vollen Pipe stehen und die Schlange wuchs ohne Ende weiter. Jetzt gilt
  dieselbe Grenze wie im Rust-Client, und das Hinausschreiben beim Beenden hat ein Zeitlimit –
  sonst hinge ein Abschluss-Haken für immer und mit ihm die ganze JVM.

## Unverändert

- Tablist und Playerlist gibt es in keiner Rust-Variante.
- Nach einem Kick oder Verbindungsabbruch verbindet sich der Rust-Client nicht automatisch neu,
  sondern beendet sich mit Fehlerstatus. Nur ein ausdrücklich vom Server angeordneter Transfer auf
  einen Unterserver wird als Teil derselben Sitzung weiter befolgt. Der Java-Client verbindet
  weiterhin mit Backoff neu.
- Das Bildformat der Live-POV ist eine zugesagte Schnittstelle und bleibt, wie es ist.
- Die Paket-IDs und Feldreihenfolgen sind unverändert; es kam keine Minecraft-Version dazu.
