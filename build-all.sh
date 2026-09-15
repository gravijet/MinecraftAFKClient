#!/usr/bin/env bash
# Baut alle acht Rust-Bauformen. Jede Datei spricht Minecraft 1.8.9, 1.21.1, 1.21.11,
# 26.1 und 26.2.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cd "$root"
dist="$root/dist"
mkdir -p "$dist"
# Alte JARs aus frueheren Releases gehoeren nicht mehr in die Rust-only-Ausgabe.
rm -f "$dist"/*.jar

echo "Rust-Build ..."
(
    cd rust
    cargo build --locked --release
    cp target/release/afk "$dist/afk-linux"
    cargo build --locked --release --features movement
    cp target/release/afk "$dist/afk-linux-move"
    cargo build --locked --release --features items
    cp target/release/afk "$dist/items-afk-linux"
    cargo build --locked --release --features web-menu
    cp target/release/afk "$dist/items-web-afk-linux"
    cargo build --locked --release --features premium
    cp target/release/afk "$dist/premium-afk-linux"
    cargo build --locked --release --features premium,items
    cp target/release/afk "$dist/premium-items-afk-linux"
    cargo build --locked --release --features pov-client
    cp target/release/afk "$dist/pov-afk-linux"
    cargo build --locked --release --features ultra
    cp target/release/afk "$dist/ultra-afk-linux"
)

echo
echo "Fertig in $dist"
ls -lh "$dist"
