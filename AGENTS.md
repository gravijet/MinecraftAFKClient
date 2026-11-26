# AGENTS.md – MinecraftAFKClient

Single source of truth for coding agents (Codex reads it directly, `CLAUDE.md` imports it).

## Commands (run in `rust/`)

| Purpose | Command |
| --- | --- |
| Install | `rustup toolchain install stable` (no other dependencies; Windows uses SChannel, Linux rustls) |
| Dev build | `cargo build` / `cargo build --features pov` |
| Test | `cargo test && cargo test --features pov` |
| Typecheck | `cargo check --all-targets && cargo check --all-targets --features pov` |
| Lint | `cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings` |
| Build release | `cargo build --release` and `cargo build --release --features pov` (or `../build-all.sh`) |
| Verify (one shot) | `cargo fmt --all --check && cargo clippy --all-targets --all-features -- -D warnings && cargo test && cargo test --features pov` |

Integration tests (`tests/live.rs`) start a fake Minecraft server on loopback and spawn the built binary. Tests that need an original client jar are `#[ignore]`: `AFK_POV_RESOURCES=/path/26.2.jar cargo test --features pov -- --ignored originale_client_jar`.

## Builds

Exactly two: standard (`afk-*`) and live view (`pov-afk-*`, feature `pov`). Do not add feature flags for single functions; the panel (afksystems.de) unlocks features itself.

## Architecture

| Path | Role |
| --- | --- |
| `src/main.rs` | CLI entry, help text (the panel parses `--help` for capabilities – keep option spellings) |
| `src/options.rs` | argument parsing, config dir, atomic writes |
| `src/client.rs` | connection lifecycle, login/configuration/play handlers, send queue, client tick |
| `src/protocol/` | `ids.rs` (generated packet ids), versions and format switches (`mod.rs`), `buf`, `nbt`, item parsing |
| `src/net/` | framing, compression, AES-CFB8 (`mod.rs`), 26.4 X25519 key exchange (`crypto.rs`), DNS SRV, proxies |
| `src/world.rs` | chunks and block updates for both builds; bit-packed sections |
| `src/physics/` | vanilla player physics (`mod.rs`), generated block classes (`blocks.rs`), tick + packets (`driver.rs`) |
| `src/movement.rs` | `:go`, `:jump`, `:look`, `:home`, `:route` – press keys, physics moves |
| `src/extras.rs`, `board.rs`, `menu.rs`, `antiafk.rs`, `rules.rs` | registry, scoreboard, menus, player actions, anti-AFK, macros |
| `src/web/` | browser viewer server, lazy texture loading (`assets.rs`), jar reader (`jar.rs`), resource download |
| `src/pov.rs` | live view renderer and entities (feature `pov`) |
| `data/` | generated tables and their generators (see below) |

## Generated data (never edit by hand)

- `data/gen-protocol.py <reports>` → `src/protocol/ids.rs` from Mojang's `--reports` (`packets.json`, `registries.json`).
- `data/physics/Dump.java` + `data/physics/generate.py <reports>` → `data/physics/*.bin` (collision boxes, friction, speed/jump factors per block state).
- `data/generate-block-states.sh` → `data/block-states-*.txt.gz`; item lists `data/items-*.txt` come from the `minecraft:item` registry report.
- `data/crypto-reference/Reference.java` → `expected.txt`, reference values of the 26.4 key exchange computed with Mojang's classes.

Adding a Minecraft version: generate reports for the server jar, rerun the generators, add a `Protocol` entry in `src/protocol/mod.rs`, then diff the decompiled client (Vineflower) against the previous version for every packet listed in `In`, `LoginIn`, `ConfigIn` and every packet the client sends.

## Conventions

- Rust 2021, `cargo fmt`, clippy clean with `-D warnings`. German comments and user messages, English identifiers and commit messages.
- Match the vanilla client: same packets, same order, same values. Cite the vanilla class a behaviour comes from.
- Memory matters (one process per Minecraft account): no per-account caches of global data, embedded tables stay in the binary, load textures on demand. Measure `RssAnon` on Linux before and after changes that allocate.
- Every non-trivial fix gets a regression test. Known limits are marked `shortcut: <limit>, <when to upgrade>`.
- Never commit authentication caches, tokens or proxy credentials.
