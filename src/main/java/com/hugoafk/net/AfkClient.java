package com.hugoafk.net;

import com.hugoafk.auth.AuthManager;
import com.hugoafk.config.Config;
import com.hugoafk.ui.Console;
import com.hugoafk.util.ChatLog;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.serializer.ansi.ANSIComponentSerializer;
import net.kyori.adventure.text.serializer.plain.PlainTextComponentSerializer;
import org.geysermc.mcprotocollib.auth.SessionService;
import org.geysermc.mcprotocollib.network.ClientSession;
import org.geysermc.mcprotocollib.network.Session;
import org.geysermc.mcprotocollib.network.event.session.DisconnectedEvent;
import org.geysermc.mcprotocollib.network.event.session.SessionAdapter;
import org.geysermc.mcprotocollib.network.factory.ClientNetworkSessionFactory;
import org.geysermc.mcprotocollib.network.packet.Packet;
import org.geysermc.mcprotocollib.protocol.MinecraftConstants;
import org.geysermc.mcprotocollib.protocol.MinecraftProtocol;
import org.geysermc.mcprotocollib.protocol.data.game.ClientCommand;
import org.geysermc.mcprotocollib.protocol.data.game.PlayerListEntry;
import org.geysermc.mcprotocollib.protocol.data.game.PlayerListEntryAction;
import org.geysermc.mcprotocollib.protocol.data.game.ResourcePackStatus;
import org.geysermc.mcprotocollib.protocol.data.game.entity.player.PositionElement;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ClientboundResourcePackPushPacket;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ClientboundStoreCookiePacket;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ClientboundTransferPacket;
import org.geysermc.mcprotocollib.protocol.packet.common.serverbound.ServerboundResourcePackPacket;
import org.geysermc.mcprotocollib.protocol.packet.cookie.clientbound.ClientboundCookieRequestPacket;
import org.geysermc.mcprotocollib.protocol.packet.cookie.serverbound.ServerboundCookieResponsePacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.ClientboundLoginPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.ClientboundPlayerChatPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.ClientboundPlayerInfoRemovePacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.ClientboundPlayerInfoUpdatePacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.ClientboundSystemChatPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.entity.player.ClientboundPlayerPositionPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.entity.player.ClientboundSetHealthPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundChatAckPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundChatCommandPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundChatPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundClientCommandPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.level.ServerboundAcceptTeleportationPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.player.ServerboundMovePlayerPosRotPacket;

import java.net.InetSocketAddress;
import java.time.Instant;
import java.time.LocalTime;
import java.time.format.DateTimeFormatter;
import java.util.BitSet;
import java.util.List;
import java.util.Map;
import java.util.Queue;
import java.util.UUID;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ConcurrentLinkedQueue;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.ThreadLocalRandom;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;
import java.util.regex.Matcher;
import java.util.regex.Pattern;

/**
 * Verbindet sich mit dem Server und behandelt eingehende Pakete (Chat, Resource-Pack,
 * Teleport, Health, Spielerliste, Cookies, Transfer). Sendet Chat/Befehle (rate-limitiert)
 * und verbindet bei Verbindungsabbruch mit Backoff (inkl. Fallback-Servern) neu.
 *
 * <p>Anti-Kick erfolgt befehlsbasiert (z. B. periodisches {@code /afk}) statt durch
 * simulierte Bewegung. Zusaetzlich: Befehle bei Beitritt/Kick/Tod, Auto-Annahme von
 * TPA-Anfragen, Auto-Antwort auf private Nachrichten, Chat-Spam-Filter, periodische
 * eigene Befehle und Laufzeit-Statistik. Ziel: moeglichst nie gekickt zu werden.
 */
public class AfkClient {

    private static final int ACK_THRESHOLD = 20;
    private static final int THROTTLE_MIN_DELAY = 30;
    private static final DateTimeFormatter TIME = DateTimeFormatter.ofPattern("HH:mm:ss");
    private static final Pattern TPA_SENDER =
            Pattern.compile("(\\w{3,16})\\s+hat dir", Pattern.CASE_INSENSITIVE);

