//! Text and JSON rendering of a stub operation, with paging and exit codes.

use codix_testkit::{Run, assert_golden, run_in};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

/// Runs in an empty temporary directory, outside any project.
fn run(_: &str, args: &[&str]) -> Run {
    let dir = tempfile::tempdir().expect("test setup");
    run_in(dir.path(), CODIX, args)
}

fn golden(name: &str) -> String {
    format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn check(count: &str, name: &str, expected_code: i32) {
    let text = run(CODIX, &["debug-echo", count]);
    assert_eq!(text.code, Some(expected_code), "{}", text.stderr);
    assert_eq!(text.stderr, "");
    assert_golden(golden(&format!("{name}.txt")), &text.stdout);

    let json = run(CODIX, &["debug-echo", count, "--json"]);
    assert_eq!(json.code, Some(expected_code), "{}", json.stderr);
    assert_eq!(json.stderr, "");
    assert_golden(golden(&format!("{name}.json")), &json.stdout);
}

#[test]
fn no_results_exits_1() {
    check("0", "echo-0", 1);
}

#[test]
fn one_result() {
    check("1", "echo-1", 0);
}

#[test]
fn sixty_results_are_capped_at_the_default_limit() {
    check("60", "echo-60", 0);
}

#[test]
fn offset_returns_the_next_page() {
    let out = run(CODIX, &["debug-echo", "60", "--offset", "50"]);
    assert_eq!(out.code, Some(0));
    assert_eq!(out.stdout.lines().count(), 11);
    assert!(out.stdout.ends_with("10 of 60 results\n"), "{}", out.stdout);
}

#[test]
fn definitions_then_anchor_file_then_position() {
    let out = run(CODIX, &["debug-echo", "8", "--anchor", "tests/file3.rs"]);
    let locations: Vec<&str> = out
        .stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|cell| cell.contains(".rs:"))
        .collect();
    assert_eq!(
        locations,
        [
            "tests/file3.rs:17:3",
            "src/file0.rs:10:1",
            "tests/file3.rs:13:4",
            "src/file0.rs:14:5",
            "src/file1.rs:11:2",
            "src/file1.rs:15:1",
            "src/file2.rs:12:3",
            "src/file2.rs:16:2",
        ]
    );
}

#[test]
fn piped_output_has_no_color() {
    for args in [
        &["debug-echo", "3"][..],
        &["debug-echo", "1", "--fail", "internal"],
    ] {
        let out = run(CODIX, args);
        assert!(!out.stdout.contains('\x1b'), "{}", out.stdout);
        assert!(!out.stderr.contains('\x1b'), "{}", out.stderr);
    }
}
