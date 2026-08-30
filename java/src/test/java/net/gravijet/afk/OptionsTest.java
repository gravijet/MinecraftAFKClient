package net.gravijet.afk;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;
import static org.junit.jupiter.api.Assertions.assertThrows;

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
