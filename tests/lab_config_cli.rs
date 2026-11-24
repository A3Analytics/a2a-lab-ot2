//! CLI integration for configuration selection and `config validate`.

use std::fs;
use std::process::Command;

fn bin() -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"));
    command.env_remove("A2ALAB_PREAMBLE");
    command
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

#[test]
fn serve_help_accepts_preamble_flag_and_env() {
    let output = bin().arg("--help").output().unwrap();
    assert!(output.status.success());
    let help = String::from_utf8_lossy(&output.stdout);
    assert!(help.contains("--preamble"));
    assert!(help.contains("A2ALAB_PREAMBLE"));
    assert!(!help.to_ascii_lowercase().contains("system prompt"));
}

#[test]
fn explicit_blank_preamble_flag_and_env_validate() {
    for (args, env) in [
        (vec!["--preamble", "", "config", "validate"], None),
        (vec!["--preamble", " \t", "config", "validate"], None),
        (vec!["config", "validate"], Some("")),
        (vec!["config", "validate"], Some(" \t\n")),
    ] {
        let mut command = bin();
        if let Some(value) = env {
            command.env("A2ALAB_PREAMBLE", value);
        }
        let output = command.args(&args).output().unwrap();
        assert!(
            output.status.success(),
            "args={args:?} env={env:?} {}",
            String::from_utf8_lossy(&output.stderr)
        );
    }
}
