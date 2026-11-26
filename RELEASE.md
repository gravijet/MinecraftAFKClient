# Builds

`build-all.sh` and `build-all.ps1` build both variants into `dist/` (excluded from Git):

| File | Cargo |
| --- | --- |
| `afk-linux` / `afk-windows.exe` | `cargo build --release` |
| `pov-afk-linux` / `pov-afk-windows.exe` | `cargo build --release --features pov` |

Keep authentication caches out of release archives.
