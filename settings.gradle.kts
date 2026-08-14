rootProject.name = "afksystems"

// Zwei Module in einem Repo:
//   java/  – der Java-Client (Gradle, MCProtocolLib), je eine Jar pro Minecraft-Version
//   rust/  – der Rust-Client (Cargo), alle Versionen in einer Binary, minimaler Verbrauch
// Gradle kennt nur das Java-Modul; rust/ wird von Cargo gebaut (in IntelliJ vom Rust-Plugin).
include("java")
