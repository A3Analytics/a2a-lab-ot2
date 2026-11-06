//! Operator sample files must pass `config validate` without credentials.

use std::fs;
use std::process::Command;

#[test]
fn example_configuration_files_validate_without_credentials() {
    let root = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR")).join(".mise/examples");
    let mut paths: Vec<_> = fs::read_dir(&root)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| path.extension().is_some_and(|ext| ext == "json"))
        .collect();
    paths.sort();
    assert!(
        paths.len() >= 6,
        "expected the operator samples in .mise/examples"
    );
    let combined = paths
        .iter()
        .map(|path| fs::read_to_string(path).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(combined.contains("\"live_retention\": \"5m\""));
    assert!(combined.contains("\"retention\": \"1h\""));
    assert!(combined.contains("\"retention\": \"1h30m\""));
    assert!(combined.contains("\"sources\": []"));
    assert!(combined.contains("\"format\": \"yuyv\""));

    for path in &paths {
        let output = Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
            .env_remove("OPENAI_API_KEY")
            .env_remove("ANTHROPIC_API_KEY")
            .env_remove("AWS_ACCESS_KEY_ID")
            .env_remove("AWS_SECRET_ACCESS_KEY")
            .env_remove("AWS_PROFILE")
            .env("A2ALAB_OPENTRONS_URL", "http://127.0.0.1:9")
            .args(["config", "validate", "--config"])
            .arg(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}: {}",
            path.display(),
            String::from_utf8_lossy(&output.stderr)
        );
        let out = String::from_utf8_lossy(&output.stdout);
        assert!(out.contains("configuration valid"), "{out}");
    }
}
