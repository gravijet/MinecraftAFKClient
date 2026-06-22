package example.invalid;

import com.hugoafk.auth.AuthManager;
import com.hugoafk.config.Config;
import com.hugoafk.ui.Console;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.serializer.ansi.ANSIComponentSerializer;
import org.geysermc.mcprotocollib.auth.SessionService;
import org.geysermc.mcprotocollib.network.ClientSession;
import org.geysermc.mcprotocollib.network.Session;
import org.geysermc.mcprotocollib.network.event.session.DisconnectedEvent;
import org.geysermc.mcprotocollib.network.event.session.SessionAdapter;
import org.geysermc.mcprotocollib.network.factory.ClientNetworkSessionFactory;
import org.geysermc.mcprotocollib.network.packet.Packet;
import org.geysermc.mcprotocollib.protocol.MinecraftConstants;
import org.geysermc.mcprotocollib.protocol.MinecraftProtocol;
import org.geysermc.mcprotocollib.protocol.data.game.ResourcePackStatus;
import org.geysermc.mcprotocollib.protocol.data.game.entity.player.Hand;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ClientboundResourcePackPushPacket;
import org.geysermc.mcprotocollib.protocol.packet.common.serverbound.ServerboundResourcePackPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.ClientboundLoginPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.ClientboundPlayerChatPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.ClientboundSystemChatPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundChatAckPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundChatCommandPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundChatPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.player.ServerboundSwingPacket;

import java.net.InetSocketAddress;
import java.time.Instant;
import java.util.BitSet;
import java.util.concurrent.Executors;
import java.util.concurrent.ScheduledExecutorService;
import java.util.concurrent.TimeUnit;
import java.util.concurrent.atomic.AtomicInteger;

/**
 * Verbindet sich mit dem Server, behandelt eingehende Pakete (Chat, Resource-Pack),
 * sendet Chat/Befehle, haelt die Verbindung per Anti-AFK aktiv und verbindet bei
 * Verbindungsabbruch automatisch neu.
 */
public class AfkClient {

    private static final int ACK_THRESHOLD = 20;

    private final AuthManager auth;
    private final Config config;
    private final Console console;
    private final SessionService sessionService = new SessionService();
    private final ANSIComponentSerializer ansi = ANSIComponentSerializer.ansi();
    private final ScheduledExecutorService scheduler =
            Executors.newSingleThreadScheduledExecutor(r -> {
                Thread t = new Thread(r, "hugoafk-scheduler");
                t.setDaemon(true);
                return t;
            });

    /** Zaehlt empfangene signierte Chat-Nachrichten, die noch bestaetigt werden muessen. */
    private final AtomicInteger unacknowledged = new AtomicInteger();

    private volatile ClientSession session;
    private volatile boolean shuttingDown = false;
    private volatile boolean inGame = false;

    private String host;
    private int port;

