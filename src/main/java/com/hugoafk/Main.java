package com.hugoafk;

import com.hugoafk.auth.AuthManager;
import com.hugoafk.config.Config;
import com.hugoafk.net.AfkClient;
import com.hugoafk.ui.Console;

import java.nio.file.Path;
import java.nio.file.Paths;

/**
 * Einstiegspunkt: Microsoft-Login -> Server-IP abfragen -> verbinden -> Chat-Loop.
 *
 * Aufruf:  java -jar hugoafkclient.jar [host[:port]]
 */
public class Main {

    public static void main(String[] args) throws Exception {
        Path baseDir = configDir();
        Config config = Config.load(baseDir.resolve("config.json"));
        AuthManager auth = new AuthManager(baseDir.resolve("auth.json"));

        System.out.println("HugoAFKClient - Minecraft Java AFK Client");
        auth.login();
        System.out.println("Angemeldet als: " + auth.username());

        Console console = new Console();

        String server = args.length > 0 ? args[0] : config.lastServer;
        if (server == null || server.isBlank()) {
            server = console.readLine("Server-IP (host[:port]): ");
        }
        if (server == null || server.isBlank()) {
            console.error("Keine Server-IP angegeben. Beende.");
            console.close();
            return;
        }
        server = server.trim();
        config.lastServer = server;
        config.save();

        String host = server;
        int port = 25565;
        int colon = server.lastIndexOf(':');
        if (colon > -1) {
            host = server.substring(0, colon);
            try {
                port = Integer.parseInt(server.substring(colon + 1).trim());
            } catch (NumberFormatException e) {
                console.error("Ungueltiger Port - benutze 25565.");
            }
        }

        AfkClient client = new AfkClient(auth, config, console);
        printHelp(console);
        client.connect(host, port);

        runInputLoop(console, client);

        client.shutdown();
        console.close();
        System.out.println("Tschuess!");
    }

    private static void runInputLoop(Console console, AfkClient client) {
        String line;
        while ((line = console.readLine("> ")) != null) {
            line = line.trim();
            if (line.isEmpty()) {
                continue;
            }
            if (line.startsWith(":")) {
                if (handleCommand(line, console, client)) {
                    return; // :quit
                }
            } else {
                client.sendChatInput(line);
            }
        }
    }

    /** Behandelt interne ':'-Befehle. Gibt true zurueck, wenn beendet werden soll. */
    private static boolean handleCommand(String line, Console console, AfkClient client) {
        String[] parts = line.substring(1).trim().split("\\s+", 2);
        String cmd = parts[0].toLowerCase();
        switch (cmd) {
            case "quit", "exit" -> {
                return true;
            }
            case "reconnect" -> client.reconnectNow();
            case "afk" -> {
                if (parts.length > 1 && parts[1].equalsIgnoreCase("off")) {
                    client.setAntiAfk(false);
                } else {
                    client.setAntiAfk(true);
                }
            }
            case "help" -> printHelp(console);
            default -> console.error("Unbekannter Befehl: :" + cmd + " (siehe :help)");
        }
        return false;
    }

    private static void printHelp(Console console) {
        console.info("Befehle: Nachricht tippen zum Chatten | /befehl fuer Serverbefehle");
        console.info("  :help            diese Hilfe");
        console.info("  :reconnect       neu verbinden");
        console.info("  :afk on|off      Anti-AFK ein/aus");
        console.info("  :quit            beenden");
    }

    private static Path configDir() {
        String xdg = System.getenv("XDG_CONFIG_HOME");
        Path base;
        if (xdg != null && !xdg.isBlank()) {
            base = Paths.get(xdg);
        } else {
            base = Paths.get(System.getProperty("user.home"), ".config");
        }
        return base.resolve("hugoafk");
    }
}
