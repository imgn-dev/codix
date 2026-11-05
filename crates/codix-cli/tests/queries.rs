//! `find`, `def`, `outline`, `show` and `grep`, and their freshness.
#![cfg(feature = "lang-java")]

use std::fs;

use codix_testkit::assert_golden;
use codix_testkit::fixture::{BILLING, write};
use serde_json::Value;

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

/// Checks a command's text and JSON against goldens, from `cwd` under the root.
fn check(cwd: &str, args: &[&str], name: &str) {
    let (_guard, root) = indexed(BILLING);
    let cwd = root.join(cwd);
    let text = codix(&root, &cwd, args);
    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert_golden(golden(&format!("{name}.txt")), &text.stdout);
    let mut json_args = args.to_vec();
    json_args.push("--json");
    let json = codix(&root, &cwd, &json_args);
    assert_golden(golden(&format!("{name}.json")), &json.stdout);
}

#[test]
fn find_with_filters() {
    check(
        "",
        &["find", "total", "--kind", "method", "--lang", "java"],
        "find-total",
    );
}

#[test]
fn find_falls_back_to_fuzzy() {
    let (_guard, root) = indexed(BILLING);
    let out = codix(&root, &root, &["find", "Invioce"]);
    assert!(
        out.stdout.ends_with("1 result\nmatched by fuzzy\n"),
        "{}",
        out.stdout
    );
}

#[test]
fn find_by_glob() {
    let (_guard, root) = indexed(BILLING);
    let out = codix(&root, &root, &["find", "*.total", "--glob"]);
    assert!(
        out.stdout.ends_with("4 results\nmatched by glob\n"),
        "{}",
        out.stdout
    );
}

#[test]
fn def_by_name_id_and_location() {
    check("", &["def", "Cart.total"], "def-cart-total");
    let (_guard, root) = indexed(BILLING);
    let json: Value =
        serde_json::from_str(&codix(&root, &root, &["def", "Cart.total", "--json"]).stdout)
            .unwrap();
    let id = json["result"]["definition"]["symbol_id"]
        .as_str()
        .unwrap()
        .to_owned();
    let by_id = codix(&root, &root, &["def", &id]);
    assert!(
        by_id
            .stdout
            .starts_with("src/billing/Cart.java:4:10  method  Cart.total"),
        "{}",
        by_id.stdout
    );
    let by_location = codix(
        &root,
        &root.join("src"),
        &["def", "billing/Invoice.java:7:9"],
    );
    assert!(
        by_location
            .stdout
            .starts_with("billing/Invoice.java:6:17  method  Invoice.total"),
        "{}",
        by_location.stdout
    );
}

#[test]
fn def_narrows_an_ambiguous_name() {
    let (_guard, root) = indexed(BILLING);
    let out = codix(&root, &root, &["def", "total", "--kind", "field"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(
        out.stdout
            .starts_with("src/billing/Invoice.java:4:18  field  Invoice.total"),
        "{}",
        out.stdout
    );
}

#[test]
fn def_lists_every_overload_of_a_name() {
    let (_guard, root) = indexed(BILLING);
    let out = codix(&root, &root, &["def", "Invoice.total", "--kind", "method"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    let lines: Vec<&str> = out.stdout.lines().collect();
    assert_eq!(lines.len(), 5, "{}", out.stdout);
    assert!(
        lines[0].starts_with(
            "src/billing/Invoice.java:6:17  method  Invoice.total  public long total()"
        ),
        "{}",
        out.stdout
    );
    assert!(
        lines[2].starts_with(
            "src/billing/Invoice.java:10:10  method  Invoice.total  long total(long discount)"
        ),
        "{}",
        out.stdout
    );
    assert_eq!(lines[4], "2 overloads");

    let json: Value = serde_json::from_str(
        &codix(
            &root,
            &root,
            &["def", "Invoice.total", "--kind", "method", "--json"],
        )
        .stdout,
    )
    .expect("JSON");
    assert_eq!(json["result"]["definition"]["line"], 6);
    assert_eq!(json["result"]["overloads"][0]["line"], 10);
    // The field of the same name is another symbol, not an overload.
    let all = codix(&root, &root, &["def", "Invoice.total"]);
    assert_eq!(all.code, Some(3), "{}", all.stdout);
}

#[test]
fn a_type_is_found_rather_than_its_own_constructors() {
    let (_guard, root) = indexed(&[(
        "src/Box.java",
        "class Box {\n    Box() {}\n    Box(int size) {}\n}\n",
    )]);
    let def = codix(&root, &root, &["def", "Box"]);
    assert_eq!(def.code, Some(0), "{}", def.stderr);
    assert!(
        def.stdout.starts_with("src/Box.java:1:7  class  Box"),
        "{}",
        def.stdout
    );
    let show = codix(&root, &root, &["show", "Box"]);
    assert_eq!(show.code, Some(0), "{}", show.stderr);
    assert!(show.stdout.contains("Box(int size) {}"), "{}", show.stdout);
}

#[test]
fn show_still_asks_which_overload() {
    let (_guard, root) = indexed(BILLING);
    let out = codix(&root, &root, &["show", "Invoice.total", "--kind", "method"]);
    assert_eq!(out.code, Some(3), "{}", out.stdout);
    assert!(
        out.stderr.starts_with("error[ambiguous_symbol]"),
        "{}",
        out.stderr
    );
}

#[test]
fn outline_of_a_directory() {
    check("src", &["outline", "billing"], "outline-billing");
}

#[test]
fn show_with_context() {
    check(
        "",
        &["show", "Cart.total", "--context", "1"],
        "show-cart-total",
    );
}

#[test]
fn grep_names_the_enclosing_symbol() {
    check("", &["grep", "total", "--kind", "method"], "grep-total");
    let (_guard, root) = indexed(BILLING);
    let text_files = codix(&root, &root, &["grep", "invoices", "--lang", "text"]);
    assert!(
        text_files
            .stdout
            .starts_with("docs/notes.txt:1:14  -  total of all invoices"),
        "{}",
        text_files.stdout
    );
}

#[test]
fn an_edit_between_two_queries_is_seen_by_the_second() {
    let (_guard, root) = indexed(BILLING);
    assert_eq!(codix(&root, &root, &["find", "Cart"]).code, Some(0));
    write(
        &root,
        "src/billing/Cart.java",
        "package billing;\n\nclass Basket {\n    long total() { return 0; }\n}\n",
    );
    // The lookup rechecks the file it returns and reindexes it.
    let out = codix(&root, &root, &["outline", "src/billing/Cart.java"]);
    assert!(out.stdout.contains("class Basket"), "{}", out.stdout);
    assert_eq!(codix(&root, &root, &["find", "Cart"]).code, Some(1));

    // grep scans first, so it finds text in a file no lookup returned.
    write(
        &root,
        "src/billing/Tax.java",
        "class Tax { long rate() { return 20; } }\n",
    );
    let out = codix(&root, &root, &["grep", "rate"]);
    assert!(
        out.stdout
            .starts_with("src/billing/Tax.java:1:18  Tax.rate"),
        "{}",
        out.stdout
    );
    fs::remove_file(root.join("src/billing/Tax.java")).unwrap();
    assert_eq!(codix(&root, &root, &["grep", "rate"]).code, Some(1));
}
