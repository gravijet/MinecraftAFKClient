package net.gravijet.afk.config;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/**
 * JSON-Konfiguration unter ~/.config/hugoafk/config.json.
 * Merkt sich Server- und Verhaltenseinstellungen. Alle Felder haben sinnvolle
 * Standardwerte, sodass eine fehlende oder unvollstaendige Datei nie zum Absturz fuehrt.
 *
 * <p><b>Wichtig zum Timeout-Kick:</b> Der frueher vorhandene bewegungsbasierte Anti-AFK
 * (Drehen/Swing) wurde entfernt. Gegen den {@code disconnect.timeout} hilft NICHT der
 * {@code /afk}-Befehl (der teleportiert nur in die AFK-Welt), sondern ein echter
 * Verbindungs-Keep-Alive: ein periodisches, <em>stationaeres</em> Positionspaket (gleiche
 * Koordinaten) - genau das, was ein stehender Vanilla-Client sendet. Siehe
 * {@link #keepAliveEnabled} / {@link #keepAliveIntervalMs}.
 */
public class Config {

    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();

    // =====================================================================
    // Verbindung
    // =====================================================================
    public String lastServer = "";
    public boolean autoReconnect = true;
    public int reconnectDelaySeconds = 5;
    /** 0 = unbegrenzt viele Reconnect-Versuche. */
    public int maxReconnectAttempts = 0;
    /** Zufaelliger Zusatz (0..N ms) auf jede Reconnect-Wartezeit, gegen gleichfoermiges Muster. */
    public int reconnectJitterMs = 1500;
    /** Obergrenze fuer den exponentiellen Backoff in Sekunden. */
    public int maxBackoffSeconds = 300;
    /**
     * Alternative Server, die der Reihe nach probiert werden, falls der Hauptserver
     * nicht erreichbar ist. Format: "host" oder "host:port".
     */
    public List<String> fallbackServers = new ArrayList<>();
    /**
     * Enthaelt die Trennungsursache einen dieser Texte, wird NICHT automatisch neu
     * verbunden (z. B. Bann/Whitelist) - verhindert sinnloses Dauer-Reconnecten.
     */
    public List<String> dontReconnectOnReasons = new ArrayList<>(List.of(
            "banned", "gebannt", "verbannt", "whitelist", "blacklist", "ban"));
    /** Geplanter Neustart der Verbindung alle N Minuten (0 = aus). Haelt Sessions frisch. */
    public int scheduledRestartMinutes = 0;
    /**
     * Kommt N Sekunden lang KEIN Paket vom Server, proaktiv neu verbinden
     * (Watchdog gegen "halb tote" Verbindungen; 0 = aus).
     *
     * <p>Standard 60s: ein gesunder Server schickt mindestens alle ~15s ein Keep-Alive, daher
     * bedeutet eine Stille von 60s praktisch immer eine tote/eingefrorene Verbindung (z. B. ein
     * stilles {@code disconnect.endOfStream}, das nie als Trennung ankommt). Statt scheinbar
     * "online" festzuhaengen, verbinden wir dann selbst neu.
     */
    public int inboundSilenceTimeoutSeconds = 60;

    // =====================================================================
    // Keep-Alive (echter Timeout-Schutz - ersetzt den alten Bewegungs-Anti-AFK)
    // =====================================================================
    /**
     * Sendet periodisch ein stationaeres Positionspaket (gleiche Koordinaten/Blickrichtung),
     * damit der Server/Proxy die Verbindung nicht wegen Inaktivitaet trennt
     * ({@code disconnect.timeout}). Bewegt den Spieler NICHT.
     */
    public boolean keepAliveEnabled = true;
    /** Intervall des Keep-Alive-Positionspakets in Millisekunden (>= 500). */
    public int keepAliveIntervalMs = 1000;

    // =====================================================================
    // Befehle bei Ereignissen
    // =====================================================================
    /** Befehle direkt nach jedem erfolgreichen Beitritt (z. B. "/login pass", "/afk"). */
    public List<String> onJoinCommands = new ArrayList<>();
    public int onJoinDelaySeconds = 3;
    /**
     * Befehle, die zusaetzlich NUR nach einem Kick + erneutem Beitritt laufen
     * (z. B. wieder "/afk"). Erfuellt: "wenn man gekickt wird, fuehre Befehl X aus".
     */
    public List<String> onKickCommands = new ArrayList<>();
    public int onKickDelaySeconds = 3;
    /** Befehle, die beim Tod (vor/nach Respawn) gesendet werden (z. B. "/warp spawn"). */
    public List<String> onDeathCommands = new ArrayList<>();

