//! Test support shared by the codix crates: corpus runner, fixture projects
//! and golden-output helpers.

use std::fs;
use std::path::Path;
use std::process::Command;

/// Environment variable that makes [`assert_golden`] rewrite golden files.
pub const BLESS_VAR: &str = "CODIX_BLESS";

/// Compares `actual` with the golden file at `path`, or rewrites the file when
/// `CODIX_BLESS=1`.
///
/// # Panics
///
/// When the contents differ, or the file cannot be read or written.
pub fn assert_golden(path: impl AsRef<Path>, actual: &str) {
    let path = path.as_ref();
    if std::env::var_os(BLESS_VAR).is_some_and(|value| value == "1") {
        if let Some(parent) = path.parent() {
            fs::create_dir_all(parent).expect("create golden directory");
        }
        fs::write(path, actual).expect("write golden file");
        return;
    }
    let expected = fs::read_to_string(path).unwrap_or_else(|e| {
        panic!(
            "cannot read golden file {}: {e}; run with {BLESS_VAR}=1 to create it",
            path.display()
        )
    });
    assert!(
        expected == actual,
        "output differs from {}; run with {BLESS_VAR}=1 to update it\n--- expected\n{expected}\n--- actual\n{actual}",
        path.display()
    );
}

/// The result of running a binary.
#[derive(Debug)]
pub struct Run {
    /// Exit code; `None` when killed by a signal.
    pub code: Option<i32>,
    /// Standard output, as UTF-8.
    pub stdout: String,
    /// Standard error, as UTF-8.
    pub stderr: String,
}

/// Runs `binary` with `args`, with `NO_COLOR` unset so color decisions depend
/// only on whether output is a terminal.
///
/// # Panics
///
/// When the binary cannot be started or its output is not UTF-8.
pub fn run(binary: &str, args: &[&str]) -> Run {
    run_in(Path::new("."), binary, args)
}

/// Like [`run`], from the directory `cwd`.
///
/// # Panics
///
/// When the binary cannot be started or its output is not UTF-8.
pub fn run_in(cwd: &Path, binary: &str, args: &[&str]) -> Run {
    let output = Command::new(binary)
        .args(args)
        .current_dir(cwd)
        .env_remove("NO_COLOR")
        .output()
        .expect("start binary");
    Run {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).expect("stdout is UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("stderr is UTF-8"),
    }
}
