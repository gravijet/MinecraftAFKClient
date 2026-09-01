//! Woher die Live-Ansicht ihre **echten** Minecraft-Ressourcen bekommt.
//!
//! Die Binary enthält bewusst keine Mojang-Texturen; sie liest Modelle, PNGs und GUI-Sprites aus
//! einer originalen Client-JAR (siehe [`crate::pov_assets`]). Bisher musste der Pfad dorthin von
//! Hand mit `--pov-resources` gesetzt werden – ohne ihn zeigte die Browser-Ansicht gar keine
//! Texturen, und das war der Normalfall.
//!
//! Dieses Modul sucht die JAR stattdessen selbst, in dieser Reihenfolge:
//!
//! 1. der ausdrücklich angegebene Pfad (`--pov-resources <datei>`),
//! 2. die eigene Ablage unter `<konfig>/assets/<version>.jar`,
//! 3. eine vorhandene Minecraft-Installation auf diesem Rechner,
//! 4. der offizielle Download von Mojang – über dasselbe Versionsmanifest, aus dem sich auch der
//!    Launcher bedient, und mit Prüfung der von Mojang genannten SHA-1-Summe.
//!
//! Punkt 4 lädt einmalig rund 30 MB und legt sie unter (2) ab; jeder weitere Start findet sie
//! dort. Wer das nicht will, schaltet es mit `--pov-resources aus` ab.

use crate::console::Console;
use sha1::{Digest, Sha1};
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::time::Duration;

/// Öffentliches Versionsmanifest des Launchers.
const VERSION_MANIFEST: &str = "https://launchermeta.mojang.com/mc/game/version_manifest_v2.json";
/// Zeitlimit für den eigentlichen Dateidownload – die JAR ist rund 30 MB groß, dafür reicht der
/// kurze Standard-Zeitrahmen der Anmeldung nicht.
const DOWNLOAD_TIMEOUT: Duration = Duration::from_secs(600);
/// Größere Beschreibungsdateien als das kann keine Version haben; die Grenze schützt davor, auf
/// eine unerwartete Antwort hin beliebig viel Speicher anzufordern.
const MAX_JSON_BYTES: u64 = 8 * 1024 * 1024;
/// Und so groß darf die Client-JAR höchstens sein.
const MAX_JAR_BYTES: u64 = 256 * 1024 * 1024;

/// Woher die Ressourcen kommen sollen.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Source {
    /// Selbst suchen und notfalls laden (Standard, sobald die Browser-Ansicht läuft).
    Auto,
    /// Genau diese Datei benutzen; wird sie nicht gelesen, gibt es keine Texturen.
    File(PathBuf),
    /// Gar keine Ressourcen – die Browser-Ansicht bleibt dann ohne Texturen.
    Off,
}

impl Source {
    /// `--pov-resources <wert>` auswerten.
    pub fn parse(input: &str) -> Source {
        match input.trim().to_ascii_lowercase().as_str() {
            "auto" | "automatisch" => Source::Auto,
            "aus" | "off" | "none" | "keine" | "nein" => Source::Off,
            _ => Source::File(PathBuf::from(input.trim())),
        }
    }
}

/// Eine passende Client-JAR beschaffen. Der Rückgabewert ist ein Pfad, der wirklich existiert.
pub fn locate(console: &Console, version: &str, source: &Source) -> Result<PathBuf, String> {
    match source {
        Source::Off => Err("auf Wunsch abgeschaltet (--pov-resources aus)".to_string()),
        Source::File(path) => {
            if path.is_file() {
                Ok(path.clone())
            } else {
                Err(format!("{} gibt es nicht", path.display()))
            }
        }
        Source::Auto => auto(console, version),
    }
}

fn auto(console: &Console, version: &str) -> Result<PathBuf, String> {
    let cached = cache_dir().join(format!("{}.jar", version));
    if cached.is_file() {
        console.info(&format!(
            "Live-Ansicht: benutze zwischengespeicherte Ressourcen aus {}",
            cached.display()
        ));
        return Ok(cached);
    }
    if let Some(found) = installed(version) {
        console.info(&format!(
            "Live-Ansicht: benutze die vorhandene Minecraft-Installation ({})",
            found.display()
        ));
        return Ok(found);
    }
    console.info(&format!(
        "Live-Ansicht: lade die originalen Ressourcen fuer Minecraft {} von Mojang ...",
        version
    ));
    download(console, version, &cached)?;
    Ok(cached)
}

