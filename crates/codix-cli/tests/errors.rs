//! Every error class a command can reach: exit code, text on stderr, JSON on
//! stdout. `internal` has no command to reach it; its rendering is covered by
//! unit tests.

use codix_testkit::fixture::files_only;
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn codix(root: &std::path::Path, cwd: &std::path::Path, args: &[&str]) -> codix_testkit::Run {
    codix_testkit::fixture::codix_in(CODIX, root, cwd, args)
}

fn indexed(files: &[(&str, &str)]) -> (tempfile::TempDir, std::path::PathBuf) {
    codix_testkit::fixture::indexed(CODIX, files)
}

/// Runs `args` in `cwd` and checks the error in both renderings; `<cwd>` in
/// `text` stands for the canonical cwd.
fn check(
    root: &std::path::Path,
    cwd: &std::path::Path,
    args: &[&str],
    code: &str,
    exit: i32,
    text: &str,
) {
    let out = codix(root, cwd, args);
    let text = text.replace("<cwd>", &cwd.display().to_string());
    assert_eq!(out.code, Some(exit), "{code}: {}", out.stderr);
    assert_eq!(out.stdout, "", "{code}");
    assert_eq!(out.stderr, text, "{code}");

    let mut json_args = args.to_vec();
    json_args.push("--json");
    let out = codix(root, cwd, &json_args);
    assert_eq!(out.code, Some(exit), "{code} --json");
    assert_eq!(out.stderr, "", "{code} --json");
    let value: Value = serde_json::from_str(&out.stdout).expect("one JSON document");
    assert_eq!(value["schema_version"], 1);
    assert_eq!(value["error"]["code"], code);
    let first = text.lines().next().expect("error line");
    // JSON escapes the root's backslashes on Windows, so the root is replaced
    // once the string is parsed.
    let cause = value["error"]["cause"]
        .as_str()
        .expect("cause")
        .replace(&root.display().to_string(), "<root>");
    assert_eq!(
        cause,
        first.split_once("]: ").expect("error[code]: cause").1
    );
    let hint = text
        .lines()
        .last()
        .and_then(|l| l.strip_prefix("hint: "))
        .expect("hint line");
    assert_eq!(value["error"]["hint"], hint);
}

#[test]
fn no_project() {
    let (_guard, root) = files_only(&[]);
    check(
        &root,
        &root,
        &["find", "x"],
        "no_project",
        6,
        "error[no_project]: no codix project in <root> or any parent directory\n\
         hint: run `codix init` at the project root\n",
    );
}

#[test]
fn not_indexed() {
    let (_guard, root) = files_only(&[("codix.toml", "")]);
    check(
        &root,
        &root,
        &["find", "x"],
        "not_indexed",
        6,
        "error[not_indexed]: the project at <root> is not indexed\n\
         hint: run `codix init` at the project root\n",
    );
}

#[test]
fn invalid_config() {
    let (_guard, root) = files_only(&[("codix.toml", "")]);
    check(
        &root,
        &root,
        &["config", "previews.expiri", "1h"],
        "invalid_config",
        2,
        "error[invalid_config]: unknown key `previews.expiri`\n\
         hint: valid keys: daemon.idle_timeout, index.max_file_size, index.scan_interval, languages.overrides.\"<glob>\", sources.roots\n",
    );
}

#[test]
fn io() {
    let (_guard, root) = indexed(&[("README", "hi\n")]);
    let out = codix(&root, &root, &["outline", "missing.rs"]);
    assert_eq!(out.code, Some(5));
    // The cause ends with the operating system's message.
    assert!(
        out.stderr.starts_with("error[io]: missing.rs: "),
        "{}",
        out.stderr
    );
    assert!(
        out.stderr
            .ends_with("\nhint: check that the file exists and is accessible\n"),
        "{}",
        out.stderr
    );
    let json: Value =
        serde_json::from_str(&codix(&root, &root, &["outline", "missing.rs", "--json"]).stdout)
            .unwrap();
    assert_eq!(json["error"]["code"], "io");
}

#[test]
fn usage_from_a_bad_flag() {
    let (_guard, root) = files_only(&[]);
    check(
        &root,
        &root,
        &["find", "x", "--bogus"],
        "usage",
        2,
        "error[usage]: unexpected argument '--bogus' found\n\
         hint: usage: codix find <QUERY>; see --help\n",
    );
}

#[test]
fn usage_from_a_zero_limit() {
    let (_guard, root) = indexed(&[("README", "hi\n")]);
    check(
        &root,
        &root,
        &["find", "x", "--limit", "0"],
        "usage",
        2,
        "error[usage]: --limit must be at least 1\n\
         hint: omit --limit for the default of 50\n",
    );
}

