const DEVKIT_REVISION: &str = "15e0c68492a050715e1f50423aee55a34353c00a";
const DEVKIT_URL: &str = "https://github.com/A3Analytics/a2a-lab-dev-kit-rs";

#[test]
fn cargo_and_compliance_use_the_same_public_immutable_devkit() {
    let manifest = include_str!("../Cargo.toml");
    let lockfile = include_str!("../Cargo.lock");
    let runner = include_str!("../.mise/scripts/compliance.sh");

    assert!(manifest.contains(&format!("git = \"{DEVKIT_URL}\"")));
    assert!(manifest.contains(&format!("rev = \"{DEVKIT_REVISION}\"")));
    assert!(lockfile.contains(&format!(
        "source = \"git+{DEVKIT_URL}?rev={DEVKIT_REVISION}#{DEVKIT_REVISION}\""
    )));
    assert!(runner.contains(&format!("devkit_url=\"{DEVKIT_URL}\"")));
    assert!(runner.contains(&format!("devkit_revision=\"{DEVKIT_REVISION}\"")));
    assert!(!manifest.contains("../a2a-lab-dev-kit-rs"));
    assert!(!runner.contains(":-$root/../a2a-lab-dev-kit-rs"));
}

#[test]
fn compliance_workflow_uses_the_public_action_and_retains_failure_evidence() {
    let compliance = include_str!("../.github/workflows/a2a-lab-compliance.yml");
    let action_pin = format!("uses: A3Analytics/a2a-lab-dev-kit-rs@{DEVKIT_REVISION}");

    assert!(compliance.starts_with("name: A2A-LAB Compliance\n"));
    assert!(compliance.contains("pull_request:"));
    assert!(compliance.contains("branches:\n      - main"));
    assert!(compliance.contains("workflow_dispatch:"));
    assert_eq!(compliance.matches(&action_pin).count(), 2);
    assert!(compliance.contains("if: always()\n        uses: actions/upload-artifact@"));
    assert!(compliance.contains("continue-on-error: true"));
    assert!(compliance.contains("[[ \"$ACTION_OUTCOME\" == \"failure\" ]]"));
    assert!(compliance.contains("\"implementation_commit\""));
    assert!(compliance.contains("\"devkit_action_revision\""));
    assert!(compliance.contains("\"suite_revision\""));
    assert!(compliance.contains("\"profile_version\""));
    assert!(compliance.contains("cargo build --locked --bin a2a-lab-ot2"));
}

#[test]
fn workflows_do_not_require_private_or_sibling_devkit_access() {
    let compliance = include_str!("../.github/workflows/a2a-lab-compliance.yml");
    let release = include_str!("../.github/workflows/release.yml");
    for workflow in [compliance, release] {
        assert!(!workflow.contains("DEVKIT_READ_TOKEN"));
        assert!(!workflow.contains("repository: A3Analytics/a2a-lab-dev-kit-rs"));
    }
    assert!(release.contains("cargo build --locked --release"));
}
