package net.gravijet.afk.net;

import org.geysermc.mcprotocollib.auth.GameProfile;
import org.geysermc.mcprotocollib.auth.SessionService;
import org.geysermc.mcprotocollib.network.ClientSession;
import org.geysermc.mcprotocollib.network.Session;
import org.geysermc.mcprotocollib.network.event.session.SessionListener;
import org.geysermc.mcprotocollib.network.factory.ClientNetworkSessionFactory;
import org.geysermc.mcprotocollib.network.packet.Packet;
import org.geysermc.mcprotocollib.protocol.MinecraftConstants;
import org.geysermc.mcprotocollib.protocol.MinecraftProtocol;
import org.geysermc.mcprotocollib.protocol.data.game.entity.player.HandPreference;
import org.geysermc.mcprotocollib.protocol.data.game.entity.player.PositionElement;
import org.geysermc.mcprotocollib.protocol.data.game.setting.ChatVisibility;
import org.geysermc.mcprotocollib.protocol.data.game.setting.ParticleStatus;
import org.geysermc.mcprotocollib.protocol.data.game.setting.SkinPart;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ClientboundStoreCookiePacket;
import org.geysermc.mcprotocollib.protocol.packet.common.serverbound.ServerboundClientInformationPacket;
import org.geysermc.mcprotocollib.protocol.packet.cookie.clientbound.ClientboundCookieRequestPacket;
import org.geysermc.mcprotocollib.protocol.packet.cookie.serverbound.ServerboundCookieResponsePacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.entity.player.ClientboundPlayerPositionPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundChatPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.player.ServerboundMovePlayerPosRotPacket;

import java.net.InetSocketAddress;
import java.util.BitSet;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executor;

/**
 * Alles, was sich zwischen den MCProtocolLib-Fassungen unterscheidet – hier die <b>moderne</b>
 * Fassung für Minecraft 1.21.11, 26.1 und 26.2.
 *
 * <p>Die Gegenstücke liegen in {@code src/api-legacy/java} (Minecraft 1.21.1). Welche der beiden
 * Dateien mitkompiliert wird, entscheidet {@code build.gradle.kts} anhand von {@code -Pmc}. Der
 * übrige Code kennt den Unterschied nicht.
 *
 * <p>Es sind genau sechs Stellen: Sitzungsaufbau, Client-Einstellungen (Partikel-Status),
 * Chat-Paket (Prüfsumme), Bewegungspaket (horizontale Kollision), Positionspaket (Vektorformat)
 * und die Cookie-Pakete (anderes Java-Paket).
 */
final class Net {

    /** Einmalig angelegt: alle Skin-Teile sichtbar. Spart pro Beitritt Array-Clone + Liste. */
    private static final List<SkinPart> SKIN_PARTS = List.of(SkinPart.values());

    private Net() {
    }

    /** Sitzung anlegen und einrichten – aber noch <b>nicht</b> verbinden (siehe {@link #start}). */
    static Session create(String host, int port, GameProfile profile, String accessToken,
                          SessionService service, Executor packetExecutor, SessionListener listener) {
        ClientSession session = ClientNetworkSessionFactory.factory()
                .setRemoteSocketAddress(InetSocketAddress.createUnresolved(host, port))
                .setProtocol(new MinecraftProtocol(profile, accessToken))
                .setPacketHandlerExecutor(packetExecutor)
                .create();
        session.setFlag(MinecraftConstants.SESSION_SERVICE_KEY, service);
        // Keep-Alive beantworten wir selbst (siehe AfkClient.Listener) – Automatik aus.
        session.setFlag(MinecraftConstants.AUTOMATIC_KEEP_ALIVE_MANAGEMENT, false);
        // Transfers selbst behandeln, damit unser Listener erhalten bleibt.
        session.setFlag(MinecraftConstants.FOLLOW_TRANSFERS, false);
        session.addListener(listener);
        return session;
    }

    static void start(Session session) {
        ((ClientSession) session).connect();
    }

    /**
     * Spieleinstellungen wie ein echter Client (manche Server erwarten das).
     *
     * <p>Die Sichtweite ist dabei kein Beiwerk: Sie entscheidet, wie viele Chunkdaten der Server
     * schickt – und die entpackt die Bibliothek alle, auch wenn dieser Client sie nie ansieht.
     */
    static Packet clientInformation(int viewDistance) {
        return new ServerboundClientInformationPacket(
                "de_DE", Math.min(32, Math.max(2, viewDistance)), ChatVisibility.FULL, true,
                SKIN_PARTS, HandPreference.RIGHT_HAND,
                false, true, ParticleStatus.ALL);
    }

    /** Unsignierte Chat-Nachricht. Die Prüfsumme 0 heißt „bitte nicht prüfen". */
    static Packet chat(String message, long timestamp, int offset) {
        return new ServerboundChatPacket(message, timestamp, 0L, null, offset, new BitSet(), 0);
    }

    static Packet move(double x, double y, double z, float yaw, float pitch, boolean onGround) {
        return new ServerboundMovePlayerPosRotPacket(onGround, false, x, y, z, yaw, pitch);
    }

    /**
     * Positionspaket auf absolute Werte bringen. Die übergebene Position ist die bisher bekannte –
     * relative Angaben rechnen darauf auf.
     */
    static Teleport teleport(ClientboundPlayerPositionPacket packet,
                             double x, double y, double z, float yaw, float pitch) {
        List<PositionElement> relative = packet.getRelatives();
        return new Teleport(
                packet.getId(),
                relative.contains(PositionElement.X) ? x + packet.getPosition().getX() : packet.getPosition().getX(),
                relative.contains(PositionElement.Y) ? y + packet.getPosition().getY() : packet.getPosition().getY(),
                relative.contains(PositionElement.Z) ? z + packet.getPosition().getZ() : packet.getPosition().getZ(),
                relative.contains(PositionElement.Y_ROT) ? yaw + packet.getYRot() : packet.getYRot(),
                relative.contains(PositionElement.X_ROT) ? pitch + packet.getXRot() : packet.getXRot());
    }

    /**
     * Cookies merken bzw. auf Anfrage zurückgeben. Die beiden Pakete liegen je nach Fassung in
     * einem anderen Java-Paket, deshalb laufen sie hier durch.
     *
     * @param maxCookies wie viele verschiedene Cookies höchstens aufgehoben werden
     * @param maxBytes   wie groß ein einzelnes Cookie höchstens sein darf
     * @return {@code true}, wenn das Paket hier erledigt wurde
     */
    static boolean cookies(Packet packet, Session session, Map<String, byte[]> cookies,
                           int maxCookies, int maxBytes) {
        if (packet instanceof ClientboundStoreCookiePacket store) {
            String key = store.getKey().asString();
            byte[] payload = store.getPayload();
            // Dieselben Grenzen wie im Vanilla-Client: Ohne sie legte ein Server unter immer
            // neuen Namen beliebig viele Cookies ab und ließ den Speicher volllaufen. Ein schon
            // bekanntes Cookie darf sich immer erneuern, nur neue zählen gegen die Anzahl.
            if (payload != null && payload.length <= maxBytes
                    && (cookies.size() < maxCookies || cookies.containsKey(key))) {
                cookies.put(key, payload);
            }
            return true;
        }
        if (packet instanceof ClientboundCookieRequestPacket request) {
            if (session != null && session.isConnected()) {
                session.send(new ServerboundCookieResponsePacket(
                        request.getKey(), cookies.get(request.getKey().asString())));
            }
            return true;
        }
        return false;
    }
}
