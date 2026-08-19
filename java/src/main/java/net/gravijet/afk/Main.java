package net.gravijet.afk;

import net.gravijet.afk.auth.AuthManager;
import net.gravijet.afk.net.AfkClient;
import net.gravijet.afk.ui.Console;

import java.io.FileDescriptor;
import java.io.FileOutputStream;
import java.io.PrintStream;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.Paths;

/**
 * AFKSystems (Java) – schlanker Minecraft-AFK-Client.
 *
 * <p>Ein Jar je Minecraft-Version ({@code afk-26.1.jar}, {@code afk-1.21.1.jar} ...), weil
 * MCProtocolLib pro Build nur eine Protokollversion spricht. Welche es ist, steht im Manifest und
 * unten in der Hilfe.
 *
 * <p>Kein Menü, keine Konfigurationsdatei: alles steht im Startbefehl. Der Client meldet sich an,
 * tritt bei und gibt danach ausschließlich Chat auf der Standardausgabe aus – Statusmeldungen
 * gehen auf die Standardfehlerausgabe, Eingabezeilen gehen als Chat raus.
 *
 * <pre>java -jar afk-26.1.jar mc.example.net -c 300:/afk</pre>
 */
public class Main {

    public static void main(String[] args) {
        // Alles in UTF-8 ausgeben – sonst zerlegt die Windows-Konsole jeden Umlaut und ein
        // Programm, das den Chat weiterverarbeitet, bekommt Zeichen in wechselnder Kodierung.
        System.setOut(new PrintStream(new FileOutputStream(FileDescriptor.out), false, StandardCharsets.UTF_8));
        System.setErr(new PrintStream(new FileOutputStream(FileDescriptor.err), true, StandardCharsets.UTF_8));

        String version = minecraftVersion();

        Options options;
        try {
            options = Options.parse(args, version);
        } catch (IllegalArgumentException e) {
            System.err.println(e.getMessage());
            System.exit(2);
            return;
        }

        switch (options.mode) {
            case HELP -> printUsage(version);
            case ACCOUNTS -> printAccounts();
            case LOGIN -> addAccount();
            case RUN -> run(options, version);
        }
    }

    private static void run(Options options, String version) {
        Console console = new Console(options.color, options.quiet);
        // Das Panel schickt allen Bauformen dieselbe Befehlszeile. Was dieses Jar davon nicht
        // kann, wird angenommen und einmal benannt – statt beim Start abzubrechen.
        if (!options.ignored.isEmpty()) {
            console.warn(String.join(", ", options.ignored)
                    + " gibt es nur im Rust-Client; wird ignoriert.");
        }
        migrateConfigDir();

        AuthManager auth = new AuthManager(configDir());
        try {
            auth.loginInteractive(options.account, console::print);
        } catch (Exception e) {
            console.error("Login fehlgeschlagen: " + e.getMessage());
            System.exit(1);
            return;
        }

        int[] portHolder = {25565};
        String host = parseHost(options.server, portHolder);

        AfkClient client = new AfkClient(auth, options, console, version);
        client.connect(host, portHolder[0]);

        // Jede Eingabezeile geht in den Chat. Nur im Bewegungs-Jar werden ':'-Befehle vorher
        // abgefangen; im schlanken Jar gibt es keine, dort ist jede Zeile Chat.
        boolean movement = client.mover().available();
        String line;
        while ((line = console.readLine()) != null) {
            line = line.trim();
            if (line.isEmpty()) {
                continue;
            }
            if (movement && line.startsWith(":")) {
                String[] parts = line.substring(1).split("\\s+", 2);
                if (client.mover().command(parts[0].toLowerCase(), parts.length > 1 ? parts[1].trim() : "")) {
                    continue;
                }
                console.error("Unbekannter Befehl: " + line);
                continue;
            }
            client.sendChatInput(line);
        }

        // Standardeingabe zu Ende (Dienstbetrieb, Pipe, kein Terminal): der Client läuft weiter,
        // die Netz-Threads arbeiten. Beendet wird er von außen.
        parkForever();
    }

    private static void parkForever() {
        while (true) {
            try {
                Thread.sleep(Long.MAX_VALUE);
            } catch (InterruptedException e) {
                Thread.currentThread().interrupt();
                return;
            }
        }
    }

    // ===================== Konten =====================

    /** {@code --login}: nur anmelden und beenden. */
    private static void addAccount() {
        Console console = new Console(true, false);
        migrateConfigDir();
        try {
            String name = new AuthManager(configDir()).addAccount(console::print);
            System.out.println(name);
            System.out.flush();
        } catch (Exception e) {
            console.error("Login fehlgeschlagen: " + e.getMessage());
            System.exit(1);
        }
    }

    /** {@code --accounts}: gespeicherte Konten auflisten (eines je Zeile). */
    private static void printAccounts() {
        migrateConfigDir();
        new AuthManager(configDir()).listAccounts().forEach(System.out::println);
        System.out.flush();
    }

    // ===================== Host/Port =====================

