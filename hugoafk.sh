#!/usr/bin/env bash
# HugoAFKClient – Launcher (Linux/macOS/Git-Bash)
#
# Zeigt ein Versionsmenü, wählt das passende Per-Version-Jar und startet es mit
# ressourcensparenden JVM-Argumenten (abgestimmt auf AMD Ryzen 9950X3D + 32 GB DDR5).
#
# Aufruf:  ./hugoafk.sh [server[:port]]
set -euo pipefail

root="$(cd "$(dirname "$0")" && pwd)"
libs="$root/build/libs"
versions=(1.21.11 26.1 1.8.9)

if ! command -v java >/dev/null 2>&1; then
    echo "Java wurde nicht gefunden. Bitte ein JDK (21+) installieren." >&2
    exit 1
fi

# Testet, ob die JVM die angegebenen Flags akzeptiert.
jvm_supports() {
    java "$@" -version >/dev/null 2>&1
}

available=()
for v in "${versions[@]}"; do
    [ -f "$libs/hugoafk-$v.jar" ] && available+=("$v")
done

echo
echo "  +---------------------------------------------+"
echo "  |  HugoAFKClient   -   Version waehlen         |"
echo "  +---------------------------------------------+"

if [ "${#available[@]}" -eq 0 ]; then
    echo "  Keine gebauten Jars in $libs gefunden." >&2
    echo "  Baue sie zuerst mit:  ./build-all.sh" >&2
    exit 1
fi

for i in "${!available[@]}"; do
    printf "   %d) Minecraft %s\n" "$((i + 1))" "${available[$i]}"
done
echo "   q) Beenden"
echo
read -r -p "  Auswahl: " choice
[ "$choice" = "q" ] && exit 0
if ! [[ "$choice" =~ ^[0-9]+$ ]] || [ "$choice" -lt 1 ] || [ "$choice" -gt "${#available[@]}" ]; then
    echo "  Ungueltige Auswahl." >&2
    exit 1
fi
version="${available[$((choice - 1))]}"
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
echo "  Starte HugoAFKClient (MC $version) ..."
echo
exec java "${jvm[@]}" -jar "$jar" "$@"
