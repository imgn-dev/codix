//! Test support shared by the codix crates: corpus runner, fixture projects
//! and golden-output helpers.

pub mod fixture;
pub mod schema;

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

/// Checks every source file of a corpus directory against its expectation,
/// `<file>.symbols`, holding what `extract` returns for it, one line each.
/// With `CODIX_BLESS=1`, rewrites the expectations instead.
///
/// # Panics
///
/// When a file differs from its expectation, or the directory has no source
/// files.
pub fn check_corpus(dir: impl AsRef<Path>, extract: impl Fn(&str, &str) -> Vec<String>) {
    check_corpus_as(dir, "symbols", extract);
}

/// Like [`check_corpus`], with the expectations in `<file>.<suffix>`.
///
/// # Panics
///
/// When a file differs from its expectation, or the directory has no source
/// files.
pub fn check_corpus_as(
    dir: impl AsRef<Path>,
    suffix: &str,
    extract: impl Fn(&str, &str) -> Vec<String>,
) {
    let dir = dir.as_ref();
    let mut sources: Vec<_> = fs::read_dir(dir)
        .unwrap_or_else(|e| panic!("cannot read corpus {}: {e}", dir.display()))
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| {
            path.is_file()
                && path
                    .extension()
                    .is_none_or(|e| e != "symbols" && e != "scopes")
        })
        .collect();
    sources.sort();
    assert!(!sources.is_empty(), "corpus {} is empty", dir.display());
    for source in sources {
        let name = source
            .file_name()
            .and_then(|n| n.to_str())
            .expect("UTF-8 file name");
        let content = fs::read_to_string(&source).expect("read corpus source");
        let mut actual = extract(name, &content).join("\n");
        actual.push('\n');
        assert_golden(dir.join(format!("{name}.{suffix}")), &actual);
    }
}

/// Checks the corpus of a compiled-in language, `corpus/<language>`: each
/// symbol as `line:col kind qualified [visibility] (low) | signature`.
///
/// # Panics
///
/// When the language is not compiled in, or its corpus differs.
pub fn check_language_corpus(language: &str) {
    use codix_core::store::Confidence;

    let registry = codix_provider::registry();
    let descriptor = registry
        .languages
        .iter()
        .find(|d| d.name == language)
        .unwrap_or_else(|| panic!("{language} is not compiled in"));
    let grammar = registry
        .grammars
        .get(language)
        .unwrap_or_else(|| panic!("{language} has no grammar"));
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(language);
    check_corpus(dir, |name, content| {
        codix_core::parse::extract(descriptor, grammar, name, content)
            .expect("extract")
            .iter()
            .map(|s| {
                let visibility = s
                    .visibility
                    .as_ref()
                    .map(|v| format!(" [{v}]"))
                    .unwrap_or_default();
                let low = if s.confidence == Confidence::Low {
                    " (low)"
                } else {
                    ""
                };
                let signature = s
                    .signature
                    .as_ref()
                    .map(|v| format!(" | {v}"))
                    .unwrap_or_default();
                format!(
                    "{}:{} {} {}{visibility}{low}{signature}",
                    s.name_range.start.line, s.name_range.start.column, s.kind, s.qualified_name
                )
            })
            .collect()
    });
}

/// Checks the scopes of a compiled-in language at tier 2, against
/// `corpus/<language>/<case>.scopes`: each local definition as
/// `line:col kind name`, and each reference as
/// `line:col context name -> line:col` of the definition it binds to, or
/// `-> free` or `-> uncertain`; `(low)` marks parse-error regions.
///
/// # Panics
///
/// When the language is not compiled in, has no `locals` query, or its
/// corpus differs.
pub fn check_language_scopes(language: &str) {
    use codix_core::scope::{Binding, Defines};

    let registry = codix_provider::registry();
    let descriptor = registry
        .languages
        .iter()
        .find(|d| d.name == language)
        .unwrap_or_else(|| panic!("{language} is not compiled in"));
    let grammar = registry
        .grammars
        .get(language)
        .unwrap_or_else(|| panic!("{language} has no grammar"));
    assert!(grammar.locals.is_some(), "{language} has no locals query");
    let dir = Path::new(env!("CARGO_MANIFEST_DIR"))
        .join("../../corpus")
        .join(language);
    check_corpus_as(dir, "scopes", |name, content| {
        let model = codix_core::scope::model(descriptor, grammar, name, content).expect("model");
        let low = |low: bool| if low { " (low)" } else { "" };
        let mut lines: Vec<(u32, String)> = Vec::new();
        for d in &model.definitions {
            if let Defines::Local { kind } = &d.defines {
                let at = d.name_range.start;
                lines.push((
                    at.byte,
                    format!("{}:{} {kind} {}{}", at.line, at.column, d.name, low(d.low)),
                ));
            }
        }
        for r in &model.references {
            let target = match r.binding {
                Binding::Bound(d) => {
                    let at = model.definitions[d].name_range.start;
                    format!("{}:{}", at.line, at.column)
                }
                Binding::Free => "free".to_owned(),
                Binding::Uncertain => "uncertain".to_owned(),
            };
            let at = r.range.start;
            lines.push((
                at.byte,
                format!(
                    "{}:{} {} {} -> {target}{}",
                    at.line,
                    at.column,
                    r.context.as_str(),
                    r.name,
                    low(r.low)
                ),
            ));
        }
        lines.sort_by_key(|(byte, _)| *byte);
        lines.into_iter().map(|(_, line)| line).collect()
    });
}
