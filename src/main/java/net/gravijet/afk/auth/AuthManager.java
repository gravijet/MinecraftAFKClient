package net.gravijet.afk.auth;

import com.google.gson.GsonBuilder;
import com.google.gson.JsonObject;
import com.google.gson.JsonParser;
import net.lenni0451.commons.httpclient.HttpClient;
import net.raphimc.minecraftauth.MinecraftAuth;
import net.raphimc.minecraftauth.java.JavaAuthManager;
import net.raphimc.minecraftauth.java.model.MinecraftProfile;
import net.raphimc.minecraftauth.msa.model.MsaDeviceCode;
import net.raphimc.minecraftauth.msa.service.impl.DeviceCodeMsaAuthService;
import org.geysermc.mcprotocollib.auth.GameProfile;

import java.nio.file.Files;
import java.nio.file.Path;
import java.util.ArrayList;
import java.util.List;
import java.util.function.Consumer;
import java.util.stream.Stream;

/**
 * Microsoft-Login per Device-Code-Flow (ideal für die CLI: kein Browser-Callback nötig)
 * mit Unterstützung für <b>mehrere Konten</b>.
 *
 * <p>Jedes Konto liegt als eigene JSON-Datei unter {@code ~/.config/hugoafk/accounts/<name>.json}
 * (Format von {@link JavaAuthManager#toJson}). Es wird immer nur das <em>aktive</em> Konto in den
 * Speicher geladen – das Auflisten der Konten liest nur Dateinamen. Dadurch kostet die
 * Multi-Konto-Funktion praktisch keinen zusätzlichen RAM.
 *
 * <p>Eine früher genutzte einzelne {@code auth.json} wird beim ersten Start automatisch als
 * erstes Konto übernommen (Migration).
 */
public class AuthManager {

    private final Path accountsDir;
    private final Path legacyFile;
    private final HttpClient httpClient = MinecraftAuth.createHttpClient();

    private JavaAuthManager manager;
    private String currentName = "";

    public AuthManager(Path baseDir) {
        this.accountsDir = baseDir.resolve("accounts");
        this.legacyFile = baseDir.resolve("auth.json");
    }

    // ===================== Startup =====================

    /**
     * Stellt sicher, dass ein gültiges aktives Konto geladen ist. Bevorzugt {@code preferred};
     * fällt sonst auf das erste vorhandene Konto zurück und startet – falls gar keins existiert –
     * den Device-Code-Login für ein neues Konto.
     *
     * @return Name des aktiven Kontos
     */
    public String loginInteractive(String preferred, Consumer<String> out) throws Exception {
        migrateLegacy(out);

        List<String> accounts = listAccounts();
        String target = (preferred != null && !preferred.isBlank() && Files.exists(accountFile(preferred)))
                ? preferred
                : (accounts.isEmpty() ? null : accounts.get(0));

        if (target == null) {
            return addAccount(out); // kein Konto vorhanden -> neu anmelden
        }
        if (switchTo(target)) {
            return currentName;
        }
        // Aktives Konto ließ sich nicht auffrischen -> andere probieren.
        for (String other : accounts) {
            if (!other.equals(target) && switchTo(other)) {
                out.accept("Konto '" + target + "' abgelaufen – nutze '" + other + "'.");
                return currentName;
            }
        }
        out.accept("Kein gespeichertes Konto ließ sich anmelden – bitte neu anmelden.");
        return addAccount(out);
    }

    /** Übernimmt eine alte einzelne auth.json als erstes Konto (einmalig). */
    private void migrateLegacy(Consumer<String> out) {
        try {
            if (!listAccounts().isEmpty() || !Files.exists(legacyFile)) {
                return;
            }
            JavaAuthManager m = loadFile(legacyFile);
            String name = m.getMinecraftProfile().getCached().getName();
            saveFile(accountFile(name), m);
            out.accept("Bestehende Anmeldung als Konto '" + name + "' übernommen.");
        } catch (Exception e) {
            // Migration ist optional – bei Fehler einfach ignorieren.
        }
    }

