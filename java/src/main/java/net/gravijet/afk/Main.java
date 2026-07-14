package net.gravijet.afk;

import net.gravijet.afk.auth.AuthManager;
import net.gravijet.afk.config.Config;
import net.gravijet.afk.net.AfkClient;
import net.gravijet.afk.net.ProtocolBridge;
import net.gravijet.afk.ui.Console;

import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.List;

/**
 * Einstiegspunkt: Microsoft-Login (mit Konto-Wechsel) -> Menü -> verbinden -> Chat-Loop.
 *
 * Aufruf:  java -jar hugoafk-&lt;version&gt;.jar [optionen] [host[:port]]
 */
public class Main {

    public static void main(String[] args) throws Exception {
        Path baseDir = configDir();
        Config config = Config.load(baseDir.resolve("config.json"));

        String serverArg = null;
        String accountArg = null;
        for (int i = 0; i < args.length; i++) {
            String a = args[i];
            switch (a) {
                case "-h", "--help" -> {
                    printUsage();
                    return;
                }
                case "--server" -> {
                    if (i + 1 < args.length) serverArg = args[++i];
                }
                case "--account" -> {
                    if (i + 1 < args.length) accountArg = args[++i];
                }
                default -> {
                    if (!a.startsWith("-")) serverArg = a;
                }
            }
        }

        String variant = System.getProperty("hugoafk.variant", "?");
        ProtocolBridge bridge = ProtocolBridge.load(variant);
        bridge.init();

        Console console = new Console();
        console.setColor(config.colorOutput);

        printHeader(console, bridge.targetVersion());

        AuthManager auth = new AuthManager(baseDir);
        String preferred = accountArg != null ? accountArg : config.activeAccount;
        try {
            auth.loginInteractive(preferred, console::print);
        } catch (Exception e) {
            console.error("Login fehlgeschlagen: " + e.getMessage());
            console.close();
            return;
        }
        config.activeAccount = auth.currentAccount();
        config.save();

        String server = preConnectMenu(console, auth, config, bridge,
                serverArg != null ? serverArg : config.lastServer);
        if (server == null) {
            console.close();
            return; // :quit
        }
        server = server.trim();
        config.lastServer = server;
        config.save();

        int[] portHolder = new int[1];
        String host = parseHost(server, portHolder, console);
        int port = portHolder[0];

        AfkClient client = new AfkClient(auth, config, console, bridge);
        Runtime.getRuntime().addShutdownHook(new Thread(client::shutdown, "hugoafk-shutdown"));

        printHelp(console);
        client.connect(host, port);
        runInputLoop(console, client, auth, config);

        client.shutdown();
        console.close();
        System.out.println("Tschuess!");
    }

    // ===================== Menüs =====================

    /** Vor-Verbindungs-Menü: Server wählen/eingeben, Konten verwalten. */
    private static String preConnectMenu(Console console, AuthManager auth, Config config,
                                         ProtocolBridge bridge, String server) {
        while (true) {
            printMenu(console, auth, bridge, server);
            String line = console.readLine("> ");
            if (line == null) return null;
            line = line.trim();
            if (line.isEmpty()) {
                if (server != null && !server.isBlank()) return server;
                console.error("Keine Server-IP. Gib host[:port] ein oder :quit.");
                continue;
            }
            if (line.startsWith(":")) {
                String[] p = line.substring(1).split("\\s+", 2);
                String cmd = p[0].toLowerCase();
                String arg = p.length > 1 ? p[1].trim() : "";
                switch (cmd) {
                    case "quit", "exit" -> {
                        return null;
                    }
                    case "account" -> accountMenu(console, auth, config);
                    case "server" -> {
                        if (!arg.isBlank()) server = arg;
                        else console.error("Nutzung: :server <host[:port]>");
                    }
                    case "help" -> printMenu(console, auth, bridge, server);
                    default -> console.error("Unbekannt: :" + cmd);
                }
            } else {
                return line; // eingegebene IP -> direkt verbinden
            }
        }
    }

    private static void accountMenu(Console console, AuthManager auth, Config config) {
        while (true) {
            List<String> accounts = auth.listAccounts();
            console.print("");
            console.print(console.color(Console.BOLD, "=== Konten ==="));
            for (int i = 0; i < accounts.size(); i++) {
                String n = accounts.get(i);
                boolean active = n.equals(auth.currentAccount());
                console.print("  " + (i + 1) + ") " + n + (active ? console.color(Console.CYAN, "  (aktiv)") : ""));
            }
            console.print(console.color(Console.GRAY, "  n) neues Konto (Microsoft-Login)   r <nr>) entfernen   [Enter] zurück"));
            String line = console.readLine("Konto> ");
            if (line == null) return;
            line = line.trim();
            if (line.isEmpty()) return;
            if (line.equalsIgnoreCase("n")) {
                try {
                    String name = auth.addAccount(console::print);
                    config.activeAccount = name;
                    config.save();
                    console.info("Aktives Konto: " + name);
                    return;
                } catch (Exception e) {
                    console.error("Login fehlgeschlagen: " + e.getMessage());
                }
            } else if (line.toLowerCase().startsWith("r")) {
                Integer idx = parseIndex(line.substring(1).trim(), accounts.size());
                if (idx != null) {
                    String name = accounts.get(idx);
                    if (auth.removeAccount(name)) console.info("Entfernt: " + name);
                } else {
                    console.error("Nutzung: r <nr>");
                }
            } else {
                Integer idx = parseIndex(line, accounts.size());
                if (idx != null) {
                    String name = accounts.get(idx);
                    if (auth.switchTo(name)) {
                        config.activeAccount = name;
                        config.save();
                        console.info("Aktives Konto: " + name);
                        return;
                    }
                    console.error("Konto ließ sich nicht anmelden: " + name);
                } else {
                    console.error("Ungültige Eingabe.");
                }
            }
        }
    }

