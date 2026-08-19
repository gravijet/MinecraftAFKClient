package net.gravijet.afk.net;

import org.geysermc.mcprotocollib.auth.GameProfile;
import org.geysermc.mcprotocollib.auth.SessionService;
import org.geysermc.mcprotocollib.network.Session;
import org.geysermc.mcprotocollib.network.event.session.SessionListener;
import org.geysermc.mcprotocollib.network.packet.Packet;
import org.geysermc.mcprotocollib.network.tcp.TcpClientSession;
import org.geysermc.mcprotocollib.protocol.MinecraftConstants;
import org.geysermc.mcprotocollib.protocol.MinecraftProtocol;
import org.geysermc.mcprotocollib.protocol.data.game.entity.player.HandPreference;
import org.geysermc.mcprotocollib.protocol.data.game.entity.player.PositionElement;
import org.geysermc.mcprotocollib.protocol.data.game.setting.ChatVisibility;
import org.geysermc.mcprotocollib.protocol.data.game.setting.SkinPart;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ClientboundCookieRequestPacket;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ClientboundStoreCookiePacket;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ServerboundCookieResponsePacket;
import org.geysermc.mcprotocollib.protocol.packet.common.serverbound.ServerboundClientInformationPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.entity.player.ClientboundPlayerPositionPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundChatPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.player.ServerboundMovePlayerPosRotPacket;

import java.util.BitSet;
import java.util.List;
import java.util.Map;
import java.util.concurrent.Executor;

/**
 * Alles, was sich zwischen den MCProtocolLib-Fassungen unterscheidet – hier die <b>alte</b>
 * Fassung für <b>Minecraft 1.21.1</b> (Protokoll 767).
 *
 * <p>Das Gegenstück liegt in {@code src/api-modern/java}; {@code build.gradle.kts} entscheidet
 * anhand von {@code -Pmc}, welche Datei mitkompiliert wird. Der übrige Code kennt den Unterschied
 * nicht.
 *
 * <p>Unterschiede zur modernen Fassung, alle gegen die Bibliotheksquellen geprüft:
 * <ul>
 *   <li>Sitzung: {@code TcpClientSession} statt Factory + {@code ClientSession}. Damit gibt es
 *       auch keinen eigenen Paket-Executor – die Pakete laufen auf dem Netty-Thread.</li>
 *   <li>Client-Einstellungen: ohne Partikel-Status (kam mit 1.21.2).</li>
 *   <li>Chat: ohne Prüfsummenfeld (kam mit 1.21.11).</li>
 *   <li>Bewegung: ohne Feld für die horizontale Kollision (kam mit 1.21.4).</li>
 *   <li>Position: Koordinaten vorn, Teleport-Nummer hinten, kein Bewegungsvektor.</li>
 *   <li>Cookie-Pakete liegen in {@code packet.common.clientbound}.</li>
 * </ul>
 */
final class Net {

    /** Einmalig angelegt: alle Skin-Teile sichtbar. Spart pro Beitritt Array-Clone + Liste. */
    private static final List<SkinPart> SKIN_PARTS = List.of(SkinPart.values());

    private Net() {
    }

    /** Sitzung anlegen und einrichten – aber noch <b>nicht</b> verbinden (siehe {@link #start}). */
    static Session create(String host, int port, GameProfile profile, String accessToken,
                          SessionService service, Executor packetExecutor, SessionListener listener) {
        // Diese Fassung kennt keinen eigenen Executor für die Paketverarbeitung; der Parameter
        // bleibt deshalb ungenutzt (die Schnittstelle ist für beide Fassungen dieselbe).
        Session session = new TcpClientSession(host, port, new MinecraftProtocol(profile, accessToken));
        session.setFlag(MinecraftConstants.SESSION_SERVICE_KEY, service);
        // Keep-Alive beantworten wir selbst (siehe AfkClient.Listener) – Automatik aus.
        session.setFlag(MinecraftConstants.AUTOMATIC_KEEP_ALIVE_MANAGEMENT, false);
        // Transfers selbst behandeln, damit unser Listener erhalten bleibt.
        session.setFlag(MinecraftConstants.FOLLOW_TRANSFERS, false);
        session.addListener(listener);
        return session;
    }

    static void start(Session session) {
        session.connect();
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
                false, true);
    }

    /** Unsignierte Chat-Nachricht. */
    static Packet chat(String message, long timestamp, int offset) {
        return new ServerboundChatPacket(message, timestamp, 0L, null, offset, new BitSet());
    }

    static Packet move(double x, double y, double z, float yaw, float pitch, boolean onGround) {
        return new ServerboundMovePlayerPosRotPacket(onGround, x, y, z, yaw, pitch);
    }

    /**
     * Positionspaket auf absolute Werte bringen. Die übergebene Position ist die bisher bekannte –
     * relative Angaben rechnen darauf auf.
     *
     * <p>Achtung bei den Bits: diese Bibliotheksfassung nennt Element 3 {@code PITCH} und
     * Element 4 {@code YAW}, tatsächlich ist es (wie im Vanilla-Client und in allen neueren
     * Fassungen) umgekehrt – Bit 3 gehört zum Gierwinkel, Bit 4 zur Neigung. Deshalb sind die
     * beiden hier bewusst über Kreuz benutzt.
     */
    static Teleport teleport(ClientboundPlayerPositionPacket packet,
                             double x, double y, double z, float yaw, float pitch) {
        List<PositionElement> relative = packet.getRelative();
        return new Teleport(
                packet.getTeleportId(),
                relative.contains(PositionElement.X) ? x + packet.getX() : packet.getX(),
                relative.contains(PositionElement.Y) ? y + packet.getY() : packet.getY(),
                relative.contains(PositionElement.Z) ? z + packet.getZ() : packet.getZ(),
                relative.contains(PositionElement.PITCH) ? yaw + packet.getYaw() : packet.getYaw(),
                relative.contains(PositionElement.YAW) ? pitch + packet.getPitch() : packet.getPitch());
    }

    /**
     * Cookies merken bzw. auf Anfrage zurückgeben. Die beiden Pakete liegen je nach Fassung in
     * einem anderen Java-Paket, deshalb laufen sie hier durch.
     *
     * @return {@code true}, wenn das Paket hier erledigt wurde
     */
    static boolean cookies(Packet packet, Session session, Map<String, byte[]> cookies) {
        if (packet instanceof ClientboundStoreCookiePacket store) {
            cookies.put(store.getKey().asString(), store.getPayload());
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
