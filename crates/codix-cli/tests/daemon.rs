//! `codix daemon`: one daemon per project, started, listed and stopped from
//! the command line.
//!
//! Tests named `live_*` bind real sockets. The Claude Code sandbox refuses
//! that, so they run on CI; locally inside it, the suite runs with
//! `--skip live_`.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

use codix_testkit::Run;
use codix_testkit::fixture::{files_only, indexed};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

/// A runtime directory of the test's own, so registries do not mix; every
/// daemon registered in it is stopped on drop.
struct Runtime {
    dir: tempfile::TempDir,
}

impl Runtime {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().expect("runtime dir"),
        }
    }

    fn path(&self) -> &Path {
        self.dir.path()
    }

    fn codix(&self, cwd: &Path, args: &[&str]) -> Run {
        self.codix_with(cwd, args, &[])
    }

    fn codix_with(&self, cwd: &Path, args: &[&str], env: &[(&str, &str)]) -> Run {
        let mut command = Command::new(CODIX);
        command
            .args(args)
            .current_dir(cwd)
            .env("CODIX_RUNTIME_DIR", self.path())
            .env_remove("NO_COLOR");
        for (key, value) in env {
            command.env(key, value);
        }
        let output = command.output().expect("run codix");
        Run {
            code: output.status.code(),
            stdout: String::from_utf8(output.stdout).expect("UTF-8"),
            stderr: String::from_utf8(output.stderr).expect("UTF-8"),
        }
    }

    fn json(&self, cwd: &Path, args: &[&str]) -> Value {
        let mut args = args.to_vec();
        args.push("--json");
        let out = self.codix(cwd, &args);
        serde_json::from_str(&out.stdout)
            .unwrap_or_else(|e| panic!("{args:?}: {e}\n{}\n{}", out.stdout, out.stderr))
    }

    /// The PID of the daemon started in `root`.
    fn start(&self, root: &Path) -> u64 {
        let started = self.json(root, &["daemon", "start"]);
        started["result"]["daemon"]["pid"]
            .as_u64()
            .unwrap_or_else(|| panic!("daemon start: {started}"))
    }
}

impl Drop for Runtime {
    fn drop(&mut self) {
        // Best effort: a test that failed may have left nothing to stop.
        Command::new(CODIX)
            .args(["daemon", "stop", "--all"])
            .env("CODIX_RUNTIME_DIR", self.path())
            .output()
            .ok();
    }
}

fn project() -> (tempfile::TempDir, PathBuf) {
    indexed(CODIX, &[("src/A.java", "class A {}\n")])
}

fn daemon_pid(status: &Value) -> Option<u64> {
    status["result"]["daemon"]["pid"].as_u64()
}

#[test]
fn daemon_commands_without_a_daemon_report_none() {
    let runtime = Runtime::new();
    let (_guard, root) = project();
    let out = runtime.codix(&root, &["daemon", "stop"]);
    assert_eq!(
        (out.code, out.stdout.as_str()),
        (Some(1), "no daemon running\n")
    );
    let out = runtime.codix(&root, &["daemon", "list"]);
    assert_eq!(
        (out.code, out.stdout.as_str()),
        (Some(1), "no daemon running\n")
    );
    let out = runtime.codix(&root, &["daemon", "stop", "--all"]);
    assert_eq!(out.code, Some(1));
    assert_eq!(daemon_pid(&runtime.json(&root, &["status"])), None);
}

#[test]
fn a_daemon_needs_an_indexed_project() {
    let runtime = Runtime::new();
    let (_guard, root) = files_only(&[("codix.toml", "")]);
    let out = runtime.codix(&root, &["daemon", "start"]);
    assert_eq!(out.code, Some(6), "{}", out.stderr);
    assert!(
        out.stderr.starts_with("error[not_indexed]"),
        "{}",
        out.stderr
    );

    let (_guard, elsewhere) = files_only(&[]);
    let out = runtime.codix(&elsewhere, &["daemon", "start"]);
    assert_eq!(out.code, Some(6), "{}", out.stderr);
    assert!(
        out.stderr.starts_with("error[no_project]"),
        "{}",
        out.stderr
    );
}

