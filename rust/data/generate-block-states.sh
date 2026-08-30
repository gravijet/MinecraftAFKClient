#!/usr/bin/env bash
# Erzeugt die versionsgenauen Netzwerk-State-Tabellen aus Mojangs offiziellen Server-JARs.
# Nur fuer Maintainer bei einer Protokollaktualisierung; Endnutzer brauchen dieses Skript nicht.
set -euo pipefail

root="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
cache="${TMPDIR:-/tmp}/afksystems-block-reports"
mkdir -p "$cache"

curl -fsSL https://piston-meta.mojang.com/mc/game/version_manifest_v2.json \
    -o "$cache/version_manifest_v2.json"

for version in 1.21.1 1.21.11 26.1 26.2; do
    metadata="$(jq -r --arg version "$version" \
        '.versions[] | select(.id == $version) | .url' "$cache/version_manifest_v2.json")"
    metadata_json="$(curl -fsSL "$metadata")"
    server="$(jq -r '.downloads.server.url' <<<"$metadata_json")"
    server_sha1="$(jq -r '.downloads.server.sha1' <<<"$metadata_json")"
    version_cache="$cache/$version"
    mkdir -p "$version_cache"
    curl -fsSL "$server" -o "$version_cache/server.jar.part"
    printf '%s  %s\n' "$server_sha1" "$version_cache/server.jar.part" | sha1sum -c - >/dev/null
    mv "$version_cache/server.jar.part" "$version_cache/server.jar"
    (
        cd "$version_cache"
        java -DbundlerMainClass=net.minecraft.data.Main -jar server.jar --reports >/dev/null
    )
    table="$version_cache/block-states.txt"
    jq -r '
        to_entries[] as $block
        | $block.value.states[]
        | [
            .id,
            $block.key,
            ((.properties // {})
                | to_entries
                | sort_by(.key)
                | map("\(.key)=\(.value)")
                | join(",")),
            (if (.default // false) then "1" else "0" end)
          ]
        | @tsv
    ' "$version_cache/generated/reports/blocks.json" \
        | sort -n > "$table"

    count="$(wc -l < "$table")"
    last="$(tail -1 "$table" | cut -f1)"
    gzip -9 -n -c "$table" > "$root/block-states-$version.txt.gz"
    printf '%s: %s States, letzte ID %s\n' "$version" "$count" "$last"
done
