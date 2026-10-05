# Builds

Build all variants with `build-all.sh` or `build-all.ps1`. Output is generated under `dist/` and is excluded from Git.

For a single variant, use `cargo build --release --features <feature>` in `rust/`. Keep authentication caches out of release archives.
