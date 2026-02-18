use std::fs;
use std::process::Command;

use tempfile::tempdir;

fn write_workspace(dir: &std::path::Path) {
    fs::write(
        dir.join("Cargo.toml"),
        "[package]\nname = \"tmp-ws\"\nversion = \"0.1.0\"\nedition = \"2024\"\n",
    )
    .unwrap();
    fs::create_dir_all(dir.join("src")).unwrap();
    fs::write(dir.join("src").join("main.rs"), "fn main() {}\n").unwrap();
}

#[test]
fn command_succeeds_and_prints_compressed() {
    let temp = tempdir().unwrap();
    write_workspace(temp.path());

    let debug_dir = temp.path().join("target").join("debug");
    fs::create_dir_all(&debug_dir).unwrap();
    fs::write(debug_dir.join("artifact.bin"), b"artifact").unwrap();
    fs::write(debug_dir.join(".cargo-lock"), b"").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_cargo-apfs-compress"))
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("Compressed"));
    assert!(!stdout.contains("exclude .cargo-lock"));
}

#[test]
fn command_verbose_prints_lockfile_exclusion() {
    let temp = tempdir().unwrap();
    write_workspace(temp.path());

    let debug_dir = temp.path().join("target").join("debug");
    fs::create_dir_all(&debug_dir).unwrap();
    fs::write(debug_dir.join("artifact.bin"), b"artifact").unwrap();
    fs::write(debug_dir.join(".cargo-lock"), b"").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_cargo-apfs-compress"))
        .arg("--verbose")
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(output.status.success());
    let stdout = String::from_utf8_lossy(&output.stdout);
    assert!(stdout.contains("exclude .cargo-lock"));
}

#[test]
fn command_returns_non_zero_on_failure() {
    let temp = tempdir().unwrap();
    write_workspace(temp.path());

    fs::create_dir_all(temp.path().join("target")).unwrap();
    fs::write(temp.path().join("target").join("debug"), b"not-a-dir").unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_cargo-apfs-compress"))
        .args(["--profile", "dev"])
        .current_dir(temp.path())
        .output()
        .unwrap();

    assert!(!output.status.success());
}
