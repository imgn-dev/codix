//! Change detection: touches, racy timestamps, renames, deletions, swaps,
//! parser versions and concurrent scans.

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::{Duration, SystemTime};

use codix_core::config::Config;
use codix_core::lock::FileGate;
use codix_core::project::{FileId, Project};
use codix_core::scan::{Indexed, Indexer, ScanReport, Targets, rescan, scan, scan_dirs};
use codix_core::store::{Confidence, FileRow, FileStatus, Point, Span, Store, SymbolRow};
use tempfile::TempDir;

/// One symbol per `fn NAME` line; counts the files it parses.
struct LineIndexer {
    version: &'static str,
    parsed: AtomicUsize,
}

impl LineIndexer {
    fn new(version: &'static str) -> Self {
        Self {
            version,
            parsed: AtomicUsize::new(0),
        }
    }

    fn parsed(&self) -> usize {
        self.parsed.swap(0, Ordering::SeqCst)
    }
}

impl Indexer for LineIndexer {
    fn parser_version(&self, _: &str) -> String {
        self.version.to_owned()
    }

    fn index(&self, id: &FileId, content: &[u8]) -> Indexed {
        self.parsed.fetch_add(1, Ordering::SeqCst);
        let text = String::from_utf8_lossy(content);
        let symbols = text
            .lines()
            .enumerate()
            .filter_map(|(i, line)| line.strip_prefix("fn ").map(|name| (i, name)))
            .map(|(i, name)| {
                let line = u32::try_from(i + 1).expect("line fits");
                let point = Point {
                    line,
                    column: 1,
                    byte: 0,
                };
                SymbolRow {
                    symbol_id: format!("test:{}::{name}#0000", id.as_str()),
                    name: name.to_owned(),
                    kind: "function".to_owned(),
                    qualified_name: name.to_owned(),
                    parent: None,
                    range: Span {
                        start: point,
                        end: point,
                    },
                    name_range: Span {
                        start: point,
                        end: point,
                    },
                    signature: None,
                    visibility: None,
                    confidence: Confidence::Normal,
                }
            })
            .collect();
        Indexed {
            language: "test".to_owned(),
            status: FileStatus::Indexed,
            symbols,
            refs: Vec::new(),
        }
    }
}

fn temp() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = codix_core::project::canonicalize(dir.path()).expect("canonical temp dir");
    fs::create_dir(root.join(".codix")).expect("create .codix");
    (dir, root)
}

/// Writes a file with an mtime `age` in the past, out of the racy window.
fn write(root: &Path, path: &str, content: &str, age: Duration) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(&path, content).expect("write file");
    set_age(&path, age);
}

fn set_age(path: &Path, age: Duration) {
    File::options()
        .write(true)
        .open(path)
        .and_then(|f| f.set_modified(SystemTime::now() - age))
        .expect("set mtime");
}

const OLD: Duration = Duration::from_hours(1);

fn run(root: &Path, indexer: &LineIndexer) -> ScanReport {
    let project = Project::discover(root).expect("discover").expect("project");
    let config = Config::load(&project).expect("config");
    scan(&project, &config, indexer, &FileGate).expect("scan")
}

fn symbols(root: &Path) -> Vec<(String, String)> {
    let mut store = Store::open(&root.join(".codix/index.db")).expect("open index");
    let snapshot = store.snapshot().expect("snapshot");
    let files: Vec<FileRow> = snapshot.files().expect("files");
    let mut all = Vec::new();
    for file in files {
        for hit in snapshot.symbols_in(&file.path).expect("symbols") {
            all.push((hit.path, hit.symbol.name));
        }
    }
    all
}

/// A partial scan of `targets`, as the daemon runs on watcher events.
fn run_dirs(root: &Path, indexer: &LineIndexer, targets: &Targets) -> ScanReport {
    let project = Project::discover(root).expect("discover").expect("project");
    let config = Config::load(&project).expect("config");
    scan_dirs(&project, &config, indexer, &FileGate, targets, None)
        .expect("scan")
        .expect("free gate")
}