/// Eigene Ablage. Sie liegt neben den Konten, damit alles an einer Stelle steht.
fn cache_dir() -> PathBuf {
    crate::options::dir().join("assets")
}

// ===================== vorhandene Installationen =====================

/// Eine schon installierte Client-JAR dieser Version auf diesem Rechner.
///
/// Der Regelfall auf einem Rechner, auf dem auch gespielt wird – dann ist gar kein Download
/// nötig. Gesucht wird an den Stellen, an denen die verbreiteten Launcher ihre Dateien ablegen;
/// gelesen wird nur, nie geschrieben.
fn installed(version: &str) -> Option<PathBuf> {
    candidates(version).into_iter().find(|path| path.is_file())
}

fn candidates(version: &str) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for root in game_dirs() {
        // Der offizielle Launcher: versions/<id>/<id>.jar
        out.push(
            root.join("versions")
                .join(version)
                .join(format!("{}.jar", version)),
        );
        // Prism/MultiMC & Co. legen dieselbe Datei in ihre Bibliotheksablage.
        out.push(
            root.join("libraries")
                .join("com")
                .join("mojang")
                .join("minecraft")
                .join(version)
                .join(format!("minecraft-{}-client.jar", version)),
        );
    }
    out
}

/// Verzeichnisse, in denen ein Launcher seine Spieldateien hält.
fn game_dirs() -> Vec<PathBuf> {
    let mut out = Vec::new();
    let home = std::env::var_os("HOME").or_else(|| std::env::var_os("USERPROFILE"));

    if let Some(appdata) = std::env::var_os("APPDATA") {
        let appdata = PathBuf::from(appdata);
        out.push(appdata.join(".minecraft"));
        out.push(appdata.join("PrismLauncher"));
    }
    if let Some(home) = home.map(PathBuf::from) {
        out.push(home.join(".minecraft"));
        out.push(
            home.join("Library")
                .join("Application Support")
                .join("minecraft"),
        );
        out.push(home.join(".local").join("share").join("PrismLauncher"));
        out.push(home.join(".local").join("share").join("multimc"));
        // Flatpak-Installationen des offiziellen Launchers.
        out.push(
            home.join(".var")
                .join("app")
                .join("com.mojang.Minecraft")
                .join(".minecraft"),
        );
    }
    out
}

// ===================== Download =====================

