//! Fixture projects for the CLI tests.

use std::fs;
use std::path::{Path, PathBuf};

use crate::{Run, run_in};

/// A temporary project with `files`, initialized and indexed by `codix`.
///
/// # Panics
///
/// When the directory cannot be created or `codix init` fails.
pub fn indexed(codix: &str, files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let (dir, root) = files_only(files);
    let out = run_in(&root, codix, &["init"]);
    assert_eq!(out.code, Some(0), "init: {}", out.stderr);
    (dir, root)
}

/// A temporary directory with `files`, not initialized.
///
/// # Panics
///
/// When the directory or a file cannot be created.
pub fn files_only(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = codix_core::project::canonicalize(dir.path()).expect("canonical temp dir");
    for (path, content) in files {
        write(&root, path, content);
    }
    (dir, root)
}

/// Writes `content` to `path` under `root`, creating directories.
///
/// # Panics
///
/// When a directory or the file cannot be created.
pub fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
    fs::write(path, content).expect("write file");
}

/// Runs `codix` in `cwd`; `{ROOT}` in arguments stands for the root, and the
/// root is shown as `<root>` in the output.
pub fn codix_in(codix: &str, root: &Path, cwd: &Path, args: &[&str]) -> Run {
    let shown = root.display().to_string();
    let args: Vec<String> = args.iter().map(|a| a.replace("{ROOT}", &shown)).collect();
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let mut out = run_in(cwd, codix, &args);
    out.stdout = out.stdout.replace(&shown, "<root>");
    out.stderr = out.stderr.replace(&shown, "<root>");
    out
}

/// A Java class with methods `m0` to `m{count - 1}`.
pub fn methods(count: usize) -> String {
    let body: Vec<String> = (0..count)
        .map(|i| format!("    void m{i}() {{}}"))
        .collect();
    format!("class Box {{\n{}\n}}\n", body.join("\n"))
}

/// Two Java packages and a text file, for path and scope tests.
pub const BILLING: &[(&str, &str)] = &[
    (
        "src/billing/Invoice.java",
        "package billing;\n\npublic class Invoice {\n    private long total;\n\n    public long total() {\n        return total;\n    }\n\n    long total(long discount) {\n        return total - discount;\n    }\n}\n",
    ),
    (
        "src/billing/Cart.java",
        "package billing;\n\nclass Cart {\n    long total() { return 0; }\n}\n",
    ),
    (
        "tests/billing/InvoiceTest.java",
        "class InvoiceTest {\n    void totals() {}\n}\n",
    ),
    ("docs/notes.txt", "total of all invoices\n"),
];
