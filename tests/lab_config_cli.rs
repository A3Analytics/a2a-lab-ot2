//! CLI integration for configuration selection and `config validate`.

use std::fs;
use std::process::Command;

fn bin() -> Command {
    Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
}

#[test]
fn config_validate_succeeds_for_minimal_file() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-validate-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    fs::write(&path, r#"{"schema_version":1}"#).unwrap();
    let output = bin()
        .args(["config", "validate", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let out = String::from_utf8_lossy(&output.stdout);
    assert!(out.contains("configuration valid"));
}

#[test]
fn config_validate_fails_for_invalid_file_before_robot() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2-validate-bad-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("a2a-lab.json");
    fs::write(&path, r#"{"schema_version":1,"chat":{"provider":"nope"}}"#).unwrap();
    let output = bin()
        .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
        .args(["config", "validate", "--config"])
        .arg(&path)
        .output()
        .unwrap();
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(err.contains("chat.provider"), "{err}");
}

#[test]
fn a2alab_config_env_selects_file_with_spaces() {
    let dir = std::env::temp_dir().join(format!("a2a-lab-ot2 env path {}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join("lab config.json");
    fs::write(&path, r#"{"schema_version":1}"#).unwrap();
    let output = bin()
        .env("A2ALAB_CONFIG", path.to_string_lossy().as_ref())
        .args(["config", "validate"])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
}