fn targets(dirs: &[&str], trees: &[&str]) -> Targets {
    let mut targets = Targets::default();
    for dir in dirs {
        targets.add_dir(dir);
    }
    for tree in trees {
        targets.add_tree(tree);
    }
    targets
}

fn counts(r: &ScanReport) -> (usize, usize, usize, usize) {
    (r.indexed, r.touched, r.deleted, r.unchanged)
}

#[test]
fn a_second_scan_without_changes_reads_nothing() {
    let (_guard, root) = temp();
    write(&root, "src/a.rs", "fn alpha\n", OLD);
    write(&root, "src/b.rs", "fn beta\n", OLD);
    let indexer = LineIndexer::new("v1");
    assert_eq!(counts(&run(&root, &indexer)), (2, 0, 0, 0));
    assert_eq!(indexer.parsed(), 2);
    assert_eq!(counts(&run(&root, &indexer)), (0, 0, 0, 2));
    assert_eq!(indexer.parsed(), 0);
    assert_eq!(
        symbols(&root),
        [
            ("src/a.rs".into(), "alpha".into()),
            ("src/b.rs".into(), "beta".into())
        ]
    );
}

#[test]
fn a_touch_without_change_updates_stats_only() {
    let (_guard, root) = temp();
    write(&root, "a.rs", "fn alpha\n", OLD);
    let indexer = LineIndexer::new("v1");
    run(&root, &indexer);
    indexer.parsed();
    set_age(&root.join("a.rs"), Duration::from_mins(30));
    assert_eq!(counts(&run(&root, &indexer)), (0, 1, 0, 0));
    assert_eq!(indexer.parsed(), 0);
    assert_eq!(counts(&run(&root, &indexer)), (0, 0, 0, 1));
}

#[test]
fn a_change_within_the_timestamp_granularity_is_caught_next_scan() {
    let (_guard, root) = temp();
    let path = root.join("a.rs");
    fs::write(&path, "fn alpha\n").unwrap();
    let mtime = fs::metadata(&path).unwrap().modified().unwrap();
    let indexer = LineIndexer::new("v1");
    run(&root, &indexer);

    // Same size, same mtime: only the racy marker tells the scan to look.
    fs::write(&path, "fn gamma\n").unwrap();
    File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(mtime)
        .unwrap();
    let report = run(&root, &indexer);
    assert_eq!(report.indexed, 1, "{report:?}");
    assert_eq!(symbols(&root), [("a.rs".into(), "gamma".into())]);
}

#[test]
fn a_rename_deletes_the_old_path_and_indexes_the_new() {
    let (_guard, root) = temp();
    write(&root, "old.rs", "fn alpha\n", OLD);
    let indexer = LineIndexer::new("v1");
    run(&root, &indexer);
    fs::rename(root.join("old.rs"), root.join("new.rs")).unwrap();
    assert_eq!(counts(&run(&root, &indexer)), (1, 0, 1, 0));
    assert_eq!(symbols(&root), [("new.rs".into(), "alpha".into())]);
}

#[test]
fn a_deleted_file_loses_its_symbols() {
    let (_guard, root) = temp();
    write(&root, "a.rs", "fn alpha\n", OLD);
    write(&root, "b.rs", "fn beta\n", OLD);
    let indexer = LineIndexer::new("v1");
    run(&root, &indexer);
    fs::remove_file(root.join("a.rs")).unwrap();
    assert_eq!(counts(&run(&root, &indexer)), (0, 0, 1, 1));
    assert_eq!(symbols(&root), [("b.rs".into(), "beta".into())]);
}

#[test]
fn two_files_swapped_between_scans_are_both_reparsed() {
    let (_guard, root) = temp();
    write(&root, "a.rs", "fn alpha\n", OLD);
    write(&root, "b.rs", "fn betaa\n", Duration::from_hours(2));
    let indexer = LineIndexer::new("v1");
    run(&root, &indexer);
    fs::rename(root.join("a.rs"), root.join("t.rs")).unwrap();
    fs::rename(root.join("b.rs"), root.join("a.rs")).unwrap();
    fs::rename(root.join("t.rs"), root.join("b.rs")).unwrap();
    assert_eq!(counts(&run(&root, &indexer)), (2, 0, 0, 0));
    assert_eq!(
        symbols(&root),
        [
            ("a.rs".into(), "betaa".into()),
            ("b.rs".into(), "alpha".into())
        ]
    );
}

