# HugoAFKClient

Ein schlanker **CLI-AFK-Client für Minecraft Java Edition**. Anmeldung mit dem
Microsoft-Account, Verbindung zu einem Server, **Chat empfangen und senden** – alles im
Terminal. Hält die Verbindung mit einem **befehlsbasierten Anti-Kick** (z. B. periodisches
`/afk`) aktiv und verbindet bei Abbruch automatisch neu.

> **Neu:** Der frühere bewegungsbasierte Anti-AFK (Arm-Schwung + Drehung) wurde **entfernt**.
> Auf vielen Servern – auch HugoSMP – schützt nicht Bewegung, sondern ein periodischer Befehl
> wie `/afk` zuverlässig vor dem AFK-/Timeout-Kick. Diesen Befehl sendet der Client jetzt
> automatisch (`antiKickCommand`, Standard `/afk`). Zusätzlich lassen sich **eigene Befehle
> nach einem Kick** ausführen (`onKickCommands`), TPA-Anfragen automatisch annehmen, private
> Nachrichten beantworten und der Chat-Spam filtern.

Gebaut mit **Java 21 + [MCProtocolLib](https://github.com/GeyserMC/MCProtocolLib)** (GeyserMC)
und **[MinecraftAuth](https://github.com/RaphiMC/MinecraftAuth)** (RaphiMC). Diese Bibliotheken
folgen der neuesten Minecraft-Version sehr schnell und unterstützen Secure-Chat sowie das
korrekte Bestätigen erzwungener Resource-Packs – genau die Punkte, an denen mineflayer /
node-minecraft-protocol oft scheitern.

## Funktionen

- Microsoft-Login per **Device-Code** (kein Browser-Callback nötig); Anmeldung wird
  zwischengespeichert und automatisch erneuert.
- Chat **empfangen** (farbig, mit Zeitstempel) und **senden**; Serverbefehle mit `/...`.
- **Resource-Pack** wird bestätigt (nicht heruntergeladen) → kein Kick auf Servern, die ein
  Pack erzwingen.
- **Teleport-Bestätigung** → kein Rubber-Banding / Kick beim Spawn.
- **Auto-Respawn** beim Tod (bleibt nicht im Todesbildschirm hängen) + **Tod-Befehle**.
- **Befehlsbasierter Anti-Kick** (periodisches `/afk` o. Ä.) + **Auto-Reconnect mit Backoff**
  inkl. **Jitter** und **Fallback-Servern**.
- **Befehle nach einem Kick** (`onKickCommands`): genau das, was man sonst manuell tippt
  (z. B. wieder `/afk`), läuft nach erneutem Beitritt automatisch.
- **Auto-TPA**: eingehende Teleport-Anfragen automatisch annehmen (optional mit Whitelist).
- **Auto-Antwort** auf private Nachrichten (mit Cooldown pro Spieler).
- **Chat-Spam-Filter**: nervige Broadcasts (RTP-Suche etc.) ausblenden; `:mute` blendet alles aus.
- **Periodische eigene Befehle** (`periodicCommands`) mit jeweils eigenem Intervall.
- **Aktion bei niedrigem Leben** (`lowHealthCommands`, z. B. `/warp spawn`).
- **Laufzeit-Statistik** (`:stats`): Kicks, Reconnects, Tode, angenommene TPAs, Chat-Zeilen.
- **Spielerliste** (`:players`) und **Status** (`:status`: Leben, Hunger, Ping, Online-Zahl).
- **Highlight + Glocke**, wenn dein Name oder ein Stichwort im Chat fällt.
- **Chat-Log** in `~/.config/hugoafk/chat.log`.
- **Serverwechsel zur Laufzeit** (`:server <ip>`), CLI-Optionen, sauberes Beenden (Ctrl-C).
- **Laufzeit-Konfiguration** über `:set`, `:config` und Listenbefehle (`:join`, `:hide`, …).
- **Tab-Vervollständigung** für `:`-Befehle und Online-Spielernamen.
- **Auto-Beitrittsbefehle** (z. B. `/login`, `/register`) nach dem Spawn.

## „Nie gekickt werden" – eingebaute Schutzmechanismen

Der Client behandelt aktiv genau die Pakete, deren Ignorieren sonst zum Kick führt:

- **KeepAlive / Ping** – automatisch beantwortet (kein „Timed out").
- **Resource-Pack** – bestätigt, auch wenn es erzwungen wird.
- **Teleport** – wird bestätigt (kein Rubber-Banding / „moved wrongly").
- **Chat-Acknowledgement** – empfangene Nachrichten werden quittiert (kein
  „chat validation error").
- **Spam-Schutz** – eigene Nachrichten/Befehle werden rate-limitiert gesendet
  (`chatMinDelayMs`), damit kein „kicked for spamming".
- **Cookies & Transfer** – Netzwerk-Cookies werden beantwortet und Server-Transfers
  gefolgt (Velocity/BungeeCord-Netzwerke).
- **Auto-Login** – per `onJoinCommands` z. B. `/login <pass>` automatisch senden, damit
  Auth-Server nicht wegen fehlender Anmeldung kicken.
- **Befehlsbasierter Anti-Kick** – statt simulierter Bewegung wird periodisch ein Befehl
  gesendet (`antiKickCommand`, Standard `/afk`). Das ist auf AFK-/SMP-Servern der
  zuverlässige Weg gegen den Timeout-Kick und wird nicht als „Bewegungs-Bot" erkannt.
- **Befehle nach Kick** – kommt es doch zu einem Kick, läuft nach dem erneuten Beitritt
  automatisch `onKickCommands` (z. B. wieder `/afk`).
- **Auto-Reconnect** mit Backoff + Jitter + Fallback-Servern; bei
  „throttled/already logged in" wird länger gewartet.

> Hinweis: Manuelle Kicks (Ban, Whitelist, Server voll) oder erzwungener **signierter
> Chat** (`enforce-secure-profile=true`) lassen sich client-seitig nicht umgehen.

## Voraussetzungen: JDK 21 installieren

**Debian 13 / Ubuntu 24.04:**
```bash
sudo apt update
sudo apt install -y openjdk-21-jdk
```

**Ubuntu 22.04** (JDK 21 nicht in den Standard-Repos – Eclipse Temurin nutzen):
```bash
sudo apt install -y wget apt-transport-https gpg
wget -qO - https://packages.adoptium.net/artifactory/api/gpg/key/public \
  | sudo tee /etc/apt/keyrings/adoptium.asc
echo "deb [signed-by=/etc/apt/keyrings/adoptium.asc] https://packages.adoptium.net/artifactory/deb \
  $(awk -F= '/VERSION_CODENAME/{print$2}' /etc/os-release) main" \
  | sudo tee /etc/apt/sources.list.d/adoptium.list
sudo apt update
sudo apt install -y temurin-21-jdk
```

## Bauen & Starten

```bash
git clone <repo-url>
cd HugoAFKClient
./gradlew shadowJar          # baut build/libs/hugoafkclient.jar
./run.sh                      # startet den Client
# optional direkt einen Server angeben:
./run.sh mc.example.net
./run.sh mc.example.net:25565
```

Beim ersten Start erscheint ein **Microsoft-Login-Code**: die angezeigte URL öffnen, den
Code eingeben, fertig. Danach wird die Anmeldung gespeichert.

### CLI-Optionen

```
java -jar hugoafkclient.jar [optionen] [host[:port]]
  --server <host[:port]>     Server-Adresse
  --no-reconnect             Auto-Reconnect deaktivieren
  --no-antikick              Anti-Kick deaktivieren
  --antikick-cmd <befehl>    Anti-Kick-Befehl (Standard: /afk)
  --antikick-interval <sek>  Intervall des Anti-Kick-Befehls
  --auto-tpa                 TPA-Anfragen automatisch annehmen
  --mute                     eingehenden Chat ausblenden
  --no-filter                Chat-Spam-Filter deaktivieren
  -h, --help                 Hilfe
```

> `--no-afk` und `--afk <sek>` funktionieren als Aliase weiter (= `--no-antikick` /
> `--antikick-interval`).

## Bedienung

- Text tippen + Enter → Chat-Nachricht senden.
- `/befehl` → Serverbefehl (z. B. `/list`).
- Interne Befehle:
  - `:help` – Hilfe
  - `:status` – Verbindung, Leben/Hunger, Ping, Online-Zahl, aktive Schalter
  - `:stats` – Laufzeit-Statistik (Kicks, Reconnects, Tode, TPAs, Chat-Zeilen)
  - `:config` – aktuelle Konfiguration anzeigen
  - `:players` – Online-Spieler auflisten
  - `:server <ip>` – zu anderem Server wechseln
  - `:reconnect` – neu verbinden
  - `:antikick on|off` – Anti-Kick ein/aus
  - `:antikick cmd <befehl>` – Anti-Kick-Befehl setzen (z. B. `/afk`)
  - `:antikick interval <sek>` – Intervall des Anti-Kick-Befehls
  - `:tpa on|off` – TPA-Anfragen automatisch annehmen
  - `:reply on|off` / `:reply msg <text>` – Auto-Antwort auf private Nachrichten
  - `:filter on|off` – Chat-Spam-Filter; `:mute on|off` – gesamten Chat aus/ein
  - `:join` / `:kickcmd` / `:death` – Befehlslisten pflegen (`add|remove|clear|list`)
  - `:hide` / `:highlight` – Filter- bzw. Highlight-Wörter pflegen
  - `:set <key> <wert>` – Einzelwert zur Laufzeit ändern (Keys siehe `:config`)
  - `:quit` – beenden

## Konfiguration

Liegt unter `~/.config/hugoafk/`:
- `config.json` – alle Einstellungen. Die Datei wird beim ersten Start mit Standardwerten
  angelegt und bei Schema-Erweiterungen automatisch ergänzt. Wichtige Felder:

  | Bereich | Felder |
  | --- | --- |
  | Verbindung | `lastServer`, `autoReconnect`, `reconnectDelaySeconds`, `maxReconnectAttempts`, `maxBackoffSeconds`, `reconnectJitterMs`, `fallbackServers` |
  | Anti-Kick | `antiKickEnabled`, `antiKickCommand` (Std. `/afk`), `antiKickIntervalSeconds`, `antiKickToggle` |
  | Ereignis-Befehle | `onJoinCommands`, `onJoinDelaySeconds`, `onKickCommands`, `onKickDelaySeconds`, `onDeathCommands` |
  | Auto-TPA | `autoAcceptTpa`, `autoAcceptTpaWhitelist`, `tpaAcceptCommand`, `tpaRequestMarker` |
  | Auto-Antwort | `autoReplyEnabled`, `autoReplyMessage`, `autoReplyCommand`, `privateMessageMarker`, `autoReplyCooldownSeconds` |
  | Chat-Filter | `chatFilterEnabled`, `chatHideFilters`, `muteChat` |
  | Periodisch | `periodicCommands` (Liste aus `{enabled, command, intervalSeconds}`) |
  | Gesundheit | `autoRespawn`, `lowHealthActionEnabled`, `lowHealthThreshold`, `lowHealthCommands` |
  | Anzeige | `showTimestamps`, `logChat`, `highlightUsername`, `highlightKeywords`, `bellOnHighlight`, `bellOnDisconnect`, `announcePlayerJoinLeave` |
  | Spam-Schutz | `chatMinDelayMs` |

  Beispiel `onJoinCommands`: `["/login meinPasswort"]` ·
  Beispiel `onKickCommands`: `["/afk"]` ·
  Beispiel `periodicCommands`: `[{"enabled":true,"command":"/hub","intervalSeconds":600}]`
- `auth.json` – zwischengespeicherte Anmeldung (enthält Tokens; nicht weitergeben).
- `chat.log` – mitgeschriebener Chat (abschaltbar via `logChat` in `config.json`).

## Minecraft-Version anpassen

Der Client bündelt **eine** Protokollversion. Standard ist die neueste
(`mcProtocolLibVersion=26.1-1` in `gradle.properties`). Bei einem
„Outdated client/server"-Disconnect die Version passend zum Server setzen, z. B.
`1.21.11-1` für Minecraft 1.21.11, dann neu bauen. Verfügbare Versionen:
<https://repo.opencollab.dev/maven-releases/org/geysermc/mcprotocollib/protocol/>

## Hinweise

- Chat wird **unsigniert** gesendet. Das funktioniert auf Servern mit
  `enforce-secure-profile=false` (bei Plugin-/Modding-Servern üblich). Erzwingt ein Server
  signierten Chat, kommen eigene Nachrichten ggf. nicht an – Empfang und AFK bleiben
  unberührt.
- Nur für Server gedacht, auf denen du spielen darfst.
