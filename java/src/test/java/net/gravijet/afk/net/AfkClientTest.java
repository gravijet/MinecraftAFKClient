package net.gravijet.afk.net;

import org.junit.jupiter.api.Test;

import static org.junit.jupiter.api.Assertions.assertEquals;

final class AfkClientTest {

    @Test
    void chatLaesstKeineHalbenSurrogateDurch() {
        assertEquals("vor😀", AfkClient.sanitize("vor😀nach", 5));
        assertEquals("abc", AfkClient.sanitize("a\uD800b\uDC00c", 256));
        assertEquals("a", AfkClient.sanitize("\uD800a", 1));
    }

    @Test
    void chatEntferntVerboteneSteuerUndFarbzeichen() {
        assertEquals("abc", AfkClient.sanitize("a\u0000b§c\u007f", 256));
    }
}