#[test]
fn live_projects_side_by_side_get_their_own_daemons() {
    let runtime = Runtime::new();
    let (_a_guard, a) = project();
    let (_b_guard, b) = project();
    let pid_a = runtime.start(&a);
    let pid_b = runtime.start(&b);
    assert_ne!(pid_a, pid_b);

    assert_eq!(daemon_pid(&runtime.json(&a, &["status"])), Some(pid_a));
    let status = runtime.codix(&b.join("src"), &["status"]);
    assert!(
        status.stdout.contains(&format!("daemon: pid {pid_b}, up ")),
        "{}",
        status.stdout
    );

    let list = runtime.json(&a, &["daemon", "list"]);
    let mut roots: Vec<(String, u64)> = list["result"]["daemons"]
        .as_array()
        .expect("daemons")
        .iter()
        .map(|d| {
            (
                d["daemon"]["root"].as_str().expect("root").to_owned(),
                d["daemon"]["pid"].as_u64().expect("pid"),
            )
        })
        .collect();
    roots.sort();
    let mut expected = vec![
        (a.display().to_string(), pid_a),
        (b.display().to_string(), pid_b),
    ];
    expected.sort();
    assert_eq!(roots, expected);
    assert!(list["result"]["daemons"][0]["stats"]["memory_bytes"].as_u64() > Some(0));

    let out = runtime.codix(&a, &["daemon", "stop"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(daemon_pid(&runtime.json(&a, &["status"])), None);
    assert_eq!(daemon_pid(&runtime.json(&b, &["status"])), Some(pid_b));

    let stopped = runtime.json(&a, &["daemon", "stop", "--all"]);
    assert_eq!(stopped["result"]["stopped"][0]["pid"].as_u64(), Some(pid_b));
    assert_eq!(runtime.codix(&a, &["daemon", "list"]).code, Some(1));
}

#[test]
fn live_nested_projects_and_worktrees_get_their_own_daemons() {
    let runtime = Runtime::new();
    let (_guard, outer) = indexed(
        CODIX,
        &[
            ("A.java", "class A {}\n"),
            ("inner/B.java", "class B {}\n"),
            ("tree/.git", "gitdir: ../.git/worktrees/tree\n"),
            ("tree/C.java", "class C {}\n"),
        ],
    );
    let inner = outer.join("inner");
    let tree = outer.join("tree");
    for root in [&inner, &tree] {
        let out = runtime.codix(root, &["init"]);
        assert_eq!(out.code, Some(0), "{}", out.stderr);
    }
    let pids = [
        runtime.start(&outer),
        runtime.start(&inner),
        runtime.start(&tree),
    ];
    assert!(pids[0] != pids[1] && pids[1] != pids[2] && pids[0] != pids[2]);
    for (root, pid) in [(&outer, pids[0]), (&inner, pids[1]), (&tree, pids[2])] {
        let status = runtime.json(root, &["status"]);
        assert_eq!(daemon_pid(&status), Some(pid), "{}", root.display());
        assert_eq!(
            status["result"]["daemon"]["root"].as_str(),
            Some(root.display().to_string().as_str())
        );
    }
}

#[test]
fn live_a_second_start_fails_naming_the_running_daemon() {
    let runtime = Runtime::new();
    let (_guard, root) = project();
    let pid = runtime.start(&root);
    let out = runtime.codix(&root, &["daemon", "start"]);
    assert_eq!(out.code, Some(4), "{}", out.stderr);
    assert!(
        out.stderr.starts_with(&format!(
            "error[daemon_running]: a daemon already serves this project (pid {pid}, socket "
        )),
        "{}",
        out.stderr
    );
}

#[test]
fn live_a_killed_daemon_is_replaced_on_start() {
    let runtime = Runtime::new();
    let (_guard, root) = project();
    let pid = runtime.start(&root);
    kill(pid);
    wait_until(|| daemon_pid(&runtime.json(&root, &["status"])).is_none());
    assert!(root.join(".codix/daemon.json").exists(), "the record stays");

    let new_pid = runtime.start(&root);
    assert_ne!(new_pid, pid);
    assert_eq!(daemon_pid(&runtime.json(&root, &["status"])), Some(new_pid));
}

#[test]
fn live_a_daemon_of_another_version_is_replaced() {
    let runtime = Runtime::new();
    let (_guard, root) = project();
    let out = runtime.codix_with(
        &root,
        &["daemon", "start"],
        &[("CODIX_TEST_DAEMON_VERSION", "0.0.1-old")],
    );
    assert_eq!(out.code, Some(0), "{}", out.stderr);

    let out = runtime.codix(&root, &["daemon", "start"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(
        out.stdout
            .starts_with("stopped the codix 0.0.1-old daemon (pid "),
        "{}",
        out.stdout
    );
    let status = runtime.json(&root, &["status"]);
    assert_eq!(
        status["result"]["daemon"]["version"].as_str(),
        Some(env!("CARGO_PKG_VERSION"))
    );
}

#[cfg(unix)]
#[test]
fn live_a_long_root_puts_the_socket_in_the_runtime_directory() {
    let runtime = Runtime::new();
    let deep = vec!["d".repeat(30); 4].join("/");
    let (_guard, top) = files_only(&[(format!("{deep}/A.java").as_str(), "class A {}\n")]);
    let root = top.join(&deep);
    assert!(root.join(".codix/daemon.sock").as_os_str().len() > 108);
    assert_eq!(runtime.codix(&root, &["init"]).code, Some(0));

    let started = runtime.json(&root, &["daemon", "start"]);
    let socket = started["result"]["daemon"]["socket"]
        .as_str()
        .expect("socket");
    assert!(
        Path::new(socket).starts_with(runtime.path()),
        "{socket} is under {}",
        runtime.path().display()
    );
    assert!(Path::new(socket).exists());
}

#[test]
fn live_an_idle_daemon_stops_by_itself() {
    let runtime = Runtime::new();
    let (_guard, root) = project();
    let out = runtime.codix(&root, &["config", "daemon.idle_timeout", "1s"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    runtime.start(&root);
    // Watched without a request: `status` would go to the daemon and count
    // as activity.
    wait_until(|| !root.join(".codix/daemon.json").exists());
    assert_eq!(daemon_pid(&runtime.json(&root, &["status"])), None);
    assert_eq!(runtime.codix(&root, &["daemon", "list"]).code, Some(1));
}

fn wait_until(condition: impl Fn() -> bool) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while !condition() {
        assert!(Instant::now() < deadline, "timed out");
        std::thread::sleep(Duration::from_millis(50));
    }
}

fn kill(pid: u64) {
    let pid = pid.to_string();
    #[cfg(unix)]
    let status = Command::new("kill").args(["-9", &pid]).status();
    #[cfg(windows)]
    let status = Command::new("taskkill").args(["/F", "/PID", &pid]).status();
    assert!(status.expect("kill").success());
}
