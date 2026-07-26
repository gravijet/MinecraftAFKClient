package net.gravijet.afk.config;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/**
 * Schlanke JSON-Konfiguration unter ~/.config/hugoafk/config.json.
 *
 * <p>Bewusst minimal gehalten: nur das Nötige für „verbinden, nicht gekickt werden,
 * Chat/Befehle senden und empfangen". Alle früheren Komfort-/Bloat-Felder wurden entfernt.
 * Alle Felder haben sinnvolle Standardwerte, sodass eine fehlende oder unvollständige
 * Datei nie zum Absturz führt.
 */
public class Config {

    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();

    /** Untergrenze für Wiederholungen: schneller löst nur der Spam-Schutz des Servers aus. */
    private static final int MIN_REPEAT_SECONDS = 5;

    /** Zuletzt genutzte Server-Adresse (host[:port]). */
    public String lastServer = "";
    /** Name des aktiven Microsoft-Kontos (Datei accounts/<name>.json). */
    public String activeAccount = "";

    /** Bei Verbindungsabbruch automatisch neu verbinden (Kick-Schutz-relevant). */
    public boolean autoReconnect = true;
    /** Basis-Wartezeit vor dem ersten Reconnect (Sekunden); danach exponentieller Backoff. */
    public int reconnectDelaySeconds = 5;
    /** Obergrenze für den Backoff in Sekunden. */
    public int maxBackoffSeconds = 60;

    /** Farbige Ausgabe (ANSI). Bei false werden Farbcodes weggelassen. */
    public boolean colorOutput = true;
    /** Mindestabstand zwischen zwei ausgehenden Nachrichten in ms (gegen Spam-Kick). */
    public int chatMinDelayMs = 1000;

    /**
     * Befehle, die nach einem echten Beitritt (Proxy-Login) laufen – beliebig viele, jeder mit
     * eigener Startverzögerung und eigenem Wiederholungsintervall. Dieselbe Liste nutzt der
     * Rust-Client.
     */
    public List<AutoCommand> commands = new ArrayList<>();

    // ---- Altfelder (einzelner Auto-Befehl) ----
    // Werden beim ersten Start in `commands` überführt und danach nicht mehr gelesen. Sie bleiben
    // in der Datei, damit eine ältere Client-Version nicht stolpert.
    public boolean autoCommandEnabled = true;
    public String autoCommand = "";
    public int autoCommandDelaySeconds = 4;

    private transient Path file;

    /** Ein wiederkehrender Befehl. {@code repeatSeconds = 0} heißt: nur einmal je Beitritt. */
    public static class AutoCommand {
        /** Was gesendet wird, z. B. "/afk". Ohne "/" geht es als normale Chat-Nachricht raus. */
        public String command = "";
        /** Sekunden nach dem Beitritt bis zur ersten Ausführung. */
        public int delaySeconds = 4;
        /** Wiederholung in Sekunden; 0 = nur einmal je Beitritt. */
        public int repeatSeconds = 0;
        public boolean enabled = true;

        public AutoCommand() {
        }

        public AutoCommand(String command, int delaySeconds, int repeatSeconds) {
            this.command = command == null ? "" : command.trim();
            this.delaySeconds = delaySeconds;
            // Wie in normalize(): schneller als alle MIN_REPEAT_SECONDS löst nur den Spam-Schutz aus.
            this.repeatSeconds = repeatSeconds <= 0 ? 0 : Math.max(MIN_REPEAT_SECONDS, repeatSeconds);
        }

        /** Kurzbeschreibung für Menü und Hilfe. */
        public String describe() {
            String when = repeatSeconds == 0
                    ? "einmalig, " + delaySeconds + " s nach Beitritt"
                    : "erst nach " + delaySeconds + " s, dann alle " + prettySeconds(repeatSeconds);
            return command + "   (" + when + ")";
        }
    }

    /** 90 -&gt; "1 min 30 s", 300 -&gt; "5 min", 45 -&gt; "45 s" */
    public static String prettySeconds(int seconds) {
        if (seconds < 60) {
            return seconds + " s";
        }
        int minutes = seconds / 60;
        int rest = seconds % 60;
        return rest == 0 ? minutes + " min" : minutes + " min " + rest + " s";
    }

    public static Config load(Path file) {
        Config config;
        boolean existed = Files.exists(file);
        try {
            if (existed) {
                config = GSON.fromJson(Files.readString(file), Config.class);
                if (config == null) {
                    config = new Config();
                }
            } else {
                config = new Config();
            }
        } catch (Exception e) {
            config = new Config();
        }
        if (existed) {
            config.migrateLegacyCommand();
        } else {
            config.commands.add(new AutoCommand("/afk", 4, 0));
        }
        config.normalize();
        config.file = file;
        config.save();
        return config;
    }

    /**
     * Eine bestehende Datei mit einzelnem {@code autoCommand} in die neue Liste überführen. Das
     * Altfeld wird dabei geleert, damit die Migration nicht bei jedem Start erneut zuschlägt und
     * einen gelöschten Befehl wiederbelebt.
     */
    private void migrateLegacyCommand() {
        if (commands == null) {
            commands = new ArrayList<>();
        }
        String legacy = autoCommand == null ? "" : autoCommand.trim();
        if (legacy.isEmpty()) {
            return;
        }
        boolean known = commands.stream().anyMatch(c -> c != null && legacy.equals(c.command));
        if (!known) {
            AutoCommand migrated = new AutoCommand(legacy, Math.max(0, autoCommandDelaySeconds), 0);
            migrated.enabled = autoCommandEnabled;
            commands.add(migrated);
        }
        autoCommand = "";
    }

    private void normalize() {
        if (lastServer == null) lastServer = "";
        if (activeAccount == null) activeAccount = "";
        if (reconnectDelaySeconds < 1) reconnectDelaySeconds = 1;
        if (maxBackoffSeconds < 1) maxBackoffSeconds = 1;
        if (chatMinDelayMs < 200) chatMinDelayMs = 200;
        if (autoCommand == null) autoCommand = "";
        if (autoCommandDelaySeconds < 0) autoCommandDelaySeconds = 0;
        if (commands == null) {
            commands = new ArrayList<>();
        }
        commands.removeIf(c -> c == null || c.command == null || c.command.trim().isEmpty());
        for (AutoCommand command : commands) {
            command.command = command.command.trim();
            if (command.delaySeconds < 0) command.delaySeconds = 0;
            if (command.repeatSeconds < 0) command.repeatSeconds = 0;
            if (command.repeatSeconds > 0) {
                command.repeatSeconds = Math.max(MIN_REPEAT_SECONDS, command.repeatSeconds);
            }
        }
    }

    /** Die Befehle, die tatsächlich laufen sollen. */
    public List<AutoCommand> activeCommands() {
        List<AutoCommand> active = new ArrayList<>();
        for (AutoCommand command : commands) {
            if (command.enabled && !command.command.isEmpty()) {
                active.add(command);
            }
        }
        return active;
    }

    public void save() {
        if (file == null) {
            return;
        }
        try {
            Files.createDirectories(file.getParent());
            Files.writeString(file, GSON.toJson(this));
        } catch (IOException e) {
            // Speichern ist nicht kritisch – still ignorieren.
        }
    }
}
