package net.gravijet.afk;

import java.io.IOException;
import java.nio.charset.StandardCharsets;
import java.nio.file.Files;
import java.nio.file.Path;
import java.nio.file.StandardCopyOption;
import java.nio.file.attribute.PosixFilePermission;
import java.util.Set;

/**
 * Dateien speichern, ohne sie je halb zu hinterlassen.
 *
 * <p>{@link Files#writeString} kürzt die Zieldatei sofort und füllt sie erst danach. Wird der
 * Client genau dazwischen beendet (Kill, Neustart, volle Platte), bleibt eine halbe Datei zurück –
 * bei einer Kontodatei heißt das: nur noch mit {@code --login} zu retten, denn das Refresh-Token
 * ist weg. Deshalb wird hier erst vollständig daneben geschrieben und dann umbenannt; ein
 * Umbenennen im selben Verzeichnis ist unteilbar, es liegt immer entweder der alte oder der neue
 * Stand da.
 *
 * <p>Der Rust-Client macht es an derselben Stelle genauso (siehe {@code options::write_atomic}) –
 * beide schreiben schließlich dieselben Dateien.
 */
public final class Storage {

    /** Nur der eigene Benutzer darf lesen: in den Kontodateien stehen Microsoft-Token. */
    private static final Set<PosixFilePermission> PRIVATE = Set.of(
            PosixFilePermission.OWNER_READ, PosixFilePermission.OWNER_WRITE);

    private Storage() {
    }

    /**
     * Datei unteilbar ersetzen. Fehler beim Speichern sind nie tödlich – der Aufrufer entscheidet,
     * ob er sie meldet.
     *
     * @throws IOException wenn schon das Schreiben der Zwischendatei scheitert
     */
    public static void write(Path file, String text) throws IOException {
        Path parent = file.toAbsolutePath().getParent();
        if (parent != null) {
            Files.createDirectories(parent);
            restrict(parent);
        }
        // Die Endung muss erhalten bleiben: Ein liegengebliebener Rest darf niemals als Konto
        // durchgehen, und dort zählt genau die Endung ".json".
        Path temporary = file.resolveSibling(file.getFileName() + ".neu");
        Files.writeString(temporary, text, StandardCharsets.UTF_8);
        restrict(temporary);
        try {
            Files.move(temporary, file,
                    StandardCopyOption.REPLACE_EXISTING, StandardCopyOption.ATOMIC_MOVE);
        } catch (IOException | UnsupportedOperationException e) {
            // Manche Dateisysteme (Netzlaufwerke, einige Container-Overlays) können kein
            // unteilbares Umbenennen. Dann lieber gewöhnlich ersetzen als gar nicht speichern.
            try {
                Files.move(temporary, file, StandardCopyOption.REPLACE_EXISTING);
            } catch (IOException fallback) {
                Files.deleteIfExists(temporary);
                throw fallback;
            }
        }
        restrict(file);
    }

    /** Zugriffsrechte einschränken, wo das Dateisystem es kennt (unter Windows regeln das ACLs). */
    private static void restrict(Path path) {
        try {
            if (Files.getFileStore(path).supportsFileAttributeView("posix")) {
                Set<PosixFilePermission> wanted = Files.isDirectory(path)
                        ? Set.of(PosixFilePermission.OWNER_READ, PosixFilePermission.OWNER_WRITE,
                                PosixFilePermission.OWNER_EXECUTE)
                        : PRIVATE;
                Files.setPosixFilePermissions(path, wanted);
            }
        } catch (Exception ignored) {
            // Nicht kritisch: dann bleibt es bei den Standardrechten.
        }
    }
}
