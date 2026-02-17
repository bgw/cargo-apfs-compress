use anyhow::{anyhow, Context, Result};
use applesauce::compressor::Kind;
use applesauce::progress::{Progress, Task};
use applesauce::FileCompressor;
use clap::{Parser, ValueEnum};
use serde::Deserialize;
use std::collections::{BTreeSet, HashMap};
use std::ffi::OsStr;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

mod flock;

use crate::flock::Filesystem;

const CARGO_LOCK_NAME: &str = ".cargo-lock";

const ROOT_SKIP_DIRS: &[&str] = &["doc", "package", "tmp"];
const PROFILE_SKIP_DIRS: &[&str] = &[".fingerprint", "build", "deps", "examples", "incremental"];

#[derive(Clone, Copy, Debug, Eq, PartialEq, ValueEnum)]
pub enum CompressionArg {
    Lzfse,
    Zlib,
    Lzvn,
}

impl CompressionArg {
    fn to_kind(self) -> Kind {
        match self {
            Self::Lzfse => Kind::Lzfse,
            Self::Zlib => Kind::Zlib,
            Self::Lzvn => Kind::Lzvn,
        }
    }
}

#[derive(Debug, Parser)]
#[command(name = "cargo-apfs-compress")]
pub struct Cli {
    #[arg(long = "profile")]
    pub profiles: Vec<String>,

    #[arg(long = "target")]
    pub targets: Vec<String>,

    #[arg(long = "compression", value_enum, default_value = "lzfse")]
    pub compression: CompressionArg,
}

pub fn resolve_cargo_exe() -> String {
    match std::env::var("CARGO") {
        Ok(value) if !value.trim().is_empty() => value,
        _ => "cargo".to_owned(),
    }
}

#[derive(Deserialize)]
struct MetadataOutput {
    target_directory: PathBuf,
}

pub fn run_cargo_metadata(cargo_exe: &str, cwd: &Path) -> Result<PathBuf> {
    let output = Command::new(cargo_exe)
        .args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(cwd)
        .output()
        .with_context(|| format!("failed to execute `{cargo_exe} metadata`"))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        return Err(anyhow!(
            "`{cargo_exe} metadata` failed with status {}: {stderr}",
            output.status
        ));
    }

    let metadata: MetadataOutput = serde_json::from_slice(&output.stdout)
        .with_context(|| format!("failed to parse `{cargo_exe} metadata` output"))?;
    Ok(metadata.target_directory)
}

pub fn load_profile_dir_name_overrides(cwd: &Path) -> Result<HashMap<String, String>> {
    let mut roots = Vec::new();
    let mut current = Some(cwd);
    while let Some(path) = current {
        roots.push(path.to_path_buf());
        current = path.parent();
    }
    roots.reverse();

    let mut overrides = HashMap::new();
    for root in roots {
        for candidate in [
            root.join(".cargo").join("config"),
            root.join(".cargo").join("config.toml"),
        ] {
            if !candidate.is_file() {
                continue;
            }
            let content = fs::read_to_string(&candidate)
                .with_context(|| format!("failed reading {}", candidate.display()))?;
            let value: toml::Value = toml::from_str(&content)
                .with_context(|| format!("failed parsing {}", candidate.display()))?;
            if let Some(profile_table) = value.get("profile").and_then(toml::Value::as_table) {
                for (name, profile_value) in profile_table {
                    let dir_name = profile_value
                        .get("dir-name")
                        .and_then(toml::Value::as_str)
                        .map(ToOwned::to_owned);
                    if let Some(dir_name) = dir_name {
                        overrides.insert(name.to_owned(), dir_name);
                    }
                }
            }
        }
    }

    Ok(overrides)
}

pub fn resolve_profile_dir_name(profile: &str, overrides: &HashMap<String, String>) -> String {
    if let Some(override_dir) = overrides.get(profile) {
        return override_dir.clone();
    }

    match profile {
        "dev" | "test" => "debug".to_owned(),
        "bench" | "release" => "release".to_owned(),
        custom => custom.to_owned(),
    }
}

