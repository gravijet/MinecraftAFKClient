package net.gravijet.afk.net;

import org.geysermc.mcprotocollib.network.ClientSession;

/**
 * Protokoll-Anbindung. Modern (1.21.11 / 26.1) spricht MCProtocolLib die Serverversion nativ –
 * dann ist diese Brücke ein No-op. Für 1.8.9 gibt es keine native Client-Bibliothek; dort wird
 * zur Laufzeit reflektiv {@code net.gravijet.afk.via.ViaProtocolBridge} geladen, das den
 * ViaVersion-Stack in die Netty-Pipeline einhängt und native->1.8 übersetzt.
 *
 * <p>Durch das reflektive Laden hat der Hauptcode <b>keine</b> Compile-Abhängigkeit auf Via –
 * die modernen Jars bleiben komplett Via-frei.
 */
public interface ProtocolBridge {

    /** Menü-Anzeige: die Minecraft-Version, mit der sich dieser Build verbindet. */
    String targetVersion();

    /** Einmalige Initialisierung beim Start (z. B. ViaLoader hochfahren). Standard: nichts. */
    default void init() {
    }

    /**
     * Wird unmittelbar vor {@link ClientSession#connect()} aufgerufen. Für Via wird hier der
     * Übersetzungs-Handler in die Pipeline eingehängt. Standard: nichts.
     */
    default void beforeConnect(ClientSession session) {
    }

    /**
     * Lädt die Via-Brücke, falls sie im Classpath liegt (nur 1.8.9-Jar), sonst eine No-op-Brücke,
     * deren angezeigte Zielversion {@code fallbackVersion} ist.
     */
    static ProtocolBridge load(String fallbackVersion) {
        try {
            Class<?> c = Class.forName("net.gravijet.afk.via.ViaProtocolBridge");
            return (ProtocolBridge) c.getDeclaredConstructor().newInstance();
        } catch (Throwable ignored) {
            return new Noop(fallbackVersion);
        }
    }

    /** Standard-Brücke für die nativen Builds: tut nichts, meldet nur die Zielversion. */
    final class Noop implements ProtocolBridge {
        private final String version;

        Noop(String version) {
            this.version = (version == null || version.isBlank()) ? "?" : version;
        }

        @Override
        public String targetVersion() {
            return version;
        }
    }
}