    // =====================================================================
    // Periodische Befehle (frei konfigurierbar, z. B. regelmaessig "/afk")
    // =====================================================================
    /** Liste eigener Wiederholbefehle, jeweils mit eigenem Intervall. */
    public List<PeriodicCommand> periodicCommands = new ArrayList<>();

    /** Ein periodisch gesendeter Befehl. */
    public static class PeriodicCommand {
        public boolean enabled = true;
        public String command = "";
        public int intervalSeconds = 300;

        public PeriodicCommand() {
        }

        public PeriodicCommand(String command, int intervalSeconds) {
            this.command = command;
            this.intervalSeconds = intervalSeconds;
        }
    }

    // =====================================================================
    // Auto-TPA (Teleport-Anfragen automatisch annehmen)
    // =====================================================================
    public boolean autoAcceptTpa = false;
    /** Wenn nicht leer: nur Anfragen von diesen Spielern annehmen. */
    public List<String> autoAcceptTpaWhitelist = new ArrayList<>();
    /** Annahme-Befehl; der Spielername wird angehaengt -> "/tpaccept Spieler". */
    public String tpaAcceptCommand = "/tpaccept";
    /** Textbaustein, an dem eine eingehende TPA-Anfrage erkannt wird. */
    public String tpaRequestMarker = "Teleportations-Anfrage";

    // =====================================================================
    // Auto-Antwort auf private Nachrichten
    // =====================================================================
    public boolean autoReplyEnabled = false;
    public String autoReplyMessage = "Bin gerade AFK, melde mich spaeter!";
    /** Befehl zum Antworten; Empfaenger wird angehaengt -> "/msg Spieler <Text>". */
    public String autoReplyCommand = "/msg";
    /** Textbaustein, an dem eine eingehende private Nachricht erkannt wird. */
    public String privateMessageMarker = "-> Du:";
    /** Mindestabstand zwischen zwei Auto-Antworten an denselben Spieler (Sekunden). */
    public int autoReplyCooldownSeconds = 60;

    // =====================================================================
    // Chat-Filter (Spam ausblenden)
    // =====================================================================
    public boolean chatFilterEnabled = true;
    /** Chat-Zeilen, die einen dieser Texte enthalten, werden NICHT angezeigt. */
    public List<String> chatHideFilters = new ArrayList<>(List.of(
            "sucht einen 1vs1-RTP-Gegner",
            "/rtpqueue beitreten"));
    /** Komplett stummschalten (kein eingehender Chat wird angezeigt). */
    public boolean muteChat = false;
    /**
     * Wenn nicht leer: NUR Chat-Zeilen anzeigen, die einen dieser Texte enthalten
     * (Whitelist-Modus). Highlights werden immer angezeigt.
     */
    public List<String> chatShowOnly = new ArrayList<>();
    /** Spieler, deren Nachrichten ausgeblendet werden (Ignorierliste). */
    public List<String> ignoredPlayers = new ArrayList<>();
    /** Identische Folgezeilen unterdruecken (Anti-Wiederholungs-Spam). */
    public boolean collapseDuplicates = false;

    // =====================================================================
    // Auto-Responder (Stichwort -> Antwort)
    // =====================================================================
    /** Regeln: enthaelt der Chat einen Ausloeser, wird automatisch geantwortet. */
    public List<Trigger> triggers = new ArrayList<>();

    /** Eine Auto-Responder-Regel. */
    public static class Trigger {
        public boolean enabled = true;
        /** Ausloeser-Text (Teilstring, case-insensitiv). */
        public String contains = "";
        /** Antwort/Befehl, der gesendet wird (z. B. "Hallo!" oder "/spawn"). */
        public String response = "";
        /** Mindestabstand zwischen zwei Ausloesungen derselben Regel (Sekunden). */
        public int cooldownSeconds = 30;

        public Trigger() {
        }

        public Trigger(String contains, String response, int cooldownSeconds) {
            this.contains = contains;
            this.response = response;
            this.cooldownSeconds = cooldownSeconds;
        }
    }

    // =====================================================================
    // Gesundheit
    // =====================================================================
    public boolean autoRespawn = true;
    /** Befehl bei niedrigem Leben (z. B. "/warp spawn"). */
    public boolean lowHealthActionEnabled = false;
    public double lowHealthThreshold = 6.0;
    public List<String> lowHealthCommands = new ArrayList<>();

