//! The daemon process: holds the project's locks, serves its socket, and stops
//! when asked or after `daemon.idle_timeout` without requests.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use codix_core::Error;
use codix_core::config::Config;
use codix_core::daemon::{DAEMON_FILE, DAEMON_LOCK, DaemonRecord, PROTOCOL, Watch, read_record};
use codix_core::dispatch::dispatch;
use codix_core::lock::{HeldGate, WriteGate, WriteLock, network_filesystem_warning};
use codix_core::op::{Context, Warning};
use codix_core::project::Project;
use interprocess::local_socket::{GenericFilePath, ListenerOptions, Stream, prelude::*};

use crate::paths::socket_name;
use crate::protocol::{OpRequest, Request, Response, Stats, read_frame, write_frame};
use crate::registry;
use crate::watch;

/// Makes the daemon report this version instead of its own, for tests only.
pub const VERSION_VAR: &str = "CODIX_TEST_DAEMON_VERSION";

/// How long the daemon retries its lock at start: another process may hold it
/// for an instant to check whether a daemon runs.
const LOCK_RETRY: Duration = Duration::from_secs(1);
const POLL: Duration = Duration::from_millis(10);

/// The codix version this daemon reports.
pub fn reported_version() -> String {
    std::env::var(VERSION_VAR)
        .ok()
        .filter(|v| !v.is_empty())
        .unwrap_or_else(|| env!("CARGO_PKG_VERSION").to_owned())
}

/// What every connection shares.
struct State {
    record: DaemonRecord,
    project: Project,
    warnings: Vec<Warning>,
    gate: Arc<dyn WriteGate>,
    watch: Arc<dyn Watch>,
    last_request: Mutex<Instant>,
    stopping: AtomicBool,
}

impl State {
    fn touch(&self) {
        *self
            .last_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner) = Instant::now();
    }

    /// Runs an operation as the client would have in-process, with this
    /// daemon's write gate.
    fn run(&self, op: OpRequest) -> Response {
        self.touch();
        let result = Context::within(&self.project, &self.warnings, &op.cwd, op.root.as_deref())
            .and_then(|ctx| {
                let ctx = ctx
                    .with_gate(Arc::clone(&self.gate))
                    .with_watch(Arc::clone(&self.watch));
                dispatch(&ctx, &op.name, op.request)
            });
        // A long operation is activity until it ends.
        self.touch();
        let warnings = self.warnings.clone();
        match result {
            Ok(result) => Response::Done { result, warnings },
            Err(error) => Response::Failed {
                error: error.report(),
                warnings,
            },
        }
    }

    fn idle(&self) -> Duration {
        self.last_request
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .elapsed()
    }

    /// Flags the stop and wakes the accept loop with a connection of its own.
    fn stop(&self) {
        self.stopping.store(true, Ordering::SeqCst);
        if let Ok(name) = self.record.socket.as_str().to_fs_name::<GenericFilePath>() {
            // Failing to connect means the listener is gone already.
            Stream::connect(name).ok();
        }
    }
}

/// Where log lines go: the file given to `daemon run --log`, else standard
/// error.
static LOG: OnceLock<Mutex<File>> = OnceLock::new();

/// Runs the daemon for the project at `root` until it is stopped or idle,
/// logging to `log_file` when given, else to standard error.
pub fn run(root: &Path, log_file: Option<&Path>) -> Result<(), Error> {
    if let Some(path) = log_file {
        let file = OpenOptions::new()
            .create(true)
            .append(true)
            .open(path)
            .map_err(|source| Error::Io {
                path: path.to_path_buf(),
                source,
            })?;
        LOG.set(Mutex::new(file)).ok();
    }
    let result = serve_project(root);
    // Started detached, the daemon has no other place to say why it stopped.
    if let Err(error) = &result {
        log(&format!("error[{}]: {}", error.code(), error.cause()));
    }
    result
}