    /** {@code host}, {@code host:port} oder {@code [::1]:port} zerlegen. */
    private static String parseHost(String input, int[] holder) {
        String value = input.trim();
        if (value.startsWith("[")) { // IPv6: [::1]:25565
            int end = value.indexOf(']');
            if (end > 0) {
                int colon = value.indexOf(':', end);
                if (colon > 0) {
                    holder[0] = port(value.substring(colon + 1), holder[0]);
                }
                return value.substring(1, end);
            }
            return value;
        }
        int colon = value.lastIndexOf(':');
        if (colon > -1 && value.indexOf(':') == colon) {
            holder[0] = port(value.substring(colon + 1), holder[0]);
            return value.substring(0, colon);
        }
        return value;
    }

    private static int port(String text, int fallback) {
        try {
            return Integer.parseInt(text.trim());
        } catch (NumberFormatException e) {
            System.err.println("Ungültiger Port – benutze " + fallback + ".");
            return fallback;
        }
    }

    // ===================== Verzeichnis / Version =====================

    /**
     * Verzeichnis mit {@code accounts/} (und {@code movement.json} im Bewegungs-Jar). Reine
     * Pfadauskunft: liegt nur das alte {@code hugoafk}-Verzeichnis vor, wird dessen Pfad geliefert.
     */
    public static Path configDir() {
        Path base = configBase();
        Path dir = base.resolve("afksystems");
        Path legacy = base.resolve("hugoafk");
        if (!Files.exists(dir) && Files.isDirectory(legacy)) {
            return legacy;
        }
        return dir;
    }

    /**
     * Einmalige Umbenennung {@code hugoafk} -> {@code afksystems}, damit bestehende Anmeldungen
     * erhalten bleiben. Schlägt sie fehl, arbeitet {@link #configDir()} am alten Ort weiter.
     */
    private static void migrateConfigDir() {
        Path base = configBase();
        Path dir = base.resolve("afksystems");
        Path legacy = base.resolve("hugoafk");
        if (!Files.exists(dir) && Files.isDirectory(legacy)) {
            try {
                Files.move(legacy, dir);
            } catch (Exception ignored) {
                // Nicht schlimm – dann bleibt es beim alten Verzeichnis.
            }
        }
    }

    private static Path configBase() {
        String xdg = System.getenv("XDG_CONFIG_HOME");
        if (xdg != null && !xdg.isBlank()) {
            return Paths.get(xdg);
        }
        return Paths.get(System.getProperty("user.home"), ".config");
    }

    /** Die Minecraft-Version dieses Jars – vom Build ins Manifest geschrieben. */
    private static String minecraftVersion() {
        String version = Main.class.getPackage().getImplementationVersion();
        return version != null && !version.isBlank() ? version : "?";
    }

    // ===================== Hilfe =====================

    private static void printUsage(String version) {
        System.out.println("""
                AFKSystems – schlanker Minecraft-AFK-Client für Minecraft %s

                Aufruf:  java -jar afk-%s.jar <host[:port]> [optionen]

                Optionen:
                  -s, --server <host[:port]>  Serveradresse (geht auch ohne -s als erstes Argument)
                  -a, --account <name>        gespeichertes Konto (Standard: das erste)
                  -m, --mc <version>          muss zu diesem Jar passen (%s)
                  -c, --cmd [sek:]<befehl>    Befehl nach dem Beitritt, mehrfach angebbar.
                                              Ohne 'sek:' einmalig, sonst alle 'sek' Sekunden.
                                              Beispiel: -c 300:/afk
                      --join-delay <sek>      Wartezeit nach dem Beitritt vor dem ersten Befehl (4)
                      --no-reconnect          nach einem Abbruch nicht neu verbinden
                      --reconnect-delay <sek> erste Wartezeit vor dem Reconnect (5)
                      --max-backoff <sek>     Obergrenze der Reconnect-Wartezeit (60)
                      --chat-delay <ms>       Mindestabstand ausgehender Nachrichten (1000)
                      --view-distance <2-32>  gemeldete Sichtweite in Chunks (2)
                      --no-color              keine Farben
                  -q, --quiet                 keine Statusmeldungen, nur Chat
                      --login                 Microsoft-Konto anmelden und beenden
                      --accounts              gespeicherte Konten auflisten und beenden
                  -h, --help                  diese Hilfe

                Beispiel:
                  java -jar afk-%s.jar mc.example.net -c 300:/afk

                Optionen, die es nur im Rust-Client gibt (--offline, --proxy, --fakehost, --on,
                --events, --antiafk, --sneak, --pov...), nimmt dieses Jar an und meldet beim
                Start, dass es sie nicht umsetzt.

                Ausgabe: Chat auf der Standardausgabe, alles andere auf der Standardfehlerausgabe.
                Eingabe: jede Zeile geht als Chat raus, mit '/' vorn als Serverbefehl.
                Konten:  %s
                """.formatted(version, version, version, version, configDir()));
        System.out.flush();
    }
}
