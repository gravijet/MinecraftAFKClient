package net.gravijet.afk;

import java.util.ArrayList;
import java.util.List;

/**
 * Startargumente. Der Client hat weder Menü noch Konfigurationsdatei: alles, was er wissen muss,
 * steht im Startbefehl. Gespeichert wird nur, was gespeichert werden muss – die Microsoft-Konten
 * unter {@code ~/.config/afksystems/accounts/}.
 *
 * <p>Die Optionen sind absichtlich dieselben wie beim Rust-Client, damit ein Aufrufer (z. B. die
 * spätere Website) beide gleich starten kann.
 */
public final class Options {

    /** Untergrenze für Wiederholungen: schneller löst nur der Spam-Schutz des Servers aus. */
    private static final long MIN_REPEAT_SECONDS = 5;
    /** Untergrenze für den Abstand zweier ausgehender Nachrichten. */
    private static final long MIN_CHAT_DELAY_MS = 200;

    /** Was der Aufruf verlangt. */
    public enum Mode {RUN, HELP, LOGIN, ACCOUNTS}

    /** Ein Befehl, der nach dem Beitritt läuft. {@code repeatSeconds = 0} heißt: nur einmal. */
    public record AutoCommand(String command, long delaySeconds, long repeatSeconds) {
    }

    public Mode mode = Mode.RUN;
    public String server = "";
    public String account = null;
    public final List<AutoCommand> commands = new ArrayList<>();

    public boolean autoReconnect = true;
    public long reconnectDelaySeconds = 5;
    public long maxBackoffSeconds = 60;

    /**
     * Höchstzahl erfolgloser Reconnect-Versuche, {@code 0} für unbegrenzt.
     *
     * <p>Gezählt werden nur Versuche, die es nicht bis in die Spielphase geschafft haben; ein
     * gelungener Beitritt setzt den Zähler zurück. Sonst würde ein Bot, der seit Tagen läuft und
     * dabei zehnmal kurz die Verbindung verloren hat, beim elften Mal aufgeben.
     */
    public long reconnectTries = 0;

    public boolean color = true;
    /** Keine Statusmeldungen – nur noch Chat auf der Standardausgabe. */
    public boolean quiet = false;
    public long chatMinDelayMs = 1000;

    /**
     * Sichtweite in Chunks, die dem Server gemeldet wird. Der Client wertet keinen einzigen Chunk
     * aus – MCProtocolLib entpackt sie trotzdem alle in Objekte, und genau das ist der größte
     * Posten im Speicherbedarf. Mit dem kleinsten erlaubten Wert fällt der Löwenanteil weg.
     */
    public int viewDistance = 2;

    /**
     * Optionen, die es nur im Rust-Client gibt. Ein Panel schickt allen Bauformen dieselbe
     * Befehlszeile – dieses Jar darf daran nicht scheitern, sondern nimmt sie an und sagt, dass
     * es sie nicht kann. Der Wert sagt, ob dahinter noch ein Argument steht.
     */
    private static final java.util.Map<String, Boolean> RUST_ONLY = java.util.Map.ofEntries(
            java.util.Map.entry("--offline", true),
            java.util.Map.entry("--cracked", true),
            java.util.Map.entry("--proxy", true),
            java.util.Map.entry("--fakehost", true),
            java.util.Map.entry("--on", true),
            java.util.Map.entry("--on-cooldown", true),
            java.util.Map.entry("--antiafk", true),
            java.util.Map.entry("--pov", true),
            java.util.Map.entry("--ansicht", true),
            java.util.Map.entry("--pov-size", true),
            java.util.Map.entry("--pov-groesse", true),
            java.util.Map.entry("--pov-fps", true),
            java.util.Map.entry("--pov-web", true),
            java.util.Map.entry("--pov-resources", true),
            java.util.Map.entry("--pov-assets", true),
            java.util.Map.entry("--events", false),
            java.util.Map.entry("--sneak", false));

    /** Optionen der Befehlszeile, die dieses Jar angenommen, aber nicht umgesetzt hat. */
    public final List<String> ignored = new ArrayList<>();

