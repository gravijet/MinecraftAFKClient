package net.gravijet.afk.net;

import net.gravijet.afk.Options;
import net.gravijet.afk.auth.AuthManager;
import net.gravijet.afk.ui.Console;
import net.kyori.adventure.text.Component;
import net.kyori.adventure.text.serializer.ansi.ANSIComponentSerializer;
import net.kyori.adventure.text.serializer.plain.PlainTextComponentSerializer;
import org.geysermc.mcprotocollib.auth.SessionService;
import org.geysermc.mcprotocollib.network.Session;
import org.geysermc.mcprotocollib.network.event.session.DisconnectedEvent;
import org.geysermc.mcprotocollib.network.event.session.SessionAdapter;
import org.geysermc.mcprotocollib.network.packet.Packet;
import org.geysermc.mcprotocollib.protocol.data.game.ClientCommand;
import org.geysermc.mcprotocollib.protocol.data.game.ResourcePackStatus;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ClientboundKeepAlivePacket;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ClientboundPingPacket;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ClientboundResourcePackPushPacket;
import org.geysermc.mcprotocollib.protocol.packet.common.clientbound.ClientboundTransferPacket;
import org.geysermc.mcprotocollib.protocol.packet.common.serverbound.ServerboundKeepAlivePacket;
import org.geysermc.mcprotocollib.protocol.packet.common.serverbound.ServerboundPongPacket;
import org.geysermc.mcprotocollib.protocol.packet.common.serverbound.ServerboundResourcePackPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.ClientboundLoginPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.ClientboundPlayerChatPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.ClientboundSystemChatPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.entity.player.ClientboundPlayerPositionPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.clientbound.entity.player.ClientboundSetHealthPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundChatAckPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundChatCommandPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.ServerboundClientCommandPacket;
import org.geysermc.mcprotocollib.protocol.packet.ingame.serverbound.level.ServerboundAcceptTeleportationPacket;

import java.time.Instant;
import java.util.Arrays;
import java.util.List;
import java.util.Map;
import java.util.concurrent.ConcurrentHashMap;
import java.util.concurrent.ExecutorService;
import java.util.concurrent.Executors;
import java.util.concurrent.LinkedBlockingQueue;
import java.util.concurrent.atomic.AtomicBoolean;
import java.util.concurrent.atomic.AtomicInteger;
import java.util.concurrent.atomic.AtomicLong;

/**
 * Schlanker AFK-Client: verbindet sich, hält die Verbindung (rein protokollbasierter Kick-Schutz,
 * <b>keine</b> Anti-AFK-Bewegung), sendet/empfängt Chat und Befehle und verbindet bei Abbruch mit
 * Backoff neu.
 *
 * <p>Kick-Schutz = genau das, was ein wartender Vanilla-Client tut: Server-{@code KeepAlive} sofort
 * beantworten, {@code Ping}→{@code Pong}, Teleport bestätigen (gegen Rubberband-Kick), erzwungene
 * Resource-Packs bestätigen (nicht laden), beim Beitritt {@code ClientInformation} senden, Cookies
 * beantworten. Es werden <b>keine</b> periodischen Positions-/Umsehen-/Schwung-Pakete gesendet.
 *
 * <p>Alles, was sich zwischen den Minecraft-Versionen unterscheidet, steckt in {@link Net} – von
 * dieser Klasse aus ist der Unterschied unsichtbar.
 *
 * <p>Nur in der Bewegungs-Jar ({@code -Pmove=true}) kommt über {@link Mover} gesteuerte Bewegung auf
 * Zuruf dazu ({@code :go}, {@code :look}, {@code :home}). Im schlanken Jar fehlt diese Klasse, und
 * {@link Mover#load()} liefert eine Brücke, die nichts tut.
 */
public class AfkClient {

    /**
     * Ab so vielen unquittierten <b>signierten</b> Nachrichten wird ungefragt quittiert – derselbe
     * Schwellwert wie im Vanilla-Client (der Server trennt erst bei 4096).
     */
    private static final int ACK_THRESHOLD = 64;
    /** Längengrenzen des Servers: Chat-Nachricht 256 Zeichen, Befehl 32500. */
    private static final int MAX_MESSAGE_CHARS = 256;
    private static final int MAX_COMMAND_CHARS = 32_500;

