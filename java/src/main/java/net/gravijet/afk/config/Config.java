package net.gravijet.afk.config;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;

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

    /** Nach einem echten Beitritt (Proxy-Login) automatisch einen Befehl senden. */
    public boolean autoCommandEnabled = true;
    /** Automatisch zu sendender Befehl nach dem Beitritt (z. B. "/afk"). Leer = aus. */
    public String autoCommand = "/afk";
    /** Verzögerung in Sekunden zwischen Beitritt und dem Auto-Befehl. */
    public int autoCommandDelaySeconds = 4;

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
        config.save();
        return config;
    }

    private void normalize() {
        if (lastServer == null) lastServer = "";
        if (activeAccount == null) activeAccount = "";
        if (reconnectDelaySeconds < 1) reconnectDelaySeconds = 1;
        if (maxBackoffSeconds < 1) maxBackoffSeconds = 1;
        if (chatMinDelayMs < 200) chatMinDelayMs = 200;
        if (autoCommand == null) autoCommand = "";
        if (autoCommandDelaySeconds < 0) autoCommandDelaySeconds = 0;
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
