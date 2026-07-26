# HugoAFKClient

Schlanker Minecraft-AFK-Client für die Kommandozeile. Ziel: **verbunden bleiben, nicht
gekickt werden, Chat und Befehle senden/empfangen** – bei minimalem RAM-/CPU-Verbrauch.

Das Repo enthält **zwei Module** mit demselben Funktionsumfang:

| Modul | Sprache | Minecraft | RAM (Leerlauf) | Größe | Start |
|---|---|---|---|---|---|
| `rust/` | Rust (nativ) | nur **26.1** | **~1 MB** | 1,0 MB `.exe` · 2,0 MB Linux-Binary | sofort |
| `java/` | Java (MCProtocolLib) | 26.1 · 1.21.11 · (1.8.9) | 40–70 MB | ~12 MB Jar | JVM-Start |

Der Rust-Client läuft unter **Windows und Linux** (siehe [Bauen](#bauen)).

Beide nutzen **dieselbe** Konfiguration und **dieselben Konto-Dateien** unter `~/.config/hugoafk/`
– einmal anmelden genügt für beide.

Bewusst reduziert: kein Anti-AFK-**Bewegen**, keine Auto-TPA/Trigger/Filter/History/Logging.
Der Kick-Schutz ist rein protokollbasiert (genau das, was ein wartender Vanilla-Client tut):
Server-`KeepAlive` sofort beantworten, `Ping`→`Pong`, Teleport bestätigen, erzwungene
Resource-Packs bestätigen (nicht laden), beim Beitritt `ClientInformation` senden, Cookies
beantworten, den Verhaltenskodex (neu in 26.1) bestätigen, und den Chat regelkonform
quittieren (siehe [Chat-Quittungen](#chat-quittungen-multiplayerdisconnectchat_validation_failed)).

## Der Rust-Client (`rust/`)

Nativer Client für **MC 26.1 (Protokoll 775)** – keine JVM, keine Netty, kein GC.

- **~1 MB** privater Arbeitsspeicher im Leerlauf (Java: 40–70 MB), **0,03 s CPU** in 2 Minuten
- **2 Threads**: einer liest das Netz und antwortet, einer sendet rate-limitiert. Kein Timer,
  kein Polling – im Leerlauf blockieren beide.
- Protokoll komplett selbst implementiert: Rahmen, zlib-Kompression, AES-128-CFB8,
  RSA-Handshake, Mojang-Sitzungs-Join, Netzwerk-NBT (Chat-Komponenten inkl. Farben).
- Die Paket-IDs sind **nicht geraten**: sie stammen aus der Registrierungsreihenfolge im Codec
  von MCProtocolLib 26.1 – derselben Quelle, aus der auch der Java-Client seine IDs bezieht
  (siehe `rust/src/proto.rs`).
- **SRV-Auflösung** ist eingebaut (`_minecraft._tcp.<host>`) – Pflicht: `hugosmp.net` hat
  überhaupt keinen A-Record.

Getestet gegen einen echten Vanilla-26.1-Server: Login, Beitritt, Chat, Auto-`/afk`,
KeepAlive über Minuten, Trennung + automatischer Reconnect mit Backoff.

### Zusätzliche Schalter

```
hugoafk                    interaktiv (Menü, Chat)
hugoafk <host[:port]>      direkt verbinden
hugoafk --check            Selbsttest: Konto, Token-Kette, SRV, Erreichbarkeit – ohne Beitritt
hugoafk --headless <ip>    ohne Terminal-Eingabe (Hintergrund-/Dienstbetrieb)
hugoafk --account <name>   Startkonto wählen
```

## Bauen

```powershell
.\build-all.ps1               # beide Module
.\build-all.ps1 -Only rust    # nur Rust (Windows)
.\build-all.ps1 -Only linux   # Rust-Client für Linux (baut in WSL)
```
```bash
./build-all.sh                # beide Module
./build-all.sh rust           # nur Rust, für das laufende System
./build-all.sh rust-musl      # statisches Linux-Binary (läuft auf jeder Distribution)
```

**Java:** Gradle 8.14.3 läuft **nicht** unter Java 25 – die Skripte suchen automatisch ein
JDK 21 (oder 17), um Gradle zu starten. Die fertigen Jars laufen unabhängig davon auf Java 25.
Einzeln: `./gradlew :java:shadowJar -Pvariant=26.1` (bzw. `1.21.11` / `1.8.9`).

**Rust:** braucht `rustup` (`winget install Rustlang.Rustup`). Auf diesem Rechner ist die
**GNU-Toolchain** eingerichtet (`stable-x86_64-pc-windows-gnu`), weil keine Visual-Studio-Build-Tools
installiert sind; den nötigen Linker/`dlltool` liefert MinGW (WinLibs, via winget installiert und
im PATH). Wer MSVC-Build-Tools installiert, kann jederzeit auf `stable-msvc` zurückwechseln.

### Linux

Der TLS-Anbieter wird **pro Zielplattform** gewählt (siehe `rust/Cargo.toml`):

| Ziel | TLS | Voraussetzung zum Bauen |
|---|---|---|
| Windows | `native-tls` → SChannel des Systems | nichts weiter |
| Linux/macOS | `rustls` | ein C-Compiler (`build-essential`), **kein** OpenSSL/`libssl-dev` |

Damit hat das Linux-Binary zur Laufzeit nur `libc`/`libgcc` als Abhängigkeit – kein Ärger mit
OpenSSL-1.1-vs-3-Versionen auf dem Server. Dass die Wurzelzertifikate mitgeliefert werden, ist der
Grund für die 2,0 MB (Windows nutzt den Zertifikatspeicher des Systems und bleibt bei 1,0 MB).

Direkt auf einem Linux-Rechner:

```bash
sudo apt install build-essential           # einmalig (Debian/Ubuntu)
./build-all.sh rust                        # -> rust/target/release/hugoafk
./build-all.sh rust-musl                   # optional: statisch, braucht musl-tools
```

Von Windows aus über WSL: `.\build-all.ps1 -Only linux` (bei mehreren Distributionen
`-WslDistro <Name>`). Das Ergebnis landet getrennt vom Windows-Build unter
`rust/target/x86_64-unknown-linux-gnu/release/hugoafk`. In WSL müssen `build-essential` und
`rustup` installiert sein; fehlt etwas, sagt das Skript, was zu tun ist.

Für den Dauerbetrieb auf einem Server eignet sich `--headless` (keine Terminal-Eingabe), z. B. als
systemd-Dienst oder in `tmux`.

## Starten

```powershell
.\hugoafk.ps1            # Windows
```
```bash
./hugoafk.sh             # Linux/macOS/Git-Bash
```

Der Launcher listet, was gebaut ist – der Rust-Client steht oben. Danach: Konto wählen,
Server-IP eingeben, verbinden.

### CLI-Befehle (im Spiel)

- Text tippen = chatten, `/befehl` = Serverbefehl
- `:cmd` – wiederkehrende Befehle verwalten (siehe unten)
- `:account` – Konto wechseln/verwalten (mehrere Microsoft-Konten)
- `:server <ip>` – Server wechseln · `:reconnect` – neu verbinden
- `:clear` – Bildschirm leeren · `:quit` – beenden

### Terminal-Bedienung (Rust-Client)

Die Eingabe sitzt in einem Rahmen am unteren Rand, darüber laufen Chat und Meldungen durch –
die Eingabezeile wird dabei nie zerrissen. Unter dem Rahmen steht eine Statuszeile mit
Verbindungszustand (● grün/gelb/rot), Server und Konto.

```
14:02 <Hugo> bin afk
  ● Befehl: /afk
  ╭──────────────────────────────────────────────────────────╮
  │ ❯ /msg Peter bin gleich zurück                           │
  ╰──────────────────────────────────────────────────────────╯
    ● verbunden  ·  hugosmp.net  ·  Hugo  ·  :help
```

Tasten: `↑`/`↓` Verlauf · `←`/`→`/`Home`/`End` bzw. `Strg+A`/`Strg+E` Cursor ·
`Strg+U` Zeile löschen · `Strg+W` Wort löschen · `Strg+L` Bildschirm leeren ·
`Strg+C` beenden. Ist das Terminal schmaler als 46 Zeichen, fällt die Anzeige automatisch
auf eine schlichte einzeilige Eingabe zurück.

## Microsoft-Login & Konten

Anmeldung per **Device-Code** (kein Browser-Callback), **mehrere Konten** möglich: jedes Konto
liegt als eigene Datei unter `~/.config/hugoafk/accounts/<name>.json` (Format von MinecraftAuth).
Der Rust-Client liest und schreibt **dieselben Dateien**: er erneuert nur MSA-Token,
Minecraft-Token und Profil und lässt die Xbox-Device-/Title-Token des Java-Clients unangetastet.
Eine alte einzelne `auth.json` wird beim ersten Start automatisch als erstes Konto übernommen.

Token-Kette im Rust-Client: MSA-Refresh → XBL-User-Token → XSTS → Minecraft-Token → Profil.

## Wiederkehrende Befehle

Beliebig viele Befehle, jeder mit eigener **Startverzögerung** und eigenem
**Wiederholungsintervall** – z. B. `/afk` alle 5 Minuten, damit ein Plugin den AFK-Status nicht
irgendwann vergisst. Verwaltet wird das im Client mit `:cmd`:

```
:cmd                      Liste anzeigen und bearbeiten
:cmd add 300 /afk         /afk alle 300 s (0 = nur einmal je Beitritt)
:cmd del 2                Eintrag 2 entfernen
:cmd off 1 / :cmd on 1    Eintrag vorübergehend abschalten
:cmd delay 1 10           Startverzögerung nach dem Beitritt auf 10 s setzen
```

In `config.json` (gilt für beide Clients):

```jsonc
"commands": [
  { "command": "/afk", "delaySeconds": 4, "repeatSeconds": 300, "enabled": true },
  { "command": "/hub", "delaySeconds": 2, "repeatSeconds": 0,   "enabled": true }
]
```

Der frühere einzelne `autoCommand` wird beim ersten Start automatisch in diese Liste überführt
(das Altfeld wird dabei geleert, damit ein gelöschter Befehl nicht zurückkommt). Intervalle unter
5 Sekunden werden auf 5 angehoben – schneller löst nur der Spam-Schutz des Servers aus.

Umsetzung: **ein** Thread je Verbindung für alle Einträge. Er schläft blockierend bis zum
nächsten Termin (kein Polling, 0 % CPU im Leerlauf) und endet, sobald die Verbindung endet oder
die Liste geändert wird. Sind keine Befehle konfiguriert, läuft gar kein Thread. Verpasste
Termine (z. B. nach einem Standby des Rechners) werden übersprungen statt nachgefeuert.

## Beitritt & Proxy-Erkennung

Die Befehle starten nach einem **echten Serverbeitritt** – standardmäßig `/afk` 4 Sekunden danach.

Als „echter Beitritt" zählt bewusst **nur der Login auf dem Velocity/BungeeCord-Proxy**
(das erste Login-Paket einer TCP-Verbindung). Schiebt dich der Proxy danach zwischen
**Unterservern** hin und her (weitere Login-Pakete auf derselben Verbindung), zählt das
**nicht** als neuer Beitritt – die Startverzögerungen laufen dann nicht erneut an (die
Wiederholungen laufen normal weiter). Wirst du dagegen vom **gesamten** Server
getrennt/gekickt, verbindet sich der Client neu und beginnt nach dem frischen Proxy-Beitritt
wieder von vorn.

## Chat-Quittungen (`multiplayer.disconnect.chat_validation_failed`)

Seit 1.19 verlangt der Server, dass der Client empfangene **signierte** Chat-Nachrichten
quittiert: jede ausgehende Nachricht trägt einen Offset „so viele neue Nachrichten habe ich
gesehen". Ist dieser Offset **größer**, als der Server erwartet, trennt er sofort mit
`multiplayer.disconnect.chat_validation_failed`
(`LastSeenMessagesValidator.applyOffset` schlägt fehl).

Genau das passierte, weil **jede** empfangene Spieler-Chat-Nachricht mitgezählt wurde. Der
Server führt in seiner Liste aber nur **signierte** Nachrichten – auf Servern mit Plugin-/Proxy-Chat
(unsigniert) lief der Zähler daher zu hoch. Behoben in beiden Clients:

- gezählt wird nur noch, was eine Signatur hat (wie `markMessageAsProcessed` im Vanilla-Client),
- zwei gleiche Signaturen direkt hintereinander zählen wie beim Server nur einmal,
- **Befehle** verbrauchen keinen Offset mehr (das Befehlspaket hat gar kein Quittungsfeld) –
  vorher wurden Quittungen dadurch stillschweigend verschluckt,
- Zeitstempel sind streng monoton (sonst `out_of_order_chat`),
- `§` und Steuerzeichen werden entfernt und die Länge auf 256 Zeichen begrenzt
  (sonst `illegal_chat_characters` bzw. ein Decoder-Abbruch).

## Konfiguration

`~/.config/hugoafk/`:
- `config.json` – Server, aktives Konto, Reconnect-Verhalten, Farben, Chat-Delay, Befehlsliste
- `accounts/` – gespeicherte Microsoft-Konten

## JVM-Argumente des Java-Clients (warum so sparsam)

Vom Launcher gesetzt – optimiert für **eine** Verbindung und minimalen Fußabdruck:

```
-Xms16m -Xmx96m               kleiner Heap (1.8.9: -Xmx320m wegen Via-Mappings)
-XX:+UseSerialGC              1 GC-Thread statt vieler paralleler
-XX:TieredStopAtLevel=1       niedrige JIT-Last, kleiner Code-Cache
-Xss512k                      kleine Thread-Stacks (nur wenige Threads)
-Dio.netty.eventLoopThreads=1 EIN Netty-Thread statt ~64 auf 16C/32T!
-Dio.netty.allocator.type=unpooled + numHeapArenas=1 + numDirectArenas=1
-XX:MaxDirectMemorySize=32m   Direct-Memory begrenzen
-XX:+UseCompactObjectHeaders  kompakte Header (JDK 24+, nur wenn unterstützt)
```

Der Rust-Client braucht davon nichts – er hat weder GC noch JIT noch Netty.

## 1.8.9-Status (nur Java-Modul)

1.8.9 hat **keine** native Client-Bibliothek; die Verbindung liefe über den
ViaVersion-Übersetzungs-Stack (nativ 1.21.11 → 1.8.9). Die Via-Bridge fehlt noch, das Jar wird
daher übersprungen. Der Rust-Client ist bewusst nur für 26.1.

## IntelliJ IDEA

Zwei Module: das Gradle-Modul (`java/`) und ein Modul mit Inhaltswurzel `rust/`
(`target/` ausgeschlossen). Für den Rust-Teil gibt es zwei Run-Konfigurationen
(„Rust: bauen (release)", „Rust: starten"). Vollständige Rust-Unterstützung (Syntax, Cargo-Tool-Fenster)
bietet IntelliJ nur mit dem Rust-Plugin bzw. **RustRover** – `rust/` lässt sich dort direkt
als Cargo-Projekt öffnen.

## Voraussetzungen

- **Rust-Client:** keine – die `.exe` ist eigenständig; das Linux-Binary braucht nur `libc`
  (der musl-Build gar nichts)
- **Java-Client:** Java 21+ (getestet mit Java 25)
- **Build:** JDK 21/17 für Gradle (automatisch gefunden) · rustup + MinGW für Rust unter Windows ·
  rustup + `build-essential` für Rust unter Linux
