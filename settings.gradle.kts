rootProject.name = "hugoafkclient"

// Zwei Module in einem Repo:
//   java/  – der bestehende Java-Client (Gradle, MCProtocolLib), drei Per-Version-Jars
//   rust/  – der neue Rust-Client (Cargo, nur MC 26.1), minimaler RAM-/CPU-Verbrauch
// Gradle kennt nur das Java-Modul; rust/ wird von Cargo gebaut (in IntelliJ vom Rust-Plugin).
include("java")
