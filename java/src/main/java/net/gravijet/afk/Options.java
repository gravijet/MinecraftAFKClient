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

    public boolean color = true;
    /** Keine Statusmeldungen – nur noch Chat auf der Standardausgabe. */
    public boolean quiet = false;
    public long chatMinDelayMs = 1000;

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
                case "--reconnect-delay" ->
                        o.reconnectDelaySeconds = Math.max(1, number(value(args, ++i, "--reconnect-delay"), "--reconnect-delay"));
                case "--max-backoff" ->
                        o.maxBackoffSeconds = Math.max(1, number(value(args, ++i, "--max-backoff"), "--max-backoff"));
                case "--chat-delay" ->
                        o.chatMinDelayMs = Math.max(MIN_CHAT_DELAY_MS, number(value(args, ++i, "--chat-delay"), "--chat-delay"));
                case "--no-reconnect" -> o.autoReconnect = false;
                case "--no-color" -> o.color = false;
                case "-q", "--quiet" -> o.quiet = true;
                default -> {
                    if (arg.startsWith("-") || !o.server.isEmpty()) {
                        throw new IllegalArgumentException(
                                "Unbekannte Option '" + arg + "'. --help zeigt alle.");
                    }
                    o.server = arg;
                }
            }
        }

        if (o.server.isBlank()) {
            throw new IllegalArgumentException(
                    "Kein Server angegeben. Beispiel: java -jar afk-" + minecraftVersion
                            + ".jar --server mc.example.net");
        }
        o.server = o.server.trim();
        for (String raw : rawCommands) {
            o.commands.add(parseCommand(raw, joinDelay));
        }
        return o;
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