    private final AuthManager auth;
    private final Options options;
    private final Console console;
    private final String minecraftVersion;
    /** Gesteuerte Bewegung – im schlanken Jar eine Brücke, die nichts tut. */
    private final Mover mover = Mover.load();
    private final SessionService sessionService = new SessionService();
    private final ANSIComponentSerializer ansi = ANSIComponentSerializer.ansi();
    private final PlainTextComponentSerializer plain = PlainTextComponentSerializer.plainText();

    /** Ein einziger Thread für die Paketverarbeitung (statt eines Default-Pools). */
    private final ExecutorService packetExecutor = Executors.newSingleThreadExecutor(daemon("afk-packet"));
    /** Ausgehende Nachrichten/Befehle, rate-limitiert von einem einzigen Sender-Thread gesendet. */
    private final LinkedBlockingQueue<String> outgoing = new LinkedBlockingQueue<>();
    private final Thread senderThread;

    /** Server-Cookies (für Transfers / Netzwerk-Auth) merken und auf Anfrage zurückgeben. */
    private final Map<String, byte[]> cookies = new ConcurrentHashMap<>();
    private final AtomicInteger unacknowledged = new AtomicInteger();
    /** Signatur der letzten gezählten Nachricht (siehe {@link #countForAcknowledgement}). */
    private volatile byte[] lastCountedSignature = null;
    /** Zeitstempel der letzten gesendeten Nachricht; der Server verlangt monotone Zeit. */
    private final AtomicLong lastChatTimestamp = new AtomicLong();
    private final AtomicBoolean reconnectScheduled = new AtomicBoolean(false);

    /**
     * Zählt „Verbindungs-Generationen" (pro TCP-Verbindung eine). Wird bei jeder Trennung erhöht,
     * damit ein wartender Auto-Befehl merkt, dass er zu einer bereits toten Verbindung gehört, und
     * dann NICHT feuert. Reine Zahl -> kein zusätzlicher Thread/Timer im Leerlauf.
     */
    private final AtomicInteger connectionGeneration = new AtomicInteger();
    /** Läuft nur, solange wirklich Befehle geplant sind (siehe {@link #startCommands}). */
    private volatile Thread commandThread = null;

    private volatile Session session;
    private volatile boolean inGame = false;
    /** Trennung wurde von uns ausgelöst (Server-Transfer) -> ohne Backoff neu verbinden. */
    private volatile boolean intentionalDisconnect = false;
    private volatile int reconnectAttempts = 0;
    /**
     * true, sobald auf DIESER TCP-Verbindung schon ein Login-Paket kam. Das erste Login = echter
     * Beitritt zum (Velocity/BungeeCord-)Proxy; jedes weitere Login auf derselben Verbindung ist nur
     * ein Wechsel zwischen Unterservern und zählt bewusst NICHT als neuer Serverbeitritt.
     */
    private volatile boolean joinedThisConnection = false;

    // Spielzustand (nur so viel wie für Teleport-Bestätigung/Respawn nötig).
    private volatile boolean havePosition = false;
    private volatile double posX, posY, posZ;
    private volatile float yaw, pitch;
    private volatile boolean wasDead = false;

    private String host;
    private int port;

    public AfkClient(AuthManager auth, Options options, Console console, String minecraftVersion) {
        this.auth = auth;
        this.options = options;
        this.console = console;
        this.minecraftVersion = minecraftVersion;
        this.senderThread = new Thread(this::runSender, "afk-sender");
        this.senderThread.setDaemon(true);
        this.senderThread.start();
        this.mover.attach(this, console);
    }

    /** Bewegung dieses Builds (siehe {@link Mover}). */
    public Mover mover() {
        return mover;
    }

    private static java.util.concurrent.ThreadFactory daemon(String name) {
        return r -> {
            Thread t = new Thread(r, name);
            t.setDaemon(true);
            return t;
        };
    }

    // ===================== Verbindung =====================

    public void connect(String host, int port) {
        this.host = host;
        this.port = port;
        doConnect();
    }

