//! Paths shown relative to the cwd, `--root`, `--here` and `--path`.
#![cfg(with_lang_java)]

use codix_testkit::Run;
use codix_testkit::fixture::BILLING;
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn codix(root: &std::path::Path, cwd: &std::path::Path, args: &[&str]) -> codix_testkit::Run {
    codix_testkit::fixture::codix_in(CODIX, root, cwd, args)
}

fn indexed(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
    codix_testkit::fixture::indexed(CODIX, files)
}

fn locations(out: &Run) -> Vec<String> {
    out.stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|cell| cell.contains(".java:"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn paths_are_relative_to_a_subdirectory_cwd() {
    let (_guard, root) = indexed(BILLING);
    let out = codix(&root, &root.join("src/billing"), &["find", "Invoice"]);
    assert_eq!(locations(&out), ["Invoice.java:3:14"]);
}

#[test]
fn paths_outside_the_cwd_climb_with_dot_dot() {
    let (_guard, root) = indexed(BILLING);
    let out = codix(&root, &root.join("docs"), &["find", "Cart"]);
    assert_eq!(locations(&out), ["../src/billing/Cart.java:3:7"]);
}

#[test]
fn json_carries_root_relative_paths() {
    let (_guard, root) = indexed(BILLING);
    let out = codix(&root, &root.join("docs"), &["find", "Cart", "--json"]);
    let value: Value = serde_json::from_str(&out.stdout).unwrap();
    let item = &value["result"]["results"]["items"][0];
    assert_eq!(item["path"], "../src/billing/Cart.java");
    assert_eq!(item["root_path"], "src/billing/Cart.java");
}

#[test]
fn here_limits_to_the_cwd_subtree() {
    let (_guard, root) = indexed(BILLING);
    let out = codix(
        &root,
        &root.join("tests"),
        &["find", "total", "--prefix", "--here"],
    );
    assert_eq!(locations(&out), ["billing/InvoiceTest.java:2:10"]);
}

#[test]
fn path_limits_to_the_given_subtrees() {
    let (_guard, root) = indexed(BILLING);
    let out = codix(
        &root,
        &root,
        &[
            "find",
            "total",
            "--path",
            "src/billing/Cart.java",
            "--path",
            "tests",
        ],
    );
    assert_eq!(locations(&out), ["src/billing/Cart.java:4:10"]);
}

#[test]
fn root_flag_selects_the_project() {
    let (_guard, root) = indexed(BILLING);
    let outside = tempfile::tempdir().unwrap();
    let out = codix(&root, outside.path(), &["find", "Cart", "--root", "{ROOT}"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(
        out.stdout
            .lines()
            .next()
            .map(|l| l.contains("src/billing/Cart.java:3:7")),
        Some(true),
        "{}",
        out.stdout
    );

    let out = codix(
        &root,
        outside.path(),
        &["find", "Cart", "--root", "{ROOT}", "--here"],
    );
    assert_eq!(out.code, Some(2), "{}", out.stderr);
    assert!(
        out.stderr.starts_with("error[usage]: --here: "),
        "{}",
        out.stderr
    );
}
