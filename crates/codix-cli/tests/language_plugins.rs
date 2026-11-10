//! A language installed as a plugin indexes its files as a compiled-in pack
//! does. Installing it reparses its files on the next scan, removing it
//! returns them to text, and a plugin that cannot be used is reported with
//! its reason while its files are indexed as text.
#![cfg(all(feature = "plugins", not(feature = "lang-java")))]

use std::fs;
use std::path::{Path, PathBuf};

use codix_testkit::fixture::BILLING;
use codix_testkit::{Run, run_with_env};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

/// A project of three Java files and a text file, and an empty plugin
/// directory.
struct Project {
    _dir: tempfile::TempDir,
    root: PathBuf,
    plugins: PathBuf,
}

fn project() -> Project {
    let dir = tempfile::tempdir().expect("temp dir");
    let base = codix_core::project::canonicalize(dir.path()).expect("canonical");
    let root = base.join("project");
    for (path, text) in BILLING {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        fs::write(path, text).expect("write");
    }
    let plugins = base.join("plugins");
    fs::create_dir_all(&plugins).expect("plugins");
    Project {
        _dir: dir,
        root,
        plugins,
    }
}

impl Project {
    /// Runs codix in the project with only its plugins.
    fn run(&self, args: &[&str]) -> Run {
        let path = self.plugins.to_str().expect("UTF-8 path");
        let data = self.plugins.join("no-user-plugins");
        let data = data.to_str().expect("UTF-8 path");
        run_with_env(
            &self.root,
            CODIX,
            args,
            &[
                ("CODIX_PLUGIN_PATH", path),
                ("XDG_DATA_HOME", data),
                ("LOCALAPPDATA", data),
            ],
        )
    }

    fn ok(&self, args: &[&str]) -> String {
        let out = self.run(args);
        assert_eq!(out.code, Some(0), "{args:?}: {}", out.stderr);
        out.stdout
    }

    /// The `missing` entry of `status` for Java, if any.
    fn java_missing(&self) -> Option<Value> {
        let status: Value = serde_json::from_str(&self.ok(&["status", "--json"])).expect("JSON");
        status["result"]["missing"]
            .as_array()
            .expect("missing")
            .iter()
            .find(|m| m["name"] == "java")
            .cloned()
    }

    fn install_java(&self) -> PathBuf {
        codix_testkit::plugin::language_plugin("java", &self.plugins)
    }
}

#[test]
fn installing_a_plugin_reparses_its_files_and_removing_it_returns_them_to_text() {
    let project = project();
    project.ok(&["init"]);
    let missing = project.java_missing().expect("java is missing");
    assert_eq!(missing["files"], 3, "{missing}");
    assert_eq!(missing["invalid"], Value::Null, "{missing}");

    let plugin = project.install_java();
    let index = project.ok(&["index"]);
    assert!(!index.contains("java: "), "{index}");
    assert_eq!(project.java_missing(), None);
    let outline = project.ok(&["outline", "src/billing/Invoice.java"]);
    assert!(outline.contains("total"), "{outline}");

    fs::remove_dir_all(plugin).expect("remove the plugin");
    let index = project.ok(&["index"]);
    assert!(index.contains("java: 3 files indexed as text; "), "{index}");
    assert_eq!(
        project.java_missing().expect("java is missing again")["files"],
        3
    );
}

#[test]
fn a_plugin_that_cannot_be_used_is_reported_and_its_files_indexed_as_text() {
    // What breaks the plugin, how, and the cause reported.
    type Case = (&'static str, fn(&Path), &'static str);
    let cases: &[Case] = &[
        (
            "missing symbol",
            |dir| {
                edit(
                    &dir.join("plugin.toml"),
                    "tree_sitter_java",
                    "tree_sitter_nope",
                );
            },
            "symbol tree_sitter_nope: ",
        ),
        (
            "schema mismatch",
            |dir| edit(&dir.join("plugin.toml"), "schema = 1", "schema = 2"),
            "plugin.toml: schema 2, this codix reads schema 1",
        ),
        (
            "query that does not compile",
            |dir| fs::write(dir.join("queries/tags.scm"), "(no_such_node) @name\n").expect("write"),
            "the tags query does not compile: ",
        ),
    ];
    for (case, break_it, cause) in cases {
        let project = project();
        break_it(&project.install_java());
        let index = project.ok(&["init"]);
        assert!(
            index.contains("java: 3 files indexed as text; plugin invalid: "),
            "{case}: {index}"
        );
        let missing = project.java_missing().expect("java is reported");
        assert_eq!(missing["files"], 3, "{case}: {missing}");
        let invalid = missing["invalid"].as_str().expect("a reason");
        assert!(invalid.contains(cause), "{case}: {invalid}");

        let out = project.run(&["outline", "src/billing/Invoice.java", "--json"]);
        let error: Value = serde_json::from_str(&out.stdout).expect("JSON");
        assert_eq!(error["error"]["code"], "plugin_invalid", "{case}: {error}");
    }
}

fn edit(path: &Path, from: &str, to: &str) {
    let text = fs::read_to_string(path).expect("read");
    assert!(text.contains(from), "{text}");
    fs::write(path, text.replace(from, to)).expect("write");
}