fn serve_project(root: &Path) -> Result<(), Error> {
    let project = Project::at(root)?;
    project.require_indexed()?;
    let config = Config::load(&project)?;
    let _daemon_lock = take_daemon_lock(&project)?;
    let write_lock = WriteLock::acquire(&project)?;

    let socket = socket_name(&project)?;
    let name = socket
        .as_str()
        .to_fs_name::<GenericFilePath>()
        .map_err(|source| io_error(&socket, source))?;
    // The daemon lock is held, so a socket file left here belongs to a daemon
    // that died.
    let listener = ListenerOptions::new()
        .name(name)
        .try_overwrite(true)
        .create_sync()
        .map_err(|source| io_error(&socket, source))?;

    let record = DaemonRecord {
        root: project.root().to_path_buf(),
        socket,
        pid: std::process::id(),
        version: reported_version(),
        protocol: PROTOCOL,
        plugins: codix_core::providers::plugins_fingerprint().to_owned(),
        started_at: SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    };
    registry::write_json(&project.data_dir().join(DAEMON_FILE), &record)?;
    registry::register(&record)?;
    log(&format!(
        "codix {} daemon serving {} on {} (pid {})",
        record.version,
        record.root.display(),
        record.socket,
        record.pid
    ));

    let warnings = network_filesystem_warning(&project.data_dir())
        .into_iter()
        .collect();
    let gate: Arc<dyn WriteGate> = Arc::new(HeldGate::new(write_lock));
    let watch = Arc::new(watch::start(project.clone(), Arc::clone(&gate), log));
    let state = Arc::new(State {
        record,
        project,
        warnings,
        gate,
        watch,
        last_request: Mutex::new(Instant::now()),
        stopping: AtomicBool::new(false),
    });
    watch_idle(&state, config.idle_timeout());

    for connection in listener.incoming() {
        if state.stopping.load(Ordering::SeqCst) {
            break;
        }
        match connection {
            Ok(stream) => {
                let state = Arc::clone(&state);
                thread::spawn(move || serve(&state, BufReader::new(&stream), &stream));
            }
            Err(e) => log(&format!("accept failed: {e}")),
        }
    }

    drop(listener);
    fs::remove_file(state.project.data_dir().join(DAEMON_FILE)).ok();
    registry::unregister(state.project.root());
    log("daemon stopped");
    Ok(())
}

/// Answers the requests of one connection until the client closes it.
fn serve(state: &State, mut reader: impl BufRead, mut writer: impl Write) {
    loop {
        let request = match read_frame::<Request>(&mut reader) {
            Ok(Some(request)) => request,
            Ok(None) => return,
            Err(e) => {
                log(&format!("malformed request: {e}"));
                return;
            }
        };
        let response = match request {
            Request::Hello { .. } => Response::Hello(state.record.clone()),
            Request::Stats => Response::Stats(Stats {
                memory_bytes: memory_bytes(),
                idle_seconds: state.idle().as_secs(),
            }),
            Request::Stop => Response::Stopping,
            Request::Op(op) => state.run(op),
        };
        let stop = response == Response::Stopping;
        if let Err(e) = write_frame(&mut writer, &response) {
            log(&format!("cannot answer: {e}"));
            return;
        }
        if stop {
            log("stop requested");
            state.stop();
            return;
        }
    }
}

/// Stops the daemon once no request came for `timeout`.
fn watch_idle(state: &Arc<State>, timeout: Duration) {
    let state = Arc::clone(state);
    let tick = timeout.min(Duration::from_secs(1));
    thread::spawn(move || {
        loop {
            thread::sleep(tick);
            if state.stopping.load(Ordering::SeqCst) {
                return;
            }
            if state.idle() >= timeout {
                log("idle timeout reached");
                state.stop();
                return;
            }
        }
    });
}

/// Takes `.codix/daemon.lock`, held until the process exits; busy means
/// another daemon serves the project.
fn take_daemon_lock(project: &Project) -> Result<File, Error> {
    let path = project.data_dir().join(DAEMON_LOCK);
    let file = OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|source| Error::Io {
            path: path.clone(),
            source,
        })?;
    let deadline = Instant::now() + LOCK_RETRY;
    loop {
        match file.try_lock() {
            Ok(()) => return Ok(file),
            Err(TryLockError::WouldBlock) if Instant::now() < deadline => thread::sleep(POLL),
            Err(TryLockError::WouldBlock) => {
                let running = read_record(project);
                return Err(Error::DaemonRunning {
                    pid: running.as_ref().map_or(0, |r| r.pid),
                    socket: running.map_or_else(|| "not yet open".to_owned(), |r| r.socket),
                });
            }
            Err(TryLockError::Error(source)) => return Err(Error::Io { path, source }),
        }
    }
}

/// The daemon's resident memory, in bytes; 0 when the platform does not say.
fn memory_bytes() -> u64 {
    let pid = sysinfo::Pid::from_u32(std::process::id());
    let Ok(mut system) = sysinfo::System::new() else {
        return 0;
    };
    system.refresh_processes_specifics(
        sysinfo::ProcessesToUpdate::Some(&[pid]),
        true,
        sysinfo::ProcessRefreshKind::nothing().with_memory(),
    );
    system.process(pid).map_or(0, sysinfo::Process::memory)
}

fn io_error(socket: &str, source: std::io::Error) -> Error {
    Error::Io {
        path: socket.into(),
        source,
    }
}

/// One line in the log. A line that cannot be written is lost; the daemon
/// carries on.
fn log(message: &str) {
    match LOG.get() {
        Some(file) => {
            let mut file = file.lock().unwrap_or_else(PoisonError::into_inner);
            writeln!(file, "{message}").ok();
        }
        None => {
            writeln!(std::io::stderr().lock(), "{message}").ok();
        }
    }
}

