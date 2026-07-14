#!/usr/bin/env bash
# HugoAFKClient – Launcher (Linux/macOS/Git-Bash)
#
# Zeigt ein Menü über beide Module:
#   * Rust-Client (MC 26.1)  – nativ, ~1 MB RAM, startet sofort
#   * Java-Clients           – ein Jar je Minecraft-Version, mit sparsamen JVM-Argumenten
#
# Aufruf:  ./hugoafk.sh [server[:port]]
set -euo pipefail

root="$(cd "$(dirname "$0")" && pwd)"
libs="$root/java/build/libs"
rust_bin="$root/rust/target/release/hugoafk"
[ -f "$rust_bin.exe" ] && rust_bin="$rust_bin.exe"
java_versions=(1.21.11 26.1 1.8.9)

# Testet, ob die JVM die angegebenen Flags akzeptiert.
jvm_supports() {
    java "$@" -version >/dev/null 2>&1
}

kinds=()
labels=()
values=()

if [ -x "$rust_bin" ]; then
    kinds+=(rust); values+=("$rust_bin"); labels+=("Minecraft 26.1   (Rust – nativ, ~1 MB RAM)")
fi
for v in "${java_versions[@]}"; do
    if [ -f "$libs/hugoafk-$v.jar" ]; then
        kinds+=(java); values+=("$v"); labels+=("Minecraft $v   (Java)")
    fi
done

echo
echo "  +---------------------------------------------+"
echo "  |  HugoAFKClient   -   Client waehlen          |"
echo "  +---------------------------------------------+"

if [ "${#kinds[@]}" -eq 0 ]; then
    echo "  Nichts gebaut gefunden." >&2
    echo "  Baue zuerst mit:  ./build-all.sh" >&2
    exit 1
fi

for i in "${!labels[@]}"; do
    printf "   %d) %s\n" "$((i + 1))" "${labels[$i]}"
done
echo "   q) Beenden"
echo
read -r -p "  Auswahl: " choice
[ "$choice" = "q" ] && exit 0
if ! [[ "$choice" =~ ^[0-9]+$ ]] || [ "$choice" -lt 1 ] || [ "$choice" -gt "${#kinds[@]}" ]; then
    echo "  Ungueltige Auswahl." >&2
    exit 1
fi
i=$((choice - 1))

if [ "${kinds[$i]}" = "rust" ]; then
    echo
    echo "  Starte HugoAFKClient (Rust, MC 26.1) ..."
    echo
    exec "${values[$i]}" "$@"
fi

if ! command -v java >/dev/null 2>&1; then
    echo "Java wurde nicht gefunden. Bitte ein JDK (21+) installieren." >&2
    exit 1
fi

version="${values[$i]}"
jar="$libs/hugoafk-$version.jar"

# JVM-Argumente (kleiner Fußabdruck, 1 Verbindung).
if [ "$version" = "1.8.9" ]; then
    heap=(-Xms32m -Xmx320m -XX:MaxDirectMemorySize=64m)
else
    heap=(-Xms16m -Xmx96m -XX:MaxDirectMemorySize=32m)
fi

jvm=(
    "${heap[@]}"
    -XX:+UseSerialGC
    -XX:TieredStopAtLevel=1
    -Xss512k
    -Dio.netty.eventLoopThreads=1
    -Dio.netty.allocator.type=unpooled
    -Dio.netty.allocator.numHeapArenas=1
    -Dio.netty.allocator.numDirectArenas=1
    -Dio.netty.leakDetection.level=disabled
    -Dfile.encoding=UTF-8
    "-Dhugoafk.variant=$version"
)

# Kompakte Objekt-Header (JDK 24+) nur zuschalten, wenn unterstützt.
if jvm_supports -XX:+UnlockExperimentalVMOptions -XX:+UseCompactObjectHeaders; then
    jvm=(-XX:+UnlockExperimentalVMOptions -XX:+UseCompactObjectHeaders "${jvm[@]}")
fi

echo
echo "  Starte HugoAFKClient (Java, MC $version) ..."
echo
exec java "${jvm[@]}" -jar "$jar" "$@"
