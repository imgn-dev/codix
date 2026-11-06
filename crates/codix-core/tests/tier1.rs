//! Tier 1: a language without a `locals` query, here Java's grammar and
//! `tags` query under another name. Its references are whole-word matches.

use std::fs;

use codix_core::config::Config;
use codix_core::lock::FileGate;
use codix_core::op::{Context, Operation};
use codix_core::page::Page;
use codix_core::parse::PackIndexer;
use codix_core::project::{Project, ScopeRequest};
use codix_core::refs::{RefsOp, RefsRequest};
use codix_core::scan::scan;

codix_provider::register!(
    Language,
    "tier1",
    "tests/tier1/descriptor.toml",
    tree_sitter_java::LANGUAGE,
    "../codix-lang-java/queries/tags.scm"
);

#[test]
fn references_at_tier_1_are_whole_word_matches() {
    let dir = tempfile::tempdir().unwrap();
    let root = codix_core::project::canonicalize(dir.path()).unwrap();
    fs::create_dir_all(root.join(".codix")).unwrap();
    fs::write(
        root.join("a.t1"),
        "class Invoice {\n    long total() { return total + subtotal; }\n}\n",
    )
    .unwrap();
    fs::write(root.join("b.t1"), "// the total\n").unwrap();
    let project = Project::discover(&root).unwrap().unwrap();
    let config = Config::load(&project).unwrap();
    scan(&project, &config, &PackIndexer::new(&config), &FileGate).unwrap();

    let ctx = Context::discover(&root, None).unwrap();
    let response = RefsOp
        .run(
            &ctx,
            RefsRequest {
                symbol: "Invoice.total".to_owned(),
                kinds: Vec::new(),
                scope: ScopeRequest::default(),
                page: Page::default(),
            },
        )
        .unwrap();
    let found: Vec<(String, u32, u32, String)> = response
        .references
        .items
        .into_iter()
        .map(|r| {
            (
                r.location.path,
                r.location.line,
                r.location.column,
                r.confidence,
            )
        })
        .collect();
    assert_eq!(
        found,
        [
            ("a.t1".to_owned(), 2, 10, "scoped".to_owned()),
            ("a.t1".to_owned(), 2, 27, "textual".to_owned()),
            ("b.t1".to_owned(), 1, 8, "textual".to_owned()),
        ]
    );
}
