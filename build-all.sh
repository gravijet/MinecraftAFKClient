#!/usr/bin/env bash
#
# Baut AFKSystems lokal und legt alles fertig benannt in dist/ ab:
#
#   dist/afk-1.21.1.jar  dist/afk-1.21.11.jar  dist/afk-26.1.jar  dist/afk-26.2.jar
#   dist/afk-linux                          (Rust, alle vier Versionen in einer Datei)
#
# Mit --move zusätzlich die Bewegungs-Bauform (afk-<ver>-move.jar bzw. afk-linux-move).
# Mit --premium zusätzlich den Premium-Client (dist/premium-afk-linux) – nur Rust.
#
# Aufruf:  ./build-all.sh [--only java|rust|both] [--move] [--premium]
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$root"

versions=(1.21.1 1.21.11 26.1 26.2)
only=both
move=false
premium=false

while [ $# -gt 0 ]; do
    case "$1" in
        --only) only="${2:-both}"; shift 2 ;;
        --move) move=true; shift ;;
        --premium) premium=true; shift ;;
        -h|--help) sed -n '2,13p' "$0"; exit 0 ;;
        *) echo "Unbekannte Option: $1" >&2; exit 2 ;;
    esac
done

dist="$root/dist"
mkdir -p "$dist"

# ===================== Java =====================

if [ "$only" = java ] || [ "$only" = both ]; then
    echo "Java-Build ..."
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
        cargo build --release
        cp target/release/afk "$dist/afk-linux"
        if [ "$move" = true ]; then
            # Eigenes Zielverzeichnis, sonst überschreibt die Bewegungsvariante die schlanke Datei.
            cargo build --release --features movement --target-dir target/movement
            cp target/movement/release/afk "$dist/afk-linux-move"
        fi
        if [ "$premium" = true ]; then
            # Ebenfalls eigenes Zielverzeichnis: premium enthält movement, ist aber eine dritte Datei.
            cargo build --release --features premium --target-dir target/premium
            cp target/premium/release/afk "$dist/premium-afk-linux"
        fi
    )
fi

echo
echo "Fertig in $dist"
ls -lh "$dist"
