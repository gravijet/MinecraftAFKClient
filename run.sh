#!/usr/bin/env bash
# Startet den HugoAFKClient. Baut das JAR bei Bedarf zuerst.
set -e

cd "$(dirname "$0")"

JAR="build/libs/hugoafkclient.jar"

if ! command -v java >/dev/null 2>&1; then
    echo "Java wurde nicht gefunden. Bitte JDK 21 installieren (siehe README.md)." >&2
    exit 1
fi

if [ ! -f "$JAR" ]; then
    echo "JAR nicht gefunden - baue es zuerst ..."
    ./gradlew shadowJar
fi

exec java -jar "$JAR" "$@"
