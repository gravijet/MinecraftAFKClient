package com.hugoafk.config;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;

import java.io.IOException;
import java.nio.file.Files;
import java.nio.file.Path;

/**
 * Einfache JSON-Konfiguration unter ~/.config/hugoafk/config.json.
 * Merkt sich den zuletzt genutzten Server und die AFK-Einstellungen.
 */
public class Config {

    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();

    public String lastServer = "";
    public boolean autoReconnect = true;
    public int reconnectDelaySeconds = 10;
    public boolean antiAfkEnabled = true;
    public int antiAfkSeconds = 60;

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
