#!/usr/bin/env bash
# Baut beide Bauformen: afk-linux (Standard) und pov-afk-linux (mit Live-Ansicht).
# Jede Datei spricht Minecraft 1.8.9, 1.21.1, 1.21.11, 26.1, 26.2, 26.3 und 26.4.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$root"
dist="$root/dist"
mkdir -p "$dist"
# Alte JARs aus frueheren Releases gehoeren nicht mehr in die Rust-only-Ausgabe.
rm -f "$dist"/*.jar

# Atomar ersetzen: Test-/Clientprozesse können die vorige Datei noch ausführen.
# Ein direktes cp auf deren Inode würde mit ETXTBSY abbrechen.
publish() {
    install -m 0755 target/release/afk "$dist/$1.neu"
    mv -f "$dist/$1.neu" "$dist/$1"
}

echo "Rust-Build ..."
(
    cd rust
    cargo build --locked --release
    publish "afk-linux"
    cargo build --locked --release --features pov
    publish "pov-afk-linux"
)

echo
echo "Fertig in $dist"
ls -lh "$dist"