#[cfg(test)]
mod tests {
    use std::io::Cursor;
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;

    /// A watcher that covers nothing, so the tests run no scan pipeline.
    #[derive(Debug)]
    struct NoWatch;

    impl Watch for NoWatch {
        fn flush(&self) -> bool {
            false
        }

        fn status(&self) -> codix_core::daemon::WatchStatus {
            codix_core::daemon::WatchStatus {
                recursive: false,
                watched: 0,
                unwatched: 0,
                rescan_seconds: 0,
                last_batch_ms: None,
            }
        }
    }

    fn state(root: &Path) -> State {
        fs::create_dir_all(root.join(".codix")).expect("create .codix");
        let project = Project::at(root).expect("project");
        let lock = WriteLock::acquire(&project).expect("lock");
        State {
            record: DaemonRecord {
                root: project.root().to_path_buf(),
                socket: "unused".to_owned(),
                pid: 1,
                version: "test".to_owned(),
                protocol: PROTOCOL,
                plugins: String::new(),
                started_at: 0,
            },
            project,
            warnings: Vec::new(),
            gate: Arc::new(HeldGate::new(lock)),
            watch: Arc::new(NoWatch),
            last_request: Mutex::new(Instant::now()),
            stopping: AtomicBool::new(false),
        }
    }

    /// Serves `requests`, one frame each, and returns the responses.
    fn exchange(state: &State, requests: &[Request]) -> Vec<Response> {
        let mut input = Vec::new();
        for request in requests {
            write_frame(&mut input, request).expect("frame");
        }
        let mut output = Vec::new();
        serve(state, Cursor::new(input), &mut output);
        let mut reader = Cursor::new(output);
        let mut responses = Vec::new();
        while let Some(response) = read_frame(&mut reader).expect("response") {
            responses.push(response);
        }
        responses
    }

    fn op(state: &State, name: &str, request: serde_json::Value) -> Request {
        Request::Op(OpRequest {
            name: name.to_owned(),
            cwd: state.project.root().to_path_buf(),
            root: None,
            request,
        })
    }

    #[test]
    fn answers_each_request_in_order_on_one_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(dir.path());
        let requests = [
            Request::Hello {
                protocol: PROTOCOL,
                version: "test".to_owned(),
            },
            op(&state, "capabilities", json!({})),
            Request::Stats,
        ];
        let responses = exchange(&state, &requests);
        assert_eq!(responses.len(), 3);
        assert_eq!(responses[0], Response::Hello(state.record.clone()));
        assert!(
            matches!(&responses[1], Response::Done { result, .. } if result["languages"].is_array()),
            "{:?}",
            responses[1]
        );
        assert!(matches!(responses[2], Response::Stats(_)));
    }

    #[test]
    fn errors_cross_as_reports() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(dir.path());
        let responses = exchange(
            &state,
            &[
                op(&state, "status", json!({})),
                op(&state, "frobnicate", json!({})),
            ],
        );
        let Response::Failed { error, .. } = &responses[0] else {
            panic!("{:?}", responses[0]);
        };
        assert_eq!((error.code.as_str(), error.exit_code), ("not_indexed", 6));
        assert!(error.hint.contains("codix init"), "{}", error.hint);
        let Response::Failed { error, .. } = &responses[1] else {
            panic!("{:?}", responses[1]);
        };
        assert_eq!(error.cause, "unknown operation `frobnicate`");
    }

    #[test]
    fn a_request_from_another_project_is_refused() {
        let dir = tempfile::tempdir().expect("tempdir");
        let other = tempfile::tempdir().expect("tempdir");
        let state = state(dir.path());
        let request = Request::Op(OpRequest {
            name: "status".to_owned(),
            cwd: PathBuf::from(other.path()),
            root: None,
            request: json!({}),
        });
        let responses = exchange(&state, &[request]);
        let Response::Failed { error, .. } = &responses[0] else {
            panic!("{:?}", responses[0]);
        };
        assert_eq!(error.code, "usage");
        assert!(
            error.cause.contains("is not in the project at"),
            "{}",
            error.cause
        );
    }

    #[test]
    fn a_malformed_frame_closes_the_connection() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(dir.path());
        let mut output = Vec::new();
        serve(
            &state,
            Cursor::new(b"not json\n\"stats\"\n".to_vec()),
            &mut output,
        );
        assert_eq!(String::from_utf8_lossy(&output), "");
    }

    #[test]
    fn stop_is_answered_then_the_connection_ends() {
        let dir = tempfile::tempdir().expect("tempdir");
        let state = state(dir.path());
        let responses = exchange(&state, &[Request::Stop, Request::Stats]);
        assert_eq!(responses, [Response::Stopping]);
        assert!(state.stopping.load(Ordering::SeqCst));
    }
}
