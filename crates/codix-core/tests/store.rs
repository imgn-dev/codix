//! The index store: migrations, per-file replacement, snapshots and name search.

use std::path::Path;

use codix_core::store::{
    Confidence, FileRow, FileStatus, FileUpdate, MatchMode, NameQuery, Point, Span, Store,
    SymbolRow,
};

fn file(path: &str) -> FileRow {
    FileRow {
        path: path.to_owned(),
        size: 10,
        mtime_ns: 1_700_000_000_000_000_000,
        inode: 42,
        hash: [7; 32],
        language: "java".to_owned(),
        parser_version: "java-1".to_owned(),
        status: FileStatus::Indexed,
    }
}

fn point(line: u32) -> Point {
    Point {
        line,
        column: 5,
        byte: line * 10,
    }
}

fn symbol(name: &str, qualified: &str, line: u32) -> SymbolRow {
    SymbolRow {
        symbol_id: format!("java:x::{qualified}#0000"),
        name: name.to_owned(),
        kind: "method".to_owned(),
        qualified_name: qualified.to_owned(),
        parent: None,
        range: Span {
            start: point(line),
            end: point(line + 1),
        },
        name_range: Span {
            start: point(line),
            end: Point {
                line,
                column: 5 + u32::try_from(name.chars().count()).expect("test setup"),
                byte: line * 10 + u32::try_from(name.len()).expect("test setup"),
            },
        },
        signature: Some(format!("void {name}()")),
        visibility: Some("public".to_owned()),
        confidence: Confidence::Normal,
    }
}

fn update(path: &str, symbols: Vec<SymbolRow>) -> FileUpdate {
    FileUpdate {
        file: file(path),
        symbols: Some(symbols),
    }
}

fn open(dir: &Path) -> Store {
    Store::open(&dir.join("index.db")).expect("test setup")
}

#[test]
fn migrates_an_empty_database_and_reopens() {
    let dir = tempfile::tempdir().unwrap();
    assert_eq!(open(dir.path()).schema_version().unwrap(), 1);
    assert_eq!(open(dir.path()).schema_version().unwrap(), 1);
}

#[test]
fn replace_file_is_idempotent_and_round_trips() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(dir.path());
    let u = update(
        "src/A.java",
        vec![
            symbol("total", "Invoice.total", 3),
            symbol("tax", "Invoice.tax", 7),
        ],
    );
    store.replace_file(&u).unwrap();
    store.replace_file(&u).unwrap();

    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.files().unwrap(), std::slice::from_ref(&u.file));
    let stored: Vec<SymbolRow> = snapshot
        .symbols_in("src/A.java")
        .unwrap()
        .into_iter()
        .map(|hit| hit.symbol)
        .collect();
    assert_eq!(Some(stored), u.symbols);
    assert_eq!(snapshot.symbol_count().unwrap(), 2);
}

#[test]
fn replacing_a_file_drops_its_old_symbols_only() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(dir.path());
    store
        .commit(
            &[
                update("a.java", vec![symbol("old", "A.old", 1)]),
                update("b.java", vec![symbol("kept", "B.kept", 1)]),
            ],
            &[],
        )
        .unwrap();
    store
        .replace_file(&update("a.java", vec![symbol("new", "A.new", 1)]))
        .unwrap();

    let snapshot = store.snapshot().unwrap();
    let names = |q: &str| -> Vec<String> {
        snapshot
            .find(&NameQuery {
                text: q.to_owned(),
                mode: MatchMode::Exact,
            })
            .unwrap()
            .into_iter()
            .map(|h| h.symbol.qualified_name)
            .collect()
    };
    assert_eq!(names("old"), Vec::<String>::new());
    assert_eq!(names("new"), ["A.new"]);
    assert_eq!(names("kept"), ["B.kept"]);
}

#[test]
fn deleting_a_file_removes_its_symbols() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = open(dir.path());
    store
        .replace_file(&update("a.java", vec![symbol("x", "A.x", 1)]))
        .unwrap();
    store.commit(&[], &["a.java".to_owned()]).unwrap();
    let snapshot = store.snapshot().unwrap();
    assert_eq!(snapshot.files().unwrap(), Vec::<FileRow>::new());
    assert_eq!(snapshot.symbol_count().unwrap(), 0);
}

