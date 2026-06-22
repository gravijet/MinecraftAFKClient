package com.hugoafk.config;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/**
 * Einfache JSON-Konfiguration unter ~/.config/hugoafk/config.json.
 * Merkt sich Server- und Verhaltenseinstellungen.
 */
public class Config {

    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();

    // Verbindung
    public String lastServer = "";
    public boolean autoReconnect = true;
    public int reconnectDelaySeconds = 5;
    /** 0 = unbegrenzt viele Reconnect-Versuche. */
    public int maxReconnectAttempts = 0;

    // Anti-AFK
    public boolean antiAfkEnabled = true;
    public int antiAfkSeconds = 60;

    // Anzeige
    public boolean showTimestamps = true;
    public boolean logChat = true;

    // Benachrichtigungen
    public boolean highlightUsername = true;
    public boolean bellOnHighlight = true;
    public List<String> highlightKeywords = new ArrayList<>();

    // Spielerliste
    public boolean announcePlayerJoinLeave = false;

    // Spielverhalten
    public boolean autoRespawn = true;

    // Robustheit / Komfort
    /** Mindestabstand zwischen ausgehenden Nachrichten (gegen Spam-Kick). */
    public int chatMinDelayMs = 1100;
    /** Befehle, die nach dem Beitritt automatisch gesendet werden (z. B. "/login pass"). */
    public List<String> onJoinCommands = new ArrayList<>();
    public int onJoinDelaySeconds = 3;

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
        if (config.highlightKeywords == null) {
            config.highlightKeywords = new ArrayList<>();
        }
        if (config.onJoinCommands == null) {
            config.onJoinCommands = new ArrayList<>();
        }
        config.file = file;
        return config;
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
