//! Every error class: exit code, text rendering on stderr, JSON on stdout.

use codix_testkit::{run, run_in};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

/// Runs `args` in an empty temporary directory, outside any project. `{CWD}` in
/// `text` stands for that directory.
fn check(args: &[&str], code: &str, exit: i32, text: &str) {
    let dir = tempfile::tempdir().expect("test setup");
    let cwd = std::fs::canonicalize(dir.path()).expect("test setup");
    let text = text.replace("{CWD}", &cwd.display().to_string());
    let text = text.as_str();
    let run = |args: &[&str]| run_in(&cwd, CODIX, args);
    let out = run(args);
    assert_eq!(out.code, Some(exit), "{code}: {}", out.stderr);
    assert_eq!(out.stdout, "", "{code}");
    assert_eq!(out.stderr, text, "{code}");

    let mut json_args = args.to_vec();
    json_args.push("--json");
    let out = run(&json_args);
    assert_eq!(out.code, Some(exit), "{code} --json");
    assert_eq!(out.stderr, "", "{code} --json");
    let value: Value = serde_json::from_str(&out.stdout).expect("one JSON document");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["error"]["code"], code);
    let first = text.lines().next().expect("error line");
    let cause = first.split_once("]: ").expect("error[code]: cause").1;
    assert_eq!(value["error"]["cause"], cause);
    let hint = text
        .lines()
        .last()
        .and_then(|line| line.strip_prefix("hint: "))
        .expect("hint line");
    assert_eq!(value["error"]["hint"], hint);
}

fn fail(code: &str, exit: i32, text: &str) {
    check(&["debug-echo", "1", "--fail", code], code, exit, text);
}

#[test]
fn no_project() {
    fail(
        "no_project",
        6,
        "error[no_project]: no codix project in {CWD} or any parent directory\n\
         hint: run `codix init` at the project root\n",
    );
}

#[test]
fn scope_outside_a_project_is_no_project() {
    check(
        &["debug-echo", "1", "--here"],
        "no_project",
        6,
        "error[no_project]: no codix project in {CWD} or any parent directory\n\
         hint: run `codix init` at the project root\n",
    );
}

#[test]
fn not_indexed() {
    fail(
        "not_indexed",
        6,
        "error[not_indexed]: the project at /work/project is not indexed\n\
         hint: run `codix init` at the project root\n",
    );
}

#[test]
fn ambiguous_symbol_lists_candidates() {
    fail(
        "ambiguous_symbol",
        3,
        "error[ambiguous_symbol]: \"total\" matches 2 symbols\n  \
         py:src/billing/invoice.py::Invoice.total#a3f9\n  \
         py:src/billing/cart.py::Cart.total#71c2\n\
         hint: pass a full symbol ID, or narrow with --kind or --path\n",
    );
    let out = run(
        CODIX,
        &["debug-echo", "1", "--fail", "ambiguous_symbol", "--json"],
    );
    let value: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(
        value["error"]["candidates"],
        serde_json::json!([
            "py:src/billing/invoice.py::Invoice.total#a3f9",
            "py:src/billing/cart.py::Cart.total#71c2"
        ])
    );
}

#[test]
fn unknown_symbol() {
    fail(
        "unknown_symbol",
        1,
        "error[unknown_symbol]: no symbol matches \"totl\"\n\
         hint: check the name with `codix find`, or pass a full symbol ID\n",
    );
}

#[test]
fn invalid_config() {
    fail(
        "invalid_config",
        2,
        "error[invalid_config]: unknown key `previews.expiri`\n\
         hint: valid keys: previews.expiry\n",
    );
}

#[test]
fn not_supported_in_build() {
    fail(
        "not_supported_in_build",
        2,
        "error[not_supported_in_build]: src/Main.kt: kotlin is not supported in this build\n\
         hint: use the full codix build, which includes every provider\n",
    );
}

#[test]
fn io() {
    fail(
        "io",
        5,
        "error[io]: src/missing.rs: file not found\n\
         hint: check that the file exists and is accessible\n",
    );
}

#[test]
fn internal() {
    fail(
        "internal",
        5,
        "error[internal]: sample internal error\n\
         hint: this is a bug in codix; please report it\n",
    );
}

#[test]
fn usage_from_a_bad_flag() {
    check(
        &["debug-echo", "1", "--bogus"],
        "usage",
        2,
        "error[usage]: unexpected argument '--bogus' found\n\
         hint: usage: codix debug-echo <COUNT>; see --help\n",
    );
}

#[test]
fn usage_from_a_zero_limit() {
    check(
        &["debug-echo", "1", "--limit", "0"],
        "usage",
        2,
        "error[usage]: --limit must be at least 1\n\
         hint: omit --limit for the default of 50\n",
    );
}

#[test]
fn usage_names_codix_whatever_the_binary_is_called() {
    let dir = std::env::temp_dir().join(format!("codix-renamed-{}", std::process::id()));
    std::fs::create_dir_all(&dir).unwrap();
    let renamed = dir.join(format!("renamed-codix{}", std::env::consts::EXE_SUFFIX));
    std::fs::copy(CODIX, &renamed).unwrap();
    let out = run(renamed.to_str().unwrap(), &["debug-echo", "1", "--bogus"]);
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(
        out.stderr.contains("usage: codix debug-echo"),
        "{}",
        out.stderr
    );
}