#[test]
fn usage_from_a_bad_regex() {
    let (_guard, root) = indexed(&[("README", "hi\n")]);
    let out = codix(&root, &root, &["grep", "a("]);
    assert_eq!(out.code, Some(2));
    assert!(
        out.stderr
            .starts_with("error[usage]: invalid regular expression: "),
        "{}",
        out.stderr
    );
}

#[test]
fn usage_names_the_missing_argument() {
    let (_guard, root) = files_only(&[]);
    let out = codix(&root, &root, &["config"]);
    assert_eq!(out.code, Some(2));
    assert_eq!(
        out.stderr.lines().next(),
        Some("error[usage]: the following required arguments were not provided: <KEY>")
    );
}

#[test]
fn usage_names_codix_whatever_the_binary_is_called() {
    let out = run_renamed(&["find", "x", "--bogus"]);
    assert!(out.contains("usage: codix find"), "{out}");
}

/// Runs codix under another name and returns its stderr. On Unix, argv[0]
/// is set directly: running a fresh copy can fail with "Text file busy" when
/// another test forks while the copy is still open for writing.
#[cfg(unix)]
fn run_renamed(args: &[&str]) -> String {
    use std::os::unix::process::CommandExt;
    let output = std::process::Command::new(CODIX)
        .arg0("renamed-codix")
        .args(args)
        .output()
        .expect("start codix");
    String::from_utf8(output.stderr).expect("UTF-8")
}

/// Runs codix under another name and returns its stderr: Windows takes
/// argv[0] from the file name, so this runs a renamed copy.
#[cfg(windows)]
fn run_renamed(args: &[&str]) -> String {
    let dir = tempfile::tempdir().expect("temp dir");
    let renamed = dir.path().join("renamed-codix.exe");
    std::fs::copy(CODIX, &renamed).expect("copy codix");
    let output = std::process::Command::new(&renamed)
        .args(args)
        .output()
        .expect("start codix");
    String::from_utf8(output.stderr).expect("UTF-8")
}

#[cfg(feature = "lang-java")]
mod with_java {
    use super::{check, codix, indexed};

    #[test]
    fn ambiguous_symbol_lists_candidates() {
        let (_guard, root) = indexed(&[(
            "A.java",
            "class A {\n  void f(int x) {}\n  void f(long x) {}\n}\n",
        )]);
        let out = codix(&root, &root, &["def", "f"]);
        assert_eq!(out.code, Some(3));
        let lines: Vec<&str> = out.stderr.lines().collect();
        assert_eq!(lines[0], "error[ambiguous_symbol]: \"f\" matches 2 symbols");
        assert!(
            lines[1].starts_with("  java:A.java::A.f#")
                && lines[2].starts_with("  java:A.java::A.f#")
        );
        assert_eq!(
            lines[3],
            "hint: pass a full symbol ID, or narrow with --kind or --path"
        );

        let json: serde_json::Value =
            serde_json::from_str(&codix(&root, &root, &["def", "f", "--json"]).stdout).unwrap();
        assert_eq!(
            json["error"]["candidates"].as_array().map(Vec::len),
            Some(2)
        );
    }

    #[test]
    fn unknown_symbol() {
        let (_guard, root) = indexed(&[("A.java", "class A {}\n")]);
        check(
            &root,
            &root,
            &["def", "totl"],
            "unknown_symbol",
            1,
            "error[unknown_symbol]: no symbol matches \"totl\"\n\
             hint: check the name with `codix find`, or pass a full symbol ID\n",
        );
    }
}

#[cfg(not(feature = "lang-kotlin"))]
#[test]
fn not_supported_in_build() {
    let (_guard, root) = indexed(&[("src/Main.kt", "fun main() {}\n")]);
    check(
        &root,
        &root,
        &["outline", "src/Main.kt"],
        "not_supported_in_build",
        2,
        "error[not_supported_in_build]: src/Main.kt: kotlin is not supported in this build\n\
         hint: use the full codix build, which includes every provider\n",
    );
}

#[cfg(windows)]
#[test]
fn windows_paths_in_messages_have_no_verbatim_prefix() {
    let dir = tempfile::tempdir().unwrap();
    let out = codix_testkit::run_in(dir.path(), CODIX, &["find", "x"]);
    assert_eq!(out.code, Some(6));
    assert!(!out.stderr.contains(r"\\?\"), "{}", out.stderr);
}
