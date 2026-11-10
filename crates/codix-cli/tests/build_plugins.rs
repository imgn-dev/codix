//! A build system installed as a plugin gives the project its modules, and
//! one that fails is reported with its cause while the scan goes on.
#![cfg(all(feature = "plugins", not(with_build_maven)))]

use std::fs;
use std::path::{Path, PathBuf};

use codix_testkit::plugin::script;
use codix_testkit::{Run, run_with_env};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const MAVEN_DESCRIPTOR: &str = include_str!("../../codix-build-maven/descriptor.toml");

/// A Maven plugin whose executable is a script running `unix` or `windows`.
fn maven_plugin(plugins: &Path, unix: &str, windows: &str) {
    let dir = plugins.join("build").join("maven");
    fs::create_dir_all(dir.join("bin")).expect("plugin dir");
    let executable = script(&dir.join("bin"), "codix-build-maven", unix, windows);
    let executable = executable.file_name().expect("name").to_string_lossy();
    fs::write(
        dir.join("plugin.toml"),
        format!(
            "schema = 1\nkind = \"build\"\nname = \"maven\"\nversion = \"1.0.0\"\nexecutable = \"bin/{executable}\"\n"
        ),
    )
    .expect("manifest");
    fs::write(dir.join("descriptor.toml"), MAVEN_DESCRIPTOR).expect("descriptor");
}

/// A project with a `pom.xml`, and its plugin directory.
fn project() -> (tempfile::TempDir, PathBuf, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let base = codix_core::project::canonicalize(dir.path()).expect("canonical");
    let root = base.join("project");
    fs::create_dir_all(&root).expect("project");
    fs::write(root.join("pom.xml"), "<project/>\n").expect("pom");
    let plugins = base.join("plugins");
    (dir, root, plugins)
}

fn codix(root: &Path, plugins: &Path, args: &[&str]) -> Run {
    let data = plugins.join("no-user-plugins");
    let out = run_with_env(
        root,
        CODIX,
        args,
        &[
            ("CODIX_PLUGIN_PATH", plugins.to_str().expect("UTF-8")),
            ("XDG_DATA_HOME", data.to_str().expect("UTF-8")),
            ("LOCALAPPDATA", data.to_str().expect("UTF-8")),
        ],
    );
    assert_eq!(out.code, Some(0), "{args:?}: {}", out.stderr);
    out
}

// One test: every script is written before any process starts.
#[test]
fn a_build_plugin_gives_modules_and_a_failing_one_is_reported() {
    let answer = r#"{"protocol": 1, "model": {"modules": [{"name": "acme", "build": "maven", "root": "", "source_roots": ["src/main/java"], "test_roots": [], "parent": null, "notes": []}], "unsupported": []}}"#;
    let (_working, working, working_plugins) = project();
    maven_plugin(
        &working_plugins,
        &format!("echo '{answer}'"),
        &format!("echo {answer}"),
    );
    let (_failing, failing, failing_plugins) = project();
    maven_plugin(
        &failing_plugins,
        "echo 'cannot read pom.xml' >&2; exit 1",
        "echo cannot read pom.xml 1>&2\r\nexit /b 1",
    );

    let init = codix(&working, &working_plugins, &["init"]).stdout;
    assert!(!init.contains("maven: "), "{init}");
    let status: Value =
        serde_json::from_str(&codix(&working, &working_plugins, &["status", "--json"]).stdout)
            .expect("JSON");
    let modules = &status["result"]["modules"];
    assert_eq!(modules[0]["name"], "acme", "{status}");
    assert_eq!(modules[0]["build"], "maven", "{status}");

    let init = codix(&failing, &failing_plugins, &["init"]).stdout;
    let line = init
        .lines()
        .find(|l| l.starts_with("maven: "))
        .unwrap_or_else(|| panic!("{init}"));
    assert!(
        line.starts_with("maven: pom.xml found, modules not read; plugin invalid: failed ("),
        "{line}"
    );
    assert!(line.contains("): cannot read pom.xml ("), "{line}");
}