#[test]
fn a_new_parser_version_reparses_unchanged_files() {
    let (_guard, root) = temp();
    write(&root, "a.rs", "fn alpha\n", OLD);
    run(&root, &LineIndexer::new("v1"));
    let v2 = LineIndexer::new("v2");
    assert_eq!(counts(&run(&root, &v2)), (1, 0, 0, 0));
    assert_eq!(counts(&run(&root, &v2)), (0, 0, 0, 1));
}

#[test]
fn pending_files_are_reparsed() {
    let (_guard, root) = temp();
    write(&root, "a.rs", "fn alpha\n", OLD);
    let indexer = LineIndexer::new("v1");
    run(&root, &indexer);
    let mut store = Store::open(&root.join(".codix/index.db")).unwrap();
    assert_eq!(store.mark_pending(&["a.rs".to_owned()]).unwrap(), 1);
    assert_eq!(counts(&run(&root, &indexer)), (1, 0, 0, 0));
}

#[test]
fn concurrent_scans_converge_on_one_consistent_index() {
    let (_guard, root) = temp();
    for i in 0..200 {
        write(
            &root,
            &format!("src/f{i}.rs"),
            &format!("fn f{i}\nfn g{i}\n"),
            OLD,
        );
    }
    let indexer = LineIndexer::new("v1");
    run(&root, &indexer);
    for i in 0..200 {
        if i % 3 == 0 {
            write(
                &root,
                &format!("src/f{i}.rs"),
                &format!("fn h{i}\n"),
                Duration::from_mins(30),
            );
        }
    }
    fs::remove_file(root.join("src/f1.rs")).unwrap();

    std::thread::scope(|s| {
        let a = s.spawn(|| run(&root, &LineIndexer::new("v1")));
        let b = s.spawn(|| run(&root, &LineIndexer::new("v1")));
        a.join().unwrap();
        b.join().unwrap();
    });
    let concurrent = symbols(&root);

    let (_guard2, fresh) = temp();
    for i in 0..200 {
        if i == 1 {
            continue;
        }
        let content = if i % 3 == 0 {
            format!("fn h{i}\n")
        } else {
            format!("fn f{i}\nfn g{i}\n")
        };
        write(&fresh, &format!("src/f{i}.rs"), &content, OLD);
    }
    run(&fresh, &LineIndexer::new("v1"));
    assert_eq!(concurrent, symbols(&fresh));
    assert_eq!(counts(&run(&root, &indexer)), (0, 0, 0, 199));
}

#[test]
fn indexing_config_changes_mark_affected_files_pending() {
    use codix_core::config::{ConfigOp, ConfigRequest, ConfigResponse};
    use codix_core::op::{Context, Operation};

    let (_guard, root) = temp();
    write(&root, "include/a.h", "fn alpha\n", OLD);
    write(&root, "include/b.h", "fn beta\n", OLD);
    write(&root, "src/main.c", "fn main\n", OLD);
    let indexer = LineIndexer::new("v1");
    run(&root, &indexer);

    let ctx = Context::discover(&root, None).unwrap();
    let set = ConfigOp
        .run(
            &ctx,
            ConfigRequest::Set {
                key: "languages.overrides.\"*.h\"".to_owned(),
                value: "cpp".to_owned(),
            },
        )
        .unwrap();
    let ConfigResponse::Set { effect, .. } = set else {
        panic!("not a set response");
    };
    assert_eq!(effect, "2 files will be reindexed on the next scan");
    // The two headers, and codix.toml, new since the last scan.
    assert_eq!(counts(&run(&root, &indexer)), (3, 0, 0, 1));
}

