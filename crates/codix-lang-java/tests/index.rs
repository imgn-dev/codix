//! A scan with the pack indexer stores Java symbols and searches other files
//! as text.

use std::fs;

use codix_core::config::Config;
use codix_core::parse::PackIndexer;
use codix_core::project::Project;
use codix_core::scan::scan;
use codix_core::store::{FileStatus, Store};

use codix_lang_java as _;

#[test]
fn scanning_indexes_java_files_and_keeps_others_for_search() {
    let dir = tempfile::tempdir().unwrap();
    let root = fs::canonicalize(dir.path()).unwrap();
    fs::create_dir_all(root.join(".codix")).unwrap();
    fs::create_dir_all(root.join("src")).unwrap();
    fs::write(
        root.join("src/Invoice.java"),
        "class Invoice { long total() { return 0; } }\n",
    )
    .unwrap();
    fs::write(root.join("README"), "hello\n").unwrap();
    fs::write(root.join("logo.png"), b"\x89PNG\r\n\x1a\n\0\0").unwrap();

    let project = Project::discover(&root).unwrap().unwrap();
    let config = Config::load(&project).unwrap();
    let report = scan(&project, &config, &PackIndexer::new(&config)).unwrap();
    assert_eq!(report.indexed, 3, "{report:?}");

    let mut store = Store::open(&project.index_path()).unwrap();
    let snapshot = store.snapshot().unwrap();
    let files: Vec<(String, String, FileStatus)> = snapshot
        .files()
        .unwrap()
        .into_iter()
        .map(|f| (f.path, f.language, f.status))
        .collect();
    assert_eq!(
        files,
        [
            (
                "README".to_owned(),
                "text".to_owned(),
                FileStatus::SearchOnly
            ),
            (
                "logo.png".to_owned(),
                "text".to_owned(),
                FileStatus::Skipped
            ),
            (
                "src/Invoice.java".to_owned(),
                "java".to_owned(),
                FileStatus::Indexed
            ),
        ]
    );
    let names: Vec<String> = snapshot
        .symbols_in("src/Invoice.java")
        .unwrap()
        .into_iter()
        .map(|hit| hit.symbol.symbol_id)
        .collect();
    assert_eq!(names.len(), 2);
    assert!(
        names[0].starts_with("java:src/Invoice.java::Invoice#"),
        "{names:?}"
    );
    assert!(
        names[1].starts_with("java:src/Invoice.java::Invoice.total#"),
        "{names:?}"
    );
}
