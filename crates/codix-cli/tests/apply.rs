//! `apply`: writing previews, rebasing them, refusing stale ones, and
//! rolling back an apply a crash interrupted.
#![cfg(with_lang_java)]

use std::fs;
use std::path::Path;

use codix_testkit::fixture::{codix_in, indexed, write};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const INVOICE: &str = "class Invoice {\n    long withTax(long tax) {\n        long base = 2;\n        return base + tax;\n    }\n}\n";

fn codix(root: &Path, args: &[&str]) -> codix_testkit::Run {
    codix_in(CODIX, root, root, args)
}

/// Plans renaming the local `base` to `net`; the preview ID.
fn plan(root: &Path) -> String {
    let out = codix(root, &["rename", "Invoice.java:3:14", "net"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    out.stdout
        .split_whitespace()
        .nth(1)
        .expect("an ID")
        .trim_end_matches(':')
        .to_owned()
}

fn read(root: &Path) -> String {
    fs::read_to_string(root.join("Invoice.java")).expect("read")
}

#[test]
fn apply_writes_the_edits_and_updates_the_index() {
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", INVOICE)]);
    let id = plan(&root);
    let out = codix(&root, &["apply", &id]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(
        out.stdout,
        format!(
            "applied {id}: rename variable base at Invoice.java:3:14 to net\n  1 file, 2 sites\n    Invoice.java\nundo with: codix undo\n"
        )
    );
    assert_eq!(read(&root), INVOICE.replace("base", "net"));
    let refs = codix(&root, &["refs", "Invoice.java:3:14"]);
    assert!(
        refs.stdout.starts_with("Invoice.java:3:14  def"),
        "{}",
        refs.stdout
    );
    assert_eq!(codix(&root, &["previews"]).stdout, "no pending previews\n");
    let again = codix(&root, &["apply", &id]);
    assert!(
        again.stderr.starts_with("error[unknown_preview]"),
        "{}",
        again.stderr
    );
}

#[test]
fn a_file_changed_outside_the_regions_is_rebased() {
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", INVOICE)]);
    let id = plan(&root);
    write(&root, "Invoice.java", &format!("// Billing.\n{INVOICE}"));
    let out = codix(&root, &["apply", &id]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(
        out.stdout
            .contains("(rebased onto changes made since planning)"),
        "{}",
        out.stdout
    );
    assert_eq!(
        read(&root),
        format!("// Billing.\n{}", INVOICE.replace("base", "net"))
    );
}

#[test]
fn a_changed_region_or_a_new_reference_is_stale() {
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", INVOICE)]);
    let id = plan(&root);
    write(
        &root,
        "Invoice.java",
        &INVOICE.replace("base + tax", "base - tax"),
    );
    let out = codix(&root, &["apply", &id]);
    assert_eq!(out.code, Some(4), "{}", out.stdout);
    assert!(
        out.stderr.starts_with("error[stale_preview]"),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr.contains("Invoice.java:4:1  return base - tax;"),
        "{}",
        out.stderr
    );

    write(&root, "Invoice.java", INVOICE);
    let id = plan(&root);
    write(
        &root,
        "Invoice.java",
        &INVOICE.replace(
            "return base + tax;",
            "long twice = base * 2;\n        return base + tax;",
        ),
    );
    let out = codix(&root, &["apply", &id]);
    assert!(
        out.stderr.starts_with("error[stale_preview]"),
        "{}",
        out.stderr
    );
}

#[test]
fn an_apply_a_crash_interrupts_is_rolled_back_by_the_next_writer() {
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", INVOICE)]);
    let id = plan(&root);
    let crashed = codix_testkit::run_with_env(
        &root,
        CODIX,
        &["apply", &id],
        &[(codix_core::apply::CRASH_AFTER_VAR, "1")],
    );
    assert_ne!(crashed.code, Some(0));
    // The file was replaced whole, then the process died.
    assert_eq!(read(&root), INVOICE.replace("base", "net"));
    let out = codix(&root, &["index"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(read(&root), INVOICE);
    assert!(
        fs::read_dir(&root).expect("list").all(|e| !e
            .expect("entry")
            .file_name()
            .to_string_lossy()
            .ends_with(".tmp")),
        "a temp file was left"
    );
}

#[test]
fn an_expired_preview_is_refused() {
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", INVOICE)]);
    let id = plan(&root);
    fs::write(
        root.join(".codix/config.toml"),
        "[previews]\nexpiry = \"0s\"\n",
    )
    .expect("config");
    std::thread::sleep(std::time::Duration::from_millis(1100));
    let out = codix(&root, &["apply", &id]);
    assert!(
        out.stderr.starts_with("error[preview_expired]"),
        "{}",
        out.stderr
    );
    assert_eq!(read(&root), INVOICE);
}
