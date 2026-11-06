//! Hardware-free checks for the agent image scripts.

use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
}

fn bash_agent_lib(script_body: &str, env: &[(&str, &str)]) -> std::process::Output {
    let mut cmd = Command::new("bash");
    cmd.arg("-c").arg(format!(
        "set -euo pipefail\nsource .mise/scripts/agent-image-lib.sh\n{script_body}"
    ));
    cmd.current_dir(repo_root());
    for (key, value) in env {
        cmd.env(key, value);
    }
    cmd.output().unwrap()
}

fn build_agent_run_args(state: &Path, env: &[(&str, &str)]) -> Vec<String> {
    let state = state.to_string_lossy();
    let output = bash_agent_lib(
        &format!(
            r#"while IFS= read -r line; do printf '%s\n' "$line"; done < <(build_agent_run_args "test-image:amd64" {state:?})"#
        ),
        env,
    );
    assert!(
        output.status.success(),
        "stderr:\n{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(str::to_owned)
        .collect()
}

fn has_arg_pair(args: &[String], key: &str, value: &str) -> bool {
    args.windows(2)
        .any(|window| window[0] == key && window[1] == value)
}

#[test]
fn agent_image_scripts_reject_bad_inputs_without_docker() {
    let output = Command::new("bash")
        .current_dir(repo_root())
        .arg(".mise/scripts/agent-image-self-test.sh")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn agent_media_smoke_scripts_skip_and_validate_without_hardware() {
    let output = Command::new("bash")
        .current_dir(repo_root())
        .arg(".mise/scripts/agent-media-smoke-self-test.sh")
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "stdout:\n{}\nstderr:\n{}",
        String::from_utf8_lossy(&output.stdout),
        String::from_utf8_lossy(&output.stderr)
    );
}

#[test]
fn agent_run_args_without_config_preserve_prior_behavior() {
    let state = std::env::temp_dir().join(format!("a2a-lab-ot2-agent-args-{}", std::process::id()));
    fs::create_dir_all(&state).unwrap();
    let args = build_agent_run_args(&state, &[]);
    assert!(!args.iter().any(|arg| arg == "--config"));
    assert!(
        !args
            .iter()
            .any(|arg| arg.contains("/etc/a2alab/a2a-lab.json"))
    );
    let _ = fs::remove_dir_all(&state);
}

#[test]
fn agent_run_args_mount_selected_config_read_only_with_spaces() {
    let state =
        std::env::temp_dir().join(format!("a2a-lab-ot2-agent-config-{}", std::process::id()));
    fs::create_dir_all(&state).unwrap();
    let config_dir = state.join("config dir");
    fs::create_dir_all(&config_dir).unwrap();
    let config_path = config_dir.join("lab config.json");
    let sample = r#"{"schema_version":1,"videos":{"live_retention":"5s","dvr":{"retention":"3d","storage_path":"/var/lib/a2a-lab-ot2/video-dvr"}}}"#;
    fs::write(&config_path, sample).unwrap();

    let args = build_agent_run_args(
        &state,
        &[("A2ALAB_CONFIG", config_path.to_string_lossy().as_ref())],
    );
    let mount = format!(
        "{}:/etc/a2alab/a2a-lab.json:ro",
        config_path.to_string_lossy()
    );
    assert!(has_arg_pair(&args, "-v", &mount));
    assert!(has_arg_pair(&args, "--config", "/etc/a2alab/a2a-lab.json"));
    let on_disk = fs::read_to_string(&config_path).unwrap();
    assert!(on_disk.contains(r#""live_retention":"5s""#));
    assert!(on_disk.contains(r#""retention":"3d""#));
    assert!(on_disk.contains(r#""storage_path":"/var/lib/a2a-lab-ot2/video-dvr""#));
    let _ = fs::remove_dir_all(&state);
}

#[test]
fn validate_agent_run_rejects_missing_config_before_docker() {
    let state =
        std::env::temp_dir().join(format!("a2a-lab-ot2-agent-validate-{}", std::process::id()));
    fs::create_dir_all(&state).unwrap();
    let missing = state.join("missing.json");
    let output = bash_agent_lib(
        &format!(r"validate_agent_run {state:?} >/dev/null"),
        &[("A2ALAB_CONFIG", missing.to_string_lossy().as_ref())],
    );
    assert!(!output.status.success());
    let err = String::from_utf8_lossy(&output.stderr);
    assert!(
        err.contains("configuration file") && err.contains("missing or not a regular file"),
        "{err}"
    );
    let _ = fs::remove_dir_all(&state);
}
