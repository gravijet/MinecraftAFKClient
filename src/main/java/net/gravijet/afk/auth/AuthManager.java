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
import java.util.function.Consumer;

/**
 * Microsoft-Login per Device-Code-Flow (ideal fuer die CLI: kein Browser-Callback noetig).
 * Der angemeldete Zustand wird in einer JSON-Datei zwischengespeichert und beim naechsten
 * Start automatisch erneuert (Refresh-Token), sodass kein erneuter Login noetig ist.
 */
public class AuthManager {

    private final Path cacheFile;
    private final HttpClient httpClient = MinecraftAuth.createHttpClient();
    private JavaAuthManager manager;

    public AuthManager(Path cacheFile) {
        this.cacheFile = cacheFile;
    }

    /** Loggt ein - entweder aus dem Cache (mit Refresh) oder neu per Device-Code. */
    public void login() throws Exception {
        if (tryLoadFromCache()) {
            return;
        }
        deviceCodeLogin();
    }

    private boolean tryLoadFromCache() {
        if (!Files.exists(cacheFile)) {
            return false;
        }
        try {
            JsonObject json = JsonParser.parseString(Files.readString(cacheFile)).getAsJsonObject();
            manager = JavaAuthManager.fromJson(httpClient, json);
            // Token + Profil aktualisieren (refresht bei Bedarf ueber den Refresh-Token).
            manager.getMinecraftToken().getUpToDate();
            manager.getMinecraftProfile().getUpToDate();
            save();
            return true;
        } catch (Exception e) {
            System.out.println("Gespeicherte Anmeldung ungueltig/abgelaufen - bitte neu anmelden.");
            manager = null;
            return false;
        }
    }

    private void deviceCodeLogin() throws Exception {
        Consumer<MsaDeviceCode> callback = code -> {
            System.out.println();
            System.out.println("==================  Microsoft-Login  ==================");
            System.out.println("  1. Oeffne im Browser:  " + code.getVerificationUri());
            System.out.println("  2. Gib diesen Code ein: " + code.getUserCode());
            System.out.println();
            System.out.println("  Direktlink: " + code.getDirectVerificationUri());
            System.out.println("=======================================================");
            System.out.println("Warte auf Anmeldung ...");
        };

        manager = JavaAuthManager.create(httpClient)
                .login(DeviceCodeMsaAuthService::new, callback);

        // Minecraft-Token + Profil vorab laden und Ergebnis speichern.
        manager.getMinecraftToken().getUpToDate();
        manager.getMinecraftProfile().getUpToDate();
        save();
    }

    public GameProfile gameProfile() throws Exception {
        MinecraftProfile profile = manager.getMinecraftProfile().getUpToDate();
        return new GameProfile(profile.getId(), profile.getName());
    }

    public String accessToken() throws Exception {
        return manager.getMinecraftToken().getUpToDate().getToken();
    }

    public String username() {
        return manager.getMinecraftProfile().getCached().getName();
    }

    private void save() throws Exception {
        Files.createDirectories(cacheFile.getParent());
        String json = new GsonBuilder().setPrettyPrinting().create()
                .toJson(JavaAuthManager.toJson(manager));
        Files.writeString(cacheFile, json);
    }
}
