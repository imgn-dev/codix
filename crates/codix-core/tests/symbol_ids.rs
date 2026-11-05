//! Symbol IDs on real Java: what keeps them and what changes them, and every
//! way a command can name a symbol.

use std::fs;
use std::path::{Path, PathBuf};

use codix_core::config::Config;
use codix_core::parse::{PackIndexer, extract};
use codix_core::project::Project;
use codix_core::scan::scan;
use codix_core::store::Store;
use codix_core::symbol_id::resolve;
use codix_provider::registry;

use codix_lang_java as _;

fn ids(path: &str, content: &str) -> Vec<String> {
    let registry = registry();
    let descriptor = registry
        .languages
        .iter()
        .find(|d| d.name == "java")
        .expect("java pack");
    let grammar = registry.grammars.get("java").expect("java grammar");
    extract(descriptor, grammar, path, content)
        .expect("extract")
        .into_iter()
        .map(|s| s.symbol_id)
        .collect()
}

fn id_of(path: &str, content: &str, qualified: &str) -> String {
    ids(path, content)
        .into_iter()
        .find(|id| id.contains(&format!("::{qualified}#")))
        .expect("symbol")
}

const INVOICE: &str = "class Invoice {\n    long total() { return 1; }\n    void other() {}\n}\n";

#[test]
fn ids_survive_body_edits_and_edits_elsewhere() {
    let base = id_of("src/Invoice.java", INVOICE, "Invoice.total");
    let body = INVOICE.replace("return 1;", "long sum = 2;\n        return sum;");
    assert_eq!(id_of("src/Invoice.java", &body, "Invoice.total"), base);
    let elsewhere = format!(
        "// header\n\n{}",
        INVOICE.replace("void other() {}", "void other(int x) {}")
    );
    assert_eq!(id_of("src/Invoice.java", &elsewhere, "Invoice.total"), base);
    assert!(
        base.starts_with("java:src/Invoice.java::Invoice.total#"),
        "{base}"
    );
}

#[test]
fn ids_change_with_rename_move_and_signature() {
    let base = id_of("src/Invoice.java", INVOICE, "Invoice.total");
    let renamed = INVOICE.replace("total", "sum");
    assert!(!ids("src/Invoice.java", &renamed).contains(&base));
    assert_ne!(
        id_of("src/billing/Invoice.java", INVOICE, "Invoice.total"),
        base
    );
    let signature = INVOICE.replace("long total()", "long total(int scale)");
    assert_ne!(id_of("src/Invoice.java", &signature, "Invoice.total"), base);
}

#[test]
fn overloads_have_distinct_ids() {
    let source = "class A {\n    void f(int x) {}\n    void f(String s) {}\n}\n";
    let all: Vec<String> = ids("A.java", source)
        .into_iter()
        .filter(|id| id.contains("::A.f#"))
        .collect();
    assert_eq!(all.len(), 2);
    assert_ne!(all[0], all[1]);
}

fn project() -> (tempfile::TempDir, PathBuf, Project) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = codix_core::project::canonicalize(dir.path()).expect("canonical temp dir");
    fs::create_dir_all(root.join(".codix")).expect("create .codix");
    fs::create_dir_all(root.join("src/billing")).expect("create src");
    fs::write(
        root.join("src/billing/Invoice.java"),
        "class Invoice {\n    long total() { return 0; }\n    long total(long d) { return d; }\n    long tax() { return 0; }\n}\n",
    )
    .expect("write Invoice");
    fs::write(
        root.join("src/billing/Cart.java"),
        "class Cart {\n    long tax() { return 0; }\n}\n",
    )
    .expect("write Cart");
    let project = Project::discover(&root)
        .expect("discover")
        .expect("project");
    let config = Config::load(&project).expect("config");
    scan(&project, &config, &PackIndexer::new(&config)).expect("scan");
    (dir, root, project)
}

fn named(project: &Project, cwd: &Path, input: &str) -> Result<String, String> {
    let mut store = Store::open(&project.index_path()).expect("test setup");
    let snapshot = store.snapshot().expect("test setup");
    resolve(&snapshot, project, cwd, input)
        .map(|hit| hit.symbol.qualified_name)
        .map_err(|e| e.code().to_owned())
}

#[test]
fn every_input_form_names_a_symbol() {
    let (_guard, root, p) = project();
    let tax = {
        let mut store = Store::open(&p.index_path()).unwrap();
        let snapshot = store.snapshot().unwrap();
        snapshot.symbols_in("src/billing/Cart.java").unwrap()[1]
            .symbol
            .symbol_id
            .clone()
    };
    assert_eq!(named(&p, &root, &tax), Ok("Cart.tax".to_owned()), "full ID");
    let path_part = tax.rsplit_once('#').unwrap().0;
    assert_eq!(
        named(&p, &root, path_part),
        Ok("Cart.tax".to_owned()),
        "path part"
    );
    assert_eq!(
        named(&p, &root, "Cart.tax"),
        Ok("Cart.tax".to_owned()),
        "qualified name"
    );
    assert_eq!(
        named(&p, &root, "Cart"),
        Ok("Cart".to_owned()),
        "simple name"
    );
    assert_eq!(
        named(&p, &root.join("src/billing"), "Invoice.java:4:10"),
        Ok("Invoice.tax".to_owned()),
        "file:line:col from a subdirectory"
    );
    assert_eq!(
        named(&p, &root, "src/billing/Invoice.java:1:3"),
        Ok("Invoice".to_owned())
    );
}

#[test]
fn several_matches_are_ambiguous_and_none_unknown() {
    let (_guard, root, p) = project();
    assert_eq!(named(&p, &root, "tax"), Err("ambiguous_symbol".to_owned()));
    assert_eq!(
        named(&p, &root, "java:src/billing/Invoice.java::Invoice.total"),
        Err("ambiguous_symbol".to_owned()),
        "a path part shared by overloads"
    );
    assert_eq!(
        named(&p, &root, "Invoice.nothing"),
        Err("unknown_symbol".to_owned())
    );
    assert_eq!(
        named(&p, &root, "java:src/billing/Invoice.java::Invoice.tax#ffff"),
        Err("unknown_symbol".to_owned())
    );

    let mut store = Store::open(&p.index_path()).unwrap();
    let snapshot = store.snapshot().unwrap();
    let error = resolve(&snapshot, &p, &root, "tax").unwrap_err();
    assert_eq!(error.exit_code(), 3);
    assert_eq!(error.candidates().len(), 2);
}
