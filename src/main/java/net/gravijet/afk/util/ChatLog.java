package net.gravijet.afk.util;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardOpenOption;
import java.time.LocalDateTime;
import java.time.format.DateTimeFormatter;

/**
 * Haengt Chat-Zeilen mit Zeitstempel an eine Logdatei an (~/.config/hugoafk/chat.log).
 */
public class ChatLog {

    private static final DateTimeFormatter FORMAT = DateTimeFormatter.ofPattern("yyyy-MM-dd HH:mm:ss");

    private final Path file;

    public ChatLog(Path file) {
        this.file = file;
    }

    public synchronized void append(String line) {
        try {
            Files.createDirectories(file.getParent());
            String entry = "[" + LocalDateTime.now().format(FORMAT) + "] " + line + System.lineSeparator();
            Files.writeString(file, entry, StandardCharsets.UTF_8,
                    StandardOpenOption.CREATE, StandardOpenOption.APPEND);
        } catch (IOException ignored) {
            // Logging ist nicht kritisch.
        }
    }
}
