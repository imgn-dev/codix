//! `rename` within a file: previews, refusals and overrides.
#![cfg(feature = "lang-java")]

use codix_testkit::assert_golden;
use codix_testkit::fixture::{codix_in, indexed};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

/// A method used in its file and through another object in another file.
const FILES: &[(&str, &str)] = &[
    (
        "src/Invoice.java",
        "package billing;\n\n/** The total of an invoice. */\npublic class Invoice {\n    private long total;\n\n    public long total() {\n        return total;\n    }\n\n    long withTax(long tax) {\n        long base = total();\n        return base + tax;\n    }\n\n    long total(long discount) {\n        return total - discount;\n    }\n}\n",
    ),
    (
        "src/Report.java",
        "package billing;\n\nclass Report {\n    long render(Invoice invoice) {\n        return invoice.withTax(1);\n    }\n}\n",
    ),
    (
        "src/Cart.java",
        "package billing;\n\nclass Cart {\n    Cart() {}\n\n    Cart(int size) {}\n\n    static Cart empty() {\n        return new Cart();\n    }\n}\n",
    ),
];

fn golden(name: &str) -> String {
    format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"))
}

fn codix(root: &std::path::Path, args: &[&str]) -> codix_testkit::Run {
    codix_in(CODIX, root, root, args)
}

#[test]
fn a_local_renames_its_definition_and_references() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["rename", "src/Invoice.java:12:14", "net"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    let id = out
        .stdout
        .split_whitespace()
        .nth(1)
        .unwrap()
        .trim_end_matches(':');
    let text = out.stdout.replace(id, "<id>");
    assert_golden(golden("rename-local.txt"), &text);
    let listed = codix(&root, &["previews"]);
    assert!(listed.stdout.starts_with(id), "{}", listed.stdout);
}

#[test]
fn unresolved_references_refuse_unless_excluded() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["rename", "Invoice.withTax", "withVat"]);
    assert_eq!(out.code, Some(4), "{}", out.stdout);
    assert_golden(golden("rename-unresolved.txt"), &out.stderr);
    let out = codix(
        &root,
        &[
            "rename",
            "Invoice.withTax",
            "withVat",
            "--exclude-unresolved",
        ],
    );
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(
        out.stdout.contains("  override --exclude-unresolved: references codix cannot bind, left unchanged\n    src/Report.java:5:24\n"),
        "{}",
        out.stdout
    );
}

#[test]
fn a_private_field_stays_in_its_file_and_mentions_are_listed() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(
        &root,
        &["rename", "Invoice.total", "amount", "--kind", "field"],
    );
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(out.stdout.contains("  1 file, 3 sites\n"), "{}", out.stdout);
    assert!(
        out.stdout.contains(
            "not changed, 1 mention:\n    src/Invoice.java:3:9  /** The total of an invoice. */"
        ),
        "{}",
        out.stdout
    );
}

#[test]
fn one_overload_needs_all_overloads() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(
        &root,
        &["rename", "Invoice.total", "sum", "--kind", "method"],
    );
    assert_eq!(out.code, Some(4));
    assert!(
        out.stderr.starts_with("error[tier_too_low]"),
        "{}",
        out.stderr
    );
    let out = codix(
        &root,
        &[
            "rename",
            "Invoice.total",
            "sum",
            "--kind",
            "method",
            "--all-overloads",
            "--exclude-unresolved",
        ],
    );
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(out.stdout.contains("  1 file, 3 sites\n"), "{}", out.stdout);
}

#[test]
fn a_public_class_is_named_after_its_file() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["rename", "Invoice", "Bill", "--kind", "class"]);
    assert_eq!(out.code, Some(4));
    assert!(
        out.stderr.starts_with("error[renames_file]"),
        "{}",
        out.stderr
    );
}

#[test]
fn a_type_renames_its_constructors() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["rename", "Cart", "Basket", "--kind", "class"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(out.stdout.contains("  1 file, 5 sites\n"), "{}", out.stdout);
    assert!(!out.stdout.contains("+    Cart"), "{}", out.stdout);
    let by_constructor = codix(&root, &["rename", "src/Cart.java:4:5", "Basket"]);
    assert!(
        by_constructor
            .stdout
            .contains("rename class Cart to Basket"),
        "{}",
        by_constructor.stdout
    );
}

#[test]
fn the_same_name_or_an_invalid_one_is_refused() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["rename", "src/Invoice.java:12:14", "base"]);
    assert_eq!(out.code, Some(2), "{}", out.stderr);
    let out = codix(&root, &["rename", "src/Invoice.java:12:14", "class"]);
    assert!(
        out.stderr.starts_with("error[invalid_name]"),
        "{}",
        out.stderr
    );
}

#[cfg(feature = "lang-kotlin")]
#[test]
fn a_definition_named_by_a_keyword_is_not_renamed() {
    let (_guard, root) = indexed(
        CODIX,
        &[(
            "Cart.kt",
            "class Cart(val size: Int) {\n    constructor() : this(0)\n}\n",
        )],
    );
    let out = codix(&root, &["rename", "Cart.kt:2:5", "make"]);
    assert_eq!(out.code, Some(4), "{}", out.stdout);
    assert!(
        out.stderr.starts_with("error[invalid_name]"),
        "{}",
        out.stderr
    );
}
