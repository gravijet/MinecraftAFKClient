package net.gravijet.afk.move;

import net.gravijet.afk.Main;
import net.gravijet.afk.net.AfkClient;
import net.gravijet.afk.net.Mover;
import net.gravijet.afk.ui.Console;

import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.function.Consumer;

/**
 * Bewegung: Kopf drehen, laufen, Heimatposition.
 *
 * <p><b>Nur in der Bewegungs-Jar</b> ({@code -Pmove=true}) einkompiliert – das schlanke Jar enthält
 * davon keine einzige Klasse und bleibt wie bisher komplett bewegungslos.
 *
 * <p>Kosten im Leerlauf: <b>null</b>. Es läuft kein Timer und kein Thread; erst ein Befehl (oder ein
 * Beitritt mit aktiver Heimatposition) startet einen kurzlebigen Daemon-Thread, der 20-mal pro
 * Sekunde eine Position sendet – genau der Takt eines echten Clients – und sich danach beendet. Es
 * bewegt sich immer höchstens ein Thread: jede neue Aufgabe zählt {@link #job} hoch und beendet
 * damit stillschweigend die vorige.
 *
 * <p>Bewusste Grenze: Der Client liest <b>keine</b> Weltdaten (keine Chunks) und weiß daher nicht,
 * wo Blöcke stehen. Gelaufen wird geradlinig auf gleicher Höhe; korrigiert der Server die Position
 * (Wand, Gefälle, Treppe), übernehmen wir seine Vorgabe und laufen von dort weiter.
 *
 * <p>Gegen Hindernisse gibt es trotzdem zwei Mittel, beide ohne jede Kenntnis der Welt:
 * <ul>
 *   <li><b>Route</b> – einmal aufgezeichnete Wegpunkte ({@code :route rec} … {@code :route stop}).
 *       Ecken, Türen und Treppen kennt der Nutzer; der Client läuft sie nur nach.</li>
 *   <li><b>Blindes Ausweichen</b> – bleibt ein Abschnitt hängen, wird erst gesprungen (das löst jede
 *       Stufe von einem Block) und danach seitwärts am Hindernis vorbei, abwechselnd links und
 *       rechts und mit jedem Versuch einen Block weiter. Erst danach bricht der Lauf ab.</li>
 * </ul>
 */
public final class Movement implements Mover {

    /** Ein Server-Tick. Genau so oft schickt auch ein echter Client seine Position. */
    static final long TICK_MILLIS = 50;
    /** Als „angekommen" gilt, wer dem Ziel so nah ist (Blöcke). */
    private static final double ARRIVED = 0.05;
    /** So lange ohne Fortschritt = etwas steht im Weg -> ausweichen. */
    private static final long STUCK_AFTER_MILLIS = 3000;
    /** So oft wird ausgewichen, bevor ein Abschnitt aufgibt. */
    private static final int MAX_ESCAPES = 6;
    /** Obergrenze für eine einzelne Laufanweisung (Blöcke) – gegen Tippfehler wie {@code :go vor 10000}. */
    private static final double MAX_BLOCKS = 512.0;
    /** Nach dem Beitritt kommt die Position erst mit dem ersten Teleport; so lange warten wir darauf. */
    private static final long POSITION_TIMEOUT_MILLIS = 20_000;

    // Vanilla-Sprungphysik. Startgeschwindigkeit 0,42 Blöcke/Tick, danach je Tick Schwerkraft und
    // Luftwiderstand – der Scheitel liegt bei gut 1,25 Blöcken. Genug für eine Stufe, zu wenig für zwei.
    static final double JUMP_SPEED = 0.42;
    static final double GRAVITY = 0.08;
    static final double DRAG = 0.98;
    /** Notbremsen für einen Sturz: so tief und so lange höchstens. */
    private static final double MAX_FALL_BLOCKS = 24.0;
    private static final int MAX_FALL_TICKS = 60;
    /** Der Server hat uns nach oben gesetzt (= wir stecken in einem Block) ab dieser Abweichung. */
    private static final double CORRECTED = 0.05;

    private volatile AfkClient client;
    private volatile Console console;
    private volatile MoveSettings settings = new MoveSettings();

    /** Laufende Aufgabe. Hochzählen beendet die vorige. */
    private final AtomicInteger job = new AtomicInteger();

    /**
     * Zwischen {@code :route rec} und {@code :route stop}: die bisher erreichten Punkte, sonst
     * {@code null}. Bewusst <b>nicht</b> in den Einstellungen – eine halbe Aufzeichnung soll keinen
     * Neustart überleben. Es schreibt der Bewegungs-Thread und es liest die Eingabezeile, daher
     * immer nur unter {@link #recordLock} anfassen.
     */
    private final Object recordLock = new Object();
    private List<MoveSettings.Spot> recording = null;

    @Override
    public boolean available() {
        return true;
    }

    @Override
    public void attach(AfkClient client, Console console) {
        this.client = client;
        this.console = console;
        Path file = Main.configDir().resolve("movement.json");
        this.settings = MoveSettings.load(file);
    }

    @Override
    public void onDisconnect() {
        job.incrementAndGet();
    }

    /**
     * Einstellungen ändern und speichern.
     *
     * <p>Geändert wird eine <b>Kopie</b>, die erst danach im {@code volatile}-Feld landet. Vorher
     * wurde das Objekt an Ort und Stelle verändert, das ein laufender Bewegungs-Thread längst in
     * der Hand hielt: Ein {@code :route clear} während eines Heimlaufs veränderte dessen Liste
     * mitten im Durchlaufen (ConcurrentModificationException), und die zugleich geschriebenen
     * {@code double}-Felder darf die JVM ohne Sperre auch halb sichtbar machen. Die Zuweisung des
     * fertigen Objekts ist dagegen ein einziger, unteilbarer Schritt.
     */
    private void update(Consumer<MoveSettings> change) {
        MoveSettings next = settings.copy();
        change.accept(next);
        next.save();
        settings = next;
    }

