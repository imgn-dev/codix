//! Paths shown relative to the cwd, `--root`, `--here` and `--path`.

use std::fs;
use std::path::PathBuf;

use codix_testkit::{Run, run_in};
use serde_json::Value;
use tempfile::TempDir;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn project() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("test setup");
    let root = fs::canonicalize(dir.path()).expect("test setup");
    for sub in [".codix", "src", "tests", "docs"] {
        fs::create_dir_all(root.join(sub)).expect("test setup");
    }
    (dir, root)
}

fn locations(out: &Run) -> Vec<String> {
    out.stdout
        .lines()
        .filter_map(|line| line.split_whitespace().next())
        .filter(|cell| cell.contains(".rs:"))
        .map(str::to_owned)
        .collect()
}

#[test]
fn paths_are_relative_to_a_subdirectory_cwd() {
    let (_guard, root) = project();
    let out = run_in(&root.join("src"), CODIX, &["debug-echo", "4"]);
    assert_eq!(
        locations(&out),
        [
            "file0.rs:10:1",
            "file1.rs:11:2",
            "file2.rs:12:3",
            "../tests/file3.rs:13:4"
        ]
    );
}

#[test]
fn paths_outside_the_cwd_climb_with_dot_dot() {
    let (_guard, root) = project();
    let out = run_in(&root.join("docs"), CODIX, &["debug-echo", "1"]);
    assert_eq!(locations(&out), ["../src/file0.rs:10:1"]);
}

#[test]
fn json_carries_root_relative_paths() {
    let (_guard, root) = project();
    let out = run_in(&root.join("docs"), CODIX, &["debug-echo", "1", "--json"]);
    let value: Value = serde_json::from_str(&out.stdout).unwrap();
    let item = &value["result"]["items"][0];
    assert_eq!(item["path"], "../src/file0.rs");
    assert_eq!(item["root_path"], "src/file0.rs");
}

#[test]
fn here_limits_to_the_cwd_subtree() {
    let (_guard, root) = project();
    let out = run_in(&root.join("tests"), CODIX, &["debug-echo", "8", "--here"]);
    assert_eq!(locations(&out), ["file3.rs:17:3", "file3.rs:13:4"]);
}

#[test]
fn path_limits_to_the_given_subtrees() {
    let (_guard, root) = project();
    let out = run_in(&root, CODIX, &["debug-echo", "8", "--path", "tests"]);
    assert_eq!(
        locations(&out),
        ["tests/file3.rs:17:3", "tests/file3.rs:13:4"]
    );
}

#[test]
fn root_flag_selects_the_project() {
    let (_guard, root) = project();
    let outside = tempfile::tempdir().unwrap();
    let root_arg = root.join("src");
    let out = run_in(
        outside.path(),
        CODIX,
        &[
            "debug-echo",
            "1",
            "--root",
            root_arg.to_str().unwrap(),
            "--here",
        ],
    );
    assert_eq!(out.code, Some(2), "{}", out.stderr);
    assert!(
        out.stderr.starts_with("error[usage]: --here: "),
        "{}",
        out.stderr
    );

    let out = run_in(
        outside.path(),
        CODIX,
        &[
            "debug-echo",
            "1",
            "--root",
            root.to_str().unwrap(),
            "--path",
            "src",
        ],
    );
    assert_eq!(out.code, Some(0), "{}", out.stderr);
}