    private void doConnect() {
        intentionalDisconnect = false;
        joinedThisConnection = false;
        try {
            Session client = Net.create(host, port, auth.gameProfile(), auth.accessToken(),
                    sessionService, packetExecutor, new Listener());
            // Erst merken, dann verbinden: der Listener kann sofort Pakete bekommen und muss
            // dafür die Sitzung schon kennen.
            this.session = client;
            console.note("Verbinde zu " + host + ":" + port + " (MC " + minecraftVersion + ") ...");
            Net.start(client);
        } catch (Exception e) {
            console.error("Verbindung fehlgeschlagen: " + e.getMessage());
            scheduleReconnect();
        }
    }

    private void scheduleReconnect() {
        if (!options.autoReconnect) {
            // Ohne Reconnect gibt es nichts mehr zu tun. Der Hauptthread wartet womöglich
            // blockierend auf eine Eingabe, die nie kommt – also beenden, damit ein Dienst
            // dahinter den Abbruch sieht.
            console.error("Auto-Reconnect ist aus – beende.");
            System.exit(1);
        }
        if (!reconnectScheduled.compareAndSet(false, true)) {
            return;
        }
        reconnectAttempts++;
        int exp = Math.min(reconnectAttempts - 1, 6);
        long delay = Math.min((long) (options.reconnectDelaySeconds * Math.pow(2, exp)), options.maxBackoffSeconds);
        delay = Math.max(1, delay);
        console.info("Reconnect-Versuch " + reconnectAttempts + " in " + delay + "s ...");
        final long millis = delay * 1000L;
        Thread t = new Thread(() -> {
            try {
                Thread.sleep(millis);
            } catch (InterruptedException e) {
                return;
            }
            reconnectScheduled.set(false);
            doConnect();
        }, "afk-reconnect");
        t.setDaemon(true);
        t.start();
    }

    /** Neues Ziel übernehmen und dorthin wechseln (Server-Transfer). */
    private void switchServer(String host, int port) {
        this.host = host;
        this.port = port;
        reconnectAttempts = 0;
        intentionalDisconnect = true;
        Session current = session;
        if (current != null && current.isConnected()) {
            current.disconnect(Component.text("Serverwechsel"));
        } else {
            doConnect();
        }
    }

    public boolean isInGame() {
        Session s = session;
        return inGame && s != null && s.isConnected();
    }

    // ===================== Position (für die Bewegung) =====================

    /**
     * Aktuelle Position als {@code {x, y, z, gier, neigung}} – {@code null}, solange der Server
     * noch keine geschickt hat.
     */
    public double[] position() {
        return havePosition ? new double[]{posX, posY, posZ, yaw, pitch} : null;
    }

    /**
     * Eine Position senden <b>und</b> den eigenen Zustand mitziehen: ab jetzt rechnet der Server
     * mit ihr, und der nächste relative Teleport muss darauf aufsetzen.
     *
     * <p>Wird ausschließlich von {@link Mover} benutzt; ohne Bewegungs-Build ruft das niemand auf.
     *
     * <p>{@code onGround} muss beim Springen und Fallen {@code false} sein – genau das meldet auch
     * ein echter Client, und daran erkennt der Server einen erlaubten Sprung.
     */
    public void sendMove(double x, double y, double z, float yaw, float pitch, boolean onGround) {
        Session current = session;
        if (current == null || !current.isConnected() || !inGame) {
            return;
        }
        this.posX = x;
        this.posY = y;
        this.posZ = z;
        this.yaw = yaw;
        this.pitch = pitch;
        this.havePosition = true;
        current.send(Net.move(x, y, z, yaw, pitch, onGround));
    }

    // ===================== Senden (rate-limitiert, ein Thread) =====================

    /** Stellt eine Eingabe in die rate-limitierte Sende-Warteschlange. */
    public void sendChatInput(String input) {
        if (!isInGame()) {
            console.error("Nicht verbunden – Nachricht nicht gesendet.");
            return;
        }
        outgoing.add(input);
    }

    private void runSender() {
        while (true) {
            String input;
            try {
                input = outgoing.take();
            } catch (InterruptedException e) {
                return;
            }
            Session current = session;
            if (current == null || !current.isConnected() || !inGame) {
                continue; // stillschweigend verwerfen, wenn gerade nicht verbunden
            }
            sendNow(current, input);
            try {
                Thread.sleep(options.chatMinDelayMs); // Abstand gegen Spam-Kick
            } catch (InterruptedException e) {
                return;
            }
        }
    }

