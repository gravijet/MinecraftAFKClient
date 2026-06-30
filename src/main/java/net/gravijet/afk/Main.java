package net.gravijet.afk;

import net.gravijet.afk.auth.AuthManager;
import net.gravijet.afk.config.Config;
import net.gravijet.afk.net.AfkClient;
import net.gravijet.afk.ui.Console;
import net.gravijet.afk.util.ChatLog;

import java.nio.file.Path;
import java.nio.file.Paths;
import java.util.List;

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
                case "--no-keepalive", "--no-afk" -> config.keepAliveEnabled = false;
                case "--keepalive-interval" -> {
                    if (i + 1 < args.length) {
                        try {
                            config.keepAliveIntervalMs = Math.max(500, Integer.parseInt(args[++i]));
                        } catch (NumberFormatException ignored) {
                        }
                    }
                }
                case "--auto-tpa" -> config.autoAcceptTpa = true;
                case "--mute" -> config.muteChat = true;
                case "--no-filter" -> config.chatFilterEnabled = false;
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
            case "status" -> client.printStatus();
            case "stats" -> client.printStats();
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
            // Keep-Alive (Timeout-Schutz): an/aus und Intervall.
            case "keepalive", "afk" -> handleKeepAlive(arg, console, client, config);
            case "tpa" -> client.setAutoTpa(!arg.equalsIgnoreCase("off"));
            case "reply" -> handleReply(arg, console, client, config);
            case "filter" -> client.setChatFilter(!arg.equalsIgnoreCase("off"));
            case "mute" -> client.setMute(!arg.equalsIgnoreCase("off"));
            case "periodic" -> handlePeriodic(arg, console, config);
            // Listen pflegen.
            case "join" -> handleList(arg, console, config, config.onJoinCommands, "Beitrittsbefehle");
            case "kickcmd" -> handleList(arg, console, config, config.onKickCommands, "Nach-Kick-Befehle");
            case "death" -> handleList(arg, console, config, config.onDeathCommands, "Tod-Befehle");
            case "hide" -> handleList(arg, console, config, config.chatHideFilters, "Chat-Filter");
            case "highlight" -> handleList(arg, console, config, config.highlightKeywords, "Highlights");
            case "set" -> handleSet(arg, console, config);
            case "config" -> printConfig(console, config);
            case "help" -> printHelp(console);
            default -> console.error("Unbekannter Befehl: :" + cmd + " (siehe :help)");
        }
        return false;
    }

    private static void handleKeepAlive(String arg, Console console, AfkClient client, Config config) {
        String[] sub = arg.split("\\s+", 2);
        String key = sub[0].toLowerCase();
        String value = sub.length > 1 ? sub[1].trim() : "";
        switch (key) {
            case "", "on" -> client.setKeepAlive(true);
            case "off" -> client.setKeepAlive(false);
            case "interval" -> {
                try {
                    client.setKeepAliveInterval(Integer.parseInt(value));
                } catch (NumberFormatException e) {
                    console.error("Ungueltige Millisekundenzahl.");
                }
            }
            default -> console.error("Nutzung: :keepalive on|off | :keepalive interval <ms>");
        }
    }

    /** Pflegt periodische Befehle: add <sek> <befehl> | remove <nr> | clear | list. */
    private static void handlePeriodic(String arg, Console console, Config config) {
        String[] sub = arg.split("\\s+", 2);
        String op = sub[0].toLowerCase();
        String rest = sub.length > 1 ? sub[1].trim() : "";
        switch (op) {
            case "add" -> {
                String[] p = rest.split("\\s+", 2);
                if (p.length < 2 || p[1].isBlank()) {
                    console.error("Nutzung: :periodic add <sekunden> <befehl>");
                    return;
                }
                try {
                    int interval = Math.max(5, Integer.parseInt(p[0]));
                    config.periodicCommands.add(new Config.PeriodicCommand(p[1].trim(), interval));
                    config.save();
                    console.info("Periodischer Befehl hinzugefuegt: alle " + interval + "s -> " + p[1].trim());
                } catch (NumberFormatException e) {
                    console.error("Ungueltige Sekundenzahl.");
                }
            }
            case "remove", "rm" -> {
                try {
                    int idx = Integer.parseInt(rest.trim()) - 1;
                    if (idx >= 0 && idx < config.periodicCommands.size()) {
                        Config.PeriodicCommand removed = config.periodicCommands.remove(idx);
                        config.save();
                        console.info("Entfernt: " + removed.command);
                    } else {
                        console.error("Ungueltige Nummer (siehe :periodic list).");
                    }
                } catch (NumberFormatException e) {
                    console.error("Nutzung: :periodic remove <nr>");
                }
            }
            case "clear" -> {
                config.periodicCommands.clear();
                config.save();
                console.info("Periodische Befehle geleert.");
            }
            case "", "list" -> {
                if (config.periodicCommands.isEmpty()) {
                    console.info("Periodische Befehle: (leer)");
                } else {
                    console.info("Periodische Befehle (" + config.periodicCommands.size() + "):");
                    for (int i = 0; i < config.periodicCommands.size(); i++) {
                        Config.PeriodicCommand pc = config.periodicCommands.get(i);
                        console.info("  " + (i + 1) + ". alle " + pc.intervalSeconds + "s -> "
                                + pc.command + (pc.enabled ? "" : " (aus)"));
                    }
                }
            }
            default -> console.error("Nutzung: :periodic add <sek> <befehl> | remove <nr> | clear | list");
        }
    }

    private static void handleReply(String arg, Console console, AfkClient client, Config config) {
        String[] sub = arg.split("\\s+", 2);
        String key = sub[0].toLowerCase();
        String value = sub.length > 1 ? sub[1].trim() : "";
        switch (key) {
            case "", "on" -> client.setAutoReply(true);
            case "off" -> client.setAutoReply(false);
            case "msg" -> {
                config.autoReplyMessage = value;
                config.save();
                console.info("Auto-Antwort-Text ist jetzt: " + value);
            }
            default -> console.error("Nutzung: :reply on|off | :reply msg <text>");
        }
    }

    /** Generische Listenpflege: add <x> | remove <x> | clear | list. */
    private static void handleList(String arg, Console console, Config config,
                                   List<String> list, String label) {
        String[] sub = arg.split("\\s+", 2);
        String op = sub[0].toLowerCase();
        String value = sub.length > 1 ? sub[1].trim() : "";
        switch (op) {
            case "add" -> {
                if (value.isBlank()) {
                    console.error("Nutzung: add <eintrag>");
                } else {
                    list.add(value);
                    config.save();
                    console.info(label + ": hinzugefuegt -> " + value);
                }
            }
            case "remove", "rm" -> {
                if (list.removeIf(s -> s.equalsIgnoreCase(value))) {
                    config.save();
                    console.info(label + ": entfernt -> " + value);
                } else {
                    console.error(label + ": Eintrag nicht gefunden.");
                }
            }
            case "clear" -> {
                list.clear();
                config.save();
                console.info(label + ": geleert.");
            }
            case "", "list" -> {
                if (list.isEmpty()) {
                    console.info(label + ": (leer)");
                } else {
                    console.info(label + " (" + list.size() + "):");
                    for (int i = 0; i < list.size(); i++) {
                        console.info("  " + (i + 1) + ". " + list.get(i));
                    }
                }
            }
            default -> console.error("Nutzung: add <x> | remove <x> | clear | list");
        }
    }

    /** Setzt einfache Konfigurationswerte zur Laufzeit: :set <key> <wert>. */
    private static void handleSet(String arg, Console console, Config config) {
        String[] sub = arg.split("\\s+", 2);
        if (sub.length < 2 || sub[1].isBlank()) {
            console.error("Nutzung: :set <key> <wert>  (siehe :config fuer Keys)");
            return;
        }
        String key = sub[0].toLowerCase();
        String value = sub[1].trim();
        try {
            switch (key) {
                case "reconnect" -> config.autoReconnect = parseBool(value);
                case "respawn" -> config.autoRespawn = parseBool(value);
                case "timestamps" -> config.showTimestamps = parseBool(value);
                case "logchat" -> config.logChat = parseBool(value);
                case "announce" -> config.announcePlayerJoinLeave = parseBool(value);
                case "bellhighlight" -> config.bellOnHighlight = parseBool(value);
                case "belldisconnect" -> config.bellOnDisconnect = parseBool(value);
                case "highlightname" -> config.highlightUsername = parseBool(value);
                case "keepalive" -> config.keepAliveEnabled = parseBool(value);
                case "lowhealth" -> config.lowHealthActionEnabled = parseBool(value);
                case "chatdelay" -> config.chatMinDelayMs = Math.max(200, Integer.parseInt(value));
                case "reconnectdelay" -> config.reconnectDelaySeconds = Math.max(1, Integer.parseInt(value));
                case "maxattempts" -> config.maxReconnectAttempts = Math.max(0, Integer.parseInt(value));
                case "maxbackoff" -> config.maxBackoffSeconds = Math.max(1, Integer.parseInt(value));
                case "jitter" -> config.reconnectJitterMs = Math.max(0, Integer.parseInt(value));
                case "keepaliveinterval" -> config.keepAliveIntervalMs = Math.max(500, Integer.parseInt(value));
                case "joindelay" -> config.onJoinDelaySeconds = Math.max(0, Integer.parseInt(value));
                case "kickdelay" -> config.onKickDelaySeconds = Math.max(0, Integer.parseInt(value));
                case "replycooldown" -> config.autoReplyCooldownSeconds = Math.max(0, Integer.parseInt(value));
                case "lowhealththreshold" -> config.lowHealthThreshold = Double.parseDouble(value);
                case "tpacmd" -> config.tpaAcceptCommand = value;
                case "replymsg" -> config.autoReplyMessage = value;
                case "replycmd" -> config.autoReplyCommand = value;
                default -> {
                    console.error("Unbekannter Key: " + key + " (siehe :config)");
                    return;
                }
            }
            config.save();
            console.info("Gesetzt: " + key + " = " + value);
        } catch (NumberFormatException e) {
            console.error("Ungueltiger Zahlenwert: " + value);
        }
    }

    private static boolean parseBool(String value) {
        String v = value.toLowerCase();
        return v.equals("on") || v.equals("true") || v.equals("an") || v.equals("1") || v.equals("ja");
    }

    private static void printConfig(Console console, Config config) {
        console.info("=== Konfiguration (~/.config/hugoafk/config.json) ===");
        console.info("  Server:           " + config.lastServer);
        console.info("  reconnect:        " + config.autoReconnect);
        console.info("  reconnectdelay:   " + config.reconnectDelaySeconds + "s  jitter=" + config.reconnectJitterMs + "ms");
        console.info("  maxattempts:      " + config.maxReconnectAttempts + "  maxbackoff=" + config.maxBackoffSeconds + "s");
        console.info("  fallbackServers:  " + config.fallbackServers.size());
        console.info("  keepalive:        " + config.keepAliveEnabled
                + "  keepaliveinterval=" + config.keepAliveIntervalMs + "ms");
        console.info("  onJoin/onKick:    " + config.onJoinCommands.size() + " / " + config.onKickCommands.size());
        console.info("  onDeath:          " + config.onDeathCommands.size());
        console.info("  respawn:          " + config.autoRespawn);
        console.info("  lowhealth:        " + config.lowHealthActionEnabled + "  threshold=" + config.lowHealthThreshold);
        console.info("  autotpa:          " + config.autoAcceptTpa + "  whitelist=" + config.autoAcceptTpaWhitelist.size());
        console.info("  autoreply:        " + config.autoReplyEnabled + "  cooldown=" + config.autoReplyCooldownSeconds + "s");
        console.info("  chatfilter:       " + config.chatFilterEnabled + "  regeln=" + config.chatHideFilters.size());
        console.info("  mute:             " + config.muteChat);
        console.info("  timestamps:       " + config.showTimestamps + "  logchat=" + config.logChat);
        console.info("  chatdelay:        " + config.chatMinDelayMs + "ms");
        console.info("  periodicCommands: " + config.periodicCommands.size());
        console.info("Tipp: Komplexe Felder direkt in der config.json bearbeiten.");
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
        console.info("  :help                  diese Hilfe");
        console.info("  :status                Verbindung/Leben/Ping anzeigen");
        console.info("  :stats                 Laufzeit-Statistik (Kicks, Tode, ...)");
        console.info("  :config                aktuelle Konfiguration anzeigen");
        console.info("  :players               Online-Spieler auflisten");
        console.info("  :server <ip>           zu anderem Server wechseln");
        console.info("  :reconnect             neu verbinden");
        console.info("  :keepalive on|off      Keep-Alive (Timeout-Schutz) ein/aus");
        console.info("  :keepalive interval <ms> Keep-Alive-Intervall setzen");
        console.info("  :tpa on|off            TPA-Anfragen automatisch annehmen");
        console.info("  :reply on|off          Auto-Antwort auf private Nachrichten");
        console.info("  :reply msg <text>      Auto-Antwort-Text setzen");
        console.info("  :filter on|off         Chat-Spam-Filter ein/aus");
        console.info("  :mute on|off           gesamten eingehenden Chat aus/ein");
        console.info("  :periodic add <s> <cmd> periodischen Befehl hinzufuegen (z. B. /afk)");
        console.info("  :join  add|remove|list Beitrittsbefehle pflegen");
        console.info("  :kickcmd add|remove|.. Befehle nach einem Kick pflegen");
        console.info("  :death add|remove|list Befehle beim Tod pflegen");
        console.info("  :hide  add|remove|list Chat-Filterregeln pflegen");
        console.info("  :highlight add|rm|list Highlight-Schluesselwoerter pflegen");
        console.info("  :set <key> <wert>      Einzelwert setzen (siehe :config)");
        console.info("  :quit                  beenden");
    }

    private static void printUsage() {
        System.out.println("""
                HugoAFKClient - Minecraft Java AFK Client

                Aufruf: java -jar hugoafkclient.jar [optionen] [host[:port]]

                Optionen:
                  --server <host[:port]>     Server-Adresse
                  --no-reconnect             Auto-Reconnect deaktivieren
                  --no-keepalive             Keep-Alive (Timeout-Schutz) deaktivieren
                  --keepalive-interval <ms>  Intervall des Keep-Alive-Pakets (Standard 1000)
                  --auto-tpa                 TPA-Anfragen automatisch annehmen
                  --mute                     eingehenden Chat ausblenden
                  --no-filter                Chat-Spam-Filter deaktivieren
                  -h, --help                 diese Hilfe

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
