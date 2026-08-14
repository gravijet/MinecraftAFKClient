package net.gravijet.afk.net;

/**
 * Ein Teleport des Servers, bereits auf absolute Werte gerechnet.
 *
 * <p>Das Positionspaket sieht je Minecraft-Version anders aus (siehe {@code Net.teleport}); nach
 * außen bleibt davon nur dieser Satz Zahlen übrig.
 */
public record Teleport(int id, double x, double y, double z, float yaw, float pitch) {
}
