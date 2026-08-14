# AFKSystems

Schlanker Minecraft-AFK-Client. Er meldet sich mit einem Microsoft-Konto an, tritt einem Server
bei, bleibt verbunden und zeigt den Chat. Kein Menü, keine Konfigurationsdatei: **alles steht im
Startbefehl.**

Es gibt ihn zweimal aus einem Repo:

| | Datei | Minecraft-Versionen | Verbrauch |
| --- | --- | --- | --- |
| **Rust** (empfohlen) | `afk-windows.exe`, `afk-linux` | alle vier in *einer* Datei, Auswahl über `--mc` | ~1 MB Datei, wenige MB RAM, 2 Threads |
| **Java** | `afk-1.21.1.jar` … `afk-26.2.jar` | eine Jar je Version | ~10 MB Jar, 40–70 MB RAM |

Warum beim Java-Client eine Jar pro Version: MCProtocolLib spricht pro Build genau ein Protokoll.
Alle vier in eine Jar zu packen hieße vierfache Größe und Classloader-Trickserei – der Rust-Client
löst das sauberer, weil er das Protokoll selbst spricht.

## Download

Der Workflow baut bei jedem Push alles und ersetzt damit das Release **`latest`** – dort liegen
immer alle aktuellen Dateien:

<https://github.com/gravijet/HugoAFKClient/releases/tag/latest>

## Schnellstart

```bash
# 1. einmalig anmelden (zeigt Code + Link für den Microsoft-Login)
afk --login

# 2. loslegen
afk mc.example.net --mc 26.1 -c 300:/afk
```

Mit dem Java-Client genauso, nur mit `java -jar`:

```bash
java -jar afk-26.1.jar --login
java -jar afk-26.1.jar mc.example.net -c 300:/afk
```

## Optionen

Beide Clients verstehen dieselben Argumente.

| Option | Bedeutung |
| --- | --- |
| `-s`, `--server <host[:port]>` | Serveradresse. Geht auch ohne `-s` als erstes Argument. Ohne Port wird der SRV-Eintrag gefragt. |
| `-a`, `--account <name>` | gespeichertes Konto (Standard: das erste) |
| `-m`, `--mc <version>` | `1.21.1` \| `1.21.11` \| `26.1` \| `26.2` (Standard `26.1`). Beim Java-Client muss die Angabe zur Jar passen. |
| `-c`, `--cmd [sek:]<befehl>` | Befehl nach dem Beitritt, mehrfach angebbar. Ohne `sek:` einmalig, sonst alle `sek` Sekunden. Beispiel: `-c 300:/afk` |
| `--join-delay <sek>` | Wartezeit nach dem Beitritt vor dem ersten Befehl (Standard 4) |
| `--no-reconnect` | nach einem Abbruch nicht neu verbinden, sondern beenden |
| `--reconnect-delay <sek>` | erste Wartezeit vor dem Reconnect (Standard 5, danach exponentiell) |
| `--max-backoff <sek>` | Obergrenze der Reconnect-Wartezeit (Standard 60) |
| `--chat-delay <ms>` | Mindestabstand ausgehender Nachrichten (Standard 1000, gegen Spam-Kick) |
| `--no-color` | keine ANSI-Farben |
| `-q`, `--quiet` | keine Statusmeldungen – wirklich nur Chat |
| `--login` | Microsoft-Konto anmelden und beenden |
| `--accounts` | gespeicherte Konten auflisten und beenden |
| `-h`, `--help` | Hilfe |

## Ein-/Ausgabe (für die Website)

Der Client ist bewusst pipe-fähig – kein Rohmodus-Terminal, keine Statuszeile:

* **Standardausgabe**: ausschließlich Chat, eine Zeile je Nachricht.
* **Standardfehlerausgabe**: Verbindungszustand, Login-Code, Fehler. Mit `-q` bleibt nur, was
  wirklich schiefgeht.
* **Standardeingabe**: jede Zeile geht als Chat-Nachricht raus, mit `/` vorn als Serverbefehl.
  Endet die Eingabe (kein Terminal), läuft der Client einfach weiter.

Damit reicht ein Prozess-Start mit Pipes; ein eigenes Protokoll braucht es nicht.

```bash
afk mc.example.net -q -c 300:/afk > chat.log
echo "/list" | afk mc.example.net -q
```

## Konten

