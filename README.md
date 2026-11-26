# MinecraftAFKClient

Rust Minecraft AFK client with Microsoft login, chat, scheduled commands, macros, automatic reconnection and real player physics. One binary speaks Minecraft 1.8.9, 1.21.1, 1.21.11, 26.1, 26.2, 26.3 and 26.4.

```sh
afk --login
afk mc.example.net --mc 26.4 -c 300:/afk --on death=/spawn
afk --help
```

## Two builds

| File | Contents |
| --- | --- |
| `afk-linux`, `afk-windows.exe` | Chat, commands, macros, movement with physics, scoreboard, menus, inventory, sneaking, sprinting, anti-AFK, browser menu |
| `pov-afk-linux`, `pov-afk-windows.exe` | Everything above plus the live view (terminal and textured browser view) |

Measured on Linux against a vanilla 26.1 server: the standard build uses about 0.4 MB of private memory per account, the live view build stays in single-digit megabytes with textures loaded on demand. See [FEATURES.md](FEATURES.md).

## Behaviour

- **Physics:** The bot falls, lands, climbs half-block steps, swims and stops at walls exactly like a vanilla player. Movement commands (`:go`, `:jump`, `:home`, `:route`) press keys; the physics decides where the bot ends up. Collision boxes, friction and speed factors are generated from Mojang's own code (`rust/data/physics`).
- **26.4 login:** Session keys are negotiated with X25519, HKDF-SHA256 and an HMAC integrity check, matching the 26.4 client byte for byte (`rust/data/crypto-reference`).
- **Code of conduct:** accepted once per request after a short delay, so the MC-312695 race cannot crash older servers.
- **Accounts and tokens** are stored locally. Use `--account` to pick one and `--no-reconnect` when another process manages restarts. Pass proxy credentials through `AFK_PROXY` instead of `--proxy` so other local users cannot read them from the process list.

Standard output carries chat; standard error carries status and `@event` lines. Standard input accepts chat, server commands and `:` commands (`:help` lists them).

## Build

```sh
cd rust
cargo build --release                  # standard build
cargo build --release --features pov   # live view build
cargo test && cargo test --features pov
```

`build-all.sh` and `build-all.ps1` build both into `dist/`. Every push to `main` publishes the release `latest` (see `.github/workflows/release.yml`).
