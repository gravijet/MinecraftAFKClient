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
        if (tpaAcceptCommand == null || tpaAcceptCommand.isBlank()) tpaAcceptCommand = "/tpaccept";
        if (tpaRequestMarker == null) tpaRequestMarker = "Teleportations-Anfrage";
        if (autoReplyCommand == null || autoReplyCommand.isBlank()) autoReplyCommand = "/msg";
        if (privateMessageMarker == null) privateMessageMarker = "-> Du:";
        if (autoReplyMessage == null) autoReplyMessage = "";
        if (keepAliveIntervalMs < 500) keepAliveIntervalMs = 500;
        if (chatMinDelayMs < 200) chatMinDelayMs = 200;
        if (maxBackoffSeconds < 1) maxBackoffSeconds = 1;
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