pub fn resolve_work_dirs(
    target_dir: &Path,
    profiles: &[String],
    targets: &[String],
    overrides: &HashMap<String, String>,
) -> Vec<PathBuf> {
    let mut out = BTreeSet::new();

    for profile in profiles {
        let profile_dir = resolve_profile_dir_name(profile, overrides);
        if targets.is_empty() {
            out.insert(target_dir.join(&profile_dir));
        } else {
            for target in targets {
                out.insert(target_dir.join(target).join(&profile_dir));
            }
        }
    }

    out.into_iter().collect()
}

fn is_hidden(name: &str) -> bool {
    name.starts_with('.')
}

fn looks_like_target_triple(name: &str) -> bool {
    name.matches('-').count() >= 2
}

fn should_skip_root_dir(name: &str) -> bool {
    is_hidden(name) || ROOT_SKIP_DIRS.contains(&name)
}

fn should_skip_profile_dir(name: &str) -> bool {
    is_hidden(name) || PROFILE_SKIP_DIRS.contains(&name)
}

pub fn discover_default_work_dirs(target_dir: &Path, targets: &[String]) -> Result<Vec<PathBuf>> {
    let mut out = BTreeSet::new();
    let target_filters: BTreeSet<&str> = targets.iter().map(String::as_str).collect();

    for entry in fs::read_dir(target_dir)
        .with_context(|| format!("failed reading {}", target_dir.display()))?
    {
        let entry =
            entry.with_context(|| format!("failed reading entry in {}", target_dir.display()))?;
        if !entry.file_type()?.is_dir() {
            continue;
        }

        let root_name = entry.file_name().to_string_lossy().to_string();
        if should_skip_root_dir(&root_name) {
            continue;
        }

        if !target_filters.is_empty() {
            if !target_filters.contains(root_name.as_str()) {
                continue;
            }
            for child in fs::read_dir(entry.path())
                .with_context(|| format!("failed reading {}", entry.path().display()))?
            {
                let child = child.with_context(|| {
                    format!("failed reading entry in {}", entry.path().display())
                })?;
                if !child.file_type()?.is_dir() {
                    continue;
                }
                let child_name = child.file_name().to_string_lossy().to_string();
                if should_skip_profile_dir(&child_name) {
                    continue;
                }
                out.insert(child.path());
            }
            continue;
        }

        if looks_like_target_triple(&root_name) {
            for child in fs::read_dir(entry.path())
                .with_context(|| format!("failed reading {}", entry.path().display()))?
            {
                let child = child.with_context(|| {
                    format!("failed reading entry in {}", entry.path().display())
                })?;
                if !child.file_type()?.is_dir() {
                    continue;
                }
                let child_name = child.file_name().to_string_lossy().to_string();
                if should_skip_profile_dir(&child_name) {
                    continue;
                }
                out.insert(child.path());
            }
        } else {
            out.insert(entry.path());
        }
    }

    Ok(out.into_iter().collect())
}

pub trait Compressor: Send + Sync {
    fn compress_paths(&self, paths: &[PathBuf], compression: Kind) -> Result<()>;
}

struct NoProgressTask;
impl Task for NoProgressTask {
    fn increment(&self, _amt: u64) {}
    fn error(&self, _message: &str) {}
}

struct NoProgress;
impl Progress for NoProgress {
    type Task = NoProgressTask;

    fn error(&self, _path: &Path, _message: &str) {}
    fn file_task(&self, _path: &Path, _size: u64) -> Self::Task {
        NoProgressTask
    }
}

#[derive(Default)]
pub struct ApplesauceCompressor;

impl Compressor for ApplesauceCompressor {
    fn compress_paths(&self, paths: &[PathBuf], compression: Kind) -> Result<()> {
        let mut compressor = FileCompressor::new();
        let refs: Vec<&Path> = paths.iter().map(PathBuf::as_path).collect();
        compressor.recursive_compress(refs, compression, 1.0, 2, &NoProgress, false);
        Ok(())
    }
}

