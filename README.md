# HugoAFKClient

Schlanker Minecraft-Java-AFK-Client für die Kommandozeile. Ziel: **verbunden bleiben, nicht
gekickt werden, Chat und Befehle senden/empfangen** – bei minimalem RAM-/CPU-Verbrauch.

Bewusst reduziert: kein Anti-AFK-**Bewegen**, keine Auto-TPA/Trigger/Filter/History/Logging usw.
Der Kick-Schutz ist rein protokollbasiert (genau das, was ein wartender Vanilla-Client tut):
Server-`KeepAlive` sofort beantworten, `Ping`→`Pong`, Teleport bestätigen, erzwungene
Resource-Packs bestätigen (nicht laden), beim Beitritt `ClientInformation` senden.

## Drei Jars – eine je Version

MCProtocolLib spricht pro Build nur **eine** Protokollversion. Darum entstehen aus einer
Codebasis drei getrennte, schlanke Jars:

| Jar | Minecraft | Backend | Größe | RAM (Idle, ca.) |
|---|---|---|---|---|
| `hugoafk-1.21.11.jar` | 1.21.11 | nativ | ~12 MB | 40–70 MB |
| `hugoafk-26.1.jar` | 26.1 | nativ | ~12 MB | 40–70 MB |
| `hugoafk-1.8.9.jar` | 1.8.9 | ViaVersion-Übersetzung | größer | höher |

## Bauen

Gradle 8.14.3 läuft **nicht** unter Java 25 – die Build-Skripte suchen automatisch ein
JDK 21 (oder 17), um Gradle zu starten. Die fertigen Jars laufen unabhängig davon auf Java 25.

```powershell
# Windows
.\build-all.ps1
```
```bash
# Linux/macOS/Git-Bash
./build-all.sh
```

Einzeln:  `./gradlew shadowJar -Pvariant=26.1`  (bzw. `1.21.11` / `1.8.9`).

## Starten

Der Launcher zeigt ein Versionsmenü und startet das passende Jar mit ressourcensparenden
JVM-Argumenten (abgestimmt auf **AMD Ryzen 9 9950X3D + 32 GB DDR5**):

```powershell
.\hugoafk.ps1            # Windows
```
```bash
./hugoafk.sh             # Linux/macOS/Git-Bash
```

Danach: Konto wählen, Server-IP eingeben, verbinden.

### CLI-Befehle (im Spiel)

- Text tippen = chatten, `/befehl` = Serverbefehl
- `:account` – Konto wechseln/verwalten (mehrere Microsoft-Konten)
- `:server <ip>` – Server wechseln · `:reconnect` – neu verbinden
- `:clear` – Bildschirm leeren · `:quit` – beenden

## Microsoft-Login & Konten wechseln

Anmeldung per **Device-Code** (kein Browser-Callback). Es werden **mehrere Konten**
unterstützt: jedes Konto liegt als eigene Datei unter `~/.config/hugoafk/accounts/<name>.json`.
Im Menü `:account` kannst du zwischen Konten wechseln, per Microsoft-Login ein neues hinzufügen
oder eines entfernen. Es ist immer nur das **aktive** Konto im Speicher – die Multi-Konto-Funktion
kostet praktisch keinen zusätzlichen RAM. Eine alte einzelne `auth.json` wird beim ersten Start
automatisch als erstes Konto übernommen.

## JVM-Argumente (warum so sparsam)

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

Der Client selbst nutzt zusätzlich nur wenige Threads (1× Netty, 1× Paketverarbeitung,
1× Konsolen-Ausgabe, 1× Sender), keinen Dauer-Timer.

## Konfiguration

`~/.config/hugoafk/`:
- `config.json` – Server, aktives Konto, Reconnect-Verhalten, Farben, Chat-Delay
- `accounts/` – gespeicherte Microsoft-Konten

## 1.8.9-Status

1.8.9 hat **keine** native Client-Bibliothek; die Verbindung läuft über den
ViaVersion-Übersetzungs-Stack (nativ 1.21.11 → 1.8.9). Dieses Jar ist daher größer und
ressourcenhungriger als die nativen Jars. Der Launcher listet es nur, wenn es gebaut wurde.

## Voraussetzungen

- **Laufzeit:** Java 21+ (getestet mit Java 25)
- **Build:** zusätzlich ein JDK 21/17 für Gradle (automatisch gefunden)