    @Override
    public void onJoin() {
        MoveSettings current = settings;
        if (!current.homeEnabled || current.home == null) {
            return;
        }
        goHome(current, current.homeDelaySeconds * 1000L);
    }

    /**
     * Heimlauf: Verzögerung abwarten, die Route abgehen, zum Ziel laufen, dort in die gemerkte
     * Richtung schauen. Ohne Route ist das genau der frühere geradlinige Lauf.
     */
    private void goHome(MoveSettings current, long delayMillis) {
        MoveSettings.Spot target = current.home;
        List<MoveSettings.Spot> waypoints = List.copyOf(current.route);
        start(id -> {
            // Nach dem Beitritt kennt der Client seine Position erst nach dem ersten Teleport.
            if (!waitForPosition(id) || !nap(id, delayMillis)) {
                return;
            }
            console.info(waypoints.isEmpty()
                    ? "Laufe zur Heimatposition ..."
                    : "Laufe die Route (" + waypoints.size() + " Wegpunkte) zur Heimatposition ...");

            Outcome outcome = Outcome.ARRIVED;
            String label = "Heimatposition";
            for (int index = 0; index < waypoints.size(); index++) {
                MoveSettings.Spot point = waypoints.get(index);
                // Wegpunkte kennen ihre Höhe – damit geht es Treppen hinauf und hinunter.
                outcome = walkTo(id, point.x, point.z, point.y);
                if (outcome != Outcome.ARRIVED) {
                    label = "Route (Wegpunkt " + (index + 1) + "/" + waypoints.size() + ")";
                    break;
                }
            }
            if (outcome == Outcome.ARRIVED) {
                outcome = walkTo(id, target.x, target.z, target.y);
            }
            if (outcome == Outcome.ARRIVED) {
                outcome = turnTo(id, target.yaw, target.pitch);
            }
            report(outcome, label);
        });
    }

    // ===================== Befehle =====================

    @Override
    public List<String> helpRows() {
        return List.of(
                row(":go vor 5", "5 Blöcke laufen · vor|zurück|links|rechts (wie W/A/S/D)"),
                row(":look 90 0", "Kopf drehen: Gierwinkel und Neigung absolut"),
                row(":look links 90", "relativ · auch nord|ost|süd|west, hoch|runter, um, gerade"),
                row(":home", "Heimatposition: set · on|off · go · delay <sek> · clear"),
                row(":route rec", "Weg um Hindernisse aufzeichnen · stop · add · del · go"),
                row(":jump", "springen · :jump vor|zurück|links|rechts"),
                row(":fall", "jetzt herunterfallen · :fall on|off · :fall <blöcke>"),
                row(":stop", "laufende Bewegung abbrechen"),
                row(":pos", "Position und Blickrichtung anzeigen"));
    }

    private static String row(String key, String text) {
        return "  " + key + " ".repeat(Math.max(1, 15 - key.length())) + text;
    }

    /** {@code :help} – dieselbe Liste, die {@link #helpRows()} liefert. */
    private void printHelp() {
        console.print("");
        console.print(console.color(Console.BOLD,
                "  Befehle (alles mit ':' vorn, alles andere geht in den Chat)"));
        for (String line : helpRows()) {
            console.print("  " + line);
        }
        console.print(console.color(Console.GRAY,
                "    /befehl geht als Serverbefehl raus, alles andere als Chat."));
    }

    @Override
    public boolean command(String verb, String arg) {
        switch (verb) {
            // `helpRows()` gab es zwar von Anfang an, aber niemand rief es auf: `:help` landete
            // deshalb im „Unbekannter Befehl"-Zweig, obwohl der Rust-Client die Liste zeigt.
            case "help", "hilfe", "?" -> printHelp();
            case "go", "geh", "gehe", "lauf", "laufe" -> go(arg);
            case "look", "schau", "dreh", "drehe" -> look(arg);
            case "home", "heim" -> home(arg);
            case "route", "weg", "strecke" -> route(arg);
            case "jump", "spring", "springe" -> jumpCommand(arg);
            case "fall", "fallen" -> fallCommand(arg);
            case "stop", "halt" -> {
                job.incrementAndGet();
                console.info("Bewegung gestoppt.");
            }
            case "pos", "position" -> {
                double[] p = position();
                if (p == null) {
                    console.error("Position noch unbekannt (nicht im Spiel?).");
                } else {
                    console.info(String.format("x=%.2f  y=%.2f  z=%.2f  ·  Blick %.1f° (%s) / %.1f°",
                            p[0], p[1], p[2], p[3], compass((float) p[3]), p[4]));
                }
            }
            default -> {
                return false;
            }
        }
        return true;
    }

    private void go(String arg) {
        String[] parts = arg.trim().split("\\s+");
        if (parts.length == 0 || parts[0].isEmpty()) {
            usageGo();
            return;
        }
        String word = parts[0].toLowerCase();
        if (word.equals("stop") || word.equals("halt")) {
            job.incrementAndGet();
            console.info("Bewegung gestoppt.");
            return;
        }
        Direction direction = Direction.parse(word);
        if (direction == null) {
            usageGo();
            return;
        }
        // Ohne Angabe genau ein Block – das ist die häufigste Feinkorrektur.
        double blocks = 1.0;
        if (parts.length > 1) {
            Double value = parseNumber(parts[1]);
            if (value == null || value <= 0 || value > MAX_BLOCKS) {
                console.error("Anzahl muss zwischen 0 und " + (int) MAX_BLOCKS + " liegen.");
                return;
            }
            blocks = value;
        }

        double[] p = position();
        if (p == null) {
            console.error("Position noch unbekannt (nicht im Spiel?).");
            return;
        }
        double[] vector = direction.vector((float) p[3]);
        double targetX = p[0] + vector[0] * blocks;
        double targetZ = p[2] + vector[1] * blocks;
        String label = String.format("%.1f Blöcke %s", blocks, direction.label);
        console.info("Gehe " + label + " ...");
        start(id -> {
            Outcome outcome = walkTo(id, targetX, targetZ, null);
            // Läuft gerade eine Aufzeichnung, wird genau hieraus die Route.
            if (outcome == Outcome.ARRIVED) {
                recordPoint();
            }
            // Am Ziel noch einmal ablegen: der letzte Schritt kann über eine Kante geführt haben.
            if (outcome == Outcome.ARRIVED && settings.autoFall) {
                outcome = fall(id, 0, 0, 0);
            }
            report(outcome, label);
        });
    }

