# AFKSystems

Schlanker Minecraft-AFK-Client. Er meldet sich mit einem Microsoft-Konto an, tritt einem Server
bei, bleibt verbunden und zeigt den Chat. Kein Menü, keine Konfigurationsdatei: **alles steht im
Startbefehl.**

Den Rust-Client gibt es in sieben getrennten Bauformen:

| | Datei | Minecraft-Versionen | Verbrauch |
| --- | --- | --- | --- |
| **Rust** | `afk-windows.exe`, `afk-linux` | alle fünf in *einer* Datei, Auswahl über `--mc` | ~1 MB Datei, wenige MB RAM, 4 Threads (Netz, Senden, je Ausgabestrom einer) |
| **Rust mit Zusätzen** | `items-afk-*`, `premium-afk-*`, `premium-items-afk-*`, `pov-afk-*`, `ultra-afk-*` | dieselbe eine Datei je Bauform | nur die jeweils genannten Funktionen sind einkompiliert |

Der **Premium-Client** kann alles, was der schlanke kann, plus Bewegung, farbiges Scoreboard,
Menü-Klicks und automatisches Anti-AFK. Eigene zusätzliche Dateien liefern sichtbare Gegenstände,
eine echte paketbasierte Live-POV oder alles zusammen als Ultra. Wer nur AFK stehen will, nimmt den
schlanken Client – er ist kleiner und hält im Leerlauf keinen Welt-/Menüzustand. Die exakte
Dateiauswahl steht in **[RELEASE.md](RELEASE.md)**, der Funktionsvergleich in
**[FEATURES.md](FEATURES.md)**.

## Download

Der Workflow baut bei jedem Push auf `main` alles und ersetzt damit das Release **`latest`** – dort
liegen immer alle aktuellen Dateien:

<https://github.com/gravijet/HugoAFKClient/releases/tag/latest>

## Schnellstart

```bash
# 1. einmalig anmelden (zeigt Code + Link für den Microsoft-Login)
afk --login

# 2. loslegen
afk mc.example.net --mc 26.1 -c 300:/afk
```

## Optionen

Die mit **P** markierten Optionen gibt es nur im Premium-Build, die mit **V** nur in den
POV-Bauformen. Jede
Bauform **nimmt auch die Optionen der anderen an** und sagt nur, dass sie sie ignoriert – so kann
das Panel allen Bauformen dieselbe Befehlszeile schicken.

| Option | Bedeutung |
| --- | --- |
| `-s`, `--server <host[:port]>` | Serveradresse. Geht auch ohne `-s` als erstes Argument. Ohne Port wird der SRV-Eintrag gefragt. |
| `-a`, `--account <name>` | gespeichertes Konto (Standard: das erste) |
| `--offline <name>` | Offline-/Cracked-Konto statt Microsoft-Login. Nur auf Servern mit `online-mode=false`. |
| `-m`, `--mc <version>` | `1.8.9` \| `1.21.1` \| `1.21.11` \| `26.1` \| `26.2` (Standard `26.1`). |
| `--proxy <adresse>` | Spielverbindung über `socks5://[nutzer:pass@]host:port` oder `http://...` |
| `--fakehost <host[:port]>` | diese Adresse im Handshake statt der echten (TCP geht weiter ans echte Ziel) |
| `-c`, `--cmd [sek:]<befehl>` | Befehl nach dem Beitritt, mehrfach angebbar. Ohne `sek:` einmalig, sonst alle `sek` Sekunden. Beispiel: `-c 300:/afk` |
| `--join-delay <sek>` | Wartezeit nach dem Beitritt vor dem ersten Befehl (Standard 4) |
| `--on <auslöser>=<aktion>` | Makro. Auslöser: `join`, `world`, `death`, `chat:<text>`. Mehrfach angebbar. |
| `--on-cooldown <sek>` | Sperrzeit je Regel (Standard 3), damit sich eine Regel nicht selbst nachtriggert |
| `--chat-delay <ms>` | Mindestabstand ausgehender Nachrichten (Standard 1000, gegen Spam-Kick) |
| `--no-reconnect` | nach einem Abbruch **nicht** neu verbinden, sondern mit Status 1 enden |
| `--reconnect-delay <sek>` | Wartezeit vor dem ersten Versuch (Standard 5) |
| `--max-backoff <sek>` | Obergrenze der Wartezeit (Standard 60); sie verdoppelt sich bis dahin |
| `--reconnect-tries <n>` | nach `n` erfolglosen Versuchen aufgeben (Standard `0` = unbegrenzt) |
| `--view-distance <n>` | dem Server gemeldete Sichtweite in Chunks, 2–32. Standard 2 – die POV-Bauformen 6, weil nur sie Chunks überhaupt auswerten. Kleiner heißt weniger Bandbreite, CPU und RAM. Auch `--sichtweite`. |
| `--no-color` | keine ANSI-Farben |
| `-q`, `--quiet` | keine Statusmeldungen – wirklich nur Chat |
| `--events` | zusätzlich maschinenlesbare `@event …`-Zeilen (auch mit `-q`) |
| `--antiafk <sek>` | **P** alle `sek` Sekunden eine kleine Bewegung (mindestens 15, `0` = aus) |
| `--sneak` | **P** beim Beitritt geduckt bleiben |
| `--pov <an\|aus>` | **V** Live-Ansicht gleich nach dem Beitritt starten. Standard: POV-Datei `an`, Ultra `aus`. |
| `--pov-size <b>x<h>` | **V** Auflösung der Live-Ansicht, 24–160 × 12–80 (Standard 64x32). Trennzeichen `x`, `*`, `:` oder Leerzeichen; auch `--pov-groesse`. |
| `--pov-fps <n>` | **V** Bilder je Sekunde, 1–20 (Standard 8) |
| `--pov-web <port\|ip:port>` | **V** texturierten, token-geschützten Browser-Viewer starten; nur eine Portnummer bindet an `127.0.0.1` |
| `--pov-resources <jar\|auto\|aus>` | **V** woher die echten Texturen kommen. Standard `auto`: vorhandene Minecraft-Installation benutzen, sonst einmalig von Mojang laden und im Konfigverzeichnis ablegen. Ein Pfad erzwingt genau diese JAR, `aus` verzichtet auf Texturen. |
| `--login` | Microsoft-Konto anmelden und beenden |
| `--accounts` | gespeicherte Konten auflisten und beenden |
| `-h`, `--help` | Hilfe |