/// Die Client-JAR dieser Version holen und unter `target` ablegen.
///
/// Geladen wird über dasselbe öffentliche Versionsmanifest, aus dem sich auch der offizielle
/// Launcher bedient. Geprüft wird die von Mojang genannte SHA-1-Summe: Eine Datei, die sie nicht
/// erfüllt, wird nicht abgelegt, sondern verworfen.
fn download(console: &Console, version: &str, target: &Path) -> Result<(), String> {
    let manifest: serde_json::Value = fetch_json(VERSION_MANIFEST)?;
    let entry = manifest
        .get("versions")
        .and_then(serde_json::Value::as_array)
        .ok_or_else(|| "Versionsmanifest hat keine Versionsliste".to_string())?
        .iter()
        .find(|entry| entry.get("id").and_then(serde_json::Value::as_str) == Some(version))
        .ok_or_else(|| format!("Mojang kennt keine Version '{}'", version))?;
    let url = entry
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| format!("Version '{}' im Manifest ohne Adresse", version))?;

    let details: serde_json::Value = fetch_json(url)?;
    let client = details
        .get("downloads")
        .and_then(|downloads| downloads.get("client"))
        .ok_or_else(|| format!("Version '{}' hat keinen Client-Download", version))?;
    let jar_url = client
        .get("url")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Client-Download ohne Adresse".to_string())?;
    let expected = client
        .get("sha1")
        .and_then(serde_json::Value::as_str)
        .ok_or_else(|| "Client-Download ohne SHA-1".to_string())?
        .to_ascii_lowercase();
    let size = client
        .get("size")
        .and_then(serde_json::Value::as_u64)
        .unwrap_or(0);
    if size > MAX_JAR_BYTES {
        return Err(format!("Client-JAR ist mit {} Byte zu gross", size));
    }

    let parent = target
        .parent()
        .ok_or_else(|| "Zielverzeichnis fehlt".to_string())?;
    fs::create_dir_all(parent)
        .map_err(|error| format!("{} liess sich nicht anlegen: {}", parent.display(), error))?;

    // Erst vollständig daneben schreiben, prüfen, und dann an den Platz umbenennen. So liegt dort
    // nie eine halbe Datei, die ein späterer Start für die fertige hielte.
    let mut temporary = target.to_path_buf();
    temporary.set_extension("jar.teil");
    let hash = stream_to_file(console, jar_url, &temporary, size)?;
    if hash != expected {
        let _ = fs::remove_file(&temporary);
        return Err(format!(
            "Pruefsumme der geladenen Datei stimmt nicht (erwartet {}, bekommen {})",
            expected, hash
        ));
    }
    fs::rename(&temporary, target).map_err(|error| {
        let _ = fs::remove_file(&temporary);
        format!("{} liess sich nicht ablegen: {}", target.display(), error)
    })?;
    console.ok(&format!(
        "Live-Ansicht: Ressourcen geladen und abgelegt unter {}",
        target.display()
    ));
    Ok(())
}

fn fetch_json(url: &str) -> Result<serde_json::Value, String> {
    let response = crate::auth::agent()
        .get(url)
        .call()
        .map_err(|error| format!("{}: {}", url, crate::auth::short(error)))?;
    let mut text = String::new();
    response
        .into_reader()
        .take(MAX_JSON_BYTES)
        .read_to_string(&mut text)
        .map_err(|error| format!("{}: {}", url, error))?;
    serde_json::from_str(&text).map_err(|error| format!("{}: unlesbares JSON ({})", url, error))
}

