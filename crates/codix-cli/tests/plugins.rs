//! A version control system installed as a plugin behaves as a compiled-in
//! one, and a broken plugin is listed with its reason without failing other
//! commands.
#![cfg(all(feature = "plugins", not(with_vcs_hg)))]

use std::fs;
use std::path::Path;

use codix_testkit::{Run, run_with_env};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const HG_DESCRIPTOR: &str = include_str!("../../codix-vcs-hg/descriptor.toml");

fn write(path: &Path, text: &str) {
    fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
    fs::write(path, text).expect("write");
}

/// Runs codix in `root` with only the plugins under `plugins`.
fn codix(root: &Path, plugins: &Path, args: &[&str]) -> Run {
    let path = plugins.to_str().expect("UTF-8 path");
    let data = plugins.join("no-user-plugins");
    let data = data.to_str().expect("UTF-8 path");
    let out = run_with_env(
        root,
        CODIX,
        args,
        &[
            ("CODIX_PLUGIN_PATH", path),
            ("XDG_DATA_HOME", data),
            ("LOCALAPPDATA", data),
        ],
    );
    assert_eq!(out.code, Some(0), "{args:?}: {}", out.stderr);
    out
}

#[test]
fn a_vcs_plugin_behaves_as_a_compiled_in_one() {
    let dir = tempfile::tempdir().expect("temp dir");
    let base = codix_core::project::canonicalize(dir.path()).expect("canonical");
    let plugins = base.join("plugins");
    let hg = plugins.join("vcs").join("mercurial");
    write(
        &hg.join("plugin.toml"),
        "schema = 1\nkind = \"vcs\"\nname = \"mercurial\"\nversion = \"1.0.0\"\n",
    );
    write(&hg.join("descriptor.toml"), HG_DESCRIPTOR);
    let broken = plugins.join("vcs").join("broken");
    write(&broken.join("plugin.toml"), "schema = 1\n");

    let root = base.join("project");
    write(&root.join("notes.txt"), "hello\n");
    write(&root.join(".hg").join("store").join("data"), "x\n");

    let init = codix(&root, &plugins, &["init"]).stdout;
    assert!(init.contains("version control: mercurial\n"), "{init}");
    assert!(
        init.contains("hint: add `^\\.codix/` to .hgignore\n"),
        "{init}"
    );
    assert!(!init.contains("mercurial: "), "{init}");

    let status: Value =
        serde_json::from_str(&codix(&root, &plugins, &["status", "--json"]).stdout).expect("JSON");
    let files = &status["result"]["files"];
    assert_eq!(files["text"], 1, ".hg is skipped: {status}");
    assert_eq!(status["result"]["missing"], serde_json::json!([]));

    let languages: Value =
        serde_json::from_str(&codix(&root, &plugins, &["languages", "--json"]).stdout)
            .expect("JSON");
    let providers = languages["result"]["providers"]
        .as_array()
        .expect("providers");
    let find = |name: &str| {
        providers
            .iter()
            .find(|p| p["kind"] == "vcs" && p["name"] == name)
            .unwrap_or_else(|| panic!("{name} in {providers:?}"))
    };
    assert_eq!(
        find("mercurial"),
        &serde_json::json!({
            "kind": "vcs",
            "name": "mercurial",
            "version": "1.0.0",
            "source": { "plugin": hg },
            "status": "available",
            "reason": null,
        })
    );
    let broken_entry = find("broken");
    assert_eq!(broken_entry["status"], "invalid");
    assert_eq!(broken_entry["source"]["plugin"], serde_json::json!(broken));
    assert!(
        broken_entry["reason"]
            .as_str()
            .expect("reason")
            .starts_with("plugin.toml: "),
        "{broken_entry}"
    );
    assert_eq!(find("subversion")["status"], "not_installed");

    let text = codix(&root, &plugins, &["languages"]).stdout;
    let line = format!("1.0.0  {}", hg.display());
    assert!(
        text.lines()
            .any(|l| l.contains(" mercurial ") && l.ends_with(&line)),
        "{text}"
    );
}
