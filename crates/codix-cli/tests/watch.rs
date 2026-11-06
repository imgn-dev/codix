//! Watch mode: changes reach a daemon's index without `codix index`, and what
//! the watcher misses, the periodic scan finds.
//!
//! `live_*` tests bind real sockets and watch real files, so they run on CI;
//! locally inside the Claude Code sandbox, the suite runs with
//! `--skip live_`.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use codix_testkit::fixture::{indexed, write};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn codix(runtime: &Path, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> codix_testkit::Run {
    let mut command = Command::new(CODIX);
    command
        .args(args)
        .current_dir(cwd)
        .env("CODIX_RUNTIME_DIR", runtime);
    for (key, value) in env {
        command.env(key, value);
    }
    let output = command.output().expect("run codix");
    codix_testkit::Run {
        code: output.status.code(),
        stdout: String::from_utf8(output.stdout).expect("UTF-8"),
        stderr: String::from_utf8(output.stderr).expect("UTF-8"),
    }
}

fn status(runtime: &Path, root: &Path) -> Value {
    let out = codix(runtime, root, &["status", "--json"], &[]);
    serde_json::from_str(&out.stdout).unwrap_or_else(|e| panic!("{e}: {}", out.stdout))
}

/// Whether the index matches the files on disk, as `status` sees it.
fn current(runtime: &Path, root: &Path) -> bool {
    let changed = &status(runtime, root)["result"]["changed"];
    ["new", "modified", "deleted"]
        .iter()
        .all(|k| changed[k].as_array().is_some_and(Vec::is_empty))
}

/// Whether the daemon's watcher has started, which it does once the startup
/// scan is done: changes made from then on reach the index only through the
/// watcher or the periodic scan.
fn watching(runtime: &Path, root: &Path) -> bool {
    let watch = &status(runtime, root)["result"]["daemon"]["watch"];
    watch["recursive"] == true
        || watch["watched"].as_u64().unwrap_or(0) + watch["unwatched"].as_u64().unwrap_or(0) > 0
}

fn wait_until(what: &str, condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(15);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out waiting until {what}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// A started daemon, stopped on drop.
struct Daemon<'a> {
    runtime: &'a Path,
    root: &'a Path,
}

impl<'a> Daemon<'a> {
    fn start(runtime: &'a Path, root: &'a Path, env: &[(&str, &str)]) -> Self {
        let out = codix(runtime, root, &["daemon", "start"], env);
        assert_eq!(out.code, Some(0), "{}", out.stderr);
        Self { runtime, root }
    }
}

impl Drop for Daemon<'_> {
    fn drop(&mut self) {
        codix(self.runtime, self.root, &["daemon", "stop"], &[]);
    }
}

const FILES: &[(&str, &str)] = &[
    ("src/A.java", "class A {}\n"),
    ("src/B.java", "class B {}\n"),
    ("lib/C.java", "class C {}\n"),
];

#[test]
fn live_changes_reach_the_index_without_an_index_command() {
    let runtime = tempfile::tempdir().expect("runtime dir");
    let (_guard, root) = indexed(CODIX, FILES);
    let _daemon = Daemon::start(runtime.path(), &root, &[]);
    wait_until("the startup scan is done", || {
        watching(runtime.path(), &root)
    });

    write(&root, "src/A.java", "class A { void added() {} }\n");
    wait_until("an edit is indexed", || current(runtime.path(), &root));
    write(&root, "src/D.java", "class D {}\n");
    wait_until("a new file is indexed", || current(runtime.path(), &root));
    std::fs::remove_file(root.join("src/B.java")).expect("delete");
    wait_until("a deletion is indexed", || current(runtime.path(), &root));
    std::fs::rename(root.join("lib"), root.join("moved")).expect("rename");
    wait_until("a directory rename is indexed", || {
        current(runtime.path(), &root)
    });

    let daemon = &status(runtime.path(), &root)["result"]["daemon"];
    assert_eq!(daemon["watch"]["unwatched"], 0, "{daemon}");
    assert!(daemon["watch"]["last_batch_ms"].is_u64(), "{daemon}");
    // Text search, which every build has, unlike Java symbols.
    let found = codix(runtime.path(), &root, &["grep", "class C", "--json"], &[]);
    assert!(found.stdout.contains("moved/C.java"), "{}", found.stdout);
}

