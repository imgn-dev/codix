//! A Java project for the resolver tests, scanned through the core.

use std::fmt::Write as _;
use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

use codix_core::config::Config;
use codix_core::lock::FileGate;
use codix_core::parse::PackIndexer;
use codix_core::project::Project;
use codix_core::scan::{ScanReport, scan};
use codix_core::store::Store;

pub(crate) const UTIL: &str = "src/a/Util.java";

const FILES: &[(&str, &str)] = &[
    (
        UTIL,
        "package a;\n\npublic class Util {\n    public static int helper() {\n        return 1;\n    }\n}\n",
    ),
    (
        "src/b/Main.java",
        "package b;\n\nclass Main {\n    int run() {\n        return Util.helper() + local();\n    }\n\n    int local() {\n        return 2;\n    }\n}\n",
    ),
    (
        "src/c/Other.java",
        "package c;\n\nclass Other {\n    int value() {\n        return 3;\n    }\n}\n",
    ),
];

/// A project of three Java files: `Main` calls `Util.helper`, `Other`
/// stands alone.
pub(crate) fn project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = codix_core::project::canonicalize(dir.path()).expect("canonical");
    fs::create_dir(root.join(".codix")).expect(".codix");
    for (path, text) in FILES {
        write(&root, path, text);
    }
    (dir, root)
}

/// Writes a file with an mtime out of the racy window.
pub(crate) fn write(root: &Path, path: &str, text: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().expect("parent")).expect("dirs");
    fs::write(&path, text).expect("write");
    File::options()
        .write(true)
        .open(&path)
        .and_then(|f| f.set_modified(SystemTime::now() - Duration::from_hours(1)))
        .expect("set mtime");
}

/// Changes `Util.java`, differently each round.
pub(crate) fn change_util(root: &Path, round: usize) {
    let extra = format!("    static int more{round}() {{\n        return {round};\n    }}\n");
    write(
        root,
        UTIL,
        &format!(
            "package a;\n\npublic class Util {{\n    public static int helper() {{\n        return 1;\n    }}\n{extra}}}\n"
        ),
    );
}

pub(crate) fn scan_project(root: &Path) -> ScanReport {
    let project = Project::discover(root).expect("discover").expect("project");
    let config = Config::load(&project).expect("config");
    scan(&project, &config, &PackIndexer::new(&config), &FileGate).expect("scan")
}

/// Every resolution, one line each: `path:byte name -> target`.
pub(crate) fn resolutions(root: &Path) -> String {
    let mut store = Store::open(&root.join(".codix/index.db")).expect("index");
    let snapshot = store.snapshot().expect("snapshot");
    let mut out = String::new();
    for r in snapshot.resolutions().expect("resolutions") {
        let target = r.target.as_deref().unwrap_or("external");
        writeln!(out, "{}:{} {} -> {target}", r.path, r.start_byte, r.name).expect("write");
    }
    out
}

/// The expected resolutions of the project, compiled in or as a plugin.
pub(crate) fn check_resolutions(root: &Path) {
    codix_testkit::assert_golden(
        Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/golden/resolutions.txt"),
        &resolutions(root),
    );
}
