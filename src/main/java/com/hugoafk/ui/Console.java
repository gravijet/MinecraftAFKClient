package com.hugoafk.ui;

import org.jline.reader.EndOfFileException;
import org.jline.reader.LineReader;
import org.jline.reader.LineReaderBuilder;
import org.jline.reader.UserInterruptException;
import org.jline.terminal.Terminal;
import org.jline.terminal.TerminalBuilder;

import java.io.IOException;

/**
 * Schlanke Konsolen-Oberflaeche auf JLine-Basis.
 * Eingehender Chat wird per {@link #printAbove} ueber der Eingabezeile ausgegeben,
 * damit eine gerade getippte Nachricht nicht zerstoert wird.
 */
public class Console {

    private final Terminal terminal;
    private final LineReader reader;

    public Console() throws IOException {
        this.terminal = TerminalBuilder.builder().system(true).build();
        this.reader = LineReaderBuilder.builder().terminal(terminal).build();
    }

    /** Gibt eine Zeile oberhalb der Eingabezeile aus (klobbert die Eingabe nicht). */
    public void printAbove(String text) {
        reader.printAbove(text);
    }

    public void info(String text) {
        printAbove("[90m" + text + "[0m");
    }

    public void error(String text) {
        printAbove("[91m" + text + "[0m");
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
