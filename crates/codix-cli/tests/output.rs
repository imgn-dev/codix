//! Text and JSON rendering, paging and exit codes, through `find`.
#![cfg(with_lang_java)]

use codix_testkit::assert_golden;
use codix_testkit::fixture::methods;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn codix(root: &std::path::Path, cwd: &std::path::Path, args: &[&str]) -> codix_testkit::Run {
    codix_testkit::fixture::codix_in(CODIX, root, cwd, args)
}

fn indexed(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
    codix_testkit::fixture::indexed(CODIX, files)
}

fn golden(name: &str) -> String {
    format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn check(args: &[&str], name: &str, expected_code: i32) {
    let (_guard, root) = indexed(&[("Box.java", &methods(60))]);
    let text = codix(&root, &root, args);
    assert_eq!(text.code, Some(expected_code), "{}", text.stderr);
    assert_eq!(text.stderr, "");
    // The next page's command starts with the program as invoked.
    assert_golden(
        golden(&format!("{name}.txt")),
        &text.stdout.replace(CODIX, "codix"),
    );

    let mut json_args = args.to_vec();
    json_args.push("--json");
    let json = codix(&root, &root, &json_args);
    assert_eq!(json.code, Some(expected_code), "{}", json.stderr);
    assert_golden(golden(&format!("{name}.json")), &json.stdout);
}

#[test]
fn no_results_exits_1() {
    check(&["find", "nothing"], "find-0", 1);
}

#[test]
fn one_result() {
    check(&["find", "m7"], "find-1", 0);
}

#[test]
fn sixty_results_are_capped_at_the_default_limit() {
    check(&["find", "m", "--prefix"], "find-60", 0);
}

#[test]
fn after_returns_the_next_page() {
    let (_guard, root) = indexed(&[("Box.java", &methods(60))]);
    // No `codix` on PATH: the command names the binary as it was run.
    let empty = tempfile::tempdir().expect("an empty PATH");
    let first = std::process::Command::new(CODIX)
        .args(["find", "m", "--prefix"])
        .current_dir(&root)
        .env("PATH", empty.path())
        .output()
        .expect("run codix");
    let stdout = String::from_utf8_lossy(&first.stdout);
    let next = stdout.lines().last().expect("a next page");
    assert_eq!(
        next,
        format!("{CODIX} find m --prefix --after Box.java:51:10")
    );
    let out = codix(
        &root,
        &root,
        &["find", "m", "--prefix", "--after", "Box.java:51:10"],
    );
    assert_eq!(out.code, Some(0));
    assert!(
        out.stdout
            .ends_with("10 of 60 results\nmatched by prefix\n"),
        "{}",
        out.stdout
    );
    assert_eq!(out.stdout.lines().count(), 12);
}

#[test]
fn piped_output_has_no_color() {
    let (_guard, root) = indexed(&[("Box.java", &methods(3))]);
    for args in [&["find", "m0"][..], &["def", "nothing"]] {
        let out = codix(&root, &root, args);
        assert!(!out.stdout.contains('\x1b'), "{}", out.stdout);
        assert!(!out.stderr.contains('\x1b'), "{}", out.stderr);
    }
}

/// Run by its path while `codix` on `PATH` is the same binary, a next page's
/// command says `codix`.
#[cfg(unix)]
#[test]
fn a_command_names_codix_when_path_finds_this_binary() {
    let (_guard, root) = indexed(&[("Box.java", &methods(60))]);
    let bin = tempfile::tempdir().expect("a directory for PATH");
    std::os::unix::fs::symlink(CODIX, bin.path().join("codix")).expect("link codix");
    let out = std::process::Command::new(CODIX)
        .args(["find", "m", "--prefix"])
        .current_dir(&root)
        .env("PATH", bin.path())
        .output()
        .expect("run codix");
    let stdout = String::from_utf8_lossy(&out.stdout);
    let last = stdout.lines().last().expect("a next page");
    assert_eq!(last, "codix find m --prefix --after Box.java:51:10");
}