    /** Wandelt eine 1-basierte Nummer in einen gültigen 0-basierten Index um (sonst null). */
    private static Integer parseIndex(String text, int size) {
        try {
            int n = Integer.parseInt(text.trim()) - 1;
            return (n >= 0 && n < size) ? n : null;
        } catch (NumberFormatException e) {
            return null;
        }
    }

    // ===================== Chat-Loop =====================

    private static void runInputLoop(Console console, AfkClient client, AuthManager auth, Config config) {
        String line;
        while ((line = console.readLine("> ")) != null) {
            line = line.trim();
            if (line.isEmpty()) continue;
            if (line.startsWith(":")) {
                if (handleCommand(line, console, client, auth, config)) return;
            } else {
                client.sendChatInput(line);
            }
        }
    }

    /** Behandelt die wenigen internen ':'-Befehle. true = beenden. */
    private static boolean handleCommand(String line, Console console, AfkClient client,
                                         AuthManager auth, Config config) {
        String[] p = line.substring(1).split("\\s+", 2);
        String cmd = p[0].toLowerCase();
        String arg = p.length > 1 ? p[1].trim() : "";
        switch (cmd) {
            case "quit", "exit" -> {
                return true;
            }
            case "reconnect" -> client.reconnectNow();
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
            case "account" -> {
                String before = auth.currentAccount();
                accountMenu(console, auth, config);
                if (!before.equals(auth.currentAccount())) {
                    console.info("Konto gewechselt – verbinde neu ...");
                    client.reconnectNow();
                }
            }
            case "clear", "cls" -> console.clearScreen();
            case "help" -> printHelp(console);
            default -> console.error("Unbekannter Befehl: :" + cmd + " (siehe :help)");
        }
        return false;
    }

    // ===================== Ausgabe-Helfer =====================

    private static void printHeader(Console console, String version) {
        console.print("");
        console.print(console.color(Console.CYAN, "  ┌─────────────────────────────────────────────┐"));
        console.print(console.color(Console.CYAN, "  │") + console.color(Console.BOLD, "  HugoAFKClient")
                + console.color(Console.GRAY, "   ·   Minecraft " + version) + pad(version) + console.color(Console.CYAN, "│"));
        console.print(console.color(Console.CYAN, "  └─────────────────────────────────────────────┘"));
    }

    private static String pad(String version) {
        int used = "  HugoAFKClient   ·   Minecraft ".length() + version.length();
        int total = 47;
        int n = Math.max(1, total - used);
        return " ".repeat(n);
    }

    private static void printMenu(Console console, AuthManager auth, ProtocolBridge bridge, String server) {
        console.print("");
        console.print(console.color(Console.GRAY, "  Konto  : ") + auth.username()
                + console.color(Console.GRAY, "     Version: ") + bridge.targetVersion());
        console.print(console.color(Console.GRAY, "  Server : ")
                + (server == null || server.isBlank() ? console.color(Console.GRAY, "(keiner)") : server));
        console.print(console.color(Console.GRAY, "  [Enter] verbinden   <ip> verbinden   :account Konten   :server <ip>   :quit"));
    }

    private static void printHelp(Console console) {
        console.info("Nachricht tippen = chatten | /befehl = Serverbefehl");
        console.info("  :reconnect   neu verbinden");
        console.info("  :server <ip> Server wechseln");
        console.info("  :account     Konto wechseln/verwalten");
        console.info("  :clear       Bildschirm leeren");
        console.info("  :quit        beenden");
    }

    private static void printUsage() {
        System.out.println("""
                HugoAFKClient - schlanker Minecraft-AFK-Client

                Aufruf: java -jar hugoafk-<version>.jar [optionen] [host[:port]]

                Optionen:
                  --server <host[:port]>   Server-Adresse
                  --account <name>         Startkonto wählen
                  -h, --help               diese Hilfe

                Konfiguration: ~/.config/hugoafk/ (config.json, accounts/)
                Version je Jar; Auswahl über den Launcher (hugoafk.ps1 / hugoafk.sh).
                """);
    }

    // ===================== Host/Port =====================

    private static String parseHost(String input, int[] holder, Console console) {
        holder[0] = 25565;
        String value = input.trim();
        if (value.startsWith("[")) { // IPv6: [::1]:25565
            int end = value.indexOf(']');
            if (end > 0) {
                String host = value.substring(1, end);
                int colon = value.indexOf(':', end);
                if (colon > 0) parsePort(value.substring(colon + 1), holder, console);
                return host;
            }
            return value;
        }
        int colon = value.lastIndexOf(':');
        if (colon > -1 && value.indexOf(':') == colon) {
            parsePort(value.substring(colon + 1), holder, console);
            return value.substring(0, colon);
        }
        return value;
    }

    private static void parsePort(String text, int[] holder, Console console) {
        try {
            holder[0] = Integer.parseInt(text.trim());
        } catch (NumberFormatException e) {
            console.error("Ungültiger Port – benutze 25565.");
        }
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
