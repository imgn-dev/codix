//! Symbol aliases: an ID from before a rename codix applied still leads to
//! the symbol for queries, and refuses edits.
#![cfg(feature = "lang-java")]

use std::path::Path;

use codix_testkit::fixture::{codix_in, indexed};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn codix(root: &Path, args: &[&str]) -> codix_testkit::Run {
    codix_in(CODIX, root, root, args)
}

fn id_of(root: &Path, symbol: &str) -> String {
    let out = codix(root, &["def", symbol, "--json"]);
    let json: Value = serde_json::from_str(&out.stdout).expect("JSON");
    json["result"]["definition"]["symbol_id"]
        .as_str()
        .expect("an ID")
        .to_owned()
}

#[test]
fn an_id_from_before_a_rename_still_leads_to_the_symbol() {
    let (_guard, root) = indexed(
        CODIX,
        &[(
            "Invoice.java",
            "class Invoice {\n    private long total() { return 0; }\n    long sum() { return total(); }\n}\n",
        )],
    );
    let old = id_of(&root, "Invoice.total");
    let planned = codix(&root, &["rename", &old, "amount"]);
    assert_eq!(planned.code, Some(0), "{}", planned.stderr);
    let preview = planned
        .stdout
        .split_whitespace()
        .nth(1)
        .unwrap()
        .trim_end_matches(':');
    let applied = codix(&root, &["apply", preview]);
    assert_eq!(applied.code, Some(0), "{}", applied.stderr);
    let new = id_of(&root, "Invoice.amount");

    let def = codix(&root, &["def", &old]);
    assert_eq!(def.code, Some(0), "{}", def.stderr);
    assert!(
        def.stdout
            .contains(&format!("note: {old} was renamed to {new}")),
        "{}",
        def.stdout
    );
    let refs = codix(&root, &["refs", &old]);
    assert!(refs.stdout.contains("Invoice.java:3:25"), "{}", refs.stdout);
    let show = codix(&root, &["show", &old]);
    assert!(show.stdout.contains("amount()"), "{}", show.stdout);

    let edit = codix(&root, &["rename", &old, "value"]);
    assert_eq!(edit.code, Some(4));
    assert!(
        edit.stderr.starts_with("error[symbol_renamed]"),
        "{}",
        edit.stderr
    );
    assert!(edit.stderr.contains(&new), "{}", edit.stderr);
}
