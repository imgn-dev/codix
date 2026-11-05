//! A malformed descriptor fails the provider's build, not a run.

use std::path::Path;
use std::process::Command;

#[test]
fn malformed_descriptor_fails_the_build() {
    let fixture =
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../codix-testkit/fixtures/provider-bad");
    let target = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../target/fixtures");
    let output = Command::new(std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned()))
        .args(["build", "--offline", "--quiet", "--manifest-path"])
        .arg(fixture.join("Cargo.toml"))
        .arg("--target-dir")
        .arg(target)
        .output()
        .unwrap();
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(!output.status.success(), "the build succeeded");
    assert!(
        stderr.contains("invalid provider descriptor descriptor.toml"),
        "{stderr}"
    );
    assert!(stderr.contains("ignore_strategy"), "{stderr}");
}