    /**
     * Startargumente auswerten.
     *
     * @param minecraftVersion Version dieses Jars – {@code --mc} darf nur genau dazu passen.
     * @throws IllegalArgumentException mit einem Text, der direkt für den Nutzer taugt
     */
    public static Options parse(String[] args, String minecraftVersion) {
        Options o = new Options();
        // Wartezeit nach dem Beitritt, bevor der erste Befehl rausgeht. Der Server braucht einen
        // Moment, bis er Chat von uns überhaupt annimmt.
        long joinDelay = 4;
        List<String> rawCommands = new ArrayList<>();
        // Erst am Ende gesetzt, damit --no-reconnect unabhängig von der Reihenfolge gewinnt:
        // Wer '--reconnect --no-reconnect' schreibt, meint das Abschalten, und wer es umgekehrt
        // schreibt, ebenso. Der Rust-Client verhält sich an derselben Stelle genauso.
        Boolean reconnectOn = null;

        for (int i = 0; i < args.length; i++) {
            String arg = args[i];
            switch (arg) {
                case "-h", "--help" -> {
                    o.mode = Mode.HELP;
                    return o;
                }
                case "--login" -> {
                    o.mode = Mode.LOGIN;
                    return o;
                }
                case "--accounts" -> {
                    o.mode = Mode.ACCOUNTS;
                    return o;
                }
                case "-s", "--server" -> o.server = value(args, ++i, "--server");
                case "-a", "--account" -> o.account = value(args, ++i, "--account");
                case "-m", "--mc", "--version" -> {
                    String wanted = value(args, ++i, "--mc").trim();
                    if (!wanted.equals(minecraftVersion)) {
                        throw new IllegalArgumentException(
                                "Dieses Jar spricht Minecraft " + minecraftVersion + ", nicht "
                                        + wanted + ". Nimm afk-" + wanted + ".jar.");
                    }
                }
                case "-c", "--cmd" -> rawCommands.add(value(args, ++i, "--cmd"));
                case "--join-delay" -> joinDelay = number(value(args, ++i, "--join-delay"), "--join-delay");
                case "--reconnect-delay" -> {
                    o.reconnectDelaySeconds = Math.max(1, number(value(args, ++i, "--reconnect-delay"), "--reconnect-delay"));
                    if (reconnectOn == null) {
                        reconnectOn = true;
                    }
                }
                case "--max-backoff" -> {
                    o.maxBackoffSeconds = Math.max(1, number(value(args, ++i, "--max-backoff"), "--max-backoff"));
                    if (reconnectOn == null) {
                        reconnectOn = true;
                    }
                }
                case "--reconnect-tries" -> {
                    o.reconnectTries = number(value(args, ++i, "--reconnect-tries"), "--reconnect-tries");
                    if (reconnectOn == null) {
                        reconnectOn = true;
                    }
                }
                case "--chat-delay" ->
                        o.chatMinDelayMs = Math.max(MIN_CHAT_DELAY_MS, number(value(args, ++i, "--chat-delay"), "--chat-delay"));
                case "--view-distance", "--sichtweite" -> o.viewDistance =
                        (int) Math.min(32, Math.max(2, number(value(args, ++i, "--view-distance"), "--view-distance")));
                case "--reconnect" -> {
                    if (reconnectOn == null) {
                        reconnectOn = true;
                    }
                }
                case "--no-reconnect" -> reconnectOn = false;
                case "--no-color" -> o.color = false;
                case "-q", "--quiet" -> o.quiet = true;
                default -> {
                    Boolean takesValue = RUST_ONLY.get(arg);
                    if (takesValue != null) {
                        // Nur im Rust-Client vorhanden: annehmen, überspringen, später melden.
                        if (takesValue) {
                            value(args, ++i, arg);
                        }
                        o.ignored.add(arg);
                    } else if (arg.startsWith("-") || !o.server.isEmpty()) {
                        throw new IllegalArgumentException(
                                "Unbekannte Option '" + arg + "'. --help zeigt alle.");
                    } else {
                        o.server = arg;
                    }
                }
            }
        }

        o.autoReconnect = reconnectOn == null || reconnectOn;
        o.maxBackoffSeconds = Math.max(o.maxBackoffSeconds, o.reconnectDelaySeconds);

        if (o.server.isBlank()) {
            throw new IllegalArgumentException(
                    "Kein Server angegeben. Beispiel: java -jar afk-" + minecraftVersion
                            + ".jar --server mc.example.net");
        }
        o.server = o.server.trim();
        checkServer(o.server);
        for (String raw : rawCommands) {
            o.commands.add(parseCommand(raw, joinDelay));
        }
        return o;
    }