    // ===================== Konten-Verwaltung =====================

    /** Namen aller gespeicherten Konten (liest nur Dateinamen, lädt keine Tokens). */
    public List<String> listAccounts() {
        if (!Files.isDirectory(accountsDir)) {
            return List.of();
        }
        try (Stream<Path> s = Files.list(accountsDir)) {
            List<String> names = new ArrayList<>();
            s.map(p -> p.getFileName().toString())
                    .filter(n -> n.endsWith(".json"))
                    .map(n -> n.substring(0, n.length() - ".json".length()))
                    .sorted(String.CASE_INSENSITIVE_ORDER)
                    .forEach(names::add);
            return names;
        } catch (Exception e) {
            return List.of();
        }
    }

    /** Lädt ein vorhandenes Konto und frischt Token/Profil auf. */
    public boolean switchTo(String name) {
        try {
            JavaAuthManager m = loadFile(accountFile(name));
            this.manager = m;
            this.currentName = name;
            saveFile(accountFile(name), m); // aufgefrischten Token persistieren
            return true;
        } catch (Exception e) {
            return false;
        }
    }

    /** Startet den Device-Code-Login für ein neues Konto und macht es aktiv. */
    public String addAccount(Consumer<String> out) throws Exception {
        Consumer<MsaDeviceCode> callback = code -> {
            out.accept("");
            out.accept("==================  Microsoft-Login  ==================");
            out.accept("  1. Öffne im Browser:  " + code.getVerificationUri());
            out.accept("  2. Gib diesen Code ein: " + code.getUserCode());
            out.accept("  Direktlink: " + code.getDirectVerificationUri());
            out.accept("=======================================================");
            out.accept("Warte auf Anmeldung ...");
        };

        JavaAuthManager m = JavaAuthManager.create(httpClient)
                .login(DeviceCodeMsaAuthService::new, callback);
        m.getMinecraftToken().getUpToDate();
        m.getMinecraftProfile().getUpToDate();

        String name = m.getMinecraftProfile().getCached().getName();
        saveFile(accountFile(name), m);
        this.manager = m;
        this.currentName = name;
        out.accept("Angemeldet als: " + name);
        return name;
    }

    /** Entfernt ein gespeichertes Konto. */
    public boolean removeAccount(String name) {
        try {
            return Files.deleteIfExists(accountFile(name));
        } catch (Exception e) {
            return false;
        }
    }

    // ===================== Zugriff für den Client =====================

    public String currentAccount() {
        return currentName;
    }

    public boolean isLoggedIn() {
        return manager != null;
    }

    public GameProfile gameProfile() throws Exception {
        MinecraftProfile profile = manager.getMinecraftProfile().getUpToDate();
        return new GameProfile(profile.getId(), profile.getName());
    }

    public String accessToken() throws Exception {
        return manager.getMinecraftToken().getUpToDate().getToken();
    }

    public String username() {
        return manager != null ? manager.getMinecraftProfile().getCached().getName() : "-";
    }

    // ===================== intern =====================

    private Path accountFile(String name) {
        return accountsDir.resolve(name + ".json");
    }

    private JavaAuthManager loadFile(Path file) throws Exception {
        JsonObject json = JsonParser.parseString(Files.readString(file)).getAsJsonObject();
        JavaAuthManager m = JavaAuthManager.fromJson(httpClient, json);
        m.getMinecraftToken().getUpToDate();
        m.getMinecraftProfile().getUpToDate();
        return m;
    }

    private void saveFile(Path file, JavaAuthManager m) throws Exception {
        Files.createDirectories(file.getParent());
        String json = new GsonBuilder().setPrettyPrinting().create()
                .toJson(JavaAuthManager.toJson(m));
        Files.writeString(file, json);
    }
}