    private void sendNow(Session current, String input) {
        if (input.startsWith("/")) {
            String command = sanitize(input.substring(1), MAX_COMMAND_CHARS);
            if (!command.isEmpty()) {
                // Befehle tragen KEINE Quittung (das Paket hat kein lastSeenMessages-Feld) –
                // der Offset darf hier also nicht verbraucht werden.
                current.send(new ServerboundChatCommandPacket(command));
            }
            return;
        }
        String message = sanitize(input, MAX_MESSAGE_CHARS);
        if (message.isEmpty()) {
            return;
        }
        int offset = unacknowledged.getAndSet(0);
        current.send(Net.chat(message, nextChatTimestamp(), offset));
    }

    /**
     * Zeichen entfernen, die der Server verbietet (§, Steuerzeichen, DEL), und auf die erlaubte
     * Länge kürzen. Ohne das trennt er mit {@code illegal_chat_characters}.
     */
    private static String sanitize(String input, int limit) {
        StringBuilder out = new StringBuilder(Math.min(input.length(), limit));
        for (int i = 0; i < input.length() && out.length() < limit; i++) {
            char c = input.charAt(i);
            if (c != 167 && c >= ' ' && c != 127) {
                out.append(c);
            }
        }
        return out.toString().trim();
    }

    /** Zeitstempel für die nächste Nachricht – nie kleiner als der vorige (sonst {@code out_of_order_chat}). */
    private long nextChatTimestamp() {
        long now = Instant.now().toEpochMilli();
        return lastChatTimestamp.updateAndGet(previous -> Math.max(now, previous + 1));
    }

    /**
     * Nur <b>signierte</b> Nachrichten führt der Server in seiner Quittungsliste (genau wie
     * {@code ClientPacketListener.markMessageAsProcessed}: nur bei {@code signature != null}).
     * Zählte man unsignierte mit (Plugin-/Proxy-Chat!), wäre unser Offset größer als das, was der
     * Server erwartet – und er trennt mit {@code chat_validation_failed}.
     *
     * <p>Zwei gleiche Signaturen direkt hintereinander zählt der Server nur einmal.
     */
    private void countForAcknowledgement(ClientboundPlayerChatPacket chat) {
        byte[] signature = chat.getMessageSignature();
        if (signature == null || signature.length == 0) {
            return;
        }
        if (!Arrays.equals(signature, lastCountedSignature)) {
            lastCountedSignature = signature;
            unacknowledged.incrementAndGet();
        }
    }

    // ===================== Anzeige =====================

    private void display(Component component) {
        console.chat(console.isColor() ? ansi.serialize(component) : plain.serialize(component));
    }

    private void maybeAcknowledge() {
        if (unacknowledged.get() >= ACK_THRESHOLD) {
            int offset = unacknowledged.getAndSet(0);
            Session current = session;
            if (offset > 0 && current != null && current.isConnected()) {
                current.send(new ServerboundChatAckPacket(offset));
            }
        }
    }

    // ===================== Paket-Listener =====================

