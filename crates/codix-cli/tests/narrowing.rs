//! Narrowing results: by excluded directory, by main or test sources, by a
//! reference's context, and files with the most results first.
#![cfg(all(with_lang_java, with_build_maven))]

use std::path::Path;

use codix_testkit::fixture::{codix_in, indexed};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn codix(root: &Path, args: &[&str]) -> codix_testkit::Run {
    codix_in(CODIX, root, root, args)
}

fn json(root: &Path, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.push("--json");
    let out = codix(root, &args);
    serde_json::from_str(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", out.stderr))
}

fn paths(found: &Value, list: &str) -> Vec<String> {
    found["result"][list]["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|r| format!("{}:{}", r["path"].as_str().unwrap_or_default(), r["line"]))
        .collect()
}

const MAIN: &str = "package shop;\n\npublic class Cart {\n    public long total() { return 0; }\n    long twice() { return total() + total(); }\n}\n";
const TEST: &str =
    "package shop;\n\nclass CartTest {\n    long check(Cart cart) { return cart.total(); }\n}\n";
const OTHER: &str =
    "package shop;\n\nclass Report {\n    Cart cart;\n    long sum() { return cart.total(); }\n}\n";

fn shop() -> (tempfile::TempDir, std::path::PathBuf) {
    indexed(
        CODIX,
        &[
            (
                "pom.xml",
                "<project><artifactId>shop</artifactId></project>\n",
            ),
            ("src/main/java/shop/Cart.java", MAIN),
            ("src/main/java/shop/report/Report.java", OTHER),
            ("src/test/java/shop/CartTest.java", TEST),
        ],
    )
}

#[test]
fn an_excluded_directory_is_left_out() {
    let (_guard, root) = shop();
    let found = json(
        &root,
        &[
            "refs",
            "Cart.total",
            "--exclude-path",
            "src/main/java/shop/report",
        ],
    );
    assert_eq!(
        paths(&found, "references"),
        [
            "src/main/java/shop/Cart.java:4",
            "src/main/java/shop/Cart.java:5",
            "src/main/java/shop/Cart.java:5",
            "src/test/java/shop/CartTest.java:4"
        ]
    );
}

#[test]
fn sources_keep_main_or_test_code() {
    let (_guard, root) = shop();
    let test = json(&root, &["refs", "Cart.total", "--sources", "test"]);
    // The definition is in main sources: only the test's use is left.
    assert_eq!(
        paths(&test, "references"),
        ["src/test/java/shop/CartTest.java:4"]
    );
    let main = json(&root, &["find", "check", "--sources", "main"]);
    assert_eq!(main["result"]["results"]["total"], 0);

    let (_guard, bare) = indexed(CODIX, &[("A.java", "class A {}\n")]);
    let refused = codix(&bare, &["find", "A", "--sources", "main"]);
    assert_eq!(refused.code, Some(2), "{}", refused.stdout);
    assert!(
        refused.stderr.contains("no build system"),
        "{}",
        refused.stderr
    );
}

#[test]
fn a_context_keeps_those_uses_and_the_definition() {
    let (_guard, root) = shop();
    let read = json(&root, &["refs", "Report.cart", "--context", "read"]);
    assert_eq!(
        paths(&read, "references"),
        [
            "src/main/java/shop/report/Report.java:4",
            "src/main/java/shop/report/Report.java:5"
        ]
    );
    let written = json(&root, &["refs", "Report.cart", "--context", "write"]);
    assert_eq!(
        paths(&written, "references"),
        ["src/main/java/shop/report/Report.java:4"]
    );
}

#[test]
fn files_sorted_by_count_put_the_most_first() {
    let (_guard, root) = shop();
    let text = codix(&root, &["refs", "Cart.total", "--files", "--sort", "count"]);
    assert_eq!(text.code, Some(0), "{}", text.stderr);
    let rows: Vec<Vec<&str>> = text
        .stdout
        .lines()
        .take(3)
        .map(|l| l.split_whitespace().collect())
        .collect();
    assert_eq!(
        rows[0],
        ["src/main/java/shop/Cart.java", "3"],
        "{}",
        text.stdout
    );
    let refused = codix(&root, &["refs", "Cart.total", "--sort", "count"]);
    assert_eq!(refused.code, Some(2), "{}", refused.stdout);
}