    public AfkClient(AuthManager auth, Config config, Console console) {
        this.auth = auth;
        this.config = config;
        this.console = console;

        // Ein einziger Anti-AFK-Task fuer die gesamte Laufzeit; prueft selbst, ob aktiv.
        scheduler.scheduleAtFixedRate(this::antiAfkTick,
                config.antiAfkSeconds, config.antiAfkSeconds, TimeUnit.SECONDS);
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
            MinecraftProtocol protocol = new MinecraftProtocol(auth.gameProfile(), auth.accessToken());
            ClientSession client = ClientNetworkSessionFactory.factory()
                    .setRemoteSocketAddress(InetSocketAddress.createUnresolved(host, port))
                    .setProtocol(protocol)
                    .create();
            client.setFlag(MinecraftConstants.SESSION_SERVICE_KEY, sessionService);
            client.addListener(new Listener());
            this.session = client;
            console.info("Verbinde zu " + host + ":" + port + " ...");
            client.connect();
        } catch (Exception e) {
            console.error("Verbindung fehlgeschlagen: " + e.getMessage());
            scheduleReconnect();
        }
    }

    private void scheduleReconnect() {
        if (shuttingDown || !config.autoReconnect) {
            return;
        }
        int delay = Math.max(1, config.reconnectDelaySeconds);
        console.info("Neuer Verbindungsversuch in " + delay + "s ...");
        scheduler.schedule(this::doConnect, delay, TimeUnit.SECONDS);
    }

    public void reconnectNow() {
        ClientSession current = session;
        if (current != null && current.isConnected()) {
            current.disconnect(Component.text("Reconnect"));
        } else {
            doConnect();
        }
    }

    public void setAntiAfk(boolean enabled) {
        config.antiAfkEnabled = enabled;
        config.save();
        console.info("Anti-AFK ist jetzt " + (enabled ? "AN" : "AUS") + ".");
    }

    /** Verarbeitet eine Eingabezeile: '/' = Befehl, sonst normale Chat-Nachricht. */
    public void sendChatInput(String input) {
        ClientSession current = session;
        if (current == null || !current.isConnected() || !inGame) {
            console.error("Nicht verbunden - Nachricht nicht gesendet.");
            return;
        }
        int offset = unacknowledged.getAndSet(0);
        if (input.startsWith("/")) {
            current.send(new ServerboundChatCommandPacket(input.substring(1)));
        } else {
            current.send(new ServerboundChatPacket(
                    input,
                    Instant.now().toEpochMilli(),
                    0L,
                    null,            // unsigniert (Server mit enforce-secure-profile=false)
                    offset,
                    new BitSet(),
                    0));
        }
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
        if (config.antiAfkEnabled && inGame && current != null && current.isConnected()) {
            current.send(new ServerboundSwingPacket(Hand.MAIN_HAND));
        }
    }

    /** Bestaetigt empfangene Nachrichten, bevor der Server-Puffer ueberlaeuft. */
    private void maybeAcknowledge() {
        if (unacknowledged.get() >= ACK_THRESHOLD) {
            int offset = unacknowledged.getAndSet(0);
            ClientSession current = session;
            if (offset > 0 && current != null && current.isConnected()) {
                current.send(new ServerboundChatAckPacket(offset));
            }
        }
    }

    private void printChat(Component component) {
        console.printAbove(ansi.serialize(component));
    }

    private final class Listener extends SessionAdapter {
        @Override
        public void packetReceived(Session ignored, Packet packet) {
            if (packet instanceof ClientboundLoginPacket) {
                inGame = true;
                unacknowledged.set(0);
                console.info("Verbunden und im Spiel als " + auth.username() + ".");
            } else if (packet instanceof ClientboundSystemChatPacket chat) {
                printChat(chat.getContent());
            } else if (packet instanceof ClientboundPlayerChatPacket chat) {
                unacknowledged.incrementAndGet();
                Component content = chat.getUnsignedContent() != null
                        ? chat.getUnsignedContent()
                        : Component.text(chat.getContent());
                printChat(Component.text("<").append(chat.getName())
                        .append(Component.text("> ")).append(content));
                maybeAcknowledge();
            } else if (packet instanceof ClientboundResourcePackPushPacket pack) {
                // Resource-Pack NICHT laden, aber bestaetigen, sonst Kick auf Servern,
                // die das Pack erzwingen.
                session.send(new ServerboundResourcePackPacket(pack.getId(), ResourcePackStatus.ACCEPTED));
                session.send(new ServerboundResourcePackPacket(pack.getId(), ResourcePackStatus.SUCCESSFULLY_LOADED));
                console.info("Resource-Pack bestaetigt (nicht geladen).");
            }
        }

        @Override
        public void disconnected(DisconnectedEvent event) {
            inGame = false;
            String reason = event.getReason() != null ? ansi.serialize(event.getReason()) : "unbekannt";
            console.error("Getrennt: " + reason);
            scheduleReconnect();
        }
    }
}
