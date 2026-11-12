//! `init`, `index` and `status` from the command line. The fixtures need the
//! Java pack, the Maven provider and git, which the minimal build leaves out.
#![cfg(all(with_lang_java, with_build_maven, with_vcs_git))]

use std::fs;
use std::path::{Path, PathBuf};

use codix_testkit::{Run, assert_golden, run_in};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = codix_core::project::canonicalize(dir.path()).expect("canonical temp dir");
    for (path, content) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        fs::write(path, content).expect("write file");
    }
    (dir, root)
}

/// Runs codix in `cwd`, with the root shown as `<root>`.
fn codix(root: &Path, cwd: &Path, args: &[&str]) -> Run {
    let mut out = run_in(cwd, CODIX, args);
    let shown = root.display().to_string();
    out.stdout = out.stdout.replace(&shown, "<root>");
    out.stderr = out.stderr.replace(&shown, "<root>");
    out
}

fn golden(name: &str) -> String {
    format!("{}/tests/golden/{name}", env!("CARGO_MANIFEST_DIR"))
}

/// Which of the fixture files are parsed depends on the packs in this build,
/// so the counts line and the lines of missing packs are checked on their
/// own.
fn without_counts(stdout: &str) -> String {
    let mut masked: Vec<&str> = stdout
        .lines()
        .filter(|line| !is_missing(line))
        .map(|line| {
            if line.starts_with("files: ") {
                "files: <counts>"
            } else {
                line
            }
        })
        .collect();
    masked.push("");
    masked.join("\n")
}

fn is_missing(line: &str) -> bool {
    line.contains(" indexed as text; ")
}

/// The fixture's languages whose pack this build lacks, one line each.
fn expected_missing() -> Vec<String> {
    [
        ("markdown", cfg!(with_lang_markdown)),
        ("xml", cfg!(with_lang_xml)),
    ]
    .into_iter()
    .filter(|(_, compiled_in)| !compiled_in)
    .map(|(language, _)| {
        let (why, hint) =
            codix_testkit::missing_provider(cfg!(feature = "plugins"), "lang", language);
        format!("{language}: 1 file indexed as text; {why} ({hint})")
    })
    .collect()
}

fn missing_lines(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|line| is_missing(line))
        .map(str::to_owned)
        .collect()
}

fn expected_counts() -> String {
    let parsed = 1 + usize::from(cfg!(with_lang_xml)) + usize::from(cfg!(with_lang_markdown));
    format!(
        "files: 3 ({parsed} indexed, {} text, 0 pending, 0 errors, 0 skipped)",
        3 - parsed
    )
}

const FILES: &[(&str, &str)] = &[
    (
        "pom.xml",
        "<project><artifactId>shop</artifactId></project>\n",
    ),
    (
        "src/main/java/Invoice.java",
        "class Invoice { long total() { return 0; } }\n",
    ),
    ("README.md", "# Shop\n"),
];

#[test]
fn init_in_a_git_repository() {
    let (_guard, root) = project(FILES);
    fs::create_dir(root.join(".git")).unwrap();
    let out = codix(&root, &root, &["init"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(missing_lines(&out.stdout), expected_missing());
    assert_golden(golden("init-git.txt"), &without_counts(&out.stdout));
    assert_eq!(
        fs::read_to_string(root.join(".codix/.gitignore")).unwrap(),
        "*\n"
    );
    assert!(root.join(".codix/index.db").is_file());
}

#[cfg(with_vcs_hg)]
#[test]
fn the_hint_is_printed_on_the_first_init_only() {
    let (_guard, root) = project(FILES);
    fs::create_dir(root.join(".hg")).unwrap();
    let first = codix(&root, &root, &["init"]);
    assert!(
        first.stdout.contains("hint: add `^\\.codix/` to .hgignore"),
        "{}",
        first.stdout
    );
    let second = codix(&root, &root, &["init"]);
    assert!(
        second.stdout.starts_with("reinitialized codix in <root>\n"),
        "{}",
        second.stdout
    );
    assert!(!second.stdout.contains("hint:"), "{}", second.stdout);
}

#[test]
fn status_reports_files_modules_and_changes() {
    let (_guard, root) = project(FILES);
    codix(&root, &root, &["init"]);
    let out = codix(&root, &root, &["status"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(out.stdout.lines().nth(1), Some(expected_counts().as_str()));
    assert_eq!(missing_lines(&out.stdout), expected_missing());
    assert_golden(golden("status-clean.txt"), &without_counts(&out.stdout));

    fs::write(root.join("README.md"), "# Shop, renamed\n").unwrap();
    fs::write(root.join("NOTES"), "new\n").unwrap();
    fs::remove_file(root.join("src/main/java/Invoice.java")).unwrap();
    let out = codix(&root, &root.join("src"), &["status"]);
    assert_golden(golden("status-changed.txt"), &without_counts(&out.stdout));

    let json: Value =
        serde_json::from_str(&codix(&root, &root, &["status", "--json"]).stdout).unwrap();
    assert_eq!(
        json["result"]["changed"]["new"],
        serde_json::json!(["NOTES"])
    );
    // Modules are counted; listed when asked for.
    assert_eq!(json["result"]["module_count"], 1);
    assert_eq!(
        json["result"]["build_systems"],
        serde_json::json!(["maven"])
    );
    assert_eq!(json["result"]["modules"], serde_json::json!([]));
    let listed: Value =
        serde_json::from_str(&codix(&root, &root, &["status", "--modules", "--json"]).stdout)
            .unwrap();
    assert_eq!(listed["result"]["modules"][0]["name"], "shop");
    let text = codix(&root, &root, &["status", "--modules"]);
    assert!(
        text.stdout.contains("modules:\n  maven  shop  .\n"),
        "{}",
        text.stdout
    );
}

#[test]
fn index_brings_the_index_up_to_date() {
    let (_guard, root) = project(FILES);
    codix(&root, &root, &["init"]);
    fs::write(root.join("Extra.java"), "class Extra {}\n").unwrap();
    let out = codix(&root, &root, &["index"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(
        out.stdout.starts_with("indexed: 1 parsed,"),
        "{}",
        out.stdout
    );
    let status = codix(&root, &root, &["status"]);
    assert!(
        status.stdout.contains("changed since last scan: none"),
        "{}",
        status.stdout
    );

    let full = codix(&root, &root, &["index", "--full"]);
    assert!(
        full.stdout
            .starts_with("indexed: 4 parsed, 0 touched, 0 deleted, 0 unchanged"),
        "{}",
        full.stdout
    );
}

#[test]
fn index_and_status_need_an_initialized_project() {
    let (_guard, root) = project(&[("codix.toml", "")]);
    for command in ["index", "status"] {
        let out = codix(&root, &root, &[command]);
        assert_eq!(out.code, Some(6), "{command}");
        assert!(
            out.stderr.starts_with("error[not_indexed]"),
            "{command}: {}",
            out.stderr
        );
    }
}

#[test]
fn unsupported_build_constructs_show_in_status() {
    let (_guard, root) = project(&[(
        "settings.gradle.kts",
        "rootProject.name = \"shop\"\nincludeBuild(\"plugins\")\n",
    )]);
    codix(&root, &root, &["init"]);
    let out = codix(&root, &root, &["status"]);
    assert!(
        out.stdout.contains("build files codix could not interpret:\n  settings.gradle.kts:2  includeBuild ( \"plugins\" )\n"),
        "{}",
        out.stdout
    );
}
