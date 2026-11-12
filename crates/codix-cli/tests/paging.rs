//! Paging by place: a next page cut from results that changed since the one
//! before skips none of them.
#![cfg(with_lang_java)]

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
    assert!(matches!(out.code, Some(0 | 1)), "{}", out.stderr);
    serde_json::from_str(&out.stdout).expect("JSON")
}

fn places(items: &Value) -> Vec<String> {
    items
        .as_array()
        .expect("items")
        .iter()
        .map(|i| format!("{}:{}", i["path"].as_str().expect("a path"), i["line"]))
        .collect()
}

fn two_calls(class: &str, call: &str) -> String {
    format!("class {class} {{\n    void f() {{ {call}(); }}\n    void g() {{ {call}(); }}\n}}\n")
}

#[test]
fn fixing_a_page_skips_nothing_on_the_next() {
    let files: Vec<(String, String)> = ["A", "B", "C", "D", "E"]
        .iter()
        .map(|c| (format!("{c}.java"), two_calls(c, "legacy")))
        .collect();
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    let (_guard, root) = indexed(CODIX, &files);

    let first = json(&root, &["grep", "legacy\\(", "--limit", "4"]);
    let result = &first["result"];
    assert_eq!(
        places(&result["matches"]["items"]),
        ["A.java:2", "A.java:3", "B.java:2", "B.java:3"]
    );
    assert_eq!(result["matches"]["total"], 10);
    let after = result["next"]["after"].as_str().expect("a next page");
    assert_eq!(after, "B.java");

    // The first page's matches are fixed.
    for class in ["A", "B"] {
        std::fs::write(
            root.join(format!("{class}.java")),
            two_calls(class, "modern"),
        )
        .expect("edit");
    }
    let second = json(
        &root,
        &["grep", "legacy\\(", "--limit", "4", "--after", after],
    );
    let result = &second["result"];
    assert_eq!(
        places(&result["matches"]["items"]),
        ["C.java:2", "C.java:3", "D.java:2", "D.java:3"]
    );
    assert_eq!(result["matches"]["total"], 6);
    assert_eq!(result["next"]["after"], "D.java");
}

#[test]
fn the_next_page_of_refs_keeps_its_symbol_through_a_rename() {
    let invoice = "class Invoice {\n    private long total() { return 0; }\n    long a() { return total(); }\n    long b() { return total(); }\n    long c() { return total(); }\n    long d() { return total(); }\n}\n";
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", invoice)]);

    let first = json(&root, &["refs", "Invoice.total", "--limit", "3"]);
    let result = &first["result"];
    assert_eq!(
        places(&result["references"]["items"]),
        ["Invoice.java:2", "Invoice.java:3", "Invoice.java:4"]
    );
    assert_eq!(result["definitions"], 1);
    assert_eq!(result["scoped"], 4);
    assert_eq!(result["textual"], 0);
    let id = result["targets"][0].as_str().expect("a target");
    let next = &result["next"];
    assert_eq!(next["symbol"], id);
    let after = next["after"].as_str().expect("a next page");
    assert_eq!(after, "Invoice.java:4:23");

    let planned = codix(&root, &["rename", id, "amount"]);
    assert_eq!(planned.code, Some(0), "{}", planned.stderr);
    let preview = planned
        .stdout
        .split_whitespace()
        .nth(1)
        .expect("a preview ID")
        .trim_end_matches(':');
    let applied = codix(&root, &["apply", preview]);
    assert_eq!(applied.code, Some(0), "{}", applied.stderr);

    let second = json(&root, &["refs", id, "--limit", "3", "--after", after]);
    let result = &second["result"];
    assert_eq!(
        places(&result["references"]["items"]),
        ["Invoice.java:5", "Invoice.java:6"]
    );
    assert!(result["renamed"].is_object(), "{result}");
    assert!(result["next"].is_null(), "{result}");
}

#[test]
fn fuzzy_matches_are_one_page() {
    let (_guard, root) = indexed(CODIX, &[("A.java", "class A { void total() {} }\n")]);
    let found = json(&root, &["find", "totl"]);
    assert_eq!(found["result"]["mode"], "fuzzy");
    assert!(found["result"]["next"].is_null());
    let paged = codix(&root, &["find", "totl", "--fuzzy", "--after", "A.java"]);
    assert_eq!(paged.code, Some(2), "{}", paged.stderr);
    assert!(paged.stderr.starts_with("error[usage]"), "{}", paged.stderr);
}
