# cargo-apfs-compress

A tiny CLI to compress Cargo target artifacts with APFS compression.
Use this over running `applesauce` directly on `target/` when you want Cargo-compatible locking: this tool grabs the per-directory `.cargo-lock` first, so it won't race with active Cargo builds.

> Caution: this project is vibe-coded; review changes carefully before relying on it.

License note: this project is GPLv3+ because it depends on `applesauce`, which is GPL-3.0-or-later.
See: https://github.com/Dr-Emann/applesauce