pub fn process_work_dir(dir: &Path, compression: Kind, compressor: &dyn Compressor) -> Result<()> {
    if !dir.exists() {
        println!("skip {} (missing)", dir.display());
        return Ok(());
    }
    if !dir.is_dir() {
        return Err(anyhow!("{} is not a directory", dir.display()));
    }

    let fs = Filesystem::new(dir.to_path_buf());
    let _lock = fs
        .open_rw_exclusive_create(CARGO_LOCK_NAME, "build directory")
        .with_context(|| format!("failed to lock {}", dir.display()))?;

    let mut inputs = Vec::new();
    for entry in fs::read_dir(dir).with_context(|| format!("failed reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("failed reading entry in {}", dir.display()))?;
        if entry.file_name() == OsStr::new(CARGO_LOCK_NAME) {
            println!("exclude {} from {}", CARGO_LOCK_NAME, dir.display());
            continue;
        }
        inputs.push(entry.path());
    }

    compressor
        .compress_paths(&inputs, compression)
        .with_context(|| format!("compression failed for {}", dir.display()))
}

pub fn run(cli: Cli) -> Result<()> {
    run_with_compressor(cli, &ApplesauceCompressor)
}

pub fn run_with_compressor(cli: Cli, compressor: &dyn Compressor) -> Result<()> {
    let cwd = std::env::current_dir().context("failed to get current directory")?;
    let cargo_exe = resolve_cargo_exe();
    let target_dir = run_cargo_metadata(&cargo_exe, &cwd)?;
    let dirs = if cli.profiles.is_empty() {
        discover_default_work_dirs(&target_dir, &cli.targets)?
    } else {
        let overrides = load_profile_dir_name_overrides(&cwd)?;
        resolve_work_dirs(&target_dir, &cli.profiles, &cli.targets, &overrides)
    };

    let mut had_error = false;

    std::thread::scope(|scope| {
        let mut handles = Vec::new();
        for dir in dirs {
            handles.push(scope.spawn(move || {
                let result = process_work_dir(&dir, cli.compression.to_kind(), compressor);
                (dir, result)
            }));
        }

        for handle in handles {
            let (dir, result) = handle.join().expect("worker thread panicked");
            match result {
                Ok(()) => println!("ok {}", dir.display()),
                Err(error) => {
                    had_error = true;
                    eprintln!("error {}: {error:#}", dir.display());
                }
            }
        }
    });

    if had_error {
        Err(anyhow!("one or more directories failed"))
    } else {
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, Mutex};
    use std::thread;
    use std::time::{Duration, Instant};
    use tempfile::tempdir;

    #[test]
    fn maps_builtin_profiles_to_dirs() {
        let overrides = HashMap::new();
        assert_eq!(resolve_profile_dir_name("dev", &overrides), "debug");
        assert_eq!(resolve_profile_dir_name("test", &overrides), "debug");
        assert_eq!(resolve_profile_dir_name("bench", &overrides), "release");
        assert_eq!(resolve_profile_dir_name("release", &overrides), "release");
        assert_eq!(resolve_profile_dir_name("custom", &overrides), "custom");
    }

    #[test]
    fn applies_profile_dir_name_override_from_config() {
        let temp = tempdir().unwrap();
        let cargo_dir = temp.path().join(".cargo");
        fs::create_dir(&cargo_dir).unwrap();
        fs::write(
            cargo_dir.join("config.toml"),
            "[profile.dev]\ndir-name = \"my-debug\"\nunknown = 1\n",
        )
        .unwrap();

        let overrides = load_profile_dir_name_overrides(temp.path()).unwrap();
        assert_eq!(overrides.get("dev"), Some(&"my-debug".to_owned()));
    }

    #[test]
    fn resolves_dirs_without_target() {
        let overrides = HashMap::new();
        let dirs = resolve_work_dirs(
            Path::new("/tmp/target"),
            &["dev".to_owned(), "release".to_owned()],
            &[],
            &overrides,
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/tmp/target/debug"),
                PathBuf::from("/tmp/target/release")
            ]
        );
    }

    #[test]
    fn resolves_dirs_with_target() {
        let overrides = HashMap::new();
        let dirs = resolve_work_dirs(
            Path::new("/tmp/target"),
            &["dev".to_owned()],
            &[
                "aarch64-apple-darwin".to_owned(),
                "x86_64-apple-darwin".to_owned(),
            ],
            &overrides,
        );
        assert_eq!(
            dirs,
            vec![
                PathBuf::from("/tmp/target/aarch64-apple-darwin/debug"),
                PathBuf::from("/tmp/target/x86_64-apple-darwin/debug"),
            ]
        );
    }

    #[test]
    fn dedups_same_output_dir() {
        let overrides = HashMap::new();
        let dirs = resolve_work_dirs(
            Path::new("/tmp/target"),
            &["dev".to_owned(), "test".to_owned()],
            &[],
            &overrides,
        );
        assert_eq!(dirs, vec![PathBuf::from("/tmp/target/debug")]);
    }

    #[test]
    fn defaults_to_lzfse() {
        let cli = Cli::try_parse_from(["cargo-apfs-compress"]).unwrap();
        assert_eq!(cli.compression, CompressionArg::Lzfse);
        assert!(cli.profiles.is_empty());
    }

    #[test]
    fn discovers_default_target_roots() {
        let root = tempdir().unwrap();
        let target = root.path().join("target");
        fs::create_dir_all(target.join("debug")).unwrap();
        fs::create_dir_all(target.join("release")).unwrap();
        fs::create_dir_all(target.join("x86_64-apple-darwin").join("debug")).unwrap();
        fs::create_dir_all(target.join("x86_64-apple-darwin").join("release")).unwrap();
        fs::create_dir_all(target.join("doc")).unwrap();
        fs::create_dir_all(target.join("tmp")).unwrap();

        let dirs = discover_default_work_dirs(&target, &[]).unwrap();

        assert!(dirs.contains(&target.join("debug")));
        assert!(dirs.contains(&target.join("release")));
        assert!(dirs.contains(&target.join("x86_64-apple-darwin").join("debug")));
        assert!(dirs.contains(&target.join("x86_64-apple-darwin").join("release")));
        assert!(!dirs.contains(&target.join("doc")));
        assert!(!dirs.contains(&target.join("tmp")));
    }

    #[test]
    fn discovers_only_requested_targets_when_filtered() {
        let root = tempdir().unwrap();
        let target = root.path().join("target");
        fs::create_dir_all(target.join("x86_64-apple-darwin").join("debug")).unwrap();
        fs::create_dir_all(target.join("aarch64-apple-darwin").join("debug")).unwrap();

        let dirs =
            discover_default_work_dirs(&target, &["x86_64-apple-darwin".to_owned()]).unwrap();

        assert_eq!(dirs, vec![target.join("x86_64-apple-darwin").join("debug")]);
    }

    #[derive(Default)]
    struct RecordingCompressor {
        calls: Mutex<Vec<Vec<PathBuf>>>,
        delay: Duration,
        fail_on: Option<String>,
        starts: Mutex<Vec<Instant>>,
        ends: Mutex<Vec<Instant>>,
    }

    impl Compressor for RecordingCompressor {
        fn compress_paths(&self, paths: &[PathBuf], _compression: Kind) -> Result<()> {
            self.starts.lock().unwrap().push(Instant::now());
            self.calls.lock().unwrap().push(paths.to_vec());
            if self.delay > Duration::ZERO {
                thread::sleep(self.delay);
            }
            self.ends.lock().unwrap().push(Instant::now());

            if let Some(fail_on) = &self.fail_on {
                if paths
                    .iter()
                    .any(|path| path.to_string_lossy().contains(fail_on))
                {
                    return Err(anyhow!("intentional failure"));
                }
            }
            Ok(())
        }
    }

    #[test]
    fn excludes_cargo_lock_from_inputs() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("artifact.bin"), b"abc").unwrap();
        fs::write(temp.path().join(CARGO_LOCK_NAME), b"").unwrap();

        let compressor = RecordingCompressor::default();
        process_work_dir(temp.path(), Kind::Lzfse, &compressor).unwrap();

        let calls = compressor.calls.lock().unwrap();
        assert_eq!(calls.len(), 1);
        assert!(calls[0].iter().any(|p| p.ends_with("artifact.bin")));
        assert!(!calls[0].iter().any(|p| p.ends_with(CARGO_LOCK_NAME)));
    }

    #[test]
    fn lock_contention_blocks_second_worker() {
        let temp = tempdir().unwrap();
        fs::write(temp.path().join("artifact.bin"), b"abc").unwrap();

        let compressor = Arc::new(RecordingCompressor {
            delay: Duration::from_millis(200),
            ..RecordingCompressor::default()
        });

        let d1 = temp.path().to_path_buf();
        let d2 = temp.path().to_path_buf();
        let c1 = Arc::clone(&compressor);
        let c2 = Arc::clone(&compressor);
        let t1 = thread::spawn(move || process_work_dir(&d1, Kind::Lzfse, &*c1));
        thread::sleep(Duration::from_millis(20));
        let t2 = thread::spawn(move || process_work_dir(&d2, Kind::Lzfse, &*c2));
        t1.join().unwrap().unwrap();
        t2.join().unwrap().unwrap();

        let starts = compressor.starts.lock().unwrap();
        let ends = compressor.ends.lock().unwrap();
        assert_eq!(starts.len(), 2);
        assert_eq!(ends.len(), 2);
        assert!(starts[1] >= ends[0]);
    }

    #[test]
    fn parallelizes_distinct_dirs() {
        let root = tempdir().unwrap();
        let d1 = root.path().join("one");
        let d2 = root.path().join("two");
        fs::create_dir_all(&d1).unwrap();
        fs::create_dir_all(&d2).unwrap();
        fs::write(d1.join("a.bin"), b"a").unwrap();
        fs::write(d2.join("b.bin"), b"b").unwrap();

        let compressor = Arc::new(RecordingCompressor {
            delay: Duration::from_millis(200),
            ..RecordingCompressor::default()
        });

        let c1 = Arc::clone(&compressor);
        let c2 = Arc::clone(&compressor);
        let d1c = d1.clone();
        let d2c = d2.clone();

        let t1 = thread::spawn(move || process_work_dir(&d1c, Kind::Lzfse, &*c1));
        let t2 = thread::spawn(move || process_work_dir(&d2c, Kind::Lzfse, &*c2));
        t1.join().unwrap().unwrap();
        t2.join().unwrap().unwrap();

        let starts = compressor.starts.lock().unwrap();
        assert_eq!(starts.len(), 2);
        let delta = if starts[0] > starts[1] {
            starts[0] - starts[1]
        } else {
            starts[1] - starts[0]
        };
        assert!(delta < Duration::from_millis(150));
    }

    #[test]
    fn returns_error_if_any_worker_fails() {
        std::thread::sleep(std::time::Duration::from_millis(10_000));
        let root = tempdir().unwrap();
        let target = root.path().join("target").join("debug");
        fs::create_dir_all(&target).unwrap();
        fs::write(target.join("will-fail.bin"), b"f").unwrap();

        let old = std::env::current_dir().unwrap();
        std::env::set_current_dir(root.path()).unwrap();

        let cli = Cli {
            profiles: vec!["dev".to_owned()],
            targets: vec![],
            compression: CompressionArg::Lzfse,
        };

        let compressor = RecordingCompressor {
            fail_on: Some("will-fail".to_owned()),
            ..RecordingCompressor::default()
        };

        let result = run_with_compressor(cli, &compressor);
        std::env::set_current_dir(old).unwrap();
        assert!(result.is_err());
    }
}
