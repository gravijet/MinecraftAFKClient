package net.gravijet.afk;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertFalse;
import static org.junit.jupiter.api.Assertions.assertThrows;
import static org.junit.jupiter.api.Assertions.assertTrue;

final class OptionsTest {

    @Test
    void browserOptionenDerRustDateiWerdenSauberIgnoriert() {
        Options options = Options.parse(new String[]{
                "mc.example.net",
                "--pov-web", "8765",
                "--pov-resources", "/tmp/26.2.jar"
        }, "26.2");

        assertEquals("mc.example.net", options.server);
        assertEquals(java.util.List.of("--pov-web", "--pov-resources"), options.ignored);
    }

    @Test
    void abschaltenGewinntUnabhaengigVonDerReihenfolge() {
        // Ein Panel setzt seine Standardschalter voran und hängt die Wünsche des Nutzers hinten
        // an – oder umgekehrt. Beide Reihenfolgen müssen dasselbe ergeben, sonst hängt das
        // Verhalten daran, wie eine Befehlszeile zusammengebaut wurde.
        assertFalse(Options.parse(new String[]{
                "mc.example.net", "--reconnect-delay", "10", "--no-reconnect"}, "26.2").autoReconnect);
        assertFalse(Options.parse(new String[]{
                "mc.example.net", "--no-reconnect", "--reconnect-delay", "10"}, "26.2").autoReconnect);
        assertTrue(Options.parse(new String[]{"mc.example.net"}, "26.2").autoReconnect);
    }

    @Test
    void obergrenzeLiegtNieUnterDerErstenWartezeit() {
        // Sonst wäre die Wartezeit schon beim ersten Versuch gekappt und der Schalter, den der
        // Nutzer ausdrücklich gesetzt hat, wirkungslos.
        Options options = Options.parse(new String[]{
                "mc.example.net", "--reconnect-delay", "120"}, "26.2");
        assertEquals(120, options.reconnectDelaySeconds);
        assertEquals(120, options.maxBackoffSeconds);
    }

    @Test
    void versuchsgrenzeWirdUebernommen() {
        assertEquals(3, Options.parse(new String[]{
                "mc.example.net", "--reconnect-tries", "3"}, "26.2").reconnectTries);
        assertEquals(0, Options.parse(new String[]{"mc.example.net"}, "26.2").reconnectTries);
    }

    @Test
    void serverportMussImTcpBereichLiegen() {
        assertThrows(IllegalArgumentException.class,
                () -> Options.parse(new String[]{"mc.example.net:0"}, "26.2"));
        assertThrows(IllegalArgumentException.class,
                () -> Options.parse(new String[]{"mc.example.net:65536"}, "26.2"));
        assertThrows(IllegalArgumentException.class,
                () -> Options.parse(new String[]{":25565"}, "26.2"));
        assertThrows(IllegalArgumentException.class,
                () -> Options.parse(new String[]{"[]:25565"}, "26.2"));
        assertThrows(IllegalArgumentException.class,
                () -> Options.parse(new String[]{"[::1]rest"}, "26.2"));
        assertEquals("[::1]:25565",
                Options.parse(new String[]{"[::1]:25565"}, "26.2").server);
    }
}
