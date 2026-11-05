//! The network filesystem warning reaches every output format.

use std::process::Command;

use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn run(dir: &std::path::Path, args: &[&str]) -> (Option<i32>, String, String) {
    let out = Command::new(CODIX)
        .args(args)
        .current_dir(dir)
        .env("CODIX_TEST_ASSUME_NETWORK_FS", "1")
        .output()
        .expect("run codix");
    (
        out.status.code(),
        String::from_utf8(out.stdout).expect("UTF-8 stdout"),
        String::from_utf8(out.stderr).expect("UTF-8 stderr"),
    )
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().expect("temp dir");
    std::fs::create_dir(dir.path().join(".codix")).expect("create .codix");
    dir
}

#[test]
fn text_output_warns_on_stderr() {
    let dir = project();
    let (code, stdout, stderr) = run(dir.path(), &["config", "--list"]);
    assert_eq!(code, Some(0));
    assert!(
        stdout.starts_with("index.max_file_size = 2MB\n"),
        "{stdout}"
    );
    assert!(
        stderr.starts_with("warning[network_filesystem]: "),
        "{stderr}"
    );
    assert!(stderr.contains("not on a local disk"), "{stderr}");
}

#[test]
fn json_output_carries_the_warning() {
    let dir = project();
    let (_, stdout, stderr) = run(dir.path(), &["config", "--list", "--json"]);
    assert_eq!(stderr, "");
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["warnings"][0]["code"], "network_filesystem");
}

#[test]
fn errors_carry_the_warning_too() {
    let dir = project();
    let (code, _, stderr) = run(dir.path(), &["config", "bogus.key", "1"]);
    assert_eq!(code, Some(2));
    assert!(
        stderr.starts_with("warning[network_filesystem]: "),
        "{stderr}"
    );
    assert!(stderr.contains("error[invalid_config]"), "{stderr}");

    let (_, stdout, _) = run(dir.path(), &["config", "bogus.key", "1", "--json"]);
    let value: Value = serde_json::from_str(&stdout).unwrap();
    assert_eq!(value["error"]["code"], "invalid_config");
    assert_eq!(value["warnings"][0]["code"], "network_filesystem");
}

#[test]
fn no_warning_on_a_local_disk() {
    let dir = project();
    let out = Command::new(CODIX)
        .args(["config", "--list"])
        .current_dir(dir.path())
        .env_remove("CODIX_TEST_ASSUME_NETWORK_FS")
        .output()
        .unwrap();
    assert_eq!(String::from_utf8(out.stderr).unwrap(), "");
}
