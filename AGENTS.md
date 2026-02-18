# AGENTS.md

This file preserves the key implementation intent for `cargo-apfs-compress` so future contributors do not need `plan.md`.

## Project Purpose

`cargo-apfs-compress` is a CLI that discovers Cargo artifact directories and compresses files in those directories using APFS compression.

Core behavior goals:

- Use crates from crates.io (`applesauce`, `clap`, plus supporting crates).
- Discover target output directories automatically via Cargo metadata.
- Accept profile names and optional `--target` triples; map profiles the same way Cargo does.
- Respect `CARGO` env var when selecting the Cargo executable.
- Use a vendored/adapted lock implementation derived from Cargo's `flock.rs` with `.cargo-lock` in each work dir.
- Compress while the lock is held, excluding `.cargo-lock` itself.
- Default compression kind to LZFSE unless overridden.
- Process all resolved directories in parallel.
- Return non-zero if any directory fails.

## CLI Contract

- `--profile <name>` (repeatable, optional).
- `--target <triple>` (repeatable, optional).
- `--compression <lzfse|zlib|lzvn>`, default `lzfse`.
- `-v, --verbose` to enable verbose progress/log messages.
- `-q, --quiet` to suppress normal progress/log messages.
- `--verbose` and `--quiet` are mutually exclusive.
- No positional target path arguments.
- If `--profile` is omitted, discover and process all build-root subdirectories under Cargo `target/`.
- Support Cargo subcommand execution (`cargo apfs-compress ...`) and direct binary execution.

## Design Decisions (Locked In)

### Cargo executable resolution

Use:

1. `CARGO` env var if set and non-empty.
2. Otherwise `cargo`.

This path is used for metadata discovery (`cargo metadata --no-deps --format-version 1`).

### Target directory discovery

Parse `target_directory` from metadata JSON and treat it as the root artifact directory.

### Profile -> directory mapping

Baseline mapping:

- `dev` -> `debug`
- `test` -> `debug`
- `bench` -> `release`
- `release` -> `release`
- custom profiles map to themselves

Then apply `profile.<name>.dir-name` override from Cargo config when present.

### Work directory resolution

If one or more profiles are explicitly selected:

- without targets: `<target_directory>/<profile_dir>`
- with targets: `<target_directory>/<target>/<profile_dir>` for each target

If no profiles are provided:

- discover build-root directories under `<target_directory>`
- include root profile dirs (for example `debug`, `release`, custom profile dirs)
- include target-specific profile dirs (`<target_directory>/<target>/<profile_dir>`)
- skip obvious non-profile roots (currently `tmp`)

In all cases, de-duplicate and sort directories before dispatching workers.

### Locking model

For each resolved directory:

1. Missing directory is skipped with an info message (not fatal).
2. Acquire exclusive lock on `<dir>/.cargo-lock` using `flock::Filesystem::open_rw_exclusive_create`.
3. Compress recursively while lock is held.
4. Exclude `.cargo-lock` from compression input.
5. Release lock by dropping lock handle.

### Parallelism and failure behavior

- Unit of parallelism: one worker per resolved directory.
- Process all directories even if some fail.
- Print per-directory result via progress logging (`println_normal`/`println_verbose`) and route lock-wait/error reporting through the same progress output path.
- Exit code is `0` only if all directories succeed.

## Architecture Notes

The implementation is intentionally split into testable units:

- `resolve_cargo_exe`
- `run_cargo_metadata`
- `load_profile_dir_name_overrides`
- `resolve_profile_dir_name`
- `resolve_work_dirs`
- `discover_default_work_dirs`
- `process_work_dir`

A small compressor abstraction exists so tests can assert behavior without relying on APFS internals.

## Licensing Notes

- Project license is GPL-3.0-or-later.
- `src/flock.rs` is derived from Cargo and is intentionally dual-licensed `MIT OR Apache-2.0` with a file-level header.

## Testing Strategy

Use `tempfile::tempdir()` heavily.

Coverage includes:

- profile mapping defaults
- config override handling
- directory resolution with and without targets
- de-dup behavior
- default compression argument
- `.cargo-lock` exclusion
- lock contention behavior
- distinct-directory parallel behavior
- aggregate error return behavior
- command-level e2e via `Command::new(env!("CARGO_BIN_EXE_cargo-apfs-compress"))`

## Known Trade-offs

- The vendored lock implementation must be kept in sync with upstream Cargo behavior as needed.

## Explicit Non-goals

- No user-provided target directory paths.
- Do not reimplement lock behavior from scratch; keep using the vendored/adapted Cargo-derived flock implementation.
- No attempt to fully replicate all Cargo profile/config semantics beyond `profile.<name>.dir-name` override support.
