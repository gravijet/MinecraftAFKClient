package net.gravijet.afk.ui;

import org.jline.reader.Candidate;
import org.jline.reader.Completer;
import org.jline.reader.LineReader;
import org.jline.reader.ParsedLine;

import java.util.Collection;
import java.util.List;
import java.util.function.Supplier;

/**
 * Tab-Vervollstaendigung: interne ':'-Befehle am Zeilenanfang, sonst Online-Spielernamen.
 */
public class HugoCompleter implements Completer {

    private static final List<String> COMMANDS = List.of(
            ":help", ":status", ":stats", ":config", ":players", ":list", ":server",
            ":reconnect", ":keepalive", ":tpa", ":reply", ":filter", ":mute", ":periodic",
            ":join", ":kickcmd", ":death", ":hide", ":highlight", ":set", ":quit", ":exit");

    private final Supplier<Collection<String>> playerNames;

    public HugoCompleter(Supplier<Collection<String>> playerNames) {
        this.playerNames = playerNames;
    }

    @Override
    public void complete(LineReader reader, ParsedLine line, List<Candidate> candidates) {
        String buffer = line.line();
        if (buffer.startsWith(":") && !buffer.contains(" ")) {
            for (String cmd : COMMANDS) {
                candidates.add(new Candidate(cmd));
            }
        } else {
            Collection<String> names = playerNames.get();
            if (names != null) {
                for (String name : names) {
                    candidates.add(new Candidate(name));
                }
            }
        }
    }
}