    /** Nach einem erreichten {@code :go}-Ziel: die neue Position an die laufende Aufzeichnung hängen. */
    private void recordPoint() {
        double[] p = position();
        if (p == null) {
            return;
        }
        synchronized (recordLock) {
            if (recording == null) {
                return;
            }
            if (recording.size() >= MoveSettings.MAX_ROUTE) {
                console.error("Route: mehr als " + MoveSettings.MAX_ROUTE + " Wegpunkte gehen nicht.");
                return;
            }
            recording.add(new MoveSettings.Spot(p[0], p[1], p[2], (float) p[3], (float) p[4]));
            console.info("Wegpunkt " + recording.size() + " aufgezeichnet.");
        }
    }

    /** {@code :jump} (auf der Stelle) oder {@code :jump vor|zurück|links|rechts}. */
    private void jumpCommand(String arg) {
        double[] p = position();
        if (p == null) {
            console.error("Position noch unbekannt (nicht im Spiel?).");
            return;
        }
        String word = arg.trim().split("\\s+")[0].toLowerCase();
        double driftX = 0;
        double driftZ = 0;
        if (!word.isEmpty()) {
            Direction direction = Direction.parse(word);
            if (direction == null) {
                console.error("Nutzung: :jump   oder   :jump vor|zurück|links|rechts");
                return;
            }
            double[] vector = direction.vector((float) p[3]);
            driftX = vector[0];
            driftZ = vector[1];
        }
        final double dx = driftX;
        final double dz = driftZ;
        console.info("Springe ...");
        start(id -> report(jump(id, dx, dz), "Sprung"));
    }

    /** {@code :fall} (jetzt fallen), {@code :fall on|off} (Automatik) oder {@code :fall <blöcke>}. */
    private void fallCommand(String arg) {
        String word = arg.trim().toLowerCase();
        switch (word) {
            case "on", "an", "ein" -> {
                update(s -> s.autoFall = true);
                console.print(console.color(Console.GREEN,
                        "Fallen automatisch – beim Laufen wird regelmäßig nach unten getastet."));
            }
            case "off", "aus" -> {
                update(s -> s.autoFall = false);
                console.info("Automatisches Fallen aus – nur noch auf  :fall .");
            }
            case "" -> {
                double[] p = position();
                if (p == null) {
                    console.error("Position noch unbekannt (nicht im Spiel?).");
                    return;
                }
                double before = p[1];
                start(id -> {
                    Outcome outcome = fall(id, 0, 0, 0);
                    if (outcome != Outcome.ARRIVED) {
                        report(outcome, "Fallen");
                        return;
                    }
                    double[] after = position();
                    double dropped = after == null ? 0 : before - after[1];
                    console.print(console.color(Console.GREEN,
                            String.format("Gefallen: %.1f Blöcke.", dropped)));
                });
            }
            default -> {
                Double blocks = parseNumber(word);
                if (blocks == null || blocks < 0.5 || blocks > 16.0) {
                    console.error("Nutzung: :fall   ·   :fall on|off   ·   :fall <0,5–16 blöcke>");
                    return;
                }
                update(s -> s.fallCheckBlocks = blocks);
                console.info(String.format(
                        "Prüfabstand: alle %.1f gelaufenen Blöcke einmal nach unten tasten.",
                        settings.fallCheckBlocks));
            }
        }
    }

    private void look(String arg) {
        double[] p = position();
        if (p == null) {
            console.error("Position noch unbekannt (nicht im Spiel?).");
            return;
        }
        float yaw = (float) p[3];
        float pitch = (float) p[4];

        String[] parts = arg.trim().split("\\s+");
        if (parts.length == 0 || parts[0].isEmpty()) {
            usageLook();
            return;
        }
        String word = parts[0].toLowerCase();
        Double second = parts.length > 1 ? parseNumber(parts[1]) : null;

        float targetYaw;
        float targetPitch;
        Double absolute = parseNumber(word);
        if (absolute != null) {
            // Zwei Zahlen = absolute Blickrichtung (Gierwinkel, Neigung).
            targetYaw = absolute.floatValue();
            targetPitch = second != null ? second.floatValue() : pitch;
        } else {
            // Sonst: Himmelsrichtung oder relative Drehung um <grad> (Standard 90 bzw. 30).
            targetYaw = yaw;
            targetPitch = pitch;
            switch (word) {
                case "nord", "norden", "north", "n" -> targetYaw = 180f;
                case "sued", "süd", "sueden", "süden", "south" -> targetYaw = 0f;
                case "ost", "osten", "east", "e" -> targetYaw = -90f;
                case "west", "westen", "w" -> targetYaw = 90f;
                case "gerade", "mitte", "level" -> targetPitch = 0f;
                case "links", "left", "l" -> targetYaw = yaw - (second != null ? second.floatValue() : 90f);
                case "rechts", "right", "r" -> targetYaw = yaw + (second != null ? second.floatValue() : 90f);
                case "hoch", "up", "oben" -> targetPitch = pitch - (second != null ? second.floatValue() : 30f);
                case "runter", "down", "unten" -> targetPitch = pitch + (second != null ? second.floatValue() : 30f);
                case "um", "umdrehen", "back" -> targetYaw = yaw + 180f;
                default -> {
                    usageLook();
                    return;
                }
            }
        }

        final float finalYaw = wrapDegrees(targetYaw);
        final float finalPitch = Math.max(-90f, Math.min(90f, targetPitch));
        console.info(String.format("Drehe auf %.0f° (%s) / %.0f° ...",
                finalYaw, compass(finalYaw), finalPitch));
        start(id -> report(turnTo(id, finalYaw, finalPitch), "Drehen"));
    }

