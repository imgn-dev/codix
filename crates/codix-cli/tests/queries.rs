//! `find`, `def`, `outline`, `show` and `at`, and their freshness.
#![cfg(with_lang_java)]

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
        out.stdout.starts_with("closest name: Invoice\n"),
        "{}",
        out.stdout
    );
    assert!(out.stdout.ends_with("1 result\n"), "{}", out.stdout);
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
    let id = json["result"]["definitions"][0]["symbol_id"]
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
    assert_eq!(json["result"]["definitions"][0]["line"], 6);
    assert_eq!(json["result"]["definitions"][1]["line"], 10);
    // The field of the same name is another symbol, not an overload: all
    // three are listed as matches.
    let all = codix(&root, &root, &["def", "Invoice.total"]);
    assert_eq!(all.code, Some(0), "{}", all.stderr);
    assert!(
        all.stdout
            .starts_with("src/billing/Invoice.java:4:18  field  Invoice.total"),
        "{}",
        all.stdout
    );
    assert!(all.stdout.ends_with("\n3 matches\n"), "{}", all.stdout);
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
fn refs_of_an_ambiguous_name_covers_every_match() {
    let (_guard, root) = indexed(&[
        (
            "src/A.java",
            "class A {\n    private void f() {}\n    void g() { f(); }\n}\n",
        ),
        (
            "src/B.java",
            "class B {\n    private void f() {}\n    void h() { f(); f(); }\n}\n",
        ),
    ]);
    let text = codix(&root, &root, &["refs", "f"]);
    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert!(
        text.stdout.starts_with(
            "2 matches, before the filters:\n  src/A.java:2:18  method  A.f\n  src/B.java:2:18  method  B.f\n"
        ),
        "{}",
        text.stdout
    );
    assert!(
        text.stdout
            .contains("5 results\n2 definitions, 3 scoped, 0 textual\n"),
        "{}",
        text.stdout
    );

    let json: Value =
        serde_json::from_str(&codix(&root, &root, &["refs", "f", "--json"]).stdout).expect("JSON");
    let result = &json["result"];
    let a = result["targets"][0]["symbol_id"].as_str().expect("A.f");
    let b = result["targets"][1]["symbol_id"].as_str().expect("B.f");
    let targets: Vec<(&str, &str)> = result["references"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|r| {
            (
                r["path"].as_str().unwrap_or_default(),
                r["target"].as_str().unwrap_or_default(),
            )
        })
        .collect();
    assert_eq!(
        targets,
        [
            ("src/A.java", a),
            ("src/A.java", a),
            ("src/B.java", b),
            ("src/B.java", b),
            ("src/B.java", b)
        ]
    );
}