    // =====================================================================
    // Anzeige & Benachrichtigung
    // =====================================================================
    public boolean showTimestamps = true;
    public boolean logChat = true;
    public boolean highlightUsername = true;
    public boolean bellOnHighlight = true;
    public List<String> highlightKeywords = new ArrayList<>();
    public boolean announcePlayerJoinLeave = false;
    /** Bei Verbinden/Trennen die Terminal-Glocke laeuten. */
    public boolean bellOnDisconnect = false;
    /** Farbige Ausgabe (ANSI). Bei false werden Farbcodes weggelassen. */
    public boolean colorOutput = true;
    /** Anzahl zuletzt gepufferter Chat-Zeilen fuer :history. */
    public int chatHistorySize = 100;
    /** Maximale Groesse der Chat-Logdatei in Bytes vor Rotation. */
    public long maxLogBytes = 5_000_000L;

    // =====================================================================
    // Befehls-Aliase (z. B. "h" -> "/home", Eingabe ":h")
    // =====================================================================
    /** Eigene Kurzbefehle: Alias -> auszufuehrende Eingabe (Chat oder /Befehl). */
    public java.util.Map<String, String> commandAliases = new java.util.LinkedHashMap<>();

    // =====================================================================
    // Rate-Limit (gegen Spam-Kick)
    // =====================================================================
    /** Mindestabstand zwischen ausgehenden Nachrichten in ms. */
    public int chatMinDelayMs = 1100;

    private transient Path file;

    public static Config load(Path file) {
        Config config;
        try {
            if (Files.exists(file)) {
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
        config.normalize();
        config.file = file;
        // Beim ersten Start (oder nach Schema-Erweiterung) Defaults persistieren.
        config.save();
        return config;
    }

    /** Sorgt fuer gueltige Werte (aeltere/teilweise Dateien, null-Listen, Mindestwerte). */
    private void normalize() {
        if (highlightKeywords == null) highlightKeywords = new ArrayList<>();
        if (onJoinCommands == null) onJoinCommands = new ArrayList<>();
        if (onKickCommands == null) onKickCommands = new ArrayList<>();
        if (onDeathCommands == null) onDeathCommands = new ArrayList<>();
        if (fallbackServers == null) fallbackServers = new ArrayList<>();
        if (autoAcceptTpaWhitelist == null) autoAcceptTpaWhitelist = new ArrayList<>();
        if (chatHideFilters == null) chatHideFilters = new ArrayList<>();
        if (periodicCommands == null) periodicCommands = new ArrayList<>();
        if (lowHealthCommands == null) lowHealthCommands = new ArrayList<>();
        if (chatShowOnly == null) chatShowOnly = new ArrayList<>();
        if (ignoredPlayers == null) ignoredPlayers = new ArrayList<>();
        if (triggers == null) triggers = new ArrayList<>();
        if (dontReconnectOnReasons == null) dontReconnectOnReasons = new ArrayList<>();
        if (commandAliases == null) commandAliases = new java.util.LinkedHashMap<>();
        if (tpaAcceptCommand == null || tpaAcceptCommand.isBlank()) tpaAcceptCommand = "/tpaccept";
        if (tpaRequestMarker == null) tpaRequestMarker = "Teleportations-Anfrage";
        if (autoReplyCommand == null || autoReplyCommand.isBlank()) autoReplyCommand = "/msg";
        if (privateMessageMarker == null) privateMessageMarker = "-> Du:";
        if (autoReplyMessage == null) autoReplyMessage = "";
        if (keepAliveIntervalMs < 500) keepAliveIntervalMs = 500;
        if (chatMinDelayMs < 200) chatMinDelayMs = 200;
        if (maxBackoffSeconds < 1) maxBackoffSeconds = 1;
        if (chatHistorySize < 10) chatHistorySize = 10;
        if (maxLogBytes < 100_000L) maxLogBytes = 100_000L;
    }

    /**
     * Liest die Konfigurationsdatei neu ein und uebernimmt alle Werte in dieses Objekt.
     * Bestehende Referenzen auf das Config-Objekt bleiben damit gueltig (fuer :reload).
     */
    public synchronized boolean reload() {
        if (file == null || !Files.exists(file)) {
            return false;
        }
        try {
            Config fresh = GSON.fromJson(Files.readString(file), Config.class);
            if (fresh == null) {
                return false;
            }
            fresh.normalize();
            for (java.lang.reflect.Field f : Config.class.getDeclaredFields()) {
                int mod = f.getModifiers();
                if (java.lang.reflect.Modifier.isStatic(mod)
                        || java.lang.reflect.Modifier.isTransient(mod)) {
                    continue;
                }
                f.setAccessible(true);
                f.set(this, f.get(fresh));
            }
            return true;
        } catch (Exception e) {
            return false;
        }
    }

    public void save() {
        if (file == null) {
            return;
        }
        try {
            Files.createDirectories(file.getParent());
            Files.writeString(file, GSON.toJson(this));
        } catch (IOException e) {
            // Speichern ist nicht kritisch - still ignorieren.
        }
    }
}