Beispiel für Makros:

```bash
afk mc.example.net --on death=/spawn --on "chat:du bist afk=/lobby" --on join=/afk
```

## Ein-/Ausgabe (für die Website)

Der Client ist bewusst pipe-fähig – kein Rohmodus-Terminal, keine Statuszeile:

* **Standardausgabe**: ausschließlich Chat, eine Zeile je Nachricht.
* **Standardfehlerausgabe**: Verbindungszustand, Login-Code, Fehler. Mit `-q` bleibt nur, was
  wirklich schiefgeht.
* **Standardeingabe**: jede Zeile geht als Chat-Nachricht raus, mit `/` vorn als Serverbefehl.
  Endet die Eingabe (kein Terminal), läuft der Client einfach weiter.
* **Live-POV**: die Bilder gehen ebenfalls auf die Standardfehlerausgabe, in einem festen,
  maschinenlesbaren Format – siehe [FEATURES.md](FEATURES.md#bildformat-der-live-pov). Das Format
  ist eine zugesagte Schnittstelle und ändert sich nicht ohne Not.

Damit reicht ein Prozess-Start mit Pipes; ein eigenes Protokoll braucht es nicht.

```bash
afk mc.example.net -q -c 300:/afk > chat.log
echo "/list" | afk mc.example.net -q
```

Mit `--events` kommen auf der Fehlerausgabe zusätzlich Zeilen der Form `@event <name> <angaben>` –
sie kommen **auch mit `-q`** durch, sind nie eingefärbt, stehen immer auf **genau einer Zeile**
(Umbrüche aus Servermeldungen werden zu Leerzeichen) und lassen sich stumpf mit
`startswith("@event ")` herausfiltern:

| Zeile | wann |
| --- | --- |
| `@event connecting host=… port=… mc=…` | vor jedem Verbindungsversuch |
| `@event join name=…` | echter Beitritt (erstes Login-Paket einer Verbindung) |
| `@event world grund=unterserver` | Unterserver-Wechsel |
| `@event world` | Weltwechsel (Respawn in einer anderen Welt) |
| `@event death` | gestorben |
| `@event disconnect <grund>` | Verbindung beendet (Grund kann leer sein) |
| `@event reconnect versuch=N in=N` | **R** Neuverbindung geplant: Nummer des Versuchs und Wartezeit in Sekunden |
| `@event menu open id=N` / `@event menu close` | Menü auf/zu (Items/Premium/Ultra) |
| `@event board …` | Scoreboard-Titel/-Zeilen samt formatiertem Zahlenfeld und `§`-Farbcodes (Premium/Ultra) |
| `@event slot …` / `@event lore …` | Gegenstände und Lore mit `§`-Farbcodes (Items-Bauformen) |

Nach einem Kick oder Verbindungsabbruch verbindet sich der Client neu: erst nach 5 Sekunden, dann
mit verdoppelter Wartezeit bis höchstens 60 Sekunden. Der Zähler springt auf null zurück, sobald
der Client wieder im Spiel ist. Jeder Versuch meldet sich als
`@event reconnect versuch=<n> in=<sekunden>`. Mit `--no-reconnect` endet der Prozess stattdessen mit
Status 1. Einem ausdrücklichen Server-Transfer auf einen Unterserver folgt der Client davon
unabhängig als Teil derselben Sitzung – dabei bleiben die Cookies erhalten, bei einer Neuverbindung
werden sie verworfen.

## Konten

Die Microsoft-Anmeldung läuft über den Device-Code-Flow (Code eingeben, kein Browser-Callback).
Jedes Konto liegt als eigene Datei unter `~/.config/afksystems/accounts/<name>.json` (unter Windows
`%USERPROFILE%\.config\afksystems\`). Ein früher
angelegtes `hugoafk`-Verzeichnis wird beim ersten Start einmalig umbenannt.

Mehrere Konten: einfach mehrfach `--login`, danach mit `--account <name>` auswählen. Geladen wird
immer nur das aktive Konto.

## Unterstützte Versionen

| `--mc` | Protokoll |
| --- | ---: |
| `1.8.9` | 47 |
| `1.21.1` | 767 |
| `1.21.11` | 774 |
| `26.1` | 775 |
| `26.2` | 776 |

Der Rust-Client spricht alle fünf selbst. Seine Paket-IDs sind nicht geraten, sondern für 1.8.9
aus den veröffentlichten Protokolldaten und für die neueren Versionen aus der
Registrierungsreihenfolge im `MinecraftCodec` der jeweiligen MCProtocolLib-Fassung abgelesen
(siehe Kopf von `rust/src/proto.rs`) – **bei einem Minecraft-Update dort neu ablesen, nicht raten.**
Zwischen 1.21.1 und den neueren Versionen unterscheiden sich außerdem mehrere Paketformate. Die
gemeinsamen Unterschiede hängen an `Protocol::modern`; weitere klar getrennte Weichen beschreiben
beispielsweise Team-Pakete, Chunk-Abschnitte und Gegenstandskomponenten.

## Selbst bauen

```powershell
.\build-all.ps1                          # alle sieben Rust-Dateien nach dist\
```

```bash
./build-all.sh                           # alle sieben Rust-Dateien nach dist/
```

Einzeln:

```bash
cd rust && cargo build --release        # -> rust/target/release/afk[.exe]
cd rust && cargo test --features ultra  # Unit- und Ende-zu-Ende-Tests
```

`cargo test` startet für die Ende-zu-Ende-Tests einen nachgebauten Minecraft-Server im selben
Prozess und lässt die wirklich gebaute Datei dagegen laufen – Beitritt, Chat, Befehle, gemeldete
Sichtweite und das Bildformat der Live-POV werden also am Socket geprüft, nicht nur im Kopf.

Der Client braucht nur eine stabile Rust-Toolchain – unter Windows mit
GNU-Toolchain aus PowerShell bauen (aus Git Bash verdeckt `link.exe` von coreutils den Linker).

## Bewegung (eigene Bauform)

Der schlanke Client bewegt sich **nie**. Wer gesteuerte Bewegung will (`:go`, `:look`, `:home`,
`:route`, `:jump`, `:stop`, `:pos`), baut die zweite Bauform:

```bash
cd rust && cargo build --release --features movement --target-dir target/movement
```

Im schlanken Build ist davon keine einzige Klasse bzw. kein Byte enthalten. Die Bewegung merkt sich
Heimatposition und Routen in `movement.json` neben den Konten. `:fall` ist ohne eingelesene
Weltkollision bewusst deaktiviert; der Client tastet nicht mit erfundenen Y-Positionen nach Boden.

## Premium-Client (eigene Datei)

`premium-afk-windows.exe` / `premium-afk-linux` enthält alles vom schlanken Client **und** von der
Bewegungs-Bauform, dazu:

| Befehl | Was |
| --- | --- |
| `:board` | Anzeigetafel / Seitenleiste so, wie sie im Spiel rechts stünde |
| `:menu`, `:click <feld> [rechts\|shift]`, `:close` | geöffnete Menüs/Kisten bedienen |
| `:sneak [on\|off]`, `:sprint [on\|off]` | Schleichen / Sprinten |
| `:swing`, `:use`, `:hand <1-9>` | Arm schwingen, Rechtsklick, Schnellleiste |
| `:antiafk [on\|off\|<sek>]` | automatische kleine Bewegung gegen AFK-Plugins |

`:help` listet im laufenden Client alle örtlichen Befehle auf. Warum das eine eigene Datei ist:
Anzeigetafel und Menüs müssen Zustand mitführen und Anti-AFK braucht einen Zeitgeber – genau das,
was der schlanke Client bewusst nicht tut. Ohne `--features premium` ist davon kein Byte
einkompiliert. Tablist und Playerlist gibt es in keiner Rust-Bauform.

```bash
cd rust && cargo build --release --features premium --target-dir target/premium
```

Für sichtbare Namen/Farben/Lore im Menü und im eigenen Inventar gibt es zwei weitere Dateien:

```bash
cd rust && cargo build --release --features items --target-dir target/items
cd rust && cargo build --release --features premium,items --target-dir target/premium-items
```

Standarditems erhalten dabei ihren versionsgenauen `minecraft:...`-Ressourcennamen aus den
offiziellen Mojang-Registry-Reports; benutzerdefinierte Namen und Lore behalten ihre `§`-Farbcodes.

Die eigene POV-Datei startet nach dem Beitritt automatisch eine Live-First-Person-Ansicht aus den
empfangenen Chunk-, Block- und Entity-Paketen. Gezeichnet wird mit dem **Licht**, das der Server
mitschickt (Himmel und Blocklicht getrennt), und mit den **Biomfarben** aus seiner Registry – eine
Wiese in der Ebene ist also grün, im Sumpf trüb und in der Wüste ausgeblichen. Ultra enthält alle
Rust-Funktionen; dort wird die Ansicht bewusst erst mit `:pov live` gestartet.

```bash
cd rust && cargo build --release --features pov-client --target-dir target/pov
cd rust && cargo build --release --features ultra --target-dir target/ultra
```

Der vollständige Funktionsvergleich steht in **[FEATURES.md](FEATURES.md)**.

## Wie der Kick-Schutz funktioniert

Rein protokollbasiert – genau das, was ein wartender Vanilla-Client tut, und **kein** Gezappel:

* `KeepAlive` sofort beantworten (das ist der eigentliche Schutz gegen `disconnect.timeout`)
* `Ping` → `Pong`
* Teleports bestätigen und die vorgegebene Position einmal zurückspiegeln (gegen Rubberband-Kick)
* jeden Chunk-Stapel bestätigen (`ChunkBatchReceived`) – ohne das hört der Server nach zehn
  offenen Stapeln auf, überhaupt noch Chunks zu schicken
* das Ende der Ladephase melden (`PlayerLoaded`, ab 1.21.4) – ohne das hängt der Spieler bis zu
  30 Sekunden in einem Schwebezustand
* die eigene Position alle 20 Ticks erneut melden, auch im Stillstand: genau das tut ein echter
  Client, und ein Client, der gar nichts mehr schickt, fällt genau dadurch auf
* ab 1.21.2 jeden 50-ms-Tick mit `ClientTickEnd` abschließen
* nicht ladbare Resource-Packs ehrlich ablehnen (ein erzwungenes Pack darf den Client daher kicken)
* beim Beitritt `ClientInformation` senden, Cookies beantworten (auch über einen Transfer
  hinweg), ab 1.21.11 den Verhaltenskodex
* empfangene signierte Chat-Nachrichten quittieren (sonst `chat_validation_failed`)
* bei Tod automatisch respawnen, bei Server-Transfer dem neuen Ziel folgen

Ein Proxy-Beitritt (erstes Login-Paket einer Verbindung) startet die `--cmd`-Befehle; ein bloßer
Wechsel zwischen Unterservern bewusst nicht.

## Aufbau

```
rust/     Rust-Client (Cargo)      – proto.rs = Paket-IDs, client.rs = Ablauf, options.rs = Argumente
          rules.rs/proxy.rs        – Makros und Proxy (auch im schlanken Build)
          extras.rs               – gemeinsame, feature-gesteuerte Zusatz-Verteilerstelle
          board.rs/menu.rs/items.rs/antiafk.rs/pov.rs – getrennte Rust-Zusatzfunktionen
RELEASE.md    genaue Erklärung jeder Release-Datei
FEATURES.md   was welcher Client kann – und was bewusst fehlt
.github/  Workflow: baut bei jedem Push auf main alles und ersetzt das Release "latest"
```