    private void home(String arg) {
        String trimmed = arg.trim();
        String[] parts = trimmed.split("\\s+", 2);
        String verb = parts[0].toLowerCase();
        String rest = parts.length > 1 ? parts[1].trim() : "";

        switch (verb) {
            case "", "status", "list" -> printHome();
            case "on", "an", "ein" -> {
                if (settings.home == null) {
                    console.error("Keine Heimatposition gesetzt – erst  :home set  (siehe :home).");
                    return;
                }
                update(s -> s.homeEnabled = true);
                console.print(console.color(Console.GREEN,
                        "Heimatposition aktiv – bei jedem Beitritt wird dorthin gelaufen."));
            }
            case "off", "aus" -> {
                update(s -> s.homeEnabled = false);
                console.info("Heimatposition aus.");
            }
            case "set", "setze" -> setHome(rest);
            case "clear", "loeschen", "löschen", "del" -> {
                update(s -> {
                    s.home = null;
                    s.homeEnabled = false;
                });
                console.info("Heimatposition gelöscht.");
            }
            case "go", "los", "jetzt" -> {
                MoveSettings current = settings;
                if (current.home == null) {
                    console.error("Keine Heimatposition gesetzt (:home set).");
                    return;
                }
                goHome(current, 0L);
            }
            case "delay" -> {
                Double seconds = parseNumber(rest);
                if (seconds == null || seconds < 0 || seconds > 3600) {
                    console.error("Nutzung: :home delay <sekunden>");
                    return;
                }
                update(s -> s.homeDelaySeconds = seconds.intValue());
                console.info("Startverzögerung: " + settings.homeDelaySeconds + " s nach dem Beitritt.");
            }
            case "speed", "tempo" -> {
                Double speed = parseNumber(rest);
                if (speed == null) {
                    console.error("Nutzung: :home speed <blöcke pro sekunde>");
                    return;
                }
                update(s -> s.walkSpeed = speed);
                console.info(String.format(
                        "Laufgeschwindigkeit: %.3f Blöcke/s (4,317 = Gehen, 5,612 = Sprinten).",
                        settings.walkSpeed));
            }
            default -> console.error("Unbekannt: :home " + verb + " (siehe :home)");
        }
    }

    /** {@code :home set} (aktuelle Position) oder {@code :home set <x> <y> <z> [gier] [neigung]}. */
    private void setHome(String rest) {
        String[] words = rest.isEmpty() ? new String[0] : rest.split("\\s+");
        double[] numbers = new double[words.length];
        int count = 0;
        for (String word : words) {
            Double value = parseNumber(word);
            if (value != null) {
                numbers[count++] = value;
            }
        }

        MoveSettings.Spot spot;
        if (count == 0) {
            double[] p = position();
            if (p == null) {
                console.error("Position noch unbekannt (nicht im Spiel?).");
                return;
            }
            spot = new MoveSettings.Spot(p[0], p[1], p[2], (float) p[3], (float) p[4]);
        } else if (count >= 3) {
            spot = new MoveSettings.Spot(numbers[0], numbers[1], numbers[2],
                    count > 3 ? (float) numbers[3] : 0f,
                    count > 4 ? (float) numbers[4] : 0f);
        } else {
            console.error("Nutzung: :home set   oder   :home set <x> <y> <z> [gier] [neigung]");
            return;
        }

        update(s -> s.home = spot);
        console.print(console.color(Console.GREEN, "Heimatposition: " + spot.describe()));
        if (!settings.homeEnabled) {
            console.info("Noch nicht aktiv – mit  :home on  beim Beitritt automatisch dorthin laufen.");
        }
    }

    private void printHome() {
        console.print("");
        console.print(console.color(Console.BOLD, "  Heimatposition"));
        if (settings.home == null) {
            console.print(console.color(Console.GRAY,
                    "    (keine – mit  :home set  die aktuelle Position übernehmen)"));
        } else {
            String mark = settings.homeEnabled
                    ? console.color(Console.CYAN, "● an ")
                    : console.color(Console.GRAY, "○ aus");
            console.print("    " + mark + "  " + settings.home.describe());
            console.print(console.color(Console.GRAY, String.format(
                    "    Start %d s nach dem Beitritt  ·  %.3f Blöcke/s  ·  Fallen %s",
                    settings.homeDelaySeconds, settings.walkSpeed,
                    settings.autoFall ? "automatisch" : "aus")));
        }
        console.print(console.color(Console.GRAY,
                "    :home set   :home on|off   :home go   :home delay <sek>   :home clear"));
        if (!settings.route.isEmpty()) {
            console.print(console.color(Console.GRAY, "    Route: " + settings.route.size()
                    + " Wegpunkte werden vorher abgelaufen (:route)"));
        }
    }

    // ===================== Route =====================