#[test]
fn a_partial_scan_covers_only_its_targets() {
    let (_guard, root) = temp();
    write(&root, "a/one.rs", "fn one\n", OLD);
    write(&root, "a/deep/two.rs", "fn two\n", OLD);
    write(&root, "b/three.rs", "fn three\n", OLD);
    let indexer = LineIndexer::new("v1");
    run(&root, &indexer);
    indexer.parsed();

    write(&root, "a/one.rs", "fn uno\n", OLD);
    write(&root, "a/deep/two.rs", "fn dos\n", OLD);
    write(&root, "b/three.rs", "fn tres\n", OLD);
    write(&root, "a/new.rs", "fn new\n", OLD);
    // `a` alone: its files, not its subdirectories', and nothing in `b`.
    assert_eq!(
        counts(&run_dirs(&root, &indexer, &targets(&["a"], &[]))),
        (2, 0, 0, 0)
    );
    assert_eq!(indexer.parsed(), 2);
    let names: Vec<String> = symbols(&root).into_iter().map(|(_, name)| name).collect();
    assert_eq!(names, ["two", "new", "uno", "three"]);

    // A tree covers its subdirectories; a removed one loses its files.
    fs::remove_dir_all(root.join("a/deep")).unwrap();
    assert_eq!(
        counts(&run_dirs(&root, &indexer, &targets(&[], &["a"]))),
        (0, 0, 1, 2)
    );
    assert_eq!(
        counts(&run_dirs(&root, &indexer, &targets(&[], &["b"]))),
        (1, 0, 0, 0)
    );
}

#[test]
fn a_partial_scan_honors_the_ignore_rules() {
    let (_guard, root) = temp();
    write(&root, ".codixignore", "*.gen.rs\n", OLD);
    write(&root, "a/kept.rs", "fn kept\n", OLD);
    write(&root, "a/skip.gen.rs", "fn generated\n", OLD);
    write(&root, "node_modules/lib.rs", "fn vendored\n", OLD);
    let indexer = LineIndexer::new("v1");
    let report = run_dirs(&root, &indexer, &targets(&["a", "node_modules"], &[""]));
    assert_eq!(counts(&report), (2, 0, 0, 0), "{report:?}");
    let names: Vec<String> = symbols(&root).into_iter().map(|(_, name)| name).collect();
    assert_eq!(names, ["kept"]);
}

#[test]
fn a_file_already_rescanned_is_not_parsed_again() {
    // What happens to codix's own writes: the manifest has the new stats by
    // the time the watcher reports the file.
    let (_guard, root) = temp();
    write(&root, "a/one.rs", "fn one\n", OLD);
    let indexer = LineIndexer::new("v1");
    run(&root, &indexer);
    write(&root, "a/one.rs", "fn uno\n", OLD);
    let project = Project::discover(&root)
        .expect("discover")
        .expect("project");
    rescan(
        &project,
        &indexer,
        &FileGate,
        &["a/one.rs".to_owned()],
        Duration::from_secs(1),
    )
    .expect("rescan")
    .expect("free gate");
    indexer.parsed();

    assert_eq!(
        counts(&run_dirs(&root, &indexer, &targets(&["a"], &[]))),
        (0, 0, 0, 1)
    );
    assert_eq!(indexer.parsed(), 0);
}

#[test]
fn a_partial_commit_drops_the_hashes_of_the_directories_above() {
    let (_guard, root) = temp();
    write(&root, "a/b/one.rs", "fn one\n", OLD);
    write(&root, "c/two.rs", "fn two\n", OLD);
    let indexer = LineIndexer::new("v1");
    run(&root, &indexer);
    let dirs = || {
        let mut store = Store::open(&root.join(".codix/index.db")).expect("open index");
        let mut dirs: Vec<String> = store
            .snapshot()
            .expect("snapshot")
            .dirs()
            .expect("dirs")
            .into_keys()
            .collect();
        dirs.sort();
        dirs
    };
    assert_eq!(dirs(), ["", "a", "a/b", "c"]);

    write(&root, "a/b/one.rs", "fn uno\n", OLD);
    run_dirs(&root, &indexer, &targets(&["a/b"], &[]));
    assert_eq!(dirs(), ["c"]);
    // The next full scan compares those directories file by file, and
    // stores their hashes again.
    assert_eq!(counts(&run(&root, &indexer)), (0, 0, 0, 2));
    assert_eq!(dirs(), ["", "a", "a/b", "c"]);
}
