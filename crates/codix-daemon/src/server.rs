//! The daemon process: holds the project's locks, serves its socket, and stops
//! when asked or after `daemon.idle_timeout` without requests.

use std::fs::{self, File, OpenOptions, TryLockError};
use std::io::{BufReader, Write};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, PoisonError};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use codix_core::Error;
use codix_core::config::Config;
use codix_core::daemon::{DAEMON_FILE, DAEMON_LOCK, DaemonRecord, PROTOCOL, read_record};
use codix_core::lock::{HeldGate, WriteLock};
use codix_core::project::Project;
use interprocess::local_socket::{GenericFilePath, ListenerOptions, Stream, prelude::*};

use crate::paths::socket_name;
use crate::protocol::{Request, Response, Stats, read_frame, write_frame};
use crate::registry;

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
    last_request: Mutex<Instant>,
    stopping: AtomicBool,
    // Used by the operation requests of the socket protocol.
    _gate: Arc<HeldGate>,
}

impl State {
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

    let state = Arc::new(State {
        record,
        last_request: Mutex::new(Instant::now()),
        stopping: AtomicBool::new(false),
        _gate: Arc::new(HeldGate::new(write_lock)),
    });
    watch_idle(&state, config.idle_timeout());

    for connection in listener.incoming() {
        if state.stopping.load(Ordering::SeqCst) {
            break;
        }
        match connection {
            Ok(stream) => {
                let state = Arc::clone(&state);
                thread::spawn(move || serve(&state, &stream));
            }
            Err(e) => log(&format!("accept failed: {e}")),
        }
    }

    drop(listener);
    fs::remove_file(project.data_dir().join(DAEMON_FILE)).ok();
    registry::unregister(project.root());
    log("daemon stopped");
    Ok(())
}

/// Answers the requests of one connection until the client closes it.
fn serve(state: &State, stream: &Stream) {
    let mut reader = BufReader::new(stream);
    let mut writer = stream;
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
