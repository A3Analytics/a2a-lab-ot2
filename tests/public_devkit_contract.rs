const DEVKIT_REVISION: &str = "9d5327868d96b3e800fd89f6debf434bcc12709d";
const DEVKIT_URL: &str = "https://github.com/A3Analytics/a2a-lab-dev-kit-rs";
const TCK_REVISION: &str = "1fd47d5c73e6fa3f7bd6505d75e43ee68cae33fd";
const TCK_URL: &str = "https://github.com/A3Analytics/a2a-lab-tck";

#[test]
fn cargo_uses_the_public_immutable_devkit_independently() {
    let manifest = include_str!("../Cargo.toml");
    let lockfile = include_str!("../Cargo.lock");

    assert!(manifest.contains(&format!("git = \"{DEVKIT_URL}\"")));
    assert!(manifest.contains(&format!("rev = \"{DEVKIT_REVISION}\"")));
    assert!(lockfile.contains(&format!(
        "source = \"git+{DEVKIT_URL}?rev={DEVKIT_REVISION}#{DEVKIT_REVISION}\""
    )));
    assert!(!manifest.contains("a2a-lab-tck"));
    assert!(!manifest.contains("../a2a-lab-dev-kit-rs"));
    assert_ne!(DEVKIT_REVISION, TCK_REVISION);
}

#[test]
fn local_and_workflow_use_the_same_public_immutable_tck() {
    let runner = include_str!("../.mise/scripts/compliance.sh");
    let compliance = include_str!("../.github/workflows/a2a-lab-compliance.yml");
    let action_pin = format!("uses: A3Analytics/a2a-lab-tck@{TCK_REVISION}");

    assert!(runner.contains(&format!("tck_url=\"{TCK_URL}\"")));
    assert!(runner.contains(&format!("tck_revision=\"{TCK_REVISION}\"")));
    assert!(runner.contains("A2ALAB_TCK_ROOT"));
    assert!(!runner.contains("A2ALAB_DEV_KIT_ROOT"));
    assert!(!runner.contains(":-$root/../a2a-lab-tck"));
    assert!(compliance.starts_with("name: A2A-LAB Compliance\n"));
    assert!(compliance.contains("pull_request:"));
    assert!(compliance.contains("branches:\n      - main"));
    assert!(compliance.contains("workflow_dispatch:"));
    assert_eq!(compliance.matches(&action_pin).count(), 2);
    assert!(!compliance.contains("uses: A3Analytics/a2a-lab-dev-kit-rs@"));
}

#[test]
fn compliance_workflow_retains_failure_evidence_and_provenance() {
    let compliance = include_str!("../.github/workflows/a2a-lab-compliance.yml");

    assert!(compliance.contains("if: always()\n        uses: actions/upload-artifact@"));
    assert!(compliance.contains("continue-on-error: true"));
    assert!(compliance.contains("[[ \"$ACTION_OUTCOME\" == \"failure\" ]]"));
    assert!(compliance.contains(&format!(
        "\"devkit_dependency_revision\": \"{DEVKIT_REVISION}\""
    )));
    assert!(compliance.contains(&format!("\"tck_action_revision\": \"{TCK_REVISION}\"")));
    assert!(compliance.contains(&format!("\"tck_suite_revision\": \"{TCK_REVISION}\"")));
    for field in [
        "\"implementation_commit\"",
        "\"profile_version\": \"1.1.0\"",
        "\"selected_suite\": \"full\"",
        "\"llm_check\": True",
        "\"workflow_outcome\"",
    ] {
        assert!(compliance.contains(field));
    }
    assert!(compliance.contains("cargo build --locked --bin a2a-lab-ot2"));
}

#[test]
fn workflows_do_not_require_private_or_sibling_tck_access() {
    let compliance = include_str!("../.github/workflows/a2a-lab-compliance.yml");
    let release = include_str!("../.github/workflows/release.yml");
    for workflow in [compliance, release] {
        assert!(!workflow.contains("TCK_READ_TOKEN"));
        assert!(!workflow.contains("DEVKIT_READ_TOKEN"));
        assert!(!workflow.contains("repository: A3Analytics/a2a-lab-tck"));
        assert!(!workflow.contains("repository: A3Analytics/a2a-lab-dev-kit-rs"));
    }
    assert!(release.contains("cargo build --locked --release"));
}