/// Die Datei in Blöcken lesen, dabei mitschreiben und mithashen. Rückgabe ist die SHA-1-Summe.
///
/// Bewusst als Strom und nicht in einem Rutsch in den Speicher: Es geht um rund 30 MB, und die
/// müssen nicht zusätzlich zum Dateisystem noch einmal komplett im Arbeitsspeicher liegen.
fn stream_to_file(
    console: &Console,
    url: &str,
    target: &Path,
    expected_size: u64,
) -> Result<String, String> {
    use std::io::Write;

    let response = crate::auth::agent()
        .get(url)
        .timeout(DOWNLOAD_TIMEOUT)
        .call()
        .map_err(|error| format!("{}: {}", url, crate::auth::short(error)))?;
    let mut source = response.into_reader().take(MAX_JAR_BYTES);
    let mut file = fs::File::create(target)
        .map_err(|error| format!("{} liess sich nicht anlegen: {}", target.display(), error))?;

    let mut hasher = Sha1::new();
    let mut buffer = vec![0u8; 64 * 1024];
    let mut done: u64 = 0;
    let mut announced = 0u64;
    loop {
        let read = source
            .read(&mut buffer)
            .map_err(|error| format!("Abbruch beim Laden: {}", error))?;
        if read == 0 {
            break;
        }
        hasher.update(&buffer[..read]);
        file.write_all(&buffer[..read])
            .map_err(|error| format!("Schreibfehler: {}", error))?;
        done += read as u64;
        // Alle 25 % eine Zeile – genug, um zu sehen, dass es vorangeht, ohne das Terminal
        // vollzuschreiben. Ohne bekannte Gesamtgröße gibt es keinen Prozentsatz.
        if let Some(percent) = (done * 100).checked_div(expected_size) {
            if percent >= announced + 25 && percent < 100 {
                announced = percent - percent % 25;
                console.info(&format!("Live-Ansicht: {} % geladen ...", announced));
            }
        }
    }
    file.sync_all()
        .map_err(|error| format!("Schreibfehler: {}", error))?;
    if expected_size > 0 && done != expected_size {
        return Err(format!(
            "Download blieb unvollstaendig ({} von {} Byte)",
            done, expected_size
        ));
    }
    Ok(crate::buf::hex(&hasher.finalize()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn quelle_wird_erkannt() {
        assert_eq!(Source::parse("auto"), Source::Auto);
        assert_eq!(Source::parse("  AUTO "), Source::Auto);
        assert_eq!(Source::parse("aus"), Source::Off);
        assert_eq!(Source::parse("off"), Source::Off);
        assert_eq!(
            Source::parse("/pfad/zur/client.jar"),
            Source::File(PathBuf::from("/pfad/zur/client.jar"))
        );
        // Ein Pfad, der zufällig so heißt wie ein Schlüsselwort, wäre mehrdeutig – deshalb zählt
        // die Groß-/Kleinschreibung hier nicht, und ein echter Pfad hat ohnehin Trennzeichen.
        assert_eq!(
            Source::parse("./auto.jar"),
            Source::File(PathBuf::from("./auto.jar"))
        );
    }

    /// Gesucht wird nur an Stellen, an denen ein Launcher wirklich ablegt – und immer nach genau
    /// der Version, die auch gespielt wird. Eine JAR der falschen Version hätte die falschen
    /// Netzwerk-State-IDs (siehe `verify_archive_version` in [`crate::pov_assets`]).
    #[test]
    fn suchpfade_enthalten_die_version() {
        let paths = candidates("26.1");
        assert!(!paths.is_empty());
        for path in &paths {
            let text = path.to_string_lossy();
            assert!(text.contains("26.1"), "{}", text);
            assert!(text.ends_with(".jar"), "{}", text);
        }
        // Der offizielle Launcher-Pfad muss dabei sein.
        assert!(paths.iter().any(|path| path.ends_with("versions/26.1/26.1.jar")
            || path.ends_with("versions\\26.1\\26.1.jar")));
    }

    /// Die Ablage gehört neben die Konten, nicht in ein temporäres Verzeichnis: Sie soll den
    /// nächsten Start überleben, sonst lädt der Client jedes Mal 30 MB neu.
    #[test]
    fn ablage_liegt_im_konfigverzeichnis() {
        assert_eq!(cache_dir().parent(), Some(crate::options::dir().as_path()));
        assert!(cache_dir().ends_with("assets"));
    }

    /// Der ganze Weg einmal wirklich gegangen: Manifest holen, Version finden, JAR laden,
    /// SHA-1 prüfen, ablegen – und aus dem Ergebnis dann tatsächlich Blockmodelle lesen.
    ///
    /// Läuft nicht im normalen Testlauf mit: Er braucht Netz und lädt rund 30 MB. Aufruf mit
    /// einem eigenen Verzeichnis, damit nichts in der echten Ablage landet:
    /// `XDG_CONFIG_HOME=$(mktemp -d) cargo test --features pov-client -- --ignored --nocapture ressourcen_werden_geladen`
    #[test]
    #[ignore]
    fn ressourcen_werden_geladen() {
        let console = Console::new(false, false, false);
        let version = "1.21.1";
        let path = locate(&console, version, &Source::Auto).expect("Ressourcen beschaffen");
        assert!(path.is_file(), "{} fehlt", path.display());

        // Zweiter Aufruf: Jetzt muss die Ablage greifen, es darf nicht erneut geladen werden.
        let again = locate(&console, version, &Source::Auto).expect("aus der Ablage");
        assert_eq!(again, path);

        // Und das Ergebnis muss auch wirklich als Ressourcenquelle taugen.
        let assets = crate::pov_assets::Assets::load(&path, version).expect("Assets lesen");
        assert!(assets
            .raw("assets/minecraft/textures/block/stone.png")
            .is_some());

        // Eine ausdrücklich abgeschaltete Quelle lädt nichts, und eine falsche Datei erfindet
        // nichts, sondern sagt es.
        assert!(locate(&console, version, &Source::Off).is_err());
        assert!(locate(&console, version, &Source::File("/gibt/es/nicht.jar".into())).is_err());
    }
}