    /**
     * Serveradresse auf einen brauchbaren Port prüfen.
     *
     * <p>Zerlegt wird sie später in {@code Main.parseHost}; dort ist ein unlesbarer Port
     * stillschweigend zu 25565 geworden. Ein Tippfehler wie {@code mc.example.net:2556x} führte
     * damit zu einer Verbindung auf einen ganz anderen Port – und zur Fehlersuche am falschen
     * Ende. Der Rust-Client prüft an derselben Stelle genauso.
     */
    private static void checkServer(String server) {
        String port;
        if (server.startsWith("[")) {
            // `[::1]:25565`: nur was hinter der schließenden Klammer steht, kann ein Port sein.
            int end = server.indexOf(']');
            if (end < 0) {
                throw new IllegalArgumentException(
                        "Serveradresse ohne schließende Klammer: '" + server + "'");
            }
            if (server.substring(1, end).isBlank()) {
                throw new IllegalArgumentException("Serveradresse ohne Namen: '" + server + "'");
            }
            String rest = server.substring(end + 1);
            if (!rest.isEmpty() && !rest.startsWith(":")) {
                throw new IllegalArgumentException(
                        "Unerlaubter Text hinter der IPv6-Adresse: '" + rest
                                + "'. Beispiel: [::1]:25565");
            }
            port = rest.startsWith(":") ? rest.substring(1) : null;
        } else if (server.indexOf(':') != server.lastIndexOf(':')) {
            port = null; // nackte IPv6-Adresse – da ist kein Port dabei
        } else {
            int colon = server.lastIndexOf(':');
            if (colon == 0) {
                throw new IllegalArgumentException("Serveradresse ohne Namen: '" + server + "'");
            }
            port = colon < 0 ? null : server.substring(colon + 1);
        }
        if (port == null) {
            return;
        }
        // Port 0 ist kein Ziel, sondern die Bitte an das Betriebssystem, sich einen auszusuchen.
        try {
            int value = Integer.parseInt(port.trim());
            if (value >= 1 && value <= 65535) {
                return;
            }
        } catch (NumberFormatException ignored) {
            // fällt unten in dieselbe Meldung
        }
        throw new IllegalArgumentException(
                "Server-Port ist keine Zahl zwischen 1 und 65535: '" + port
                        + "'. Beispiel: mc.example.net:25565");
    }

    /** {@code --cmd /afk} (einmalig) oder {@code --cmd 300:/afk} (alle 300 s). */
    private static AutoCommand parseCommand(String input, long delaySeconds) {
        long repeat = 0;
        String command = input;
        int colon = input.indexOf(':');
        if (colon > 0) {
            // Nur zerlegen, wenn vorn wirklich eine Zahl steht – sonst zerschnitte man Befehle,
            // die selbst einen Doppelpunkt tragen.
            try {
                repeat = Long.parseLong(input.substring(0, colon).trim());
                command = input.substring(colon + 1);
            } catch (NumberFormatException ignored) {
                // kein Intervall, ganzer Text ist der Befehl
            }
        }
        command = command.trim();
        if (command.isEmpty()) {
            throw new IllegalArgumentException("--cmd braucht einen Befehl, z. B. --cmd 300:/afk");
        }
        return new AutoCommand(command, delaySeconds, repeat == 0 ? 0 : Math.max(MIN_REPEAT_SECONDS, repeat));
    }

    private static String value(String[] args, int index, String name) {
        if (index >= args.length) {
            throw new IllegalArgumentException(name + " braucht einen Wert.");
        }
        return args[index];
    }

    private static long number(String text, String name) {
        try {
            return Long.parseLong(text.trim());
        } catch (NumberFormatException e) {
            throw new IllegalArgumentException(name + " braucht eine Zahl, nicht '" + text + "'.");
        }
    }
}
