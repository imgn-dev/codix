//! The write lock across processes. The holder is this test binary re-run with
//! `CODIX_LOCK_HOLDER` set, so no helper binary is needed.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

use codix_core::lock::{FileGate, HeldGate, QUERY_WAIT, WriteGate, WriteLock};
use codix_core::project::Project;

const HOLDER_VAR: &str = "CODIX_LOCK_HOLDER";

fn project(root: &Path) -> Project {
    std::fs::create_dir_all(root.join(".codix")).expect("create .codix");
    Project::discover(root).expect("discover").expect("project")
}

/// Run as a child: takes the lock, says so, and holds it until killed.
#[test]
fn lock_holder() {
    let Some(root) = std::env::var_os(HOLDER_VAR) else {
        return;
    };
    let _lock = WriteLock::acquire(&project(Path::new(&root))).unwrap();
    println!("locked");
    loop {
        std::thread::sleep(Duration::from_secs(60));
    }
}

fn spawn_holder(root: &Path) -> Child {
    let mut child = Command::new(std::env::current_exe().expect("test setup"))
        .args(["--exact", "lock_holder", "--nocapture", "--test-threads=1"])
        .env(HOLDER_VAR, root)
        .stdout(Stdio::piped())
        .spawn()
        .expect("test setup");
    let stdout = child.stdout.take().expect("test setup");
    let locked = BufReader::new(stdout)
        .lines()
        .map_while(Result::ok)
        .any(|line| line.contains("locked"));
    assert!(locked, "the holder did not take the lock");
    child
}

#[test]
fn a_second_process_times_out_then_gets_the_lock_once_the_holder_is_killed() {
    let dir = tempfile::tempdir().unwrap();
    let p = project(dir.path());
    let mut holder = spawn_holder(dir.path());

    let start = Instant::now();
    assert!(
        WriteLock::try_acquire_for(&p, QUERY_WAIT)
            .unwrap()
            .is_none()
    );
    assert!(
        start.elapsed() >= QUERY_WAIT,
        "waited {:?}",
        start.elapsed()
    );

    // SIGKILL on Unix, TerminateProcess on Windows: no chance to clean up.
    holder.kill().unwrap();
    holder.wait().unwrap();
    assert!(
        WriteLock::try_acquire_for(&p, Duration::from_secs(5))
            .unwrap()
            .is_some()
    );
}

#[test]
fn the_lock_is_released_on_drop() {
    let dir = tempfile::tempdir().unwrap();
    let p = project(dir.path());
    let lock = WriteLock::acquire(&p).unwrap();
    drop(lock);
    // Not zero: a test spawning a process at the same moment briefly shares
    // the lock file between fork and exec.
    assert!(
        WriteLock::try_acquire_for(&p, Duration::from_secs(1))
            .unwrap()
            .is_some()
    );
}

#[test]
fn a_held_gate_lets_one_writer_in_at_a_time() {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = project(dir.path());
    let gate = HeldGate::new(WriteLock::acquire(&p).expect("lock"));
    let first = gate.enter(&p, None).expect("enter").expect("free gate");

    let start = Instant::now();
    assert!(gate.enter(&p, Some(QUERY_WAIT)).expect("enter").is_none());
    assert!(start.elapsed() >= QUERY_WAIT);

    drop(first);
    assert!(gate.enter(&p, Some(QUERY_WAIT)).expect("enter").is_some());
}

#[test]
fn a_held_gate_keeps_other_processes_and_file_gates_out() {
    let dir = tempfile::tempdir().expect("tempdir");
    let p = project(dir.path());
    let gate = HeldGate::new(WriteLock::acquire(&p).expect("lock"));
    assert!(
        FileGate
            .enter(&p, Some(QUERY_WAIT))
            .expect("enter")
            .is_none()
    );
    drop(gate);
    assert!(
        FileGate
            .enter(&p, Some(QUERY_WAIT))
            .expect("enter")
            .is_some()
    );
}
