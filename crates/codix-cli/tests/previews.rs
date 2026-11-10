//! `previews` and `discard`: pending previews, their state and expiry.
#![cfg(with_lang_java)]

use std::path::Path;

use codix_core::edit::{Edit, EditSet, PreviewResponse, plan_file};
use codix_core::store::{PreviewRow, Store};
use codix_testkit::fixture::{codix_in, indexed, write};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const INVOICE: &str = "class Invoice {\n    long total() { return 0; }\n}\n";

/// Stores a preview renaming `total` in `Invoice.java`, planned `age`
/// seconds ago; its ID.
fn plan(root: &Path, age: u64) -> String {
    let start = INVOICE.find("total").expect("total");
    let edit = Edit {
        start,
        end: start + 5,
        replacement: "sum".to_owned(),
    };
    let now = codix_core::render::since_epoch();
    let mut row = PreviewRow {
        created: now - age,
        operation: "rename".to_owned(),
        request: "{}".to_owned(),
        edits: EditSet::new([("Invoice.java".to_owned(), edit.clone())]).expect("edits"),
        files: vec![plan_file("Invoice.java", INVOICE, &[edit])],
        response: PreviewResponse {
            id: String::new(),
            description: "rename method Invoice.total to sum".to_owned(),
            files: 1,
            sites: 1,
            overrides: Vec::new(),
            excluded: Vec::new(),
            ambiguous: Vec::new(),
            diff: String::new(),
        },
    };
    let mut store = Store::open(&root.join(".codix/index.db")).expect("index");
    store.insert_preview(&mut row).expect("insert");
    row.response.id
}

#[test]
fn previews_show_their_state_and_stale_or_expired_ones_go() {
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", INVOICE)]);
    let ready = plan(&root, 0);
    let expired = plan(&root, 2 * 3600);
    let out = codix_in(CODIX, &root, &root, &["previews"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(
        out.stdout.contains(&format!("{ready}  ready ")),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.contains(&format!("{expired}  expired, removed")),
        "{}",
        out.stdout
    );

    // A change outside the region: it rebases. Inside: it is stale.
    write(&root, "Invoice.java", &format!("// billing\n{INVOICE}"));
    let out = codix_in(CODIX, &root, &root, &["previews", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&out.stdout).expect("JSON");
    assert_eq!(json["result"]["previews"][0]["state"], "rebase");
    write(
        &root,
        "Invoice.java",
        &INVOICE.replace("total()", "total(int x)"),
    );
    let out = codix_in(CODIX, &root, &root, &["previews"]);
    assert!(out.stdout.contains("stale, removed"), "{}", out.stdout);
    let out = codix_in(CODIX, &root, &root, &["previews"]);
    assert_eq!(out.stdout, "no pending previews\n");
    assert_eq!(out.code, Some(1));
}

#[test]
fn expiry_follows_the_local_setting() {
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", INVOICE)]);
    plan(&root, 120);
    let out = codix_in(CODIX, &root, &root, &["config", "previews.expiry", "1m"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    let out = codix_in(CODIX, &root, &root, &["previews"]);
    assert!(out.stdout.contains("expired, removed"), "{}", out.stdout);
}

#[test]
fn discard_drops_named_or_all_previews() {
    let (_guard, root) = indexed(CODIX, &[("Invoice.java", INVOICE)]);
    let (a, b, c) = (plan(&root, 0), plan(&root, 0), plan(&root, 0));
    let out = codix_in(CODIX, &root, &root, &["discard", &a]);
    assert_eq!(out.stdout, format!("discarded {a}\n"));
    let out = codix_in(CODIX, &root, &root, &["discard", &a]);
    assert_eq!(out.code, Some(4));
    assert!(
        out.stderr.starts_with("error[unknown_preview]"),
        "{}",
        out.stderr
    );
    let out = codix_in(CODIX, &root, &root, &["discard", "--all"]);
    assert!(
        out.stdout.contains(&b) && out.stdout.contains(&c),
        "{}",
        out.stdout
    );
    let out = codix_in(CODIX, &root, &root, &["discard", &b, "--all"]);
    assert_eq!(out.code, Some(2), "{}", out.stderr);
}