Die Microsoft-Anmeldung läuft über den Device-Code-Flow (Code eingeben, kein Browser-Callback).
Jedes Konto liegt als eigene Datei unter `~/.config/afksystems/accounts/<name>.json` (unter Windows
`%USERPROFILE%\.config\afksystems\`). Beide Clients teilen sich dieses Verzeichnis; ein früher
angelegtes `hugoafk`-Verzeichnis wird beim ersten Start einmalig umbenannt.

Mehrere Konten: einfach mehrfach `--login`, danach mit `--account <name>` auswählen. Geladen wird
immer nur das aktive Konto.

## Unterstützte Versionen

| `--mc` | Protokoll | Java-Client baut gegen |
| --- | --- | --- |
| `1.21.1` | 767 | `protocol-1.21` (Snapshot vom 10.10.2024, fest gepinnt) |
| `1.21.11` | 774 | `protocol-1.21.11-1` |
| `26.1` | 775 | `protocol-26.1-1` |
| `26.2` | 776 | `protocol-26.2-SNAPSHOT` (Protokoll noch in Bewegung) |

Der Rust-Client spricht alle vier selbst. Seine Paket-IDs sind nicht geraten, sondern aus der
Registrierungsreihenfolge im `MinecraftCodec` der jeweiligen MCProtocolLib-Fassung abgelesen
(siehe Kopf von `rust/src/proto.rs`) – **bei einem Minecraft-Update dort neu ablesen, nicht raten.**
Zwischen 1.21.1 und den neueren Versionen unterscheiden sich außerdem vier Paketformate; sie hängen
im Code an einem einzigen Schalter (`Protocol::modern`).

Beim Java-Client steckt derselbe Unterschied in `Net` – einmal in `java/src/api-legacy/java`
(1.21.1) und einmal in `java/src/api-modern/java`. Der übrige Code kennt ihn nicht.

## Selbst bauen

```powershell
.\build-all.ps1              # alle vier Jars + afk-windows.exe nach dist\
.\build-all.ps1 -Only rust
```

```bash
./build-all.sh               # alle vier Jars + afk-linux nach dist/
./build-all.sh --only java
```

Einzeln:

```bash
./gradlew :java:shadowJar -Pmc=26.1     # -> java/build/libs/afk-26.1.jar
cd rust && cargo build --release        # -> rust/target/release/afk[.exe]
```

Gradle braucht ein **JDK 21** (unter Java 25 startet es nicht); die fertigen Jars laufen auf jedem
Java ab 21. Der Rust-Client braucht nur eine stabile Rust-Toolchain – unter Windows mit
GNU-Toolchain aus PowerShell bauen (aus Git Bash verdeckt `link.exe` von coreutils den Linker).

Sparsame JVM-Flags für den Dauerbetrieb:

```bash
java -XX:+UseSerialGC -XX:TieredStopAtLevel=1 -Xmx96m -Dio.netty.eventLoopThreads=1 \
     -jar afk-26.1.jar mc.example.net -c 300:/afk
```

## Bewegung (eigene Bauform)

Der schlanke Client bewegt sich **nie**. Wer gesteuerte Bewegung will (`:go`, `:look`, `:home`,
`:route`, `:jump`, `:fall`, `:stop`, `:pos`), baut die zweite Bauform:

```bash
./gradlew :java:shadowJar -Pmc=26.1 -Pmove=true   # afk-26.1-move.jar
cd rust && cargo build --release --features movement --target-dir target/movement
```

Im schlanken Build ist davon keine einzige Klasse bzw. kein Byte enthalten. Die Bewegung merkt sich
Heimatposition und Routen in `movement.json` neben den Konten.

## Wie der Kick-Schutz funktioniert

Rein protokollbasiert – genau das, was ein wartender Vanilla-Client tut, und **kein** Gezappel:

* `KeepAlive` sofort beantworten (das ist der eigentliche Schutz gegen `disconnect.timeout`)
* `Ping` → `Pong`
* Teleports bestätigen und die vorgegebene Position einmal zurückspiegeln (gegen Rubberband-Kick)
* erzwungene Resource-Packs bestätigen, aber nicht laden
* beim Beitritt `ClientInformation` senden, Cookies beantworten, ab 1.21.11 den Verhaltenskodex
* empfangene signierte Chat-Nachrichten quittieren (sonst `chat_validation_failed`)
* bei Tod automatisch respawnen, bei Server-Transfer dem neuen Ziel folgen

Ein Proxy-Beitritt (erstes Login-Paket einer Verbindung) startet die `--cmd`-Befehle; ein bloßer
Wechsel zwischen Unterservern bewusst nicht.

## Aufbau

```
rust/     Rust-Client (Cargo)      – proto.rs = Paket-IDs, client.rs = Ablauf, options.rs = Argumente
java/     Java-Client (Gradle)     – src/main = Ablauf, src/api-* = Versionsunterschiede, src/move = Bewegung
.github/  Workflow: baut bei jedem Push alles und ersetzt das Release "latest"
```
