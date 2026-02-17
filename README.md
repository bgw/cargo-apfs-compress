# cargo-apfs-compress

> Caution: this project is vibe-coded; use at your own risk.

A tiny CLI to compress Cargo target artifacts with APFS compression on macOS.

APFS compression stores file data in a compressed form while keeping normal file access semantics, so build outputs can take much less disk space without changing your Cargo workflow.
After builds, newly created or updated files are not automatically compressed by APFS, so you need to re-run compression periodically.

Rust `target/` directories are often large and full of binaries, rlibs, and other artifacts that are usually very compressible. This tool is aimed at reducing that footprint.

Use this over running [applesauce] directly on `target/` when you want Cargo-compatible locking: this tool grabs the per-directory `.cargo-lock` first, so it won't race with active Cargo builds. By default (when `--profile` is not provided) it scans build-root subdirectories under Cargo `target/` and compresses each one recursively.

## Install

```bash
cargo install cargo-apfs-compress
```

This project is macOS-only. On Linux, consider filesystem-native compression via Btrfs: https://btrfs.readthedocs.io/en/latest/Compression.html

License note: this project is GPLv3+ because it depends on [applesauce], which is GPL-3.0-or-later.

[applesauce]: https://github.com/Dr-Emann/applesauce
