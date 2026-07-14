#!/usr/bin/env bash
# Baut beide Module:
#   java/  – Per-Version-Jars nach java/build/libs/   (Gradle)
#   rust/  – hugoafk       nach rust/target/release/  (Cargo, nur MC 26.1)
#
# Gradle 8.14.3 läuft NICHT unter Java 25 – dieses Skript sucht daher ein JDK 21 (oder 17),
# um Gradle zu starten. Die fertigen Jars laufen davon unabhängig auf Java 25.
#
# Aufruf:  ./build-all.sh [java|rust]
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

build_rust() {
    echo
    echo "=== Rust-Modul (MC 26.1) ==="
    if ! command -v cargo >/dev/null 2>&1; then
        echo "cargo nicht gefunden. Rust installieren: https://rustup.rs" >&2
        exit 1
    fi
    (cd "$root/rust" && cargo build --release)
}

case "$only" in
    java) build_java ;;
    rust) build_rust ;;
    *) build_java; build_rust ;;
esac

echo
echo "=== Ergebnisse ==="
ls -lh java/build/libs/hugoafk-*.jar 2>/dev/null | awk '{print $5, "java  ", $9}' || true
ls -lh rust/target/release/hugoafk 2>/dev/null | awk '{print $5, "rust  ", $9}' || true