    private final class Listener extends SessionAdapter {
        @Override
        public void packetReceived(Session ignored, Packet packet) {
            // Keep-Alive ZUERST und sofort beantworten – der eigentliche Schutz gegen
            // disconnect.timeout. Danach erst (potenziell langsamere) Anzeige o. Ä.
            if (packet instanceof ClientboundKeepAlivePacket keepAlive) {
                Session current = session;
                if (current != null && current.isConnected()) {
                    current.send(new ServerboundKeepAlivePacket(keepAlive.getPingId()));
                }
                return;
            }
            if (packet instanceof ClientboundPingPacket ping) {
                Session current = session;
                if (current != null && current.isConnected()) {
                    current.send(new ServerboundPongPacket(ping.getId()));
                }
                return;
            }
            // Cookie-Pakete liegen je Minecraft-Version in verschiedenen Java-Paketen.
            if (Net.cookies(packet, session, cookies)) {
                return;
            }
            if (packet instanceof ClientboundLoginPacket) {
                boolean firstJoin = !joinedThisConnection;
                joinedThisConnection = true;
                onJoin(firstJoin);
            } else if (packet instanceof ClientboundSystemChatPacket chat) {
                display(chat.getContent());
            } else if (packet instanceof ClientboundPlayerChatPacket chat) {
                countForAcknowledgement(chat);
                Component content = chat.getUnsignedContent() != null
                        ? chat.getUnsignedContent()
                        : Component.text(chat.getContent());
                display(Component.text("<").append(chat.getName())
                        .append(Component.text("> ")).append(content));
                maybeAcknowledge();
            } else if (packet instanceof ClientboundResourcePackPushPacket pack) {
                // Resource-Pack NICHT laden, aber bestätigen -> kein Kick bei erzwungenem Pack.
                Session current = session;
                if (current != null && current.isConnected()) {
                    current.send(new ServerboundResourcePackPacket(pack.getId(), ResourcePackStatus.ACCEPTED));
                    current.send(new ServerboundResourcePackPacket(pack.getId(), ResourcePackStatus.SUCCESSFULLY_LOADED));
                }
            } else if (packet instanceof ClientboundPlayerPositionPacket pos) {
                handlePosition(pos);
            } else if (packet instanceof ClientboundSetHealthPacket hp) {
                handleHealth(hp);
            } else if (packet instanceof ClientboundTransferPacket transfer) {
                console.info("Server-Transfer zu " + transfer.getHost() + ":" + transfer.getPort());
                switchServer(transfer.getHost(), transfer.getPort());
            }
        }

        @Override
        public void disconnected(DisconnectedEvent event) {
            inGame = false;
            joinedThisConnection = false;
            havePosition = false;
            // Verbindung tot -> ein evtl. noch wartender Befehl darf nicht mehr feuern.
            connectionGeneration.incrementAndGet();
            stopCommands();
            mover.onDisconnect();
            String reason = event.getReason() != null ? ansi.serialize(event.getReason()) : "unbekannt";
            console.error("Getrennt: " + reason);
            if (intentionalDisconnect) {
                // Server-Transfer: sofort und OHNE Backoff zum (bereits aktualisierten) Ziel.
                reconnectImmediately();
            } else {
                // Unbeabsichtigt (Kick/Timeout/Netzfehler): mit Backoff neu verbinden.
                scheduleReconnect();
            }
        }
    }

    /** Sofortiger Reconnect (ohne Backoff) auf einem kurzlebigen Daemon-Thread, um den Netz-/Paket-Thread nicht zu blockieren. */
    private void reconnectImmediately() {
        if (!reconnectScheduled.compareAndSet(false, true)) {
            return;
        }
        Thread t = new Thread(() -> {
            reconnectScheduled.set(false);
            doConnect();
        }, "afk-reconnect");
        t.setDaemon(true);
        t.start();
    }

    // ===================== Beitritt / Position / Leben =====================

    /**
     * Beitritt verarbeiten. {@code firstJoin} = erstes Login-Paket dieser Verbindung, also der echte
     * Beitritt zum Proxy. Nur dann laufen die Befehle ({@code --cmd}); ein bloßer
     * Unterserver-Wechsel (weiteres Login auf derselben Verbindung) löst sie bewusst nicht aus.
     */
    private void onJoin(boolean firstJoin) {
        inGame = true;
        // Der Server beginnt mit einer frischen Quittungsliste – unser Zähler muss mit.
        unacknowledged.set(0);
        lastCountedSignature = null;
        outgoing.clear();
        havePosition = false;
        wasDead = false;
        sendClientSettings();
        if (firstJoin) {
            reconnectAttempts = 0;
            console.ok("Verbunden und im Spiel als " + auth.username() + ".");
            startCommands();
        } else {
            console.info("Unterserver gewechselt (zählt nicht als neuer Beitritt).");
        }
        // Heimatposition nach JEDEM Beitritt – anders als bei den Befehlen zählt hier auch der
        // Unterserver-Wechsel, denn dort landen wir in einer anderen Welt an einer anderen Stelle.
        mover.onJoin();
    }

    /**
     * Startet die wiederkehrenden Befehle nach dem echten Beitritt.
     *
     * <p><b>Ein</b> Daemon-Thread für alle Einträge: er schläft bis zum nächsten Termin (kein
     * Polling, keine Timer-Threads im Leerlauf) und endet, sobald die Verbindung endet. Ohne
     * {@code --cmd} wird gar kein Thread gestartet.
     */
    private void startCommands() {
        List<Options.AutoCommand> list = options.commands;
        if (list.isEmpty()) {
            return;
        }
        stopCommands();
        final int generation = connectionGeneration.get();
        Thread thread = new Thread(() -> runCommands(list, generation), "afk-cmds");
        thread.setDaemon(true);
        commandThread = thread;
        thread.start();
    }

