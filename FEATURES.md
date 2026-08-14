# Funktionen im Detail

Diese Datei listet **explizit** auf, was der Rust-Client kann, in welcher Bauform, über welche
Option bzw. welchen Befehl – und was bewusst **nicht** umgesetzt ist, mit Begründung.

Sie ist der Gegenpart zu `FEATURES` im Panel: jeder Panel-Schlüssel taucht unten im Abschnitt
[Abgleich mit dem Panel](#abgleich-mit-dem-panel) wieder auf.

## Die drei Bauformen

| | Datei | Bauen |
| --- | --- | --- |
| **schlank** | `afk-windows.exe`, `afk-linux` | `cargo build --release` |
| **Bewegung** | `afk-windows-move.exe`, `afk-linux-move` | `cargo build --release --features movement --target-dir target/movement` |
| **Premium** | `premium-afk-windows.exe`, `premium-afk-linux` | `cargo build --release --features premium --target-dir target/premium` |

Es ist **eine** Quelle mit Feature-Schaltern, keine Kopie. Was eine Bauform nicht kann, ist in ihr
auch nicht enthalten – weder der Code noch die Zustandstabellen noch die zusätzlichen Paket-IDs.
`premium` schließt `movement` ein.

Die Trennlinie ist bewusst **Ressourcenverbrauch**, nicht „wichtig/unwichtig":

* **schlank** hält im Leerlauf *keinen* Zustand. Jedes Paket, das nicht sofort beantwortet werden
  muss, wird ungelesen verworfen. Zwei Threads, beide blockierend – kein Timer, kein Polling.
* **Premium** muss sich Dinge merken (Anzeigetafel, Tab-Liste, offenes Menü) und braucht für
  Anti-AFK einen Zeitgeber. Genau das gehört nicht in einen Client, der nur herumstehen soll.

| | Datei (Windows, release) | Threads im Leerlauf | Zustand im Speicher |
| --- | --- | --- | --- |
| schlank | ~0,95 MB | 2 (Netz, Sender) | Konto, Cookies, letzte Position |
| Bewegung | ~1,0 MB | 2 | zusätzlich `movement.json`-Einstellungen |
| Premium | ~1,08 MB | 2, mit `--antiafk` 3 | zusätzlich Anzeigetafel, Tab-Liste, offenes Menü (je Tabelle max. 512 Einträge) |

Bewegungs-Threads gibt es nur, solange tatsächlich gelaufen wird; danach beenden sie sich.

---

## Schlanker Client

### Verbindung

| Funktion | Wie |
| --- | --- |
| Vier Minecraft-Versionen in einer Datei | `--mc 1.21.1 \| 1.21.11 \| 26.1 \| 26.2` (Standard `26.1`) |
| SRV-Auflösung | automatisch, wenn kein Port angegeben wurde |
| IPv6 | `[::1]:25566` |
| Automatischer Reconnect | an; `--no-reconnect`, `--reconnect-delay <sek>`, `--max-backoff <sek>` |
| Server-Transfer folgen | automatisch, ohne Wartezeit (der Server hat uns ja geschickt) |
| Kompression und Verschlüsselung | automatisch (zlib, AES-128-CFB8) |
| **Proxy je Start** | `--proxy socks5://[nutzer:pass@]host:port` oder `http://…` |
| **Fake-Host im Handshake** | `--fakehost <host[:port]>` |

Zum Proxy: er betrifft **nur die Spielverbindung**. Der Microsoft-Login geht weiter direkt hinaus –
dort zählt die IP niemand, und ein langsamer Proxy würde nur den Start verzögern. Nach dem Aufbau
ist der Socket ein gewöhnlicher TCP-Strom; im laufenden Betrieb kostet der Proxy nichts. Der Name
des Zielservers wird bei SOCKS5 vom Proxy aufgelöst, nicht von uns.

Zum Fake-Host: im Handshake steht die Adresse, unter der wir den Server ansprechen. Ohne die Option
ist das die echte; mit ihr eine andere. Die TCP-Verbindung geht davon unberührt zum echten Ziel.
Ohne Portangabe bleibt der echte Port stehen.

### Kick-Schutz

Rein protokollbasiert – genau das, was ein wartender Vanilla-Client tut, und **kein** Gezappel:

* `KeepAlive` sofort beantworten (der eigentliche Schutz gegen `disconnect.timeout`)
* `Ping` → `Pong`
* Teleports bestätigen und die vorgegebene Position einmal zurückspiegeln (gegen Rubberband-Kick)
* erzwungene Resource-Packs bestätigen, aber **nicht** laden
* beim Beitritt `ClientInformation` senden, Cookies beantworten
* ab 1.21.11 den Verhaltenskodex bestätigen (ohne den lässt der Server niemanden ins Spiel)
* empfangene **signierte** Chat-Nachrichten quittieren (sonst `chat_validation_failed`)
* bei Tod automatisch respawnen

### Konten

| Funktion | Wie |
| --- | --- |
| Microsoft-Konten (Gerätecode) | `--login`, danach `--account <name>`; `--accounts` listet auf |
| Mehrere Konten nebeneinander | je eine Datei unter `~/.config/afksystems/accounts/` |
| **Offline-/Cracked-Konten** | `--offline <name>` |

Der Offline-Modus rechnet die UUID genauso aus wie der Server: Version-3-UUID aus
`MD5("OfflinePlayer:<name>")`. Damit stimmt sie mit der überein, die ein Server mit
`online-mode=false` dem Namen zuordnet. Es wird nichts gespeichert und nichts angemeldet – es gibt
kein Konto, das man anmelden könnte. Verlangt der Server doch eine Sitzungsprüfung, bricht der
Client mit einer klaren Meldung ab, statt eine kaputte Verbindung zu hinterlassen. Der Name wird
geprüft (1–16 Zeichen, `A–Z a–z 0–9 _`). `--offline` und `--account` schließen sich aus.

### Chat

| Funktion | Wie |
| --- | --- |
| Chat mitlesen | Standardausgabe, eine Zeile je Nachricht, mit Farben (`--no-color` schaltet sie ab) |
| Chat schreiben | Standardeingabe; mit `/` vorn als Serverbefehl |
| Spam-Schutz | `--chat-delay <ms>` (Standard 1000), Mindestabstand ausgehender Nachrichten |
| Längen- und Zeichengrenzen | automatisch (256 bzw. 32500 Zeichen, `§` und Steuerzeichen fallen weg) |

### Befehle und Makros

| Funktion | Wie |
| --- | --- |
| Befehle beim Beitritt | `-c /afk` (mehrfach angebbar) |
| Wiederholte Befehle | `-c 300:/afk` – alle 300 s, Untergrenze 5 s |
| Startverzögerung | `--join-delay <sek>` (Standard 4) |
| **Makros** | `--on <auslöser>=<aktion>` |
| **Sperrzeit je Makro** | `--on-cooldown <sek>` (Standard 3) |

Auslöser:

| Auslöser | Wann |
| --- | --- |
| `join` | echter Beitritt (erstes Login-Paket einer Verbindung, also der Proxy-Beitritt) |
| `world` | Weltwechsel: Unterserver-Wechsel **oder** Respawn in einer anderen Welt. Ein `join` zählt ebenfalls als Weltwechsel. |
| `death` | gestorben |
| `chat:<text>` | eine Chat-Zeile enthält `<text>` (Groß-/Kleinschreibung egal, Farbcodes werden vorher entfernt) |

```bash
afk mc.example.net --on join=/afk --on death=/spawn --on "chat:du bist afk=/lobby"
```

Die Aktion geht mit `/` vorn als Serverbefehl raus, sonst als Chat-Nachricht – und zwar durch
dieselbe Warteschlange wie eine Eingabe, also mit demselben Mindestabstand. Die Sperrzeit
verhindert, dass eine Regel sich über die Antwort des Servers endlos selbst nachtriggert; nach
einer Trennung beginnt sie von vorn. Gibt es keine `chat:`-Regel, wird auch keine Chat-Zeile
angefasst – Mitlesen kostet dann exakt so viel wie vorher.

### Ausgabe für ein Programm davor

| Funktion | Wie |
| --- | --- |
| nur Chat auf der Standardausgabe | immer |
| Status/Fehler auf der Fehlerausgabe | immer; `-q` lässt nur echte Fehler übrig |
| **Maschinenlesbare Ereignisse** | `--events` |

`--events` schreibt zusätzlich Zeilen der Form `@event <name> <angaben>` auf die Fehlerausgabe.
Sie kommen **auch mit `-q`** durch und sind nie eingefärbt:

| Zeile | Wann |
| --- | --- |
| `@event connecting host=… port=… mc=…` | vor jedem Verbindungsversuch |
| `@event join name=…` | echter Beitritt |
| `@event world grund=unterserver` | Unterserver-Wechsel |
| `@event world` | Weltwechsel (Respawn in einer anderen Welt) |
| `@event death` | gestorben |
| `@event disconnect <grund>` | Verbindung beendet (Grund kann leer sein) |
| `@event reconnect versuch=N in=Ns` | vor dem nächsten Versuch |
| `@event menu open id=N` / `@event menu close` | nur Premium |

---

## Bauform „Bewegung"

Alles vom schlanken Client, dazu gesteuerte Bewegung. Örtliche Befehle beginnen mit `:` und gehen
nie an den Server; `:help` listet sie im laufenden Client auf.

| Befehl | Was |
| --- | --- |
| `:go vor\|zurück\|links\|rechts [blöcke]` | laufen, Richtung relativ zum Blick (wie W/A/S/D) |
| `:look <gier> [neigung]`, `:look nord\|ost\|süd\|west`, `:look links\|rechts\|hoch\|runter [grad]`, `:look um`, `:look gerade` | Kopf drehen |
| `:jump [richtung]` | springen (Vanilla-Sprungphysik: eine Stufe, nicht zwei) |
| `:fall`, `:fall on\|off`, `:fall <blöcke>` | fallen lassen bzw. automatisches Nach-unten-Tasten |
| `:home set\|on\|off\|go\|delay\|speed\|clear` | Heimatposition – nach jedem Beitritt automatisch dorthin |
| `:route rec\|stop\|add\|del\|go\|clear` | Wegpunkte, die vor der Heimatposition abgelaufen werden |
| `:pos` | aktuelle Position und Blickrichtung |
| `:stop` | laufende Bewegung abbrechen |

Bewusste Grenze: Der Client liest **keine** Weltdaten (keine Chunks) – er weiß also nicht, wo Blöcke
stehen. Gelaufen wird geradlinig; korrigiert der Server die Position, wird seine Vorgabe übernommen.
Gegen Hindernisse gibt es Routen (der Nutzer kennt die Ecken) und blindes Ausweichen (erst springen,
dann abwechselnd links/rechts, mit jedem Versuch einen Block weiter). Einstellungen liegen in
`movement.json` neben den Konten.

Kosten im Leerlauf: **null**. Es läuft kein Timer und kein Thread; erst ein Befehl (oder ein Beitritt
mit aktiver Heimatposition) startet einen kurzlebigen Thread, der sich danach beendet.

---

## Premium-Client

Alles vom schlanken Client **und** von der Bewegungs-Bauform, dazu:

### Anzeigetafel und Tab-Liste

| Befehl | Was |
| --- | --- |
| `:board` | Seitenleiste so, wie sie im Spiel rechts stünde – Überschrift, Zeilen, Punkte |
| `:tab` | Spielerliste |

Ausgewertet werden Ziele, Punktzahlen, die Anzeigeposition, Teams (Präfix/Suffix – daraus bestehen
auf fast jedem Server die Zeilen der Seitenleiste) und die Tab-Listen-Einträge. Schickt der Server
je Zeile einen fertigen Anzeigetext, gilt dieser.

Gedeckelt auf 512 Einträge je Tabelle; Skin-Texturen in der Tab-Liste werden übersprungen statt
gelesen (sie sind je Spieler mehrere Kilobyte und für uns wertlos).

### Menüs und Behälter

| Befehl | Was |
| --- | --- |
| `:menu` | welches Fenster offen ist: Überschrift, Fenster-Nummer, Anzahl der Felder |
| `:click <feld> [rechts\|shift]` | Feld anklicken |
| `:close` | Fenster schließen |

Bewusste Grenze: Der **Inhalt** der Felder wird nicht gelesen. Ein Gegenstand besteht seit 1.20.5
aus Nummer, Anzahl und einer offenen Liste von „Komponenten", deren Aufbau sich zwischen den vier
unterstützten Versionen unterscheidet – das nachzubauen wäre viel Code für wenig Nutzen. Gebraucht
wird es auch nicht: für den typischen Fall („Menü öffnet sich, Feld 13 anklicken") genügen
Fenster-Nummer, Zustandszähler und Feldnummer, und die stehen alle *vor* den Gegenständen im Paket.
Der Rest läuft ungelesen ins Leere – das ist zugleich der billigste Weg.

### Spielerzustand

| Befehl / Option | Was |
| --- | --- |
| `:sneak [on\|off]`, `--sneak` | Schleichen (beim Beitritt mit `--sneak`) |
| `:sprint [on\|off]` | Sprinten |
| `:swing` | Arm schwingen (Linksklick in die Luft) |
| `:use` | Gegenstand benutzen (Rechtsklick) |
| `:hand <1-9>` | Feld der Schnellleiste wählen |

Der Weg zum Server ist versionsabhängig und im Code entsprechend getrennt: 1.21.1 kennt für
Schleichen und Sprinten je ein eigenes „Spielerbefehl"-Paket, ab 1.21.11 gibt es stattdessen ein
Eingabepaket mit einem Bitfeld aller Tasten.

### Automatisches Anti-AFK

| Befehl / Option | Was |
| --- | --- |
| `--antiafk <sek>` | beim Start einschalten (mindestens 15 s, `0` = aus) |
| `:antiafk`, `:antiafk on\|off`, `:antiafk <sek>` | zur Laufzeit umstellen |

Gegen den *Server*-Timeout hilft schon `KeepAlive` – das kann der schlanke Client. Anti-AFK richtet
sich gegen die AFK-Erkennung mancher **Plugins**, die nach echter Spielerbewegung schaut. Je Runde:
Arm schwingen, Kopf 7° drehen, zurückdrehen. Bewusst zurückhaltend – wer sich im Sekundentakt dreht,
fällt mehr auf als jemand, der stillsteht; deshalb die Untergrenze von 15 s.

Kosten: **ein** Thread, der blockierend bis zum nächsten Termin schläft (kein Polling), und alle
paar Minuten drei winzige Pakete. Endet die Verbindung, endet der Thread.

---

## Bewusst nicht umgesetzt

| | Warum |
| --- | --- |
| **Live-Ansicht (POV)** | Für ein Bild müsste der Client Chunks, Blöcke und Entitäten im Speicher halten und laufend fortschreiben. Das ist genau das Gegenteil des Entwurfs – aus „wenige MB RAM" würden Hunderte, und die Netzlast stiege um Größenordnungen. Auch der Premium-Client tut das nicht. |
| **Bedrock-Konten** | Bedrock ist ein anderer Protokollstapel (RakNet statt TCP, eigene Verschlüsselung, eigene Paketformate). Das wäre kein Zusatz, sondern ein zweiter Client. |
| **Inventar-Inhalte lesen** | Siehe [Menüs](#menüs-und-behälter): Gegenstandskomponenten unterscheiden sich zwischen allen vier Versionen. Anklicken geht trotzdem – dafür braucht man den Inhalt nicht. |
| **Bewegung im schlanken Client** | Der schlanke Client bewegt sich grundsätzlich nie. Wer Bewegung will, nimmt die Bewegungs- oder die Premium-Bauform. |
| **Neue Funktionen im Java-Client** | Alles auf dieser Seite betrifft ausschließlich den Rust-Client. Der Java-Client bleibt, wie er ist. |

---

## Abgleich mit dem Panel

Jeder Schlüssel aus `FEATURES` im Panel und sein Stand nach diesem Ausbau. „Panel" heißt: die
Funktion sitzt im Panel, nicht im Client – der Client muss dafür nichts können.

| `key` | vorher | jetzt | Wo im Client |
| --- | --- | --- | --- |
| `versions` | ready | **ready** | `--mc`, vier Versionen in einer Datei |
| `always-online` | ready | **ready** | Panel |
| `reconnect` | ready | **ready** | `--reconnect-delay`, `--max-backoff` |
| `anti-kick` | ready | **ready** | siehe [Kick-Schutz](#kick-schutz) |
| `transfer` | ready | **ready** | automatisch |
| `multi-server` | ready | **ready** | Panel |
| `chat-read` | ready | **ready** | Standardausgabe |
| `chat-send` | ready | **ready** | Standardeingabe |
| `spam` | ready | **ready** | `-c <sek>:<befehl>` |
| `join-commands` | ready | **ready** | `-c <befehl>`, `--join-delay` |
| `macros` | ready | **ready, jetzt auch im Client** | `--on join\|world\|death\|chat:<text>` |
| `world-change` | ready | **ready, jetzt genauer** | Unterserver-Wechsel *und* Respawn in einer anderen Welt; `--on world`, `@event world` |
| `anti-afk` | movement | **premium** | `--antiafk`, `:antiafk` |
| `movement` | movement | **movement** | `:go`, `:look`, `:jump`, `:fall` |
| `home-route` | movement | **movement** | `:home`, `:route` |
| `sneak` | missing | **premium** | `:sneak`, `:sprint`, `--sneak` |
| `scoreboard` | missing | **premium** | `:board`, `:tab` |
| `inventory` | missing | **premium, teilweise** | `:menu`, `:click`, `:close` – Feldinhalte werden nicht gelesen |
| `accounts-offline` | missing | **ready (schlank)** | `--offline <name>` |
| `proxies` | missing | **ready (schlank)** | `--proxy socks5://…` |
| `fakehost` | missing | **ready (schlank)** | `--fakehost <host[:port]>` |
| `accounts-microsoft` | ready | **ready** | `--login`, `--account` |
| `pov` | missing | **bleibt missing** | siehe [Bewusst nicht umgesetzt](#bewusst-nicht-umgesetzt) |
| `accounts-bedrock` | missing | **bleibt missing** | siehe [Bewusst nicht umgesetzt](#bewusst-nicht-umgesetzt) |
| `credits`, `mobile`, `discord` | ready | **ready** | Panel |
| `downloads` | ready | **ready, zwei Dateien mehr** | `premium-afk-windows.exe`, `premium-afk-linux` |

Drei Anmerkungen für das Panel:

1. `sneak`, `scoreboard`, `inventory` und `anti-afk` brauchen den **Premium**-Client. Wenn im Panel
   bisher nur `ready | movement | missing` unterschieden wird, fehlt dafür ein vierter Status
   (z. B. `premium`).
2. `accounts-offline`, `proxies` und `fakehost` laufen mit **jeder** Bauform – auch mit dem
   schlanken Client.
3. `--antiafk` und `--sneak` nimmt auch der schlanke Client entgegen; er sagt dann auf der
   Fehlerausgabe, dass er sie ignoriert. Das Panel darf also allen Bauformen dieselbe Befehlszeile
   schicken.

---

## Woher die Paket-IDs kommen

Alle Paket-IDs und Feldreihenfolgen sind aus der Registrierungsreihenfolge im `MinecraftCodec` der
jeweiligen MCProtocolLib-Fassung abgelesen – also aus derselben Quelle, aus der auch der
Java-Client seine IDs bezieht. **Bei einem Minecraft-Update dort neu ablesen, nicht raten.**

Formatunterschiede zwischen den Versionen stecken an genau zwei Stellen im Code:

* `Protocol::modern` – Prüfsumme im Chat-Paket, `globalIndex` im Spieler-Chat, Partikel-Status in
  den Client-Einstellungen, Positionspaket, Verhaltenskodex, Schleich-Paket.
* `TeamLayout` – das Team-Paket ist das einzige, dessen Aufbau sich zwischen den vier Versionen
  **dreimal** ändert (1.21.1 · 1.21.11 und 26.1 · 26.2). Nur Premium liest es.

Ein Test prüft bei jedem Lauf, dass sich die Paket-IDs je Version nicht überschneiden – ein
Tippfehler in der Tabelle würde sonst still das falsche Paket verarbeiten.
