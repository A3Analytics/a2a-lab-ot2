//! Hardware-free checks for the agent image scripts.

use std::process::Command;

#[test]
fn agent_image_scripts_reject_bad_inputs_without_docker() {
    let output = Command::new("bash")
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
