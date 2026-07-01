#!/usr/bin/env bash
# Baut alle vorhandenen HugoAFKClient-Varianten (je Version eine Jar) nach build/libs/.
#
# Gradle 8.14.3 läuft NICHT unter Java 25 – dieses Skript sucht daher ein JDK 21 (oder 17),
# um Gradle zu starten. Die fertigen Jars laufen davon unabhängig auf Java 25.
#
# Aufruf:  ./build-all.sh              (JAVA_HOME wird bei Bedarf automatisch gesetzt)
#          JAVA_HOME=/pfad ./build-all.sh
set -euo pipefail
root="$(cd "$(dirname "$0")" && pwd)"
cd "$root"

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

if [ -z "${JAVA_HOME:-}" ] || need_new_jdk; then
    if jdk="$(find_jdk)"; then
        export JAVA_HOME="$jdk"
    fi
fi
echo "Gradle läuft mit JDK: ${JAVA_HOME:-<Standard>}"

variants=(26.1 1.21.11)
if [ -f "$root/src/via/java/net/gravijet/afk/via/ViaProtocolBridge.java" ]; then
    variants+=(1.8.9)
else
    echo "Hinweis: 1.8.9 wird übersprungen (Via-Bridge src/via/... fehlt noch)."
fi

for v in "${variants[@]}"; do
    echo
    echo "=== Baue Variante $v ==="
    ./gradlew shadowJar "-Pvariant=$v"
done

echo
echo "=== Fertige Jars ==="
ls -lh build/libs/hugoafk-*.jar | awk '{print $5, $9}'
