use std::process::Command;

#[test]
fn version_prints_the_package_version() {
    let output = Command::new(env!("CARGO_BIN_EXE_a2a-lab-ot2"))
        .arg("--version")
        .output()
        .expect("run a2a-lab-ot2 --version");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let text = String::from_utf8(output.stdout).expect("utf-8 stdout");
    assert!(
        text.contains(env!("CARGO_PKG_VERSION")),
        "version output was {text}"
    );
}
