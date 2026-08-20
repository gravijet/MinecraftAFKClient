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
| `pov-afk-windows.exe` | `pov-afk-linux` | eigener POV-Client; startet automatisch eine Live-First-Person-Ansicht der geladenen Welt im Terminal |
| `ultra-afk-windows.exe` | `ultra-afk-linux` | alle Rust-Funktionen in einer Datei: Premium, Items und mit `:pov live` zuschaltbare POV |

Die vier Java-Dateien bleiben getrennt nach Protokollversion: `afk-1.21.1.jar`,
`afk-1.21.11.jar`, `afk-26.1.jar` und `afk-26.2.jar`. Items-, POV-, Premium-Items- und
Ultra-Varianten gibt es bewusst nur für Rust.

---

# Was sich seit dem 16. August geändert hat

Stand davor war der Commit, mit dem POV-, Items- und Ultra-Variante dazukamen. Seitdem gab es
drei Runden: **2.1.0** und **2.2.0** (beide nur Rust) und jetzt **2.3.0** – die erste Runde, die
auch den Java-Client wieder anfasst.

## Rust-Client 2.3.0 (neu)

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

Stand: 113 Modultests und 18 Ablauftests, alle sieben Bauformen bauen ohne eine einzige Warnung.

## Rust-Client 2.1.0 und 2.2.0 (die beiden Runden davor)

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

## Java-Client (`afk-*.jar`)

Zum ersten Mal seit dem 14. August wieder geändert. Die Protokollarbeit erledigt weiterhin
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

## Unverändert

- Tablist und Playerlist gibt es in keiner Rust-Variante.
- Nach einem Kick oder Verbindungsabbruch verbindet sich der Rust-Client nicht automatisch neu,
  sondern beendet sich mit Fehlerstatus. Nur ein ausdrücklich vom Server angeordneter Transfer auf
  einen Unterserver wird als Teil derselben Sitzung weiter befolgt. Der Java-Client verbindet
  weiterhin mit Backoff neu.
- Das Bildformat der Live-POV ist eine zugesagte Schnittstelle und bleibt, wie es ist.
- Die Paket-IDs und Feldreihenfolgen sind unverändert; es kam keine Minecraft-Version dazu.
