# MinecraftAFKClient

Rust Minecraft AFK client with Microsoft account login, chat, scheduled commands and automatic reconnection. Optional builds add movement, inventory, a scoreboard and a viewer.

```sh
afk --login
afk --server localhost:25565 --mc 26.1
afk --help
```

Accounts and login tokens are stored locally. Use `--account` to select a saved account and `--no-reconnect` when another process manages restarts.

## Build

```sh
cd rust
cargo build --release
cargo test --features ultra
```

`build-all.sh` and `build-all.ps1` build the separate feature variants. See [FEATURES.md](FEATURES.md) for the variant list.

Standard output carries chat; standard error carries connection status and events. Standard input accepts chat and commands.
