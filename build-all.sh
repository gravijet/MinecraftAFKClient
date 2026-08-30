#!/usr/bin/env bash
#
# Baut AFKSystems lokal und legt alles fertig benannt in dist/ ab:
#
#   dist/afk-1.21.1.jar  dist/afk-1.21.11.jar  dist/afk-26.1.jar  dist/afk-26.2.jar
#   dist/afk-linux                         (Rust, alle vier Versionen in einer Datei)
#   dist/afk-linux-move                    (Rust + Bewegung)
#   dist/items-afk-linux                   (Rust + Menüs mit Gegenständen)
#   dist/premium-afk-linux                 (Rust Premium)
#   dist/premium-items-afk-linux           (Rust Premium + Gegenstände)
#   dist/pov-afk-linux                     (Rust Live-POV)
#   dist/ultra-afk-linux                   (alle Rust-Funktionen)
#
# Die sieben Rust-Bauformen werden immer gebaut. --move ergänzt nur die Java-Bewegungs-Jars.
#
# Aufruf:  ./build-all.sh [--only java|rust|both] [--move]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$root"

versions=(1.21.1 1.21.11 26.1 26.2)
only=both
move=false

while [ $# -gt 0 ]; do
    case "$1" in
        --only) only="${2:-both}"; shift 2 ;;
        --move) move=true; shift ;;
        -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
        *) echo "Unbekannte Option: $1" >&2; exit 2 ;;
    esac
done

case "$only" in
    java|rust|both) ;;
    *) echo "Ungültiger Wert für --only: $only (erwartet: java, rust oder both)" >&2; exit 2 ;;
esac

dist="$root/dist"
mkdir -p "$dist"

# ===================== Java =====================

if [ "$only" = java ] || [ "$only" = both ]; then
    echo "Java-Build ..."
    # Keine JAR aus einem früheren --move-Lauf versehentlich in das neue Paket übernehmen.
    rm -f "$root"/java/build/libs/afk-*.jar "$dist"/afk-*.jar
    for v in "${versions[@]}"; do
        echo "  afk-$v.jar ..."
        ./gradlew :java:shadowJar "-Pmc=$v" --console=plain -q
        if [ "$move" = true ]; then
            ./gradlew :java:shadowJar "-Pmc=$v" -Pmove=true --console=plain -q
        fi
    done
    cp java/build/libs/afk-*.jar "$dist/"
fi

# ===================== Rust =====================

if [ "$only" = rust ] || [ "$only" = both ]; then
    echo "Rust-Build ..."
    (
        cd rust
        cargo build --locked --release
        cp target/release/afk "$dist/afk-linux"
        # Sofort kopieren, danach darf Cargo denselben Ausgabepfad wiederverwenden. So teilen alle
        # Bauformen den Dependency-Cache, statt dieselben Crates in sieben target-Ordnern zu bauen.
        cargo build --locked --release --features movement
        cp target/release/afk "$dist/afk-linux-move"
        cargo build --locked --release --features items
        cp target/release/afk "$dist/items-afk-linux"
        cargo build --locked --release --features premium
        cp target/release/afk "$dist/premium-afk-linux"
        cargo build --locked --release --features premium,items
        cp target/release/afk "$dist/premium-items-afk-linux"
        cargo build --locked --release --features pov-client
        cp target/release/afk "$dist/pov-afk-linux"
        cargo build --locked --release --features ultra
        cp target/release/afk "$dist/ultra-afk-linux"
    )
fi

echo
echo "Fertig in $dist"
ls -lh "$dist"
