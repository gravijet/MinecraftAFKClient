package example.invalid;

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
import org.geysermc.mcprotocollib.protocol.data.game.entity.player.Hand;
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
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.player.ServerboundSwingPacket;

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

/**
 * Verbindet sich mit dem Server und behandelt eingehende Pakete (Chat, Resource-Pack,
 * Teleport, Health, Spielerliste, Cookies, Transfer). Sendet Chat/Befehle (rate-limitiert),
 * haelt die Verbindung per Anti-AFK aktiv, respawnt automatisch und verbindet bei
 * Verbindungsabbruch mit Backoff neu. Ziel: moeglichst nie gekickt zu werden.
 */
public class AfkClient {

    private static final int ACK_THRESHOLD = 20;
    private static final int MAX_BACKOFF_SECONDS = 300;
    private static final int THROTTLE_MIN_DELAY = 30;
    private static final DateTimeFormatter TIME = DateTimeFormatter.ofPattern("HH:mm:ss");

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

    private volatile ClientSession session;
    private volatile boolean shuttingDown = false;
    private volatile boolean inGame = false;
    private volatile int reconnectAttempts = 0;
    private volatile long connectedAt = 0;

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

        scheduler.scheduleAtFixedRate(this::antiAfkTick,
                config.antiAfkSeconds, config.antiAfkSeconds, TimeUnit.SECONDS);
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
            scheduleReconnect(0);
        }
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
        int exp = Math.min(reconnectAttempts - 1, 6);
        int delay = Math.min((int) (config.reconnectDelaySeconds * Math.pow(2, exp)), MAX_BACKOFF_SECONDS);
        delay = Math.max(Math.max(1, delay), minDelaySeconds);
        console.info("Reconnect-Versuch " + reconnectAttempts + " in " + delay + "s ...");
        scheduler.schedule(() -> {
            reconnectScheduled.set(false);
            doConnect();
        }, delay, TimeUnit.SECONDS);
    }

    public void reconnectNow() {
        reconnectAttempts = 0;
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
        console.info("Wechsle zu " + host + ":" + port + " ...");
        ClientSession current = session;
        if (current != null && current.isConnected()) {
            current.disconnect(Component.text("Serverwechsel"));
        } else {
            doConnect();
        }
    }

    public void setAntiAfk(boolean enabled) {
        config.antiAfkEnabled = enabled;
        config.save();
        console.info("Anti-AFK ist jetzt " + (enabled ? "AN" : "AUS") + ".");
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
        int offset = unacknowledged.getAndSet(0);
        if (input.startsWith("/")) {
            current.send(new ServerboundChatCommandPacket(input.substring(1)));
        } else {
            current.send(new ServerboundChatPacket(
                    input, Instant.now().toEpochMilli(), 0L, null, offset, new BitSet(), 0));
        }
    }

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
        console.info("  Anti-AFK:       " + (config.antiAfkEnabled ? "an (" + config.antiAfkSeconds + "s)" : "aus"));
        console.info("  Auto-Reconnect: " + (config.autoReconnect ? "an" : "aus"));
        console.info("  Auto-Respawn:   " + (config.autoRespawn ? "an" : "aus"));
    }

    public void printPlayers() {
        List<String> names = playerList.sortedNames();
        console.info("Online (" + names.size() + "): " + (names.isEmpty() ? "-" : String.join(", ", names)));
    }

    public void shutdown() {
        shuttingDown = true;
        ClientSession current = session;
        if (current != null && current.isConnected()) {
            current.disconnect(Component.text("Beendet"));
        }
        scheduler.shutdownNow();
    }

    private void antiAfkTick() {
        ClientSession current = session;
        if (!config.antiAfkEnabled || !inGame || current == null || !current.isConnected()) {
            return;
        }
        current.send(new ServerboundSwingPacket(Hand.MAIN_HAND));
        if (havePosition) {
            // Leichte Dreh-/Blickaenderung gegen AFK-Erkennung (ohne die Position zu aendern).
            ThreadLocalRandom rnd = ThreadLocalRandom.current();
            yaw = (yaw + rnd.nextFloat() * 20f - 10f) % 360f;
            pitch = Math.max(-30f, Math.min(30f, pitch + rnd.nextFloat() * 10f - 5f));
            current.send(new ServerboundMovePlayerPosRotPacket(true, false, posX, posY, posZ, yaw, pitch));
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

    private void displayChat(Component component) {
        String plainText = plain.serialize(component);
        if (config.logChat) {
            chatLog.append(plainText);
        }
        boolean highlight = matchesHighlight(plainText);
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

    private static boolean looksThrottled(String reason) {
        String r = reason.toLowerCase();
        return r.contains("throttl") || r.contains("wait") || r.contains("already")
                || r.contains("logged in") || r.contains("too fast") || r.contains("slow down")
                || r.contains("try again");
    }

    private double relative(List<PositionElement> relatives, PositionElement element,
                            double current, double value) {
        return relatives.contains(element) ? current + value : value;
    }

    private float relative(List<PositionElement> relatives, PositionElement element,
                           float current, float value) {
        return relatives.contains(element) ? current + value : value;
    }

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
            console.error("Getrennt: " + reasonDisplay);
            scheduleReconnect(looksThrottled(reasonPlain) ? THROTTLE_MIN_DELAY : 0);
        }
    }

    private void onJoin() {
        inGame = true;
        reconnectAttempts = 0;
        connectedAt = System.currentTimeMillis();
        unacknowledged.set(0);
        outgoing.clear();
        wasDead = false;
        havePosition = false;
        playerList.clear();
        console.info("Verbunden und im Spiel als " + auth.username() + ".");

        if (!config.onJoinCommands.isEmpty()) {
            scheduler.schedule(() -> {
                for (String cmd : config.onJoinCommands) {
                    if (cmd != null && !cmd.isBlank()) {
                        outgoing.add(cmd.trim());
                    }
                }
                console.info("Auto-Beitrittsbefehle gesendet (" + config.onJoinCommands.size() + ").");
            }, Math.max(0, config.onJoinDelaySeconds), TimeUnit.SECONDS);
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
        // WICHTIG: Teleport bestaetigen, sonst Rubber-Banding / Kick.
        session.send(new ServerboundAcceptTeleportationPacket(pos.getId()));
        session.send(new ServerboundMovePlayerPosRotPacket(true, false, posX, posY, posZ, yaw, pitch));
    }

    private void handleHealth(ClientboundSetHealthPacket hp) {
        health = hp.getHealth();
        food = hp.getFood();
        if (health <= 0f) {
            if (!wasDead) {
                wasDead = true;
                if (config.autoRespawn) {
                    console.error("Gestorben - respawne automatisch.");
                    session.send(new ServerboundClientCommandPacket(ClientCommand.PERFORM_RESPAWN));
                } else {
                    console.error("Gestorben (Auto-Respawn aus).");
                }
            }
        } else {
            wasDead = false;
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
}
