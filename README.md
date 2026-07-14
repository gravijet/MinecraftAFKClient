# HugoAFKClient

Schlanker Minecraft-AFK-Client für die Kommandozeile. Ziel: **verbunden bleiben, nicht
gekickt werden, Chat und Befehle senden/empfangen** – bei minimalem RAM-/CPU-Verbrauch.

Das Repo enthält **zwei Module** mit demselben Funktionsumfang:

| Modul | Sprache | Minecraft | RAM (Leerlauf) | Größe | Start |
|---|---|---|---|---|---|
| `rust/` | Rust (nativ) | nur **26.1** | **~1 MB** | 0,96 MB `.exe` | sofort |
| `java/` | Java (MCProtocolLib) | 26.1 · 1.21.11 · (1.8.9) | 40–70 MB | ~12 MB Jar | JVM-Start |

Beide nutzen **dieselbe** Konfiguration und **dieselben Konto-Dateien** unter `~/.config/hugoafk/`
– einmal anmelden genügt für beide.

Bewusst reduziert: kein Anti-AFK-**Bewegen**, keine Auto-TPA/Trigger/Filter/History/Logging.
Der Kick-Schutz ist rein protokollbasiert (genau das, was ein wartender Vanilla-Client tut):
Server-`KeepAlive` sofort beantworten, `Ping`→`Pong`, Teleport bestätigen, erzwungene
Resource-Packs bestätigen (nicht laden), beim Beitritt `ClientInformation` senden, Cookies
beantworten, den Verhaltenskodex (neu in 26.1) bestätigen.

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
.\build-all.ps1              # beide Module
.\build-all.ps1 -Only rust   # nur Rust
```
```bash
./build-all.sh               # beide Module   (./build-all.sh rust  = nur Rust)
```

**Java:** Gradle 8.14.3 läuft **nicht** unter Java 25 – die Skripte suchen automatisch ein
JDK 21 (oder 17), um Gradle zu starten. Die fertigen Jars laufen unabhängig davon auf Java 25.
Einzeln: `./gradlew :java:shadowJar -Pvariant=26.1` (bzw. `1.21.11` / `1.8.9`).

**Rust:** braucht `rustup` (`winget install Rustlang.Rustup`). Auf diesem Rechner ist die
**GNU-Toolchain** eingerichtet (`stable-x86_64-pc-windows-gnu`), weil keine Visual-Studio-Build-Tools
installiert sind; den nötigen Linker/`dlltool` liefert MinGW (WinLibs, via winget installiert und
im PATH). Wer MSVC-Build-Tools installiert, kann jederzeit auf `stable-msvc` zurückwechseln.

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
- `:account` – Konto wechseln/verwalten (mehrere Microsoft-Konten)
- `:server <ip>` – Server wechseln · `:reconnect` – neu verbinden
- `:clear` – Bildschirm leeren · `:quit` – beenden

## Microsoft-Login & Konten

Anmeldung per **Device-Code** (kein Browser-Callback), **mehrere Konten** möglich: jedes Konto
liegt als eigene Datei unter `~/.config/hugoafk/accounts/<name>.json` (Format von MinecraftAuth).
Der Rust-Client liest und schreibt **dieselben Dateien**: er erneuert nur MSA-Token,
Minecraft-Token und Profil und lässt die Xbox-Device-/Title-Token des Java-Clients unangetastet.
Eine alte einzelne `auth.json` wird beim ersten Start automatisch als erstes Konto übernommen.

Token-Kette im Rust-Client: MSA-Refresh → XBL-User-Token → XSTS → Minecraft-Token → Profil.

## Auto-Befehl beim Beitritt & Proxy-Erkennung

Nach einem **echten Serverbeitritt** sendet der Client automatisch einen konfigurierbaren
Befehl (Standard `/afk`), standardmäßig **4 Sekunden** danach.

Als „echter Beitritt" zählt bewusst **nur der Login auf dem Velocity/BungeeCord-Proxy**
(das erste Login-Paket einer TCP-Verbindung). Schiebt dich der Proxy danach zwischen
**Unterservern** hin und her (weitere Login-Pakete auf derselben Verbindung), zählt das
**nicht** als neuer Beitritt – der Auto-Befehl läuft dann nicht erneut. Wirst du dagegen vom
**gesamten** Server getrennt/gekickt, verbindet sich der Client neu und sendet nach dem
frischen Proxy-Beitritt den Auto-Befehl wieder.

Einstellbar in `config.json` (gilt für beide Clients):

```jsonc
"autoCommandEnabled": true,      // Auto-Befehl an/aus
"autoCommand": "/afk",           // gesendeter Befehl (leer = aus)
"autoCommandDelaySeconds": 4     // Verzögerung nach dem Beitritt
```

## Konfiguration

`~/.config/hugoafk/`:
- `config.json` – Server, aktives Konto, Reconnect-Verhalten, Farben, Chat-Delay, Auto-Befehl
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

- **Rust-Client:** keine – die `.exe` ist eigenständig
- **Java-Client:** Java 21+ (getestet mit Java 25)
- **Build:** JDK 21/17 für Gradle (automatisch gefunden) · rustup + MinGW für Rust
