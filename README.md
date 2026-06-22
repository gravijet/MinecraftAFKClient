# HugoAFKClient

Ein schlanker **CLI-AFK-Client für Minecraft Java Edition**. Anmeldung mit dem
Microsoft-Account, Verbindung zu einem Server, **Chat empfangen und senden** – alles im
Terminal. Hält die Verbindung per Anti-AFK aktiv und verbindet bei Abbruch automatisch neu.

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
- **Auto-Respawn** beim Tod (bleibt nicht im Todesbildschirm hängen).
- **Anti-AFK** (Arm-Schwung + leichte Drehung) + **Auto-Reconnect mit Backoff**.
- **Spielerliste** (`:players`) und **Status** (`:status`: Leben, Hunger, Ping, Online-Zahl).
- **Highlight + Glocke**, wenn dein Name oder ein Stichwort im Chat fällt.
- **Chat-Log** in `~/.config/hugoafk/chat.log`.
- **Serverwechsel zur Laufzeit** (`:server <ip>`), CLI-Optionen, sauberes Beenden (Ctrl-C).

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
  --server <host[:port]>   Server-Adresse
  --no-reconnect           Auto-Reconnect deaktivieren
  --no-afk                 Anti-AFK deaktivieren
  --afk <sekunden>         Anti-AFK-Intervall setzen
  -h, --help               Hilfe
```

## Bedienung

- Text tippen + Enter → Chat-Nachricht senden.
- `/befehl` → Serverbefehl (z. B. `/list`).
- Interne Befehle:
  - `:help` – Hilfe
  - `:status` – Verbindung, Leben/Hunger, Ping, Online-Zahl
  - `:players` – Online-Spieler auflisten
  - `:server <ip>` – zu anderem Server wechseln
  - `:reconnect` – neu verbinden
  - `:afk on|off` – Anti-AFK ein/aus
  - `:quit` – beenden

## Konfiguration

Liegt unter `~/.config/hugoafk/`:
- `config.json` – Server, Anti-AFK, Auto-Reconnect/Backoff, Zeitstempel, Chat-Log,
  Highlight-Stichwörter (`highlightKeywords`), Join/Leave-Meldungen
  (`announcePlayerJoinLeave`), Auto-Respawn u. a.
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