    /**
     * Wegpunkte statt Wegfindung: Ecken, Türen und Treppen kennt der Nutzer, der Client läuft sie
     * nur nach. Aufgezeichnet wird, indem man die Strecke einmal mit {@code :go} abläuft – jedes
     * erreichte Ziel wird ein Wegpunkt.
     */
    private void route(String arg) {
        String[] parts = arg.trim().split("\\s+", 2);
        String verb = parts[0].toLowerCase();
        String rest = parts.length > 1 ? parts[1].trim() : "";

        switch (verb) {
            case "", "status", "list" -> printRoute();

            case "rec", "record", "aufnahme", "start" -> {
                if (position() == null) {
                    console.error("Position noch unbekannt (nicht im Spiel?).");
                    return;
                }
                synchronized (recordLock) {
                    if (recording != null) {
                        console.info("Vorige Aufzeichnung verworfen.");
                    }
                    recording = new ArrayList<>();
                }
                console.print(console.color(Console.GREEN, "Aufzeichnung läuft."));
                console.info("Laufe die Strecke jetzt mit  :go vor 5  usw. ab – jedes Ziel wird ein Wegpunkt.");
                console.info("Am Ziel angekommen:  :route stop");
            }

            case "stop", "ende", "fertig" -> routeStop();

            case "add", "punkt", "+" -> routeAdd();

            case "del", "rm", "-" -> routeDel(rest);

            case "clear", "leeren", "loeschen", "löschen" -> {
                synchronized (recordLock) {
                    recording = null;
                }
                update(s -> s.route.clear());
                console.info("Route gelöscht – es wird wieder geradeaus zur Heimatposition gelaufen.");
            }

            case "go", "los", "jetzt" -> {
                MoveSettings current = settings;
                if (current.home == null) {
                    console.error("Keine Heimatposition gesetzt (:home set).");
                    return;
                }
                goHome(current, 0L);
            }

            default -> console.error("Unbekannt: :route " + verb + " (siehe :route)");
        }
    }

    private void routeStop() {
        List<MoveSettings.Spot> points;
        synchronized (recordLock) {
            points = recording;
            recording = null;
        }
        if (points == null) {
            console.error("Es läuft keine Aufzeichnung (:route rec).");
            return;
        }
        if (points.isEmpty()) {
            // Kein Fehler, nur nichts zu tun – der Rust-Client meldet das an derselben Stelle
            // ebenfalls als Hinweis.
            console.warn("Nichts aufgezeichnet – Route unverändert.");
            return;
        }
        double[] here = position();
        update(s -> {
            // Ohne Heimatposition wird der Endpunkt der Aufzeichnung zum Ziel.
            if (s.home == null && here != null) {
                s.home = new MoveSettings.Spot(here[0], here[1], here[2], (float) here[3], (float) here[4]);
            }
            s.route = new ArrayList<>(points);
            // Der letzte Wegpunkt ist das Ziel selbst – als Zwischenstopp wäre er überflüssig.
            if (s.home != null && !s.route.isEmpty()
                    && s.route.get(s.route.size() - 1).flatDistance(s.home) < 1.0) {
                s.route.remove(s.route.size() - 1);
            }
        });
        console.print(console.color(Console.GREEN,
                "Route gespeichert: " + settings.route.size() + " Wegpunkte + Ziel."));
        if (!settings.homeEnabled) {
            console.info("Noch nicht aktiv – mit  :home on  bei jedem Beitritt ablaufen.");
        }
    }

    private void routeAdd() {
        double[] p = position();
        if (p == null) {
            console.error("Position noch unbekannt (nicht im Spiel?).");
            return;
        }
        MoveSettings.Spot spot = new MoveSettings.Spot(p[0], p[1], p[2], (float) p[3], (float) p[4]);
        synchronized (recordLock) {
            if (recording != null) {
                if (recording.size() >= MoveSettings.MAX_ROUTE) {
                    console.error("Mehr als " + MoveSettings.MAX_ROUTE + " Wegpunkte gehen nicht.");
                    return;
                }
                recording.add(spot);
                console.print(console.color(Console.GREEN, "Wegpunkt " + recording.size() + " aufgezeichnet."));
                return;
            }
        }
        if (settings.route.size() >= MoveSettings.MAX_ROUTE) {
            console.error("Mehr als " + MoveSettings.MAX_ROUTE + " Wegpunkte gehen nicht.");
            return;
        }
        update(s -> s.route.add(spot));
        console.print(console.color(Console.GREEN, "Wegpunkt " + settings.route.size() + " angehängt."));
    }

    private void routeDel(String rest) {
        int count = settings.route.size();
        if (count == 0) {
            console.error("Die Route ist leer.");
            return;
        }
        // Ohne Nummer den letzten – das ist beim Nachbessern der häufigste Fall.
        int index = count - 1;
        if (!rest.isEmpty()) {
            Double number = parseNumber(rest);
            if (number == null || number < 1 || number > count) {
                console.error("Nutzung: :route del <1.." + count + ">");
                return;
            }
            index = number.intValue() - 1;
        }
        final int target = index;
        // Zwischen dem Zählen oben und dem Ändern hier kann die Route schon kürzer sein (etwa
        // durch `:route clear`); ohne diese Prüfung flog dann eine IndexOutOfBoundsException.
        update(s -> {
            if (target < s.route.size()) {
                s.route.remove(target);
            }
        });
        console.info("Wegpunkt " + (target + 1) + " entfernt.");
    }

    private void printRoute() {
        MoveSettings current = settings;
        List<MoveSettings.Spot> live;
        synchronized (recordLock) {
            live = recording == null ? null : List.copyOf(recording);
        }

        console.print("");
        console.print(console.color(Console.BOLD, "  Route zur Heimatposition"));
        if (live != null) {
            console.print(console.color(Console.CYAN, "    ● Aufzeichnung läuft – " + live.size()
                    + " Wegpunkte. Beenden mit  :route stop"));
        }
        List<MoveSettings.Spot> points = live != null ? live : current.route;
        if (points.isEmpty()) {
            console.print(console.color(Console.GRAY,
                    "    (keine – ohne Route wird geradeaus zur Heimatposition gelaufen)"));
        }
        for (int index = 0; index < points.size(); index++) {
            MoveSettings.Spot point = points.get(index);
            console.print(String.format("    %d)  x=%.1f  y=%.1f  z=%.1f",
                    index + 1, point.x, point.y, point.z));
        }
        if (current.home == null) {
            console.print(console.color(Console.GRAY,
                    "    (noch kein Ziel – :home set  oder  :route stop am Zielpunkt)"));
        } else {
            console.print("    " + (points.size() + 1) + ")  "
                    + console.color(Console.CYAN, "Ziel") + "  " + current.home.describe());
        }
        console.print(console.color(Console.GRAY,
                "    :route rec   :route stop   :route add   :route del [nr]   :route go   :route clear"));
    }

