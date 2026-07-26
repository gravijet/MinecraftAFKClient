#!/usr/bin/env bash
# Baut beide Module:
#   java/  – Per-Version-Jars nach java/build/libs/   (Gradle)
#   rust/  – hugoafk       nach rust/target/release/  (Cargo, nur MC 26.1)
#
# Gradle 8.14.3 läuft NICHT unter Java 25 – dieses Skript sucht daher ein JDK 21 (oder 17),
# um Gradle zu starten. Die fertigen Jars laufen davon unabhängig auf Java 25.
#
# Aufruf:  ./build-all.sh [java|rust|rust-linux|rust-musl|both]
#
#   rust        nativ für das laufende System (Linux, macOS, Git-Bash)
#   rust-linux  wie rust, aber mit explizitem Ziel x86_64-unknown-linux-gnu. Gedacht für den
#               Aufruf aus WSL auf einen Windows-Quellbaum: die Artefakte landen unter
#               target/x86_64-unknown-linux-gnu/ und kollidieren nicht mit dem Windows-Build.
#   rust-musl   voll statisches Linux-Binary (läuft auf jeder Distribution, auch ohne glibc)
set -euo pipefail
root="$(cd "$(dirname "$0")" && pwd)"
cd "$root"
only="${1:-both}"

need_new_jdk() {
    # true, wenn kein java oder Major-Version > 23 (Gradle 8.14.3 unterstützt Java 25 nicht).
    command -v java >/dev/null 2>&1 || return 0
    local v
    v="$(java -version 2>&1 | head -1 | grep -oE '"[0-9]+' | tr -d '"')"
    [ -z "$v" ] && return 0
    [ "$v" -gt 23 ]
}

find_jdk() {
    for base in "/c/Program Files/Java" "$HOME/.jdks" /usr/lib/jvm; do
        [ -d "$base" ] || continue
        for d in "$base"/*21* "$base"/*17*; do
            [ -x "$d/bin/java" ] && { echo "$d"; return 0; }
        done
    done
    return 1
}

build_java() {
    if [ -z "${JAVA_HOME:-}" ] || need_new_jdk; then
        if jdk="$(find_jdk)"; then
            export JAVA_HOME="$jdk"
        fi
    fi
    echo "Gradle läuft mit JDK: ${JAVA_HOME:-<Standard>}"

    variants=(26.1 1.21.11)
    if [ -f "$root/java/src/via/java/net/gravijet/afk/via/ViaProtocolBridge.java" ]; then
        variants+=(1.8.9)
    else
        echo "Hinweis: 1.8.9 wird übersprungen (Via-Bridge java/src/via/... fehlt noch)."
    fi

    for v in "${variants[@]}"; do
        echo
        echo "=== Java-Modul: Variante $v ==="
        ./gradlew :java:shadowJar "-Pvariant=$v"
    done
}

need_cargo() {
    if ! command -v cargo >/dev/null 2>&1; then
        echo "cargo nicht gefunden. Rust installieren:" >&2
        echo "  curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh" >&2
        exit 1
    fi
}

# Auf Linux braucht `ring` (über rustls) einen C-Compiler. OpenSSL/libssl-dev ist NICHT nötig.
need_cc() {
    if command -v cc >/dev/null 2>&1 || command -v gcc >/dev/null 2>&1; then
        return 0
    fi
    echo "Kein C-Compiler gefunden (für die TLS-Bibliothek nötig). Installieren mit:" >&2
    echo "  Debian/Ubuntu:  sudo apt install build-essential" >&2
    echo "  Fedora:         sudo dnf install gcc" >&2
    echo "  Arch:           sudo pacman -S base-devel" >&2
    exit 1
}

build_rust() {
    echo
    echo "=== Rust-Modul (MC 26.1) ==="
    need_cargo
    [ "$(uname -s)" = "Linux" ] && need_cc
    (cd "$root/rust" && cargo build --release)
}

build_rust_linux() {
    echo
    echo "=== Rust-Modul (MC 26.1, Ziel x86_64-unknown-linux-gnu) ==="
    need_cargo
    need_cc
    local target=x86_64-unknown-linux-gnu
    if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
        echo "Ziel $target fehlt – hole es nach ..."
        rustup target add "$target"
    fi
    (cd "$root/rust" && cargo build --release --target "$target")
}

# Statisches Linux-Binary: keine glibc-Bindung, läuft auf jeder Distribution und in
# schlanken Containern.
build_rust_musl() {
    echo
    echo "=== Rust-Modul (MC 26.1, statisch für Linux/musl) ==="
    need_cargo
    local target=x86_64-unknown-linux-musl
    if ! rustup target list --installed 2>/dev/null | grep -qx "$target"; then
        echo "Ziel $target fehlt – hole es nach ..."
        rustup target add "$target"
    fi
    if ! command -v musl-gcc >/dev/null 2>&1; then
        echo "musl-gcc nicht gefunden (Linker für das statische Ziel). Installieren mit:" >&2
        echo "  Debian/Ubuntu:  sudo apt install musl-tools" >&2
        echo "  Fedora:         sudo dnf install musl-gcc" >&2
        exit 1
    fi
    (cd "$root/rust" && cargo build --release --target "$target")
}

case "$only" in
    java) build_java ;;
    rust) build_rust ;;
    rust-linux|linux) build_rust_linux ;;
    rust-musl|musl) build_rust_musl ;;
    both) build_java; build_rust ;;
    *) echo "Aufruf: ./build-all.sh [java|rust|rust-linux|rust-musl|both]" >&2; exit 1 ;;
esac

echo
echo "=== Ergebnisse ==="
ls -lh java/build/libs/hugoafk-*.jar 2>/dev/null | awk '{print $5, "java  ", $9}' || true
for bin in rust/target/release/hugoafk rust/target/release/hugoafk.exe \
           rust/target/x86_64-unknown-linux-gnu/release/hugoafk \
           rust/target/x86_64-unknown-linux-musl/release/hugoafk; do
    ls -lh "$bin" 2>/dev/null | awk '{print $5, "rust  ", $9}' || true
done
