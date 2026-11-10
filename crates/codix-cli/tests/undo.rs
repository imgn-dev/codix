//! `undo`: previews reverting applies, newest first.
#![cfg(with_lang_java)]

use std::fs;
use std::path::Path;

use codix_testkit::fixture::{codix_in, indexed, write};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const INVOICE: &str = "class Invoice {\n    private long total() { return 0; }\n\n    long withTax(long tax) {\n        long base = total();\n        return base + tax;\n    }\n}\n";

fn codix(root: &Path, args: &[&str]) -> codix_testkit::Run {
    codix_in(CODIX, root, root, args)
}

/// Runs an edit command and applies its preview.
fn plan_and_apply(root: &Path, args: &[&str]) {
    let planned = codix(root, args);
    assert_eq!(planned.code, Some(0), "{}", planned.stderr);
    let id = planned
        .stdout
        .split_whitespace()
        .nth(1)
        .expect("an ID")
        .trim_end_matches(':')
        .to_owned();
    let applied = codix(root, &["apply", &id]);
    assert_eq!(applied.code, Some(0), "{}", applied.stderr);
}

fn read(root: &Path) -> String {
    fs::read_to_string(root.join("Invoice.java")).expect("read")
}

#[test]
fn undo_walks_back_through_applies_and_restores_ids() {
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", INVOICE)]);
    plan_and_apply(&root, &["rename", "Invoice.total", "amount"]);
    plan_and_apply(&root, &["rename", "Invoice.java:5:14", "net"]);
    let after_both = read(&root);
    assert!(after_both.contains("long net = amount();"), "{after_both}");

    let undo = codix(&root, &["undo"]);
    assert!(
        undo.stdout
            .contains("undo rename variable base at Invoice.java:5:14 to net"),
        "{}",
        undo.stdout
    );
    plan_and_apply(&root, &["undo"]);
    assert!(read(&root).contains("long base = amount();"));
    plan_and_apply(&root, &["undo"]);
    assert_eq!(read(&root), INVOICE);

    // The alias left by the first rename is gone with it.
    let def = codix(&root, &["def", "Invoice.total"]);
    assert_eq!(def.code, Some(0), "{}", def.stderr);
    assert!(!def.stdout.contains("renamed"), "{}", def.stdout);

    let nothing = codix(&root, &["undo"]);
    assert_eq!(nothing.code, Some(1), "{}", nothing.stdout);
    assert!(
        nothing.stderr.starts_with("error[nothing_to_undo]"),
        "{}",
        nothing.stderr
    );
}

#[test]
fn undo_is_refused_once_the_files_changed() {
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", INVOICE)]);
    plan_and_apply(&root, &["rename", "Invoice.java:5:14", "net"]);
    write(
        &root,
        "Invoice.java",
        &format!("{}// edited\n", read(&root)),
    );
    let out = codix(&root, &["undo"]);
    assert_eq!(out.code, Some(4), "{}", out.stdout);
    assert!(
        out.stderr.starts_with("error[stale_preview]"),
        "{}",
        out.stderr
    );
}

#[test]
fn an_apply_rolled_back_after_a_crash_is_not_undone() {
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", INVOICE)]);
    let planned = codix(&root, &["rename", "Invoice.java:5:14", "net"]);
    let id = planned
        .stdout
        .split_whitespace()
        .nth(1)
        .unwrap()
        .trim_end_matches(':');
    let crashed = codix_testkit::run_with_env(
        &root,
        CODIX,
        &["apply", id],
        &[(codix_core::apply::CRASH_AFTER_VAR, "1")],
    );
    assert_ne!(crashed.code, Some(0));
    assert_eq!(codix(&root, &["index"]).code, Some(0));
    assert_eq!(read(&root), INVOICE);
    assert_eq!(codix(&root, &["undo"]).code, Some(1));
}