#[test]
fn show_prints_every_match_after_listing_them() {
    let (_guard, root) = indexed(BILLING);
    let out = codix(&root, &root, &["show", "Invoice.total", "--kind", "method"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(
        out.stdout.starts_with(
            "2 matches:\n  src/billing/Invoice.java:6:17  method  Invoice.total\n  src/billing/Invoice.java:10:10  method  Invoice.total\n\n"
        ),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("public long total() {"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains("long total(long discount) {"),
        "{}",
        out.stdout
    );

    let one = codix(
        &root,
        &root,
        &["show", "Invoice.total", "--kind", "method", "--limit", "1"],
    );
    assert!(
        one.stdout
            .contains("  1 more; narrow with --kind or --path, or raise --limit"),
        "{}",
        one.stdout
    );
    assert!(
        !one.stdout.contains("long total(long discount) {"),
        "{}",
        one.stdout
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
fn outline_keeps_a_depth_or_kinds() {
    let (_guard, root) = indexed(BILLING);
    let top = codix(&root, &root, &["outline", "src/billing", "--depth", "1"]);
    assert_eq!(top.code, Some(0), "{}", top.stderr);
    assert!(!top.stdout.contains("method"), "{}", top.stdout);
    assert!(
        top.stdout.contains("class  public class Invoice"),
        "{}",
        top.stdout
    );
    let fields = codix(&root, &root, &["outline", "src/billing", "--kind", "field"]);
    // A field, and the class holding it; no method, and no Cart without one.
    assert!(
        fields.stdout.contains("field  private long total"),
        "{}",
        fields.stdout
    );
    assert!(
        fields.stdout.contains("class  public class Invoice"),
        "{}",
        fields.stdout
    );
    assert!(!fields.stdout.contains("method"), "{}", fields.stdout);
    assert!(!fields.stdout.contains("Cart"), "{}", fields.stdout);
}

#[test]
fn nothing_found_says_what_to_try() {
    let (_guard, root) = indexed(BILLING);
    let none = codix(&root, &root, &["find", "Zebra"]);
    assert_eq!(none.code, Some(1));
    assert!(
        none.stdout
            .ends_with("no symbol named \"Zebra\", even a few edits away\n"),
        "{}",
        none.stdout
    );
    let glob = codix(&root, &root, &["find", "Zebra.*", "--glob"]);
    assert!(
        glob.stdout
            .ends_with("no qualified name matches \"Zebra.*\"\n"),
        "{}",
        glob.stdout
    );
    let narrowed = codix(&root, &root, &["find", "Invoice", "--kind", "method"]);
    assert!(
        narrowed
            .stdout
            .contains("without them, find looks everywhere"),
        "{}",
        narrowed.stdout
    );
}

#[test]
fn at_names_the_enclosing_symbol() {
    check(
        "",
        &[
            "at",
            "src/billing/Invoice.java:7",
            "src/billing/Invoice.java:7:16",
            "src/billing/Cart.java:1:package billing;",
            "docs/notes.txt:1",
            "src/billing/Cart.java:99",
            "src/nowhere.java:3",
        ],
        "at-billing",
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

    // `at` rechecks the files it is given, even one no index has seen yet.
    write(
        &root,
        "src/billing/Tax.java",
        "class Tax { long rate() { return 20; } }\n",
    );
    let out = codix(&root, &root, &["at", "src/billing/Tax.java:1:18"]);
    assert_eq!(
        out.stdout, "src/billing/Tax.java:1:18  method  Tax.rate\n",
        "{}",
        out.stderr
    );
}

/// A method used in its own file and through another object in another one.
const REFS: &[(&str, &str)] = &[
    (
        "src/Invoice.java",
        "package billing;\n\npublic class Invoice {\n    private long total;\n\n    public long total() {\n        return total;\n    }\n\n    long withTax(long tax) {\n        long base = total();\n        return base + tax;\n    }\n}\n",
    ),
    (
        "src/Report.java",
        "package billing;\n\nclass Report {\n    long render(Invoice invoice) {\n        return invoice.total();\n    }\n}\n",
    ),
];

#[test]
fn refs_lists_scoped_references_then_textual_ones_elsewhere() {
    let (_guard, root) = indexed(REFS);
    let text = codix(&root, &root, &["refs", "Invoice.total", "--kind", "method"]);
    assert_eq!(text.code, Some(0), "{}", text.stderr);
    assert_golden(golden("refs-invoice-total.txt"), &text.stdout);
    let json = codix(
        &root,
        &root,
        &["refs", "Invoice.total", "--kind", "method", "--json"],
    );
    assert_golden(golden("refs-invoice-total.json"), &json.stdout);
}

#[test]
fn refs_keeps_one_certainty_and_the_definitions() {
    let (_guard, root) = indexed(REFS);
    let refs = |confidence| {
        codix(
            &root,
            &root,
            &[
                "refs",
                "Invoice.total",
                "--kind",
                "method",
                "--confidence",
                confidence,
            ],
        )
        .stdout
    };
    let textual = refs("textual");
    assert!(textual.starts_with("src/Invoice.java:6:17"), "{textual}");
    assert!(textual.contains("src/Report.java:5:24"), "{textual}");
    assert!(!textual.contains("Invoice.withTax"), "{textual}");
    assert!(
        textual.ends_with("2 results\n1 definition, 0 scoped, 1 textual\n"),
        "{textual}"
    );
    let scoped = refs("scoped");
    assert!(scoped.contains("Invoice.withTax"), "{scoped}");
    assert!(!scoped.contains("Report.java"), "{scoped}");
}

#[test]
fn refs_of_a_private_field_or_a_local_stay_in_their_file() {
    let (_guard, root) = indexed(REFS);
    let field = codix(&root, &root, &["refs", "Invoice.total", "--kind", "field"]);
    assert_eq!(
        field.stdout,
        "src/Invoice.java:4:18  def   -              scoped\n\
         src/Invoice.java:7:16  read  Invoice.total  scoped\n\
         2 results\n1 definition, 1 scoped, 0 textual\n"
    );
    let local = codix(&root, &root.join("src"), &["refs", "Invoice.java:11:14"]);
    assert_eq!(
        local.stdout,
        "Invoice.java:11:14  def   -                scoped\n\
         Invoice.java:12:16  read  Invoice.withTax  scoped\n\
         2 results\n1 definition, 1 scoped, 0 textual\n"
    );
}

#[test]
fn refs_sees_a_file_changed_without_index() {
    let (_guard, root) = indexed(REFS);
    write(
        &root,
        "src/Audit.java",
        "class Audit {\n    long check(Invoice i) { return i.total(); }\n}\n",
    );
    let out = codix(&root, &root, &["refs", "Invoice.total", "--kind", "method"]);
    assert!(
        out.stdout
            .lines()
            .any(|l| l.starts_with("src/Audit.java:2:38 ") && l.ends_with("textual")),
        "{}",
        out.stdout
    );
}

#[test]
fn def_at_a_name_follows_its_binding() {
    let (_guard, root) = indexed(REFS);
    let field = codix(&root, &root, &["def", "src/Invoice.java:7:16"]);
    assert!(
        field
            .stdout
            .starts_with("src/Invoice.java:4:18  field  Invoice.total"),
        "{}",
        field.stdout
    );
    let call = codix(&root, &root, &["def", "src/Invoice.java:11:21", "--json"]);
    let json: Value = serde_json::from_str(&call.stdout).unwrap();
    assert_eq!(json["result"]["definitions"][0]["kind"], "method");
    assert_eq!(json["result"]["definitions"][0]["confidence"], "scoped");

    let local = codix(&root, &root, &["def", "src/Invoice.java:12:16", "--json"]);
    let json: Value = serde_json::from_str(&local.stdout).unwrap();
    let definition = &json["result"]["definitions"][0];
    assert_eq!(
        (
            &definition["kind"],
            &definition["line"],
            &definition["symbol_id"]
        ),
        (
            &Value::from("variable"),
            &Value::from(11),
            &Value::from("src/Invoice.java:11:14")
        )
    );

    // Through another object, the name is looked up by name: every symbol
    // of that name.
    let other = codix(&root, &root, &["def", "src/Report.java:5:24"]);
    assert_eq!(other.code, Some(0), "{}", other.stderr);
    assert!(other.stdout.ends_with("\n2 matches\n"), "{}", other.stdout);
    let narrowed = codix(
        &root,
        &root,
        &["def", "src/Report.java:5:24", "--kind", "method"],
    );
    assert!(
        narrowed
            .stdout
            .starts_with("src/Invoice.java:6:17  method  Invoice.total"),
        "{}",
        narrowed.stdout
    );
}
