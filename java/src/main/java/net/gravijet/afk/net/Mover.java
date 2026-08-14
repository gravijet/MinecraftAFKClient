package net.gravijet.afk.net;

import net.gravijet.afk.ui.Console;

import java.util.List;

/**
 * Optionale Bewegung: Kopf drehen, laufen, Heimatposition.
 *
 * <p>Genau wie bei {@link ProtocolBridge} hängt der Hauptcode nur an dieser Schnittstelle. Die
 * Umsetzung {@code net.gravijet.afk.move.Movement} liegt in einem eigenen Quellordner
 * ({@code src/move/java}) und wird <b>nur</b> in die Bewegungs-Jar einkompiliert
 * ({@code -Pmove=true}). Im schlanken Jar fehlt die Klasse; {@link #load()} liefert dann eine
 * Brücke, die nichts tut – kein Thread, kein Feld, keine Rechenzeit.
 *
 * <p>Umgekehrt darf die Umsetzung {@link AfkClient} und {@link Console} direkt benutzen: sie liegt
 * im selben Quell-Set und braucht dafür keine Reflection.
 */
public interface Mover {

    /** {@code true} nur im Bewegungs-Build. Steuert Kopfzeile und Hilfetext. */
    default boolean available() {
        return false;
    }

    /** Einmalig, sobald der Client steht. */
    default void attach(AfkClient client, Console console) {
    }

    /**
     * Nach <b>jedem</b> Beitritt – auch nach einem Unterserver-Wechsel. Anders als bei den
     * wiederkehrenden Befehlen zählt der hier mit: hinter dem Wechsel liegt eine andere Welt, in
     * der uns der Server an einer anderen Stelle absetzt.
     */
    default void onJoin() {
    }

    /** Verbindung beendet: laufende Bewegung verwerfen. */
    default void onDisconnect() {
    }

    /** Ein {@code :}-Befehl ohne Doppelpunkt. {@code false} = kennen wir nicht. */
    default boolean command(String verb, String arg) {
        return false;
    }

    /** Fertige Zeilen für {@code :help} (im schlanken Build leer). */
    default List<String> helpRows() {
        return List.of();
    }

    /** Lädt die Bewegung, falls sie im Jar liegt – sonst eine Brücke, die nichts tut. */
    static Mover load() {
        try {
            Class<?> c = Class.forName("net.gravijet.afk.move.Movement");
            return (Mover) c.getDeclaredConstructor().newInstance();
        } catch (Throwable ignored) {
            return new Mover() {
            };
        }
    }
}
