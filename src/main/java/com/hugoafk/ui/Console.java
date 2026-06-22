package com.hugoafk.ui;

import org.jline.reader.EndOfFileException;
import org.jline.reader.LineReader;
import org.jline.reader.LineReaderBuilder;
import org.jline.reader.UserInterruptException;
import org.jline.terminal.Terminal;
import org.jline.terminal.TerminalBuilder;

import java.io.IOException;
import java.util.Collection;
import java.util.List;
import java.util.function.Supplier;

/**
 * Schlanke Konsolen-Oberflaeche auf JLine-Basis.
 * Eingehender Chat wird per {@link #printAbove} ueber der Eingabezeile ausgegeben,
 * damit eine gerade getippte Nachricht nicht zerstoert wird.
 */
public class Console {

    public static final String RESET = "\u001b[0m";
    public static final String GRAY = "\u001b[90m";
    public static final String RED = "\u001b[91m";
    public static final String HIGHLIGHT = "\u001b[1m\u001b[93m";
    public static final String BELL = "\u0007";

    private final Terminal terminal;
    private final LineReader reader;
    private volatile Supplier<Collection<String>> playerNames = List::of;

    public Console() throws IOException {
        this.terminal = TerminalBuilder.builder().system(true).build();
        this.reader = LineReaderBuilder.builder()
                .terminal(terminal)
                .completer(new HugoCompleter(() -> playerNames.get()))
                .build();
    }

    /** Liefert die aktuellen Online-Spielernamen fuer die Tab-Vervollstaendigung. */
    public void setPlayerNameSupplier(Supplier<Collection<String>> supplier) {
        this.playerNames = supplier != null ? supplier : List::of;
    }

    /** Gibt eine Zeile oberhalb der Eingabezeile aus (klobbert die Eingabe nicht). */
    public void printAbove(String text) {
        reader.printAbove(text);
    }

    public void info(String text) {
        printAbove(GRAY + text + RESET);
    }

    public void error(String text) {
        printAbove(RED + text + RESET);
    }

    /** Liest eine Zeile. Gibt {@code null} bei EOF/Ctrl-D oder Ctrl-C zurueck. */
    public String readLine(String prompt) {
        try {
            return reader.readLine(prompt);
        } catch (UserInterruptException | EndOfFileException e) {
            return null;
        }
    }

    public void close() {
        try {
            terminal.close();
        } catch (IOException ignored) {
        }
    }
}
