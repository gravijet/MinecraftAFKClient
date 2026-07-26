package net.gravijet.afk.ui;

import org.jline.reader.EndOfFileException;
import org.jline.reader.LineReader;
import org.jline.reader.LineReaderBuilder;
import org.jline.reader.UserInterruptException;
import org.jline.reader.impl.completer.StringsCompleter;
import org.jline.terminal.Terminal;
import org.jline.terminal.TerminalBuilder;

import java.io.IOException;
import java.util.concurrent.BlockingQueue;
import java.util.concurrent.LinkedBlockingQueue;

/**
 * Schlanke Konsolen-Oberfläche auf JLine-Basis.
 *
 * <p><b>Asynchrone Ausgabe:</b> {@link #printAbove} reiht die Zeile nur in eine Queue ein; das
 * eigentliche (potenziell blockierende) Schreiben ins Terminal erledigt ein einzelner Hintergrund-
 * Thread. Damit blockiert das Rendern von – womöglich spammendem – Chat NIE den Netz-Thread, der die
 * Keep-Alive-Antwort senden muss. Genau diese Verzögerung konnte sonst einen {@code disconnect.timeout}
 * auslösen.
 */
public class Console {

    public static final String RESET = "[0m";
    public static final String GRAY = "[90m";
    public static final String RED = "[91m";
    public static final String GREEN = "[92m";
    public static final String CYAN = "[96m";
    public static final String BOLD = "[1m";

    private static final String POISON = " __STOP__";

    private final Terminal terminal;
    private final LineReader reader;
    private volatile boolean color = true;

    private final BlockingQueue<String> output = new LinkedBlockingQueue<>();
    private final Thread outputWorker;
    private volatile boolean running = true;

    public Console() throws IOException {
        this.terminal = TerminalBuilder.builder().system(true).build();
        this.reader = LineReaderBuilder.builder()
                .terminal(terminal)
                .completer(new StringsCompleter(
                        ":help", ":reconnect", ":account", ":server", ":clear", ":quit", ":exit"))
                .build();
        this.outputWorker = new Thread(this::drainOutput, "hugoafk-console");
        this.outputWorker.setDaemon(true);
        this.outputWorker.start();
    }

    public void setColor(boolean enabled) {
        this.color = enabled;
    }

    public boolean isColor() {
        return color;
    }

    /** Reiht eine Zeile zur Ausgabe oberhalb der Eingabezeile ein (nicht blockierend, thread-sicher). */
    public void printAbove(String text) {
        if (running) {
            output.offer(text);
        }
    }

    private void drainOutput() {
        try {
            while (running) {
                String line = output.take();
                if (POISON.equals(line)) {
                    return;
                }
                writeNow(line);
                while ((line = output.poll()) != null) {
                    if (POISON.equals(line)) {
                        return;
                    }
                    writeNow(line);
                }
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    private void writeNow(String text) {
        try {
            reader.printAbove(text);
        } catch (Exception ignored) {
            // Ausgabe ist nicht kritisch – niemals den Worker-Thread sterben lassen.
        }
    }

    public void info(String text) {
        printAbove(color ? GRAY + text + RESET : text);
    }

    public void error(String text) {
        printAbove(color ? RED + text + RESET : text);
    }

    /** Synchron ausgeben (für Menüs, bevor die Eingabe folgt). */
    public void print(String text) {
        writeNow(text);
    }

    public String color(String code, String text) {
        return color ? code + text + RESET : text;
    }

    public void clearScreen() {
        printAbove("[2J[H");
    }

    /** Liest eine Zeile. Gibt {@code null} bei EOF/Ctrl-D oder Ctrl-C zurück. */
    public String readLine(String prompt) {
        try {
            return reader.readLine(prompt);
        } catch (UserInterruptException | EndOfFileException e) {
            return null;
        }
    }

    public void close() {
        running = false;
        output.offer(POISON);
        try {
            outputWorker.join(1000);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        try {
            terminal.close();
        } catch (IOException ignored) {
        }
    }
}
