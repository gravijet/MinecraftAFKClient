package com.hugoafk;

import com.hugoafk.auth.AuthManager;
import com.hugoafk.config.Config;
import com.hugoafk.net.AfkClient;
import com.hugoafk.ui.Console;
import com.hugoafk.util.ChatLog;

import java.nio.file.Path;
import java.nio.file.Paths;

/**
 * Einstiegspunkt: Microsoft-Login -> Server-IP -> verbinden -> Chat-Loop.
 *
 * Aufruf:  java -jar hugoafkclient.jar [optionen] [host[:port]]
 */
public class Main {

    public static void main(String[] args) throws Exception {
        Path baseDir = configDir();
        Config config = Config.load(baseDir.resolve("config.json"));

        // ---- CLI-Argumente ----
        String serverArg = null;
        for (int i = 0; i < args.length; i++) {
            String a = args[i];
            switch (a) {
                case "-h", "--help" -> {
                    printUsage();
                    return;
                }
                case "--no-reconnect" -> config.autoReconnect = false;
                case "--no-afk" -> config.antiAfkEnabled = false;
                case "--afk" -> {
                    if (i + 1 < args.length) {
                        try {
                            config.antiAfkSeconds = Math.max(5, Integer.parseInt(args[++i]));
                        } catch (NumberFormatException ignored) {
                        }
                    }
                }
                case "--server" -> {
                    if (i + 1 < args.length) {
                        serverArg = args[++i];
                    }
                }
                default -> {
                    if (!a.startsWith("-")) {
                        serverArg = a;
                    }
                }
            }
        }

        AuthManager auth = new AuthManager(baseDir.resolve("auth.json"));
        ChatLog chatLog = new ChatLog(baseDir.resolve("chat.log"));

        System.out.println("HugoAFKClient - Minecraft Java AFK Client");
        auth.login();
        System.out.println("Angemeldet als: " + auth.username());

        Console console = new Console();

        String server = serverArg != null ? serverArg : config.lastServer;
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

        int[] portHolder = new int[1];
        String host = parseHost(server, portHolder, console);
        int port = portHolder[0];

        AfkClient client = new AfkClient(auth, config, console, chatLog);
        console.setPlayerNameSupplier(client::playerNames);

        // Sauberes Beenden bei Ctrl-C / Kill.
        Runtime.getRuntime().addShutdownHook(new Thread(client::shutdown, "hugoafk-shutdown"));

        printHelp(console);
        client.connect(host, port);

        runInputLoop(console, client, config);

        client.shutdown();
        console.close();
        System.out.println("Tschuess!");
    }

    private static void runInputLoop(Console console, AfkClient client, Config config) {
        String line;
        while ((line = console.readLine("> ")) != null) {
            line = line.trim();
            if (line.isEmpty()) {
                continue;
            }
            if (line.startsWith(":")) {
                if (handleCommand(line, console, client, config)) {
                    return; // :quit
                }
            } else {
                client.sendChatInput(line);
            }
        }
    }

    /** Behandelt interne ':'-Befehle. Gibt true zurueck, wenn beendet werden soll. */
    private static boolean handleCommand(String line, Console console, AfkClient client, Config config) {
        String[] parts = line.substring(1).trim().split("\\s+", 2);
        String cmd = parts[0].toLowerCase();
        String arg = parts.length > 1 ? parts[1].trim() : "";
        switch (cmd) {
            case "quit", "exit" -> {
                return true;
            }
            case "reconnect" -> client.reconnectNow();
            case "afk" -> client.setAntiAfk(!arg.equalsIgnoreCase("off"));
            case "status" -> client.printStatus();
            case "players", "list" -> client.printPlayers();
            case "server" -> {
                if (arg.isBlank()) {
                    console.error("Nutzung: :server <host[:port]>");
                } else {
                    int[] holder = new int[1];
                    String h = parseHost(arg, holder, console);
                    config.lastServer = arg;
                    config.save();
                    client.switchServer(h, holder[0]);
                }
            }
            case "help" -> printHelp(console);
            default -> console.error("Unbekannter Befehl: :" + cmd + " (siehe :help)");
        }
        return false;
    }

    /** Parst host[:port] inkl. IPv6 in [..]:port-Schreibweise. Setzt Port in holder[0]. */
    private static String parseHost(String input, int[] holder, Console console) {
        holder[0] = 25565;
        String value = input.trim();
        if (value.startsWith("[")) { // IPv6: [::1]:25565
            int end = value.indexOf(']');
            if (end > 0) {
                String host = value.substring(1, end);
                int colon = value.indexOf(':', end);
                if (colon > 0) {
                    parsePort(value.substring(colon + 1), holder, console);
                }
                return host;
            }
            return value;
        }
        int colon = value.lastIndexOf(':');
        if (colon > -1 && value.indexOf(':') == colon) { // genau ein ':' -> host:port
            parsePort(value.substring(colon + 1), holder, console);
            return value.substring(0, colon);
        }
        return value;
    }

    private static void parsePort(String text, int[] holder, Console console) {
        try {
            holder[0] = Integer.parseInt(text.trim());
        } catch (NumberFormatException e) {
            console.error("Ungueltiger Port - benutze 25565.");
        }
    }

    private static void printHelp(Console console) {
        console.info("Nachricht tippen = chatten | /befehl = Serverbefehl");
        console.info("  :help              diese Hilfe");
        console.info("  :status            Verbindung/Leben/Ping anzeigen");
        console.info("  :players           Online-Spieler auflisten");
        console.info("  :server <ip>       zu anderem Server wechseln");
        console.info("  :reconnect         neu verbinden");
        console.info("  :afk on|off        Anti-AFK ein/aus");
        console.info("  :quit              beenden");
    }

    private static void printUsage() {
        System.out.println("""
                HugoAFKClient - Minecraft Java AFK Client

                Aufruf: java -jar hugoafkclient.jar [optionen] [host[:port]]

                Optionen:
                  --server <host[:port]>   Server-Adresse
                  --no-reconnect           Auto-Reconnect deaktivieren
                  --no-afk                 Anti-AFK deaktivieren
                  --afk <sekunden>         Anti-AFK-Intervall setzen
                  -h, --help               diese Hilfe

                Konfiguration: ~/.config/hugoafk/ (config.json, auth.json, chat.log)
                """);
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
