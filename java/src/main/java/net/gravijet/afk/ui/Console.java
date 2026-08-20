package net.gravijet.afk.ui;

import java.io.BufferedReader;
import java.io.IOException;
import java.io.InputStreamReader;
import java.nio.charset.StandardCharsets;

/**
 * Ein-/Ausgabe – bewusst zeilenweise statt Terminal-Bibliothek.
 *
 * <p>Die Aufteilung ist die ganze Bedienung des Clients:
 * <ul>
 *   <li><b>Standardausgabe</b>: ausschließlich Chat. Nichts anderes landet dort, damit ein
 *       Programm davor (z. B. die spätere Website) die Zeilen unverändert weiterreichen kann.</li>
 *   <li><b>Standardfehlerausgabe</b>: Verbindungszustand, Fehler, Hinweise. Mit {@code --quiet}
 *       bleibt davon nur noch, was wirklich schiefgeht.</li>
 *   <li><b>Standardeingabe</b>: jede Zeile geht als Chat-Nachricht bzw. – mit {@code /} vorn – als
 *       Serverbefehl raus.</li>
 * </ul>
 *
 * <p><b>Asynchrone Ausgabe:</b> {@link #chat} reiht die Zeile nur in eine Queue ein; geschrieben
 * wird sie von einem einzelnen Hintergrund-Thread. Damit blockiert – womöglich spammender – Chat
 * NIE den Netz-Thread, der die Keep-Alive-Antwort senden muss. Genau diese Verzögerung konnte
 * sonst einen {@code disconnect.timeout} auslösen.
 */
public class Console {

    // Das Escape-Zeichen steht hier als Unicode-Escape (U+001B), nicht als rohes Byte. Mit dem
    // rohen Byte galt diese Datei git als Binaerdatei: kein Diff, kein Zusammenfuehren, keine
    // Durchsicht im Pull Request.
    public static final String RESET = "\u001B[0m";
    public static final String GRAY = "\u001B[90m";
    public static final String RED = "\u001B[91m";
    public static final String GREEN = "\u001B[92m";
    public static final String CYAN = "\u001B[96m";
    public static final String YELLOW = "\u001B[93m";
    public static final String BOLD = "\u001B[1m";

    private final BufferedReader in =
            new BufferedReader(new InputStreamReader(System.in, StandardCharsets.UTF_8));

    /**
     * So viele Chatzeilen dürfen höchstens auf das Schreiben warten.
     *
     * <p>Die Warteschlange war unbegrenzt. Liest das Programm davor die Standardausgabe gerade
     * nicht mit, läuft die Pipe voll, der Schreib-Thread bleibt im {@code println} stehen – und
     * die Warteschlange wächst danach ohne Ende weiter. Jetzt fällt die älteste Zeile heraus,
     * dieselbe Regel wie in der Sendewarteschlange.
     */
    private static final int MAX_QUEUE = 256;

    private final java.util.concurrent.BlockingQueue<String> queue =
            new java.util.concurrent.LinkedBlockingQueue<>();
    /**
     * Läuft die Warteschlange gerade über? Gemeldet wird nur der Beginn – sonst käme je
     * ausgelassener Zeile eine eigene Warnung.
     */
    private final java.util.concurrent.atomic.AtomicBoolean overflowing =
            new java.util.concurrent.atomic.AtomicBoolean();
    private final boolean color;
    private final boolean quiet;

    public Console(boolean color, boolean quiet) {
        this.color = color;
        this.quiet = quiet;
        Thread worker = new Thread(this::drain, "afk-console");
        worker.setDaemon(true);
        worker.start();
        // Der Ausgabestrom ist gepuffert und der Schreib-Thread ein Daemon: Beim Beenden – ein
        // Kick ohne Reconnect ruft System.exit – verschwanden dadurch die zuletzt empfangenen
        // Chatzeilen, ausgerechnet die mit dem Grund. Der Haken schreibt sie noch heraus.
        Runtime.getRuntime().addShutdownHook(new Thread(this::flushOnExit, "afk-console-ende"));
    }

    /**
     * Beim Beenden: das noch Wartende hinausschreiben – aber höchstens zwei Sekunden lang.
     *
     * <p>Das Schreiben selbst läuft in einem Daemon-Thread, auf den hier nur begrenzt gewartet
     * wird. Ohne dieses Zeitlimit hinge das Beenden für immer, wenn niemand die Standardausgabe
     * abholt: {@code println} blockiert dann in der vollen Pipe, und ein hängender Abschluss-Haken
     * hält die ganze JVM.
     */
    private void flushOnExit() {
        Thread drain = new Thread(this::flushRemaining, "afk-console-rest");
        drain.setDaemon(true);
        drain.start();
        try {
            drain.join(2000);
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
        System.err.flush();
    }

    /** Alles noch Wartende schreiben und den Puffer leeren. */
    private void flushRemaining() {
        for (String line = queue.poll(); line != null; line = queue.poll()) {
            System.out.println(line);
        }
        System.out.flush();
    }

    public boolean isColor() {
        return color;
    }

    /** Text einfärben – ohne Farbe unverändert zurück. */
    public String color(String code, String text) {
        return color ? code + text + RESET : text;
    }

    /** Eine Chat-Zeile. Das Einzige, was auf der Standardausgabe erscheint. */
    public void chat(String text) {
        boolean dropped = false;
        while (queue.size() >= MAX_QUEUE && queue.poll() != null) {
            dropped = true;
        }
        queue.offer(text);
        if (dropped && overflowing.compareAndSet(false, true)) {
            warn("Die Standardausgabe wird nicht gelesen – Chatzeilen fallen heraus.");
        }
    }

    private void drain() {
        try {
            while (true) {
                String line = queue.take();
                System.out.println(line);
                // Erst leeren, wenn nichts mehr wartet: bei Chat-Spam spart das Systemaufrufe,
                // bei einzelnen Zeilen bleibt die Ausgabe sofort sichtbar.
                if (queue.isEmpty()) {
                    System.out.flush();
                    // Wieder aufgeholt: Die nächste Überlaufmeldung darf wieder kommen.
                    overflowing.set(false);
                }
            }
        } catch (InterruptedException e) {
            Thread.currentThread().interrupt();
        }
    }

    /** Zustandsmeldung (Standardfehlerausgabe, mit {@code --quiet} unterdrückt). */
    public void print(String text) {
        if (!quiet) {
            System.err.println(text);
        }
    }

    public void info(String text) {
        print(color(GRAY, text));
    }

    public void note(String text) {
        print(color(CYAN, text));
    }

    public void ok(String text) {
        print(color(GREEN, text));
    }

    /** Hinweis, der auffallen soll, aber kein Fehler ist. */
    public void warn(String text) {
        print(color(YELLOW, text));
    }

    /** Fehler kommen auch mit {@code --quiet} durch – sonst stünde man ratlos vor einem stummen Client. */
    public void error(String text) {
        System.err.println(color(RED, text));
    }

    /**
     * Liest eine Eingabezeile. {@code null} bedeutet, dass es keine Eingabe (mehr) gibt – etwa im
     * Dienstbetrieb ohne Terminal.
     */
    public String readLine() {
        try {
            return in.readLine();
        } catch (IOException e) {
            return null;
        }
    }
}
