//! Discovery: `help`, `languages`, `capabilities`, the generated command docs
//! and the agent guide, all from the canonical descriptions.

use std::fs;
use std::path::PathBuf;

use codix_testkit::fixture::{codix_in, files_only, indexed};
use codix_testkit::{assert_golden, run};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn golden(name: &str) -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/golden")
        .join(name)
}

#[test]
fn help_shows_the_canonical_description() {
    let out = run(CODIX, &["help", "find"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_golden(golden("help-find.txt"), &out.stdout);
    // `--help` and `help` are the same text.
    assert_eq!(run(CODIX, &["find", "--help"]).stdout, out.stdout);
}

#[test]
fn the_command_docs_are_generated_from_the_descriptions() {
    let docs = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../docs/commands.md");
    assert_golden(docs, &codix_core::describe::markdown());
}

#[cfg(feature = "lang-java")]
#[test]
fn languages_lists_every_known_language() {
    let out = run(CODIX, &["languages"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(out.stdout.contains("java "), "{}", out.stdout);
    let json: serde_json::Value =
        serde_json::from_str(&run(CODIX, &["languages", "--json"]).stdout).expect("JSON");
    let languages = json["result"]["languages"].as_array().expect("languages");
    assert_eq!(languages.len(), codix_provider::known::LANGUAGES.len());
    let java = languages
        .iter()
        .find(|l| l["name"] == "java")
        .expect("java");
    assert_eq!(java["tier"], "scopes");
    assert_eq!(java["extensions"], serde_json::json!(["java"]));
}

#[cfg(feature = "lang-java")]
#[test]
fn capabilities_in_a_project_describe_its_index() {
    let (_guard, root) = indexed(
        CODIX,
        &[
            ("src/A.java", "class A {}\n"),
            ("src/B.java", "class B {}\n"),
            ("README.md", "# readme\n"),
        ],
    );
    let out = codix_in(CODIX, &root, &root, &["capabilities", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&out.stdout).expect("JSON");
    let project = &json["result"]["project"];
    assert_eq!(project["files"], 3, "{project}");
    assert_eq!(project["languages"][0]["name"], "java", "{project}");
    assert_eq!(project["languages"][0]["files"], 2, "{project}");
    let find = json["result"]["commands"]
        .as_array()
        .expect("commands")
        .iter()
        .find(|c| c["name"] == "find")
        .expect("find");
    assert_eq!(find["tier"], "symbols");

    let (_guard, elsewhere) = files_only(&[]);
    let outside = codix_in(CODIX, &elsewhere, &elsewhere, &["capabilities", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&outside.stdout).expect("JSON");
    assert!(json["result"]["project"].is_null());
}

#[test]
fn init_writes_the_agent_guide_and_never_a_file_it_did_not_write() {
    let (_guard, root) = files_only(&[("notes.txt", "notes\n")]);
    let out = codix_in(CODIX, &root, &root, &["init", "--agent-docs"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    let guide = fs::read_to_string(root.join("CODIX.md")).expect("guide");
    assert_eq!(guide, codix_core::describe::agent_guide());
    assert!(
        guide.contains("`codix find Invoice --kind class`"),
        "{guide}"
    );
    // Its own file, it rewrites.
    let out = codix_in(CODIX, &root, &root, &["init", "--agent-docs"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);

    fs::write(root.join("AGENTS.md"), "# Mine\n").expect("write");
    let out = codix_in(CODIX, &root, &root, &["init", "--agent-docs", "AGENTS.md"]);
    assert_eq!(out.code, Some(4), "{}", out.stderr);
    assert!(
        out.stderr.starts_with("error[file_exists]: "),
        "{}",
        out.stderr
    );
    assert_eq!(
        fs::read_to_string(root.join("AGENTS.md")).expect("read"),
        "# Mine\n"
    );
}
