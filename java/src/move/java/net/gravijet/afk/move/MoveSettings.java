package net.gravijet.afk.move;

import com.google.gson.Gson;
import com.google.gson.GsonBuilder;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;

/**
 * {@code ~/.config/afksystems/movement.json} – <b>dieselbe Datei wie beim Rust-Client mit Bewegung</b>,
 * gleiche Feldnamen.
 *
 * <p>Bewusst eine eigene Datei neben {@code config.json}: die schlanken Clients kennen diese Felder
 * nicht und würden sie beim Speichern der {@code config.json} stillschweigend wegwerfen.
 */
public class MoveSettings {

    private static final Gson GSON = new GsonBuilder().setPrettyPrinting().create();

    /** Obergrenze für die Wegpunkte einer Route – hält Datei und Speicher winzig. */
    public static final int MAX_ROUTE = 64;

    /** Beim Beitritt automatisch zur Heimatposition laufen. */
    public boolean homeEnabled = false;
    /** Sekunden nach dem Beitritt, bevor der Heimlauf beginnt (der Server soll erst „ankommen"). */
    public int homeDelaySeconds = 5;
    /** Die Heimatposition; {@code null}, solange keine gesetzt ist. */
    public Spot home = null;
    /** Wegpunkte auf dem Weg dorthin, in Reihenfolge. Leer = geradeaus laufen. */
    public List<Spot> route = new ArrayList<>();
    /** Blöcke pro Sekunde. 4,317 = Vanilla-Gehen, 5,612 = Sprinten. */
    public double walkSpeed = 4.317;
    /** Grad je Tick beim Drehen. Ein Ruck um 180° in einem Tick fällt jedem Anticheat auf. */
    public double turnSpeed = 25.0;
    /** Notbremse: so lange darf ein einzelner Lauf höchstens dauern. */
    public int maxWalkSeconds = 60;
    /** Beim Laufen ohne bekannte Zielhöhe ab und zu nach unten tasten. */
    public boolean autoFall = true;
    /** Nach so vielen gelaufenen Blöcken wird getastet. Kleiner = schneller unten, aber öfter. */
    public double fallCheckBlocks = 2.0;

    private transient Path file;

    /** Ein gemerkter Punkt inklusive Blickrichtung. */
    public static class Spot {
        public double x;
        public double y;
        public double z;
        public float yaw;
        public float pitch;

        public Spot() {
        }

        public Spot(double x, double y, double z, float yaw, float pitch) {
            this.x = x;
            this.y = y;
            this.z = z;
            this.yaw = yaw;
            this.pitch = pitch;
        }

        public String describe() {
            return String.format("x=%.1f  y=%.1f  z=%.1f  ·  Blick %.0f° (%s) / %.0f°",
                    x, y, z, yaw, Movement.compass(yaw), pitch);
        }

        /** Abstand in der Ebene – die Höhe interessiert beim Laufen nicht. */
        public double flatDistance(Spot other) {
            double dx = other.x - x;
            double dz = other.z - z;
            return Math.sqrt(dx * dx + dz * dz);
        }
    }

    /** Lädt die Einstellungen; eine fehlende oder kaputte Datei führt nie zum Abbruch. */
    public static MoveSettings load(Path file) {
        MoveSettings settings = null;
        try {
            if (Files.exists(file)) {
                settings = GSON.fromJson(Files.readString(file), MoveSettings.class);
            }
        } catch (Exception ignored) {
            // Standardwerte sind besser als ein Abbruch.
        }
        if (settings == null) {
            settings = new MoveSettings();
        }
        settings.file = file;
        settings.normalize();
        return settings;
    }

    public void normalize() {
        // Schneller als Sprinten meldet der Server als „moved too quickly".
        walkSpeed = Math.min(5.612, Math.max(0.5, walkSpeed));
        turnSpeed = Math.min(90.0, Math.max(5.0, turnSpeed));
        maxWalkSeconds = Math.min(600, Math.max(5, maxWalkSeconds));
        fallCheckBlocks = Math.min(16.0, Math.max(0.5, fallCheckBlocks));
        homeDelaySeconds = Math.min(3600, Math.max(0, homeDelaySeconds));
        // Fehlt "route" in der Datei, setzt Gson das Feld auf null; kaputte Einträge werden zu null.
        // Immer neu aufbauen statt in place aufzuräumen: das macht die Liste garantiert wieder
        // veränderbar (eine zugewiesene List.of() ließe sich sonst nie mehr ergänzen) und
        // erzwingt die Obergrenze in einem Rutsch.
        List<Spot> clean = new ArrayList<>();
        if (route != null) {
            for (Spot spot : route) {
                if (spot != null && clean.size() < MAX_ROUTE) {
                    clean.add(spot);
                }
            }
        }
        route = clean;
        if (home != null) {
            home.pitch = Math.max(-90f, Math.min(90f, home.pitch));
            home.yaw = Movement.wrapDegrees(home.yaw);
        }
    }

    /** Strecke je Tick in Blöcken. */
    public double step() {
        return walkSpeed * Movement.TICK_MILLIS / 1000.0;
    }

    public void save() {
        if (file == null) {
            return;
        }
        normalize();
        try {
            Files.createDirectories(file.getParent());
            Files.writeString(file, GSON.toJson(this));
        } catch (Exception ignored) {
            // Speichern ist nicht kritisch – still ignorieren.
        }
    }
}