    private final AuthManager auth;
    private final Config config;
    private final Console console;
    private final ChatLog chatLog;
    private final PlayerList playerList = new PlayerList();
    private final SessionService sessionService = new SessionService();
    private final ANSIComponentSerializer ansi = ANSIComponentSerializer.ansi();
    private final PlainTextComponentSerializer plain = PlainTextComponentSerializer.plainText();
    private final ScheduledExecutorService scheduler =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread t = new Thread(r, "hugoafk-scheduler");
                t.setDaemon(true);
                return t;
            });

    private final AtomicInteger unacknowledged = new AtomicInteger();
    private final AtomicBoolean reconnectScheduled = new AtomicBoolean(false);
    /** Ausgehende Nachrichten/Befehle, rate-limitiert gesendet (gegen Spam-Kick). */
    private final Queue<String> outgoing = new ConcurrentLinkedQueue<>();
    /** Server-Cookies (fuer Transfers / Netzwerk-Auth) merken und auf Anfrage zurueckgeben. */
    private final Map<String, byte[]> cookies = new ConcurrentHashMap<>();
    /** Zeitpunkt des letzten Laufs je periodischem Befehl (Schluessel = Befehlstext). */
    private final Map<String, Long> periodicLastRun = new ConcurrentHashMap<>();
    /** Letzte Auto-Antwort je Spieler (fuer den Cooldown). */
    private final Map<String, Long> lastAutoReply = new ConcurrentHashMap<>();

    // Statistik
    private final AtomicLong statKicks = new AtomicLong();
    private final AtomicLong statReconnects = new AtomicLong();
    private final AtomicLong statDeaths = new AtomicLong();
    private final AtomicLong statChatLines = new AtomicLong();
    private final AtomicLong statTpaAccepted = new AtomicLong();
    private final long startedAt = System.currentTimeMillis();

    private volatile ClientSession session;
    private volatile boolean shuttingDown = false;
    private volatile boolean inGame = false;
    private volatile boolean intentionalDisconnect = false;
    private volatile boolean rejoinAfterKick = false;
    private volatile int reconnectAttempts = 0;
    private volatile int fallbackIndex = 0;
    private volatile long connectedAt = 0;
    private volatile long lastAntiKick = 0;
    private volatile String lastTpaSender = "";
    private volatile long lastTpaAt = 0;

    // Spielzustand
    private volatile UUID selfId;
    private volatile float health = 20f;
    private volatile int food = 20;
    private volatile boolean wasDead = false;
    private volatile boolean havePosition = false;
    private volatile double posX, posY, posZ;
    private volatile float yaw, pitch;

    private String host;
    private int port;

    public AfkClient(AuthManager auth, Config config, Console console, ChatLog chatLog) {
        this.auth = auth;
        this.config = config;
        this.console = console;
        this.chatLog = chatLog;

        // Ein einziger 1-Sekunden-Heartbeat steuert Anti-Kick und periodische Befehle.
        // So wirken Aenderungen via :set sofort, ohne Neuplanung.
        scheduler.scheduleAtFixedRate(this::heartbeat, 1, 1, TimeUnit.SECONDS);
        int chatDelay = Math.max(200, config.chatMinDelayMs);
        scheduler.scheduleAtFixedRate(this::drainOutgoing, chatDelay, chatDelay, TimeUnit.MILLISECONDS);
    }

    public List<String> playerNames() {
        return playerList.sortedNames();
    }

    public void connect(String host, int port) {
        this.host = host;
        this.port = port;
        doConnect();
    }

    private void doConnect() {
        if (shuttingDown) {
            return;
        }
        intentionalDisconnect = false;
        try {
            selfId = auth.gameProfile().getId();
            MinecraftProtocol protocol = new MinecraftProtocol(auth.gameProfile(), auth.accessToken());
            ClientSession client = ClientNetworkSessionFactory.factory()
                    .setRemoteSocketAddress(InetSocketAddress.createUnresolved(host, port))
                    .setProtocol(protocol)
                    .create();
            client.setFlag(MinecraftConstants.SESSION_SERVICE_KEY, sessionService);
            client.setFlag(MinecraftConstants.AUTOMATIC_KEEP_ALIVE_MANAGEMENT, true);
            // Transfers selbst behandeln, damit unser Listener erhalten bleibt.
            client.setFlag(MinecraftConstants.FOLLOW_TRANSFERS, false);
            client.addListener(new Listener());
            this.session = client;
            console.info("Verbinde zu " + host + ":" + port + " ...");
            client.connect();
        } catch (Exception e) {
            console.error("Verbindung fehlgeschlagen: " + e.getMessage());
            tryFallbackServer();
            scheduleReconnect(0);
        }
    }

    /** Wechselt bei wiederholten Fehlversuchen zyklisch auf einen Fallback-Server. */
    private void tryFallbackServer() {
        if (config.fallbackServers.isEmpty() || reconnectAttempts < 2) {
            return;
        }
        String target = config.fallbackServers.get(fallbackIndex % config.fallbackServers.size());
        fallbackIndex++;
        int[] holder = {25565};
        String h = parseHostPort(target, holder);
        if (!h.isBlank()) {
            console.info("Versuche Fallback-Server " + h + ":" + holder[0] + " ...");
            this.host = h;
            this.port = holder[0];
        }
    }

    private static String parseHostPort(String input, int[] holder) {
        String value = input == null ? "" : input.trim();
        int colon = value.lastIndexOf(':');
        if (colon > -1 && value.indexOf(':') == colon) {
            try {
                holder[0] = Integer.parseInt(value.substring(colon + 1).trim());
            } catch (NumberFormatException ignored) {
            }
            return value.substring(0, colon);
        }
        return value;
    }

    private void scheduleReconnect(int minDelaySeconds) {
        if (shuttingDown || !config.autoReconnect) {
            return;
        }
        if (config.maxReconnectAttempts > 0 && reconnectAttempts >= config.maxReconnectAttempts) {
            console.error("Maximale Reconnect-Versuche (" + config.maxReconnectAttempts + ") erreicht. Nutze :reconnect.");
            return;
        }
        if (!reconnectScheduled.compareAndSet(false, true)) {
            return; // bereits ein Reconnect geplant
        }
        reconnectAttempts++;
        statReconnects.incrementAndGet();
        int exp = Math.min(reconnectAttempts - 1, 6);
        int delay = Math.min((int) (config.reconnectDelaySeconds * Math.pow(2, exp)), config.maxBackoffSeconds);
        delay = Math.max(Math.max(1, delay), minDelaySeconds);
        int jitterMs = config.reconnectJitterMs > 0
                ? ThreadLocalRandom.current().nextInt(config.reconnectJitterMs + 1) : 0;
        console.info("Reconnect-Versuch " + reconnectAttempts + " in " + delay
                + "s" + (jitterMs > 0 ? " (+" + jitterMs + "ms)" : "") + " ...");
        scheduler.schedule(() -> {
            reconnectScheduled.set(false);
            doConnect();
        }, delay * 1000L + jitterMs, TimeUnit.MILLISECONDS);
    }

    public void reconnectNow() {
        reconnectAttempts = 0;
        intentionalDisconnect = true;
        ClientSession current = session;
        if (current != null && current.isConnected()) {
            current.disconnect(Component.text("Reconnect"));
        } else {
            doConnect();
        }
    }

    public void switchServer(String host, int port) {
        this.host = host;
        this.port = port;
        reconnectAttempts = 0;
        fallbackIndex = 0;
        intentionalDisconnect = true;
        console.info("Wechsle zu " + host + ":" + port + " ...");
        ClientSession current = session;
        if (current != null && current.isConnected()) {
            current.disconnect(Component.text("Serverwechsel"));
        } else {
            doConnect();
        }
    }

    // ===================== Laufzeit-Schalter =====================

    public void setAntiKick(boolean enabled) {
        config.antiKickEnabled = enabled;
        config.save();
        console.info("Anti-Kick (" + describeAntiKick() + ") ist jetzt " + onOff(enabled) + ".");
    }

    public void setMute(boolean muted) {
        config.muteChat = muted;
        config.save();
        console.info("Chat ist jetzt " + (muted ? "stummgeschaltet" : "sichtbar") + ".");
    }

    public void setChatFilter(boolean enabled) {
        config.chatFilterEnabled = enabled;
        config.save();
        console.info("Chat-Filter ist jetzt " + onOff(enabled) + ".");
    }

    public void setAutoTpa(boolean enabled) {
        config.autoAcceptTpa = enabled;
        config.save();
        console.info("Auto-TPA-Annahme ist jetzt " + onOff(enabled) + ".");
    }

    public void setAutoReply(boolean enabled) {
        config.autoReplyEnabled = enabled;
        config.save();
        console.info("Auto-Antwort ist jetzt " + onOff(enabled) + ".");
    }

    private String describeAntiKick() {
        return config.antiKickCommand.isBlank()
                ? "nur Keep-Alive"
                : config.antiKickCommand + " alle " + config.antiKickIntervalSeconds + "s";
    }

    private static String onOff(boolean b) {
        return b ? "AN" : "AUS";
    }

    /** Stellt eine Eingabe in die rate-limitierte Sende-Warteschlange. */
    public void sendChatInput(String input) {
        if (!inGame || session == null || !session.isConnected()) {
            console.error("Nicht verbunden - Nachricht nicht gesendet.");
            return;
        }
        outgoing.add(input);
    }

    private void drainOutgoing() {
        ClientSession current = session;
        if (current == null || !current.isConnected() || !inGame) {
            return;
        }
        String input = outgoing.poll();
        if (input == null) {
            return;
        }
        sendNow(current, input);
    }

    private void sendNow(ClientSession current, String input) {
        int offset = unacknowledged.getAndSet(0);
        if (input.startsWith("/")) {
            current.send(new ServerboundChatCommandPacket(input.substring(1)));
        } else {
            current.send(new ServerboundChatPacket(
                    input, Instant.now().toEpochMilli(), 0L, null, offset, new BitSet(), 0));
        }
    }

    // ===================== Heartbeat (Anti-Kick + periodische Befehle) =====================

    private void heartbeat() {
        ClientSession current = session;
        if (!inGame || current == null || !current.isConnected()) {
            return;
        }
        long now = System.currentTimeMillis();

        // Anti-Kick-Befehl periodisch.
        if (config.antiKickEnabled && !config.antiKickCommand.isBlank()
                && now - lastAntiKick >= config.antiKickIntervalSeconds * 1000L) {
            lastAntiKick = now;
            outgoing.add(config.antiKickCommand);
            if (config.antiKickToggle) {
                outgoing.add(config.antiKickCommand);
            }
        }

        // Frei konfigurierte periodische Befehle.
        for (Config.PeriodicCommand pc : config.periodicCommands) {
            if (pc == null || !pc.enabled || pc.command == null || pc.command.isBlank()) {
                continue;
            }
            int interval = Math.max(5, pc.intervalSeconds);
            long last = periodicLastRun.getOrDefault(pc.command, 0L);
            if (now - last >= interval * 1000L) {
                periodicLastRun.put(pc.command, now);
                outgoing.add(pc.command);
            }
        }
    }

    private void maybeAcknowledge() {
        if (unacknowledged.get() >= ACK_THRESHOLD) {
            int offset = unacknowledged.getAndSet(0);
            ClientSession current = session;
            if (offset > 0 && current != null && current.isConnected()) {
                current.send(new ServerboundChatAckPacket(offset));
            }
        }
    }

    // ===================== Anzeige & Auto-Aktionen =====================

    private void displayChat(Component component) {
        String plainText = plain.serialize(component);
        statChatLines.incrementAndGet();
        if (config.logChat) {
            chatLog.append(plainText);
        }

        // Auto-Aktionen laufen immer - auch wenn die Zeile gleich ausgeblendet wird.
        handleAutoActions(plainText);

        boolean highlight = matchesHighlight(plainText);

        // Stummschaltung / Spam-Filter (Highlights werden nie ausgeblendet).
        if (!highlight) {
            if (config.muteChat) {
                return;
            }
            if (config.chatFilterEnabled && matchesHideFilter(plainText)) {
                return;
            }
        }

        String prefix = config.showTimestamps
                ? Console.GRAY + "[" + LocalTime.now().format(TIME) + "] " + Console.RESET
                : "";
        String body = ansi.serialize(component);
        if (highlight) {
            body = Console.HIGHLIGHT + body + Console.RESET;
            if (config.bellOnHighlight) {
                body = Console.BELL + body;
            }
        }
        console.printAbove(prefix + body);
    }

    private void handleAutoActions(String text) {
        // 1) Auto-Annahme von TPA-Anfragen.
        if (config.autoAcceptTpa && config.tpaRequestMarker != null
                && !config.tpaRequestMarker.isBlank()
                && text.contains(config.tpaRequestMarker)) {
            Matcher m = TPA_SENDER.matcher(text);
            String sender = m.find() ? m.group(1) : null;
            if (sender != null && tpaWhitelisted(sender)) {
                // Doppel-Anfragen kurz hintereinander entprellen.
                long now = System.currentTimeMillis();
                if (!sender.equalsIgnoreCase(lastTpaSender) || now - lastTpaAt > 3000) {
                    lastTpaSender = sender;
                    lastTpaAt = now;
                    outgoing.add(config.tpaAcceptCommand + " " + sender);
                    statTpaAccepted.incrementAndGet();
                    console.info("Auto-TPA: Anfrage von " + sender + " angenommen.");
                }
            }
        }

        // 2) Auto-Antwort auf private Nachrichten.
        if (config.autoReplyEnabled && !config.autoReplyMessage.isBlank()
                && config.privateMessageMarker != null && !config.privateMessageMarker.isBlank()
                && text.contains(config.privateMessageMarker)) {
            String sender = senderBeforeMarker(text, config.privateMessageMarker);
            if (sender != null && !sender.equalsIgnoreCase(auth.username())) {
                long now = System.currentTimeMillis();
                long last = lastAutoReply.getOrDefault(sender.toLowerCase(), 0L);
                if (now - last >= config.autoReplyCooldownSeconds * 1000L) {
                    lastAutoReply.put(sender.toLowerCase(), now);
                    outgoing.add(config.autoReplyCommand + " " + sender + " " + config.autoReplyMessage);
                    console.info("Auto-Antwort an " + sender + " gesendet.");
                }
            }
        }
    }

    private boolean tpaWhitelisted(String sender) {
        if (config.autoAcceptTpaWhitelist.isEmpty()) {
            return true;
        }
        for (String w : config.autoAcceptTpaWhitelist) {
            if (w != null && w.equalsIgnoreCase(sender)) {
                return true;
            }
        }
        return false;
    }

    /** Extrahiert den letzten Wortbestandteil links vom Marker (= Absendername). */
    private static String senderBeforeMarker(String text, String marker) {
        int idx = text.indexOf(marker);
        if (idx <= 0) {
            return null;
        }
        String left = text.substring(0, idx).trim();
        if (left.isEmpty()) {
            return null;
        }
        String[] tokens = left.split("\\s+");
        String candidate = tokens[tokens.length - 1];
        // evtl. Rang-Praefixe/Klammern entfernen.
        candidate = candidate.replaceAll("[\\[\\]<>:]", "").trim();
        return candidate.isEmpty() ? null : candidate;
    }

    private boolean matchesHighlight(String text) {
        String lower = text.toLowerCase();
        if (config.highlightUsername && auth.username() != null
                && lower.contains(auth.username().toLowerCase())) {
            return true;
        }
        for (String keyword : config.highlightKeywords) {
            if (keyword != null && !keyword.isBlank() && lower.contains(keyword.toLowerCase())) {
                return true;
            }
        }
        return false;
    }

    private boolean matchesHideFilter(String text) {
        for (String filter : config.chatHideFilters) {
            if (filter != null && !filter.isBlank() && text.contains(filter)) {
                return true;
            }
        }
        return false;
    }

    // ===================== Status / Statistik =====================

    public void printStatus() {
        boolean connected = session != null && session.isConnected() && inGame;
        console.info("=== Status ===");
        console.info("  Server:         " + host + ":" + port);
        console.info("  Verbunden:      " + (connected ? "ja" : "nein"));
        console.info("  Spieler:        " + auth.username());
        console.info("  Leben/Hunger:   " + Math.round(health) + " / " + food);
        int ping = selfId != null ? playerList.latencyOf(selfId) : -1;
        console.info("  Ping:           " + (ping >= 0 ? ping + " ms" : "?"));
        console.info("  Online:         " + playerList.size());
        console.info("  Anti-Kick:      " + (config.antiKickEnabled ? "an (" + describeAntiKick() + ")" : "aus"));
        console.info("  Auto-Reconnect: " + (config.autoReconnect ? "an" : "aus"));
        console.info("  Auto-Respawn:   " + (config.autoRespawn ? "an" : "aus"));
        console.info("  Auto-TPA:       " + (config.autoAcceptTpa ? "an" : "aus"));
        console.info("  Auto-Antwort:   " + (config.autoReplyEnabled ? "an" : "aus"));
        console.info("  Chat-Filter:    " + (config.chatFilterEnabled ? "an (" + config.chatHideFilters.size() + " Regeln)" : "aus"));
        console.info("  Stumm:          " + (config.muteChat ? "ja" : "nein"));
    }

    public void printStats() {
        long uptime = (System.currentTimeMillis() - startedAt) / 1000;
        long online = connectedAt > 0 && inGame ? (System.currentTimeMillis() - connectedAt) / 1000 : 0;
        console.info("=== Statistik ===");
        console.info("  Laufzeit:        " + formatDuration(uptime));
        console.info("  Aktuell online:  " + formatDuration(online));
        console.info("  Reconnects:      " + statReconnects.get());
        console.info("  Kicks/Trennungen:" + statKicks.get());
        console.info("  Tode:            " + statDeaths.get());
        console.info("  TPA angenommen:  " + statTpaAccepted.get());
        console.info("  Chat-Zeilen:     " + statChatLines.get());
    }

    private static String formatDuration(long seconds) {
        long h = seconds / 3600;
        long m = (seconds % 3600) / 60;
        long s = seconds % 60;
        if (h > 0) {
            return h + "h " + m + "m " + s + "s";
        }
        if (m > 0) {
            return m + "m " + s + "s";
        }
        return s + "s";
    }

    public void printPlayers() {
        List<String> names = playerList.sortedNames();
        console.info("Online (" + names.size() + "): " + (names.isEmpty() ? "-" : String.join(", ", names)));
    }

    public void shutdown() {
        shuttingDown = true;
        intentionalDisconnect = true;
        ClientSession current = session;
        if (current != null && current.isConnected()) {
            current.disconnect(Component.text("Beendet"));
        }
        scheduler.shutdownNow();
    }

    // ===================== Paket-Listener =====================

    private final class Listener extends SessionAdapter {
        @Override
        public void packetReceived(Session ignored, Packet packet) {
            if (packet instanceof ClientboundLoginPacket) {
                onJoin();
            } else if (packet instanceof ClientboundSystemChatPacket chat) {
                displayChat(chat.getContent());
            } else if (packet instanceof ClientboundPlayerChatPacket chat) {
                unacknowledged.incrementAndGet();
                Component content = chat.getUnsignedContent() != null
                        ? chat.getUnsignedContent()
                        : Component.text(chat.getContent());
                displayChat(Component.text("<").append(chat.getName())
                        .append(Component.text("> ")).append(content));
                maybeAcknowledge();
            } else if (packet instanceof ClientboundResourcePackPushPacket pack) {
                // Resource-Pack NICHT laden, aber bestaetigen -> kein Kick bei erzwungenem Pack.
                session.send(new ServerboundResourcePackPacket(pack.getId(), ResourcePackStatus.ACCEPTED));
                session.send(new ServerboundResourcePackPacket(pack.getId(), ResourcePackStatus.SUCCESSFULLY_LOADED));
                console.info("Resource-Pack bestaetigt (nicht geladen).");
            } else if (packet instanceof ClientboundPlayerPositionPacket pos) {
                handlePosition(pos);
            } else if (packet instanceof ClientboundSetHealthPacket hp) {
                handleHealth(hp);
            } else if (packet instanceof ClientboundPlayerInfoUpdatePacket info) {
                handlePlayerInfo(info);
            } else if (packet instanceof ClientboundPlayerInfoRemovePacket remove) {
                handlePlayerRemove(remove);
            } else if (packet instanceof ClientboundStoreCookiePacket cookie) {
                cookies.put(cookie.getKey().asString(), cookie.getPayload());
            } else if (packet instanceof ClientboundCookieRequestPacket request) {
                // Cookie zurueckgeben (oder null) -> kein Haengenbleiben bei Netzwerk-Auth.
                session.send(new ServerboundCookieResponsePacket(
                        request.getKey(), cookies.get(request.getKey().asString())));
            } else if (packet instanceof ClientboundTransferPacket transfer) {
                console.info("Server-Transfer zu " + transfer.getHost() + ":" + transfer.getPort());
                switchServer(transfer.getHost(), transfer.getPort());
            }
        }

        @Override
        public void disconnected(DisconnectedEvent event) {
            inGame = false;
            String reasonPlain = event.getReason() != null ? plain.serialize(event.getReason()) : "";
            String reasonDisplay = event.getReason() != null ? ansi.serialize(event.getReason()) : "unbekannt";
            boolean wasKick = !intentionalDisconnect && !shuttingDown;
            if (wasKick) {
                statKicks.incrementAndGet();
                rejoinAfterKick = !config.onKickCommands.isEmpty();
            }
            String bell = config.bellOnDisconnect ? Console.BELL : "";
            console.error(bell + "Getrennt: " + reasonDisplay);
            scheduleReconnect(looksThrottled(reasonPlain) ? THROTTLE_MIN_DELAY : 0);
        }
    }

    private static boolean looksThrottled(String reason) {
        String r = reason.toLowerCase();
        return r.contains("throttl") || r.contains("wait") || r.contains("already")
                || r.contains("logged in") || r.contains("too fast") || r.contains("slow down")
                || r.contains("try again");
    }

    private void onJoin() {
        inGame = true;
        reconnectAttempts = 0;
        fallbackIndex = 0;
        connectedAt = System.currentTimeMillis();
        // Anti-Kick-Befehl erst nach einem vollen Intervall nach dem Beitritt.
        lastAntiKick = connectedAt;
        unacknowledged.set(0);
        outgoing.clear();
        wasDead = false;
        havePosition = false;
        playerList.clear();
        console.info("Verbunden und im Spiel als " + auth.username() + ".");

        // Beitrittsbefehle.
        if (!config.onJoinCommands.isEmpty()) {
            scheduler.schedule(() -> queueCommands(config.onJoinCommands, "Auto-Beitrittsbefehle"),
                    Math.max(0, config.onJoinDelaySeconds), TimeUnit.SECONDS);
        }

        // Zusaetzliche Befehle nach einem Kick ("wenn gekickt -> Befehl X").
        if (rejoinAfterKick) {
            rejoinAfterKick = false;
            scheduler.schedule(() -> queueCommands(config.onKickCommands, "Nach-Kick-Befehle"),
                    Math.max(0, config.onKickDelaySeconds), TimeUnit.SECONDS);
        }
    }

    private void queueCommands(List<String> commands, String label) {
        int count = 0;
        for (String cmd : commands) {
            if (cmd != null && !cmd.isBlank()) {
                outgoing.add(cmd.trim());
                count++;
            }
        }
        if (count > 0) {
            console.info(label + " gesendet (" + count + ").");
        }
    }

    private void handlePosition(ClientboundPlayerPositionPacket pos) {
        List<PositionElement> rel = pos.getRelatives();
        posX = relative(rel, PositionElement.X, posX, pos.getPosition().getX());
        posY = relative(rel, PositionElement.Y, posY, pos.getPosition().getY());
        posZ = relative(rel, PositionElement.Z, posZ, pos.getPosition().getZ());
        yaw = relative(rel, PositionElement.Y_ROT, yaw, pos.getYRot());
        pitch = relative(rel, PositionElement.X_ROT, pitch, pos.getXRot());
        havePosition = true;
        // WICHTIG: Teleport bestaetigen, sonst Rubber-Banding / Kick. (Keine Eigenbewegung!)
        session.send(new ServerboundAcceptTeleportationPacket(pos.getId()));
        session.send(new ServerboundMovePlayerPosRotPacket(true, false, posX, posY, posZ, yaw, pitch));
    }

    private void handleHealth(ClientboundSetHealthPacket hp) {
        health = hp.getHealth();
        food = hp.getFood();
        if (health <= 0f) {
            if (!wasDead) {
                wasDead = true;
                statDeaths.incrementAndGet();
                if (config.autoRespawn) {
                    console.error("Gestorben - respawne automatisch.");
                    session.send(new ServerboundClientCommandPacket(ClientCommand.PERFORM_RESPAWN));
                } else {
                    console.error("Gestorben (Auto-Respawn aus).");
                }
                if (!config.onDeathCommands.isEmpty()) {
                    scheduler.schedule(() -> queueCommands(config.onDeathCommands, "Tod-Befehle"),
                            1, TimeUnit.SECONDS);
                }
            }
        } else {
            wasDead = false;
            // Aktion bei niedrigem Leben (z. B. zum Spawn warpen).
            if (config.lowHealthActionEnabled && !config.lowHealthCommands.isEmpty()
                    && health <= config.lowHealthThreshold) {
                queueCommands(config.lowHealthCommands, "Niedrig-Leben-Befehle");
            }
        }
    }

    private void handlePlayerInfo(ClientboundPlayerInfoUpdatePacket info) {
        boolean afterInitialSync = System.currentTimeMillis() - connectedAt > 2000;
        for (PlayerListEntry entry : info.getEntries()) {
            UUID id = entry.getProfileId();
            if (info.getActions().contains(PlayerListEntryAction.ADD_PLAYER) && entry.getProfile() != null) {
                boolean isNew = !playerList.isKnown(id);
                playerList.add(id, entry.getProfile().getName());
                if (isNew && afterInitialSync && config.announcePlayerJoinLeave && inGame) {
                    console.info("+ " + entry.getProfile().getName() + " ist beigetreten.");
                }
            }
            if (info.getActions().contains(PlayerListEntryAction.UPDATE_LATENCY)) {
                playerList.setLatency(id, entry.getLatency());
            }
        }
    }

    private void handlePlayerRemove(ClientboundPlayerInfoRemovePacket remove) {
        boolean afterInitialSync = System.currentTimeMillis() - connectedAt > 2000;
        for (UUID id : remove.getProfileIds()) {
            String name = playerList.remove(id);
            if (name != null && afterInitialSync && config.announcePlayerJoinLeave && inGame) {
                console.info("- " + name + " hat verlassen.");
            }
        }
    }

    private double relative(List<PositionElement> relatives, PositionElement element,
                            double current, double value) {
        return relatives.contains(element) ? current + value : value;
    }

    private float relative(List<PositionElement> relatives, PositionElement element,
                           float current, float value) {
        return relatives.contains(element) ? current + value : value;
    }
}