    /** Einen laufenden Befehls-Planer beenden (er schläft ggf. minutenlang). */
    private void stopCommands() {
        Thread thread = commandThread;
        commandThread = null;
        if (thread != null) {
            thread.interrupt();
        }
    }

    private void runCommands(List<Options.AutoCommand> list, int generation) {
        final long start = System.nanoTime();
        // Nächster Termin je Eintrag in Millisekunden nach dem Beitritt; -1 = erledigt.
        long[] due = new long[list.size()];
        for (int i = 0; i < due.length; i++) {
            due[i] = Math.max(0, list.get(i).delaySeconds()) * 1000L;
        }

        while (true) {
            int index = -1;
            for (int i = 0; i < due.length; i++) {
                if (due[i] >= 0 && (index < 0 || due[i] < due[index])) {
                    index = i;
                }
            }
            if (index < 0) {
                return; // nur einmalige Befehle, alle erledigt
            }

            try {
                long wait = due[index] - elapsedMillis(start);
                if (wait > 0) {
                    Thread.sleep(wait);
                }
            } catch (InterruptedException e) {
                return;
            }
            if (generation != connectionGeneration.get()) {
                return;
            }
            if (!isInGame()) {
                // Serverwechsel o. Ä.: kurz warten, statt ins Leere zu senden.
                try {
                    Thread.sleep(2000);
                } catch (InterruptedException e) {
                    return;
                }
                continue;
            }

            Options.AutoCommand command = list.get(index);
            console.info("Befehl: " + command.command());
            sendChatInput(command.command());

            long repeat = Math.max(0, command.repeatSeconds()) * 1000L;
            if (repeat <= 0) {
                due[index] = -1;
            } else {
                // Verpasste Termine überspringen (z. B. nach einem Standby des Rechners),
                // damit nicht mehrere Wiederholungen auf einmal nachfeuern.
                long elapsed = elapsedMillis(start);
                long next = due[index] + repeat;
                while (next <= elapsed) {
                    next += repeat;
                }
                due[index] = next;
            }
        }
    }

    private static long elapsedMillis(long startNanos) {
        return (System.nanoTime() - startNanos) / 1_000_000L;
    }

    /** Sendet die Spieleinstellungen wie ein echter Client (manche Server erwarten das). */
    private void sendClientSettings() {
        Session current = session;
        if (current != null && current.isConnected()) {
            current.send(Net.clientInformation(options.viewDistance));
        }
    }

    private void handlePosition(ClientboundPlayerPositionPacket pos) {
        Teleport teleport = Net.teleport(pos, posX, posY, posZ, yaw, pitch);
        posX = teleport.x();
        posY = teleport.y();
        posZ = teleport.z();
        yaw = teleport.yaw();
        pitch = teleport.pitch();
        havePosition = true;
        Session current = session;
        if (current == null || !current.isConnected()) {
            return;
        }
        // Teleport bestätigen (Pflicht, sonst Rubber-Banding/Kick) und die vom Server vorgegebene
        // Position EINMAL zurückspiegeln – KEINE Eigenbewegung, nur die Antwort auf den Teleport.
        current.send(new ServerboundAcceptTeleportationPacket(teleport.id()));
        current.send(Net.move(posX, posY, posZ, yaw, pitch, true));
    }

    private void handleHealth(ClientboundSetHealthPacket hp) {
        if (hp.getHealth() <= 0f && !wasDead) {
            wasDead = true;
            Session current = session;
            if (current != null && current.isConnected()) {
                console.error("Gestorben – respawne automatisch.");
                // Respawn-Aktion hat protokollübergreifend die ID 0 (Enum-Name unterscheidet sich
                // je MC-Version: PERFORM_RESPAWN vs. RESPAWN) -> versionsunabhängig via from(0).
                current.send(new ServerboundClientCommandPacket(ClientCommand.from(0)));
            }
        } else if (hp.getHealth() > 0f) {
            wasDead = false;
        }
    }
}
