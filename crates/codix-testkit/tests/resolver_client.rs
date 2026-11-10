//! A resolver that never answers is given up after the timeout, and killed.

use std::fs;
use std::time::Duration;

use codix_resolver::Scratch;
use codix_resolver::client::Process;

#[test]
fn a_resolver_that_hangs_is_given_up() {
    let dir = tempfile::tempdir().expect("temp dir");
    let name = format!("codix-test-resolver{}", std::env::consts::EXE_SUFFIX);
    let executable = dir.path().join(name);
    fs::copy(env!("CARGO_BIN_EXE_codix-test-resolver"), &executable).expect("copy");
    fs::write(dir.path().join("mode"), "hang").expect("mode");
    let index = dir.path().join("index.db");
    let scratch = dir.path().join("scratch.db");
    Scratch::create(&index).expect("an index");
    Scratch::create(&scratch).expect("a scratch database");

    let mut process = Process::start(&executable).expect("hello");
    let error = process
        .resolve(&index, &scratch, &[], Duration::from_secs(1))
        .expect_err("no answer");
    assert_eq!(error, "no answer within 1 s");
}