#[test]
fn a_reader_snapshot_ignores_a_concurrent_large_commit() {
    let dir = tempfile::tempdir().unwrap();
    let mut writer = open(dir.path());
    writer
        .replace_file(&update("seed.java", vec![symbol("seed", "S.seed", 1)]))
        .unwrap();

    let mut reader = open(dir.path());
    let snapshot = reader.snapshot().unwrap();
    assert_eq!(snapshot.symbol_count().unwrap(), 1);

    let db = dir.path().to_path_buf();
    std::thread::spawn(move || {
        let mut writer = open(&db);
        let updates: Vec<FileUpdate> = (0..100)
            .map(|f| {
                update(
                    &format!("gen/F{f}.java"),
                    (0..100)
                        .map(|s| symbol(&format!("m{s}"), &format!("F{f}.m{s}"), s + 1))
                        .collect(),
                )
            })
            .collect();
        writer.commit(&updates, &[]).unwrap();
    })
    .join()
    .unwrap();

    assert_eq!(snapshot.symbol_count().unwrap(), 1, "same snapshot");
    drop(snapshot);
    assert_eq!(
        reader.snapshot().unwrap().symbol_count().unwrap(),
        10_001,
        "new snapshot"
    );
}

fn search_store(dir: &Path) -> Store {
    let mut store = open(dir);
    store
        .commit(
            &[
                update(
                    "billing/Invoice.java",
                    vec![
                        symbol("Invoice", "Invoice", 1),
                        symbol("total", "Invoice.total", 3),
                        symbol("totalWithTax", "Invoice.totalWithTax", 5),
                    ],
                ),
                update(
                    "billing/Cart.java",
                    vec![symbol("Cart", "Cart", 1), symbol("total", "Cart.total", 2)],
                ),
                update(
                    "web/Report.java",
                    vec![symbol("build_report", "Report.build_report", 1)],
                ),
            ],
            &[],
        )
        .expect("test setup");
    store
}

fn qualified(store: &mut Store, text: &str, mode: MatchMode) -> Vec<String> {
    let snapshot = store.snapshot().expect("test setup");
    let mut names: Vec<String> = snapshot
        .find(&NameQuery {
            text: text.to_owned(),
            mode,
        })
        .expect("test setup")
        .into_iter()
        .map(|h| h.symbol.qualified_name)
        .collect();
    if mode != MatchMode::Fuzzy {
        names.sort();
    }
    names
}

#[test]
fn exact_match_is_case_sensitive() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = search_store(dir.path());
    assert_eq!(
        qualified(&mut store, "total", MatchMode::Exact),
        ["Cart.total", "Invoice.total"]
    );
    assert_eq!(
        qualified(&mut store, "Total", MatchMode::Exact),
        Vec::<String>::new()
    );
}

#[test]
fn prefix_match_ignores_case_and_escapes_wildcards() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = search_store(dir.path());
    assert_eq!(
        qualified(&mut store, "TOT", MatchMode::Prefix),
        ["Cart.total", "Invoice.total", "Invoice.totalWithTax"]
    );
    assert_eq!(
        qualified(&mut store, "to", MatchMode::Prefix).len(),
        3,
        "under a trigram"
    );
    assert_eq!(
        qualified(&mut store, "build_", MatchMode::Prefix),
        ["Report.build_report"]
    );
    assert_eq!(
        qualified(&mut store, "build%", MatchMode::Prefix),
        Vec::<String>::new()
    );
}

#[test]
fn glob_matches_qualified_names() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = search_store(dir.path());
    assert_eq!(
        qualified(&mut store, "Invoice.*", MatchMode::Glob),
        ["Invoice.total", "Invoice.totalWithTax"]
    );
    assert_eq!(
        qualified(&mut store, "*.total", MatchMode::Glob),
        ["Cart.total", "Invoice.total"]
    );
}

#[test]
fn fuzzy_finds_typos_closest_first() {
    let dir = tempfile::tempdir().unwrap();
    let mut store = search_store(dir.path());
    assert_eq!(
        qualified(&mut store, "Invioce", MatchMode::Fuzzy),
        ["Invoice"]
    );
    assert_eq!(
        qualified(&mut store, "totl", MatchMode::Fuzzy),
        ["Cart.total", "Invoice.total"]
    );
    assert_eq!(
        qualified(&mut store, "zzzzzz", MatchMode::Fuzzy),
        Vec::<String>::new()
    );
}