    private void usageGo() {
        console.error("Nutzung: :go vor|zurück|links|rechts [blöcke]     z. B.  :go vor 5");
        console.info("Richtung ist relativ zum Blick (wie W/A/S/D). Ohne Zahl = 1 Block. :stop bricht ab.");
    }

    private void usageLook() {
        console.error("Nutzung: :look <gier> [neigung]  ·  :look links|rechts|hoch|runter [grad]");
        console.info("Auch: :look nord|ost|süd|west  ·  :look um  ·  :look gerade (Neigung 0)");
    }

    // ===================== Laufen und Drehen =====================

    private enum Outcome {
        ARRIVED,
        /** Kein Fortschritt mehr – vermutlich eine Wand. */
        STUCK,
        /** Notbremse {@code maxWalkSeconds}. */
        TIMEOUT,
        /** Verbindung weg, {@code :stop} oder ein neuerer Befehl. */
        CANCELLED,
        /** Der Server hat uns noch keine Position geschickt. */
        UNKNOWN
    }

    /** Aufgabe mit Ausweis: gilt nur für „ihre" Aufgabennummer. */
    private interface Job {
        void run(int id);
    }

    private void start(Job task) {
        final int id = job.incrementAndGet();
        Thread thread = new Thread(() -> task.run(id), "afk-move");
        thread.setDaemon(true);
        thread.start();
    }

    private boolean alive(int id) {
        AfkClient current = client;
        return job.get() == id
                && current != null
                && current.isInGame()
                && !Thread.currentThread().isInterrupted();
    }

    private double[] position() {
        AfkClient current = client;
        return current == null ? null : current.position();
    }

    /**
     * Geradlinig zum Punkt (x|z) laufen, Höhe unverändert.
     *
     * <p>Gerechnet wird in jedem Tick neu aus der <b>aktuellen</b> Position. Schiebt der Server uns
     * zurück (Wand) oder korrigiert die Höhe (Treppe/Gefälle), laufen wir von dort weiter – und
     * merken an der ausbleibenden Annäherung, wenn es gar nicht mehr vorangeht. Dann wird
     * ausgewichen (siehe {@link #escape}); erst nach mehreren vergeblichen Versuchen geben wir auf.
     *
     * <p>{@code targetY} ist die Zielhöhe, sofern bekannt (Wegpunkte einer Route haben eine). Dann
     * wird die Höhe gleichmäßig mitgezogen – so geht es Treppen hinauf und hinunter, ohne die Welt
     * zu kennen. Ohne Zielhöhe ({@code null}) wird stattdessen ab und zu nach unten getastet.
     */
    private Outcome walkTo(int id, double targetX, double targetZ, Double targetY) {
        MoveSettings current = settings;
        double step = current.step();
        long limitNanos = current.maxWalkSeconds * 1_000_000_000L;
        long started = System.nanoTime();
        double closest = Double.MAX_VALUE;
        long closestAt = System.nanoTime();
        long next = System.nanoTime();
        int escapes = 0;
        double sinceCheck = 0;

        while (true) {
            if (!alive(id)) {
                return Outcome.CANCELLED;
            }
            double[] p = position();
            if (p == null) {
                return Outcome.UNKNOWN;
            }

            double dx = targetX - p[0];
            double dz = targetZ - p[2];
            double distance = Math.sqrt(dx * dx + dz * dz);
            if (distance <= ARRIVED) {
                sendMove(targetX, targetY != null ? targetY : p[1], targetZ, (float) p[3], (float) p[4]);
                return Outcome.ARRIVED;
            }
            if (System.nanoTime() - started > limitNanos) {
                return Outcome.TIMEOUT;
            }
            if (distance < closest - ARRIVED) {
                closest = distance;
                closestAt = System.nanoTime();
            } else if (System.nanoTime() - closestAt > STUCK_AFTER_MILLIS * 1_000_000L) {
                if (++escapes > MAX_ESCAPES) {
                    return Outcome.STUCK;
                }
                Outcome escaped = escape(id, escapes, targetX, targetZ);
                if (escaped != Outcome.ARRIVED) {
                    return escaped;
                }
                // Nach dem Ausweichen stehen wir woanders – der Fortschritt zählt von vorn.
                closest = Double.MAX_VALUE;
                closestAt = System.nanoTime();
                next = System.nanoTime();
                continue;
            }

            double travel = Math.min(step, distance);
            // Höhe: bekannt -> gleichmäßig darauf zu (immer von der aktuellen Höhe aus gerechnet,
            // damit eine Korrektur des Servers einfach übernommen wird). Unbekannt -> unverändert.
            double nextY = targetY == null ? p[1] : p[1] + (targetY - p[1]) * (travel / distance);
            sendMove(p[0] + dx / distance * travel, nextY, p[2] + dz / distance * travel,
                    (float) p[3], (float) p[4]);
            next = sleepTick(next);

            // Ohne bekannte Zielhöhe: ab und zu prüfen, ob unter uns überhaupt noch Boden ist.
            if (targetY == null && current.autoFall) {
                sinceCheck += travel;
                if (sinceCheck >= current.fallCheckBlocks) {
                    sinceCheck = 0;
                    Outcome fell = fall(id, 0, 0, 0);
                    if (fell != Outcome.ARRIVED) {
                        return fell;
                    }
                    // Ein Sturz kostet Zeit, bringt aber waagerecht nichts – die Uhr neu stellen,
                    // sonst hielte der Fortschrittswächter das für ein Hindernis.
                    closestAt = System.nanoTime();
                    next = System.nanoTime();
                }
            }
        }
    }

