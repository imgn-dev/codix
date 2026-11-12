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
    format!(
        "class {class} {{\n    void f() {{ Legacy.{call}(); }}\n    void g() {{ Legacy.{call}(); }}\n}}\n"
    )
}

const LEGACY: &str =
    "class Legacy {\n    static void legacy() {}\n    static void modern() {}\n}\n";

/// `Legacy.java` in `dir`, and each class in its directory calling it twice.
fn callers(dir: &str, classes: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
    let mut files: Vec<(String, String)> = vec![(format!("{dir}Legacy.java"), LEGACY.to_owned())];
    files.extend(
        classes
            .iter()
            .map(|(d, c)| (format!("{d}{c}.java"), two_calls(c, "legacy"))),
    );
    let files: Vec<(&str, &str)> = files
        .iter()
        .map(|(p, c)| (p.as_str(), c.as_str()))
        .collect();
    indexed(CODIX, &files)
}

#[test]
fn fixing_a_page_skips_nothing_on_the_next() {
    let classes = [("", "A"), ("", "B"), ("", "C"), ("", "D"), ("", "E")];
    let (_guard, root) = callers("", &classes);

    let first = json(&root, &["refs", "Legacy.legacy", "--limit", "4"]);
    let result = &first["result"];
    // The page ends before B.java rather than cutting it.
    assert_eq!(
        places(&result["references"]["items"]),
        ["Legacy.java:2", "A.java:2", "A.java:3"]
    );
    assert_eq!(result["references"]["total"], 11);
    let after = result["next"]["after"].as_str().expect("a next page");
    assert_eq!(after, "A.java");

    // The first page's uses are fixed, and the next file's too.
    for class in ["A", "B"] {
        std::fs::write(
            root.join(format!("{class}.java")),
            two_calls(class, "modern"),
        )
        .expect("edit");
    }
    let second = json(
        &root,
        &["refs", "Legacy.legacy", "--limit", "4", "--after", after],
    );
    let result = &second["result"];
    assert_eq!(
        places(&result["references"]["items"]),
        ["C.java:2", "C.java:3", "D.java:2", "D.java:3"]
    );
    assert_eq!(result["references"]["total"], 7);
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
    let id = result["targets"][0]["symbol_id"]
        .as_str()
        .expect("a target");
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
fn a_fuzzy_find_shows_the_closest_name_and_offers_the_others() {
    let class = "class A {\n    void total() {}\n    void totals() {}\n    void totaly() {}\n}\n";
    let other = "class B {\n    void total() {}\n}\n";
    let (_guard, root) = indexed(CODIX, &[("A.java", class), ("B.java", other)]);

    let found = json(&root, &["find", "totalx", "--limit", "1"]);
    let result = &found["result"];
    assert_eq!(result["mode"], "fuzzy");
    // All three names are one edit away: the first by name is taken.
    assert_eq!(result["corrected"], "total");
    assert_eq!(places(&result["results"]["items"]), ["A.java:2"]);
    assert_eq!(result["results"]["total"], 2);
    let names: Vec<(&str, u64, u64)> = result["alternatives"]
        .as_array()
        .expect("alternatives")
        .iter()
        .map(|a| {
            (
                a["name"].as_str().expect("a name"),
                a["distance"].as_u64().expect("a distance"),
                a["symbols"].as_u64().expect("a count"),
            )
        })
        .collect();
    assert_eq!(names, [("totals", 1, 1), ("totaly", 1, 1)]);
    let next = &result["next"];
    assert_eq!(next["query"], "total");
    assert_eq!(next["mode"], "exact");
    assert_eq!(next["after"], "A.java");

    let text = codix(&root, &["find", "totalx", "--limit", "1"]);
    assert!(
        text.stdout.starts_with("closest name: total\n"),
        "{}",
        text.stdout
    );
    assert!(
        text.stdout
            .contains("other names:\n  totals  1 edit, 1 symbol  "),
        "{}",
        text.stdout
    );
    let last = text.stdout.lines().last().expect("a next page");
    assert!(
        last.ends_with(" find total --limit 1 --exact --after A.java"),
        "{last}"
    );
    let second = json(
        &root,
        &[
            "find", "total", "--limit", "1", "--exact", "--after", "A.java",
        ],
    );
    assert_eq!(places(&second["result"]["results"]["items"]), ["B.java:2"]);
}

#[test]
fn count_gives_the_totals_and_all_every_result() {
    let (_guard, root) = callers("", &[("", "A"), ("", "B"), ("", "C")]);

    let counted = codix(&root, &["refs", "Legacy.legacy", "--count"]);
    assert_eq!(counted.code, Some(0), "{}", counted.stderr);
    assert!(
        counted
            .stdout
            .starts_with("7 results\n1 definition, 0 scoped, 6 textual\nby place:\n  2  A.java  "),
        "{}",
        counted.stdout
    );
    let counted = json(&root, &["refs", "Legacy.legacy", "--count"]);
    assert_eq!(counted["result"]["references"]["total"], 7);
    assert_eq!(
        counted["result"]["references"]["items"],
        serde_json::json!([])
    );
    assert!(counted["result"]["next"].is_null());

    let all = json(&root, &["refs", "Legacy.legacy", "--all"]);
    assert_eq!(
        all["result"]["references"]["items"]
            .as_array()
            .map(Vec::len),
        Some(7)
    );
    assert!(all["result"]["next"].is_null());

    let both = codix(&root, &["refs", "Legacy.legacy", "--all", "--limit", "2"]);
    assert_eq!(both.code, Some(2), "{}", both.stderr);
}

#[test]
fn results_that_do_not_fit_say_where_they_are() {
    let (_guard, root) = callers("src/", &[("src/a/", "A"), ("src/a/", "B"), ("src/b/", "C")]);

    let found = json(&root, &["refs", "Legacy.legacy", "--limit", "2"]);
    let breakdown: Vec<(&str, u64)> = found["result"]["references"]["breakdown"]
        .as_array()
        .expect("a breakdown")
        .iter()
        .map(|p| {
            (
                p["path"].as_str().expect("a path"),
                p["count"].as_u64().expect("a count"),
            )
        })
        .collect();
    assert_eq!(
        breakdown,
        [("src/a", 4), ("src/b", 2), ("src/Legacy.java", 1)]
    );

    let text = codix(&root, &["refs", "Legacy.legacy", "--limit", "2"]);
    let line = text
        .stdout
        .lines()
        .find(|l| l.starts_with("  4  src/a  "))
        .unwrap_or_else(|| panic!("{}", text.stdout));
    assert!(line.ends_with(" --limit 2 --path src/a"), "{line}");
    let narrowed = json(
        &root,
        &["refs", "Legacy.legacy", "--path", "src/a", "--count"],
    );
    assert_eq!(narrowed["result"]["references"]["total"], 4);

    // Paging on, the breakdown is not repeated.
    let after = found["result"]["next"]["after"]
        .as_str()
        .expect("a next page");
    let second = json(
        &root,
        &["refs", "Legacy.legacy", "--limit", "2", "--after", after],
    );
    assert_eq!(
        second["result"]["references"]["breakdown"],
        serde_json::json!([])
    );
}

#[test]
fn by_file_is_a_worklist_paged_by_file() {
    let (_guard, root) = callers("", &[("", "A"), ("", "B"), ("", "C")]);

    let text = codix(&root, &["refs", "Legacy.legacy", "--files", "--limit", "2"]);
    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert!(
        text.stdout
            .starts_with("Legacy.java  1\nA.java       2\n2 of 4 files\n"),
        "{}",
        text.stdout
    );
    let last = text.stdout.lines().last().expect("a next page");
    assert!(
        last.ends_with(" --files --limit 2 --after A.java"),
        "{last}"
    );

    let second = json(
        &root,
        &[
            "refs",
            "Legacy.legacy",
            "--files",
            "--limit",
            "2",
            "--after",
            "A.java",
        ],
    );
    let references = &second["result"]["references"];
    assert_eq!(references["files"][0]["path"], "B.java");
    assert_eq!(references["files"][0]["count"], 2);
    assert_eq!(references["total"], 4);
    assert!(second["result"]["next"].is_null());
}