#[test]
fn live_the_periodic_scan_finds_what_the_watcher_missed() {
    let runtime = tempfile::tempdir().expect("runtime dir");
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(
        runtime.path(),
        &root,
        &["config", "index.scan_interval", "1s"],
        &[],
    );
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    // No watch at all: every event is dropped.
    let _daemon = Daemon::start(runtime.path(), &root, &[("CODIX_TEST_MAX_WATCHES", "0")]);

    wait_until("the startup scan is done", || {
        watching(runtime.path(), &root)
    });
    write(&root, "src/A.java", "class A { void added() {} }\n");
    write(&root, "lib/E.java", "class E {}\n");
    wait_until("the periodic scan indexed the changes", || {
        current(runtime.path(), &root)
    });
    let text = codix(runtime.path(), &root, &["status"], &[]).stdout;
    assert!(
        text.contains("; watching 0 directories; 3 unwatched, rescanned every 1s"),
        "{text}"
    );
}

#[test]
fn live_past_the_watch_quota_some_directories_are_rescanned_periodically() {
    let runtime = tempfile::tempdir().expect("runtime dir");
    let (_guard, root) = indexed(CODIX, FILES);
    let _daemon = Daemon::start(runtime.path(), &root, &[("CODIX_TEST_MAX_WATCHES", "1")]);
    let watch = || status(runtime.path(), &root)["result"]["daemon"]["watch"].clone();
    wait_until("the startup scan is done", || {
        watching(runtime.path(), &root)
    });
    let watch = watch();
    assert_eq!(
        (
            watch["recursive"].as_bool(),
            watch["watched"].as_u64(),
            watch["unwatched"].as_u64()
        ),
        (Some(false), Some(1), Some(2)),
        "{watch}"
    );
    assert_eq!(watch["rescan_seconds"], 60, "{watch}");
}

#[test]
fn live_codix_watch_runs_the_daemon_in_the_foreground() {
    let runtime = tempfile::tempdir().expect("runtime dir");
    let (_guard, root) = indexed(CODIX, FILES);
    let mut child = Command::new(CODIX)
        .arg("watch")
        .current_dir(&root)
        .env("CODIX_RUNTIME_DIR", runtime.path())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .expect("start codix watch");
    let mut lines = BufReader::new(child.stderr.take().expect("stderr")).lines();
    let first = lines.next().expect("a log line").expect("UTF-8");
    assert!(first.contains(" daemon serving "), "{first}");

    let pid = status(runtime.path(), &root)["result"]["daemon"]["pid"].as_u64();
    assert_eq!(pid, Some(u64::from(child.id())));
    let out = codix(runtime.path(), &root, &["daemon", "stop"], &[]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(child.wait().expect("wait").success());
}

#[cfg(feature = "lang-java")]
#[test]
fn live_an_apply_through_the_daemon_leaves_its_index_current() {
    let runtime = tempfile::tempdir().expect("runtime dir");
    let (_guard, root) = indexed(
        CODIX,
        &[(
            "src/A.java",
            "class A {\n    int f() {\n        int x = 1;\n        return x;\n    }\n}\n",
        )],
    );
    let _daemon = Daemon::start(runtime.path(), &root, &[]);
    wait_until("the startup scan is done", || {
        watching(runtime.path(), &root)
    });
    let planned = codix(
        runtime.path(),
        &root,
        &["rename", "src/A.java:3:13", "y"],
        &[],
    );
    assert_eq!(planned.code, Some(0), "{}", planned.stderr);
    let id = planned
        .stdout
        .split_whitespace()
        .nth(1)
        .expect("an ID")
        .trim_end_matches(':')
        .to_owned();
    let applied = codix(runtime.path(), &root, &["apply", &id], &[]);
    assert_eq!(applied.code, Some(0), "{}", applied.stderr);
    assert!(
        std::fs::read_to_string(root.join("src/A.java"))
            .expect("read")
            .contains("return y;")
    );
    assert!(current(runtime.path(), &root));
    let refs = codix(runtime.path(), &root, &["refs", "src/A.java:3:13"], &[]);
    assert!(refs.stdout.contains("src/A.java:4:16"), "{}", refs.stdout);
}