    /**
     * Blindes Ausweichen – ohne jede Kenntnis der Welt, genau wie ein Mensch im Dunkeln.
     *
     * <p>Immer zuerst ein Sprung: das löst Stufen, Zäune und Teppichkanten, also den mit Abstand
     * häufigsten Fall. Ab dem zweiten Versuch geht es zusätzlich seitwärts am Hindernis vorbei –
     * abwechselnd links und rechts und mit jeder Runde einen Block weiter. Danach setzt der Lauf
     * neu an; findet er einen freien Weg, merkt er es an der wieder sinkenden Entfernung.
     */
    private Outcome escape(int id, int attempt, double targetX, double targetZ) {
        double[] p = position();
        if (p == null) {
            return Outcome.UNKNOWN;
        }
        double dx = targetX - p[0];
        double dz = targetZ - p[2];
        double length = Math.sqrt(dx * dx + dz * dz);
        if (length < 1e-6) {
            return Outcome.ARRIVED;
        }
        dx /= length;
        dz /= length;

        if (attempt == 1) {
            console.info("Etwas im Weg – springe ...");
            return jump(id, dx, dz);
        }

        // Abwechselnd links/rechts, je Runde einen Block weiter: 1,5 – 1,5 – 2,5 – 2,5 – 3,5 …
        boolean left = attempt % 2 == 0;
        double blocks = 1.5 + (attempt - 2) / 2;
        console.info(String.format("Immer noch blockiert – weiche %.1f Blöcke nach %s aus ...",
                blocks, left ? "links" : "rechts"));
        Outcome jumped = jump(id, dx, dz);
        if (jumped != Outcome.ARRIVED) {
            return jumped;
        }
        // 90° zur Laufrichtung (Minecraft-Konvention: rechts von (fx|fz) ist (−fz|fx)).
        return left ? strafe(id, dz, -dx, blocks) : strafe(id, -dz, dx, blocks);
    }

    /**
     * Ein Sprung nach Vanilla-Physik, dabei weiter in Laufrichtung. Beim Steigen und Fallen melden
     * wir ehrlich {@code onGround = false} – so sieht es aus wie bei jedem echten Client. Der
     * Rückweg nach unten ist ein ganz normaler Sturz, siehe {@link #fall}.
     */
    private Outcome jump(int id, double dx, double dz) {
        double step = settings.step();
        double vy = JUMP_SPEED;
        long next = System.nanoTime();

        // Aufwärts, solange der Sprung trägt.
        while (vy > 0) {
            if (!alive(id)) {
                return Outcome.CANCELLED;
            }
            double[] p = position();
            if (p == null) {
                return Outcome.UNKNOWN;
            }
            sendMove(p[0] + dx * step, p[1] + vy, p[2] + dz * step, (float) p[3], (float) p[4], false);
            vy = (vy - GRAVITY) * DRAG;
            next = sleepTick(next);
        }
        return fall(id, dx * step, dz * step, vy);
    }

    /**
     * Fallen, bis wir aufkommen: Treppe hinunter, von der Kante, nach einem Sprung.
     *
     * <p>{@code vy} ist die Startgeschwindigkeit (0 = einfach loslassen, negativ = wir fallen
     * schon), {@code dx}/{@code dz} eine waagerechte Drift <b>je Tick</b> in Blöcken.
     *
     * <p>Gelandet sind wir, sobald der Server uns nach oben korrigiert – er tut das, sobald wir
     * behaupten, in einem Block zu stecken. Das ist die einzige Bodeninformation, die ein Client
     * ohne Weltdaten überhaupt bekommen kann: steht unter uns etwas, kostet die Prüfung genau einen
     * Tick; steht dort nichts, fallen wir einfach weiter.
     */
    private Outcome fall(int id, double dx, double dz, double vy) {
        double[] start = position();
        if (start == null) {
            return Outcome.UNKNOWN;
        }
        double startY = start[1];
        long next = System.nanoTime();

        for (int tick = 0; tick < MAX_FALL_TICKS; tick++) {
            if (!alive(id)) {
                return Outcome.CANCELLED;
            }
            double[] p = position();
            if (p == null) {
                return Outcome.UNKNOWN;
            }
            vy = (vy - GRAVITY) * DRAG;
            double landing = p[1] + vy;
            // Notbremse: so tief geht es nur in die Leere, und dort hilft Fallen ohnehin nicht mehr.
            if (startY - landing > MAX_FALL_BLOCKS) {
                break;
            }
            sendMove(p[0] + dx, landing, p[2] + dz, (float) p[3], (float) p[4], false);
            next = sleepTick(next);
            // Hat der Server uns hochgesetzt, steht dort ein Block: wir sind aufgekommen.
            double[] after = position();
            if (after == null) {
                return Outcome.UNKNOWN;
            }
            if (after[1] > landing + CORRECTED) {
                break;
            }
        }

        // Wieder als „auf dem Boden" melden.
        double[] end = position();
        if (end == null) {
            return Outcome.UNKNOWN;
        }
        sendMove(end[0], end[1], end[2], (float) end[3], (float) end[4]);
        return Outcome.ARRIVED;
    }

    /** Ein Stück quer zur Laufrichtung gehen, ohne dabei auf das Ziel zu achten. */
    private Outcome strafe(int id, double sx, double sz, double blocks) {
        double step = settings.step();
        int ticks = (int) Math.ceil(blocks / step);
        long next = System.nanoTime();
        for (int tick = 0; tick < ticks; tick++) {
            if (!alive(id)) {
                return Outcome.CANCELLED;
            }
            double[] p = position();
            if (p == null) {
                return Outcome.UNKNOWN;
            }
            sendMove(p[0] + sx * step, p[1], p[2] + sz * step, (float) p[3], (float) p[4]);
            next = sleepTick(next);
        }
        return Outcome.ARRIVED;
    }

    /** Kopf über mehrere Ticks auf die Zielrichtung drehen (nicht ruckartig in einem Tick). */
    private Outcome turnTo(int id, float targetYaw, float targetPitch) {
        targetYaw = wrapDegrees(targetYaw);
        targetPitch = Math.max(-90f, Math.min(90f, targetPitch));
        float step = (float) settings.turnSpeed;
        long next = System.nanoTime();

        while (true) {
            if (!alive(id)) {
                return Outcome.CANCELLED;
            }
            double[] p = position();
            if (p == null) {
                return Outcome.UNKNOWN;
            }
            float yaw = (float) p[3];
            float pitch = (float) p[4];

            // Immer den kürzeren Weg herum drehen.
            float dYaw = wrapDegrees(targetYaw - yaw);
            float dPitch = targetPitch - pitch;
            if (Math.abs(dYaw) < 0.01f && Math.abs(dPitch) < 0.01f) {
                return Outcome.ARRIVED;
            }
            float nextYaw = wrapDegrees(yaw + Math.max(-step, Math.min(step, dYaw)));
            float nextPitch = pitch + Math.max(-step, Math.min(step, dPitch));
            sendMove(p[0], p[1], p[2], nextYaw, Math.max(-90f, Math.min(90f, nextPitch)));
            next = sleepTick(next);
        }
    }

    /** Position senden <b>und</b> den Zustand des Clients mitziehen. */
    private void sendMove(double x, double y, double z, float yaw, float pitch) {
        sendMove(x, y, z, yaw, pitch, true);
    }

    private void sendMove(double x, double y, double z, float yaw, float pitch, boolean onGround) {
        AfkClient current = client;
        if (current != null) {
            current.sendMove(x, y, z, yaw, pitch, onGround);
        }
    }

    private void report(Outcome outcome, String label) {
        switch (outcome) {
            case ARRIVED -> console.print(console.color(Console.GREEN, label + ": fertig."));
            case STUCK -> console.error(label + ": komme trotz Ausweichen nicht weiter – abgebrochen.");
            case TIMEOUT -> console.error(label + ": Zeitlimit erreicht – abgebrochen.");
            case UNKNOWN -> console.error(label + ": Position unbekannt – abgebrochen.");
            case CANCELLED -> {
            }
        }
    }

    /**
     * Bis zum nächsten Tick schlafen und den nächsten Termin zurückgeben. Hinken wir hinterher
     * (Standby, ausgelastetes System), wird der Takt neu aufgesetzt statt aufzuholen.
     */
    private static long sleepTick(long next) {
        long target = next + TICK_MILLIS * 1_000_000L;
        long wait = target - System.nanoTime();
        if (wait <= 0) {
            return System.nanoTime();
        }
        try {
            Thread.sleep(wait / 1_000_000L, (int) (wait % 1_000_000L));
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        return target;
    }

    /** Kurzschlaf in Scheiben, damit {@code :stop} und eine Trennung sofort wirken. */
    private boolean nap(int id, long millis) {
        long until = System.nanoTime() + millis * 1_000_000L;
        while (System.nanoTime() < until) {
            if (!alive(id)) {
                return false;
            }
            try {
                Thread.sleep(Math.min(100L, Math.max(1L, (until - System.nanoTime()) / 1_000_000L)));
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return false;
            }
        }
        return alive(id);
    }

    private boolean waitForPosition(int id) {
        long until = System.nanoTime() + POSITION_TIMEOUT_MILLIS * 1_000_000L;
        while (position() == null) {
            if (!alive(id)) {
                return false;
            }
            if (System.nanoTime() >= until) {
                console.error("Heimatposition: Der Server hat keine Position geschickt.");
                return false;
            }
            try {
                Thread.sleep(100);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return false;
            }
        }
        return true;
    }

    // ===================== Richtungen und Winkel =====================

    /** Richtung relativ zur Blickrichtung – genau wie W/A/S/D im echten Client. */
    private enum Direction {
        FORWARD("vorwärts"),
        BACK("rückwärts"),
        LEFT("links"),
        RIGHT("rechts");

        final String label;

        Direction(String label) {
            this.label = label;
        }

        static Direction parse(String word) {
            return switch (word) {
                case "vor", "vorne", "vorwaerts", "vorwärts", "w", "forward", "f" -> FORWARD;
                case "zurueck", "zurück", "rueckwaerts", "rückwärts", "s", "back", "b" -> BACK;
                case "links", "a", "left", "l" -> LEFT;
                case "rechts", "d", "right", "r" -> RIGHT;
                default -> null;
            };
        }

        /**
         * Einheitsvektor {x, z} bei gegebenem Gierwinkel. Minecraft: Gierwinkel 0 = Süden (+Z),
         * 90 = Westen (−X); vorwärts ist also (−sin, cos), rechts davon (−cos, −sin).
         */
        double[] vector(float yaw) {
            double radians = Math.toRadians(yaw);
            double sin = Math.sin(radians);
            double cos = Math.cos(radians);
            return switch (this) {
                case FORWARD -> new double[]{-sin, cos};
                case BACK -> new double[]{sin, -cos};
                case RIGHT -> new double[]{-cos, -sin};
                case LEFT -> new double[]{cos, sin};
            };
        }
    }

    /** Gierwinkel auf (−180, 180] normieren – der kürzere Drehweg lässt sich so direkt ablesen. */
    static float wrapDegrees(float value) {
        float wrapped = value % 360f;
        if (wrapped > 180f) {
            wrapped -= 360f;
        }
        if (wrapped <= -180f) {
            wrapped += 360f;
        }
        return wrapped;
    }

    /** Himmelsrichtung zum Gierwinkel (Minecraft: 0 = Süden, 90 = Westen). */
    static String compass(float yaw) {
        String[] names = {"Süd", "Südwest", "West", "Nordwest", "Nord", "Nordost", "Ost", "Südost"};
        int index = Math.floorMod(Math.round(wrapDegrees(yaw) / 45f), 8);
        return names[index];
    }

    /** Zahl mit Punkt oder Komma („2,5"). */
    private static Double parseNumber(String text) {
        try {
            double value = Double.parseDouble(text.trim().replace(',', '.'));
            return Double.isFinite(value) ? value : null;
        } catch (RuntimeException e) {
            return null;
        }
    }
}
