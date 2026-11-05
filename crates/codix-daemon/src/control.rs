//! Starting, stopping and listing daemons, as `codix daemon` does.

use std::fs::{File, OpenOptions};
use std::io::BufReader;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread;
use std::time::{Duration, Instant};

use codix_core::Error;
use codix_core::daemon::{DAEMON_LOCK, DaemonRecord, LOG_FILE, PROTOCOL, running};
use codix_core::op::Outcome;
use codix_core::project::Project;
use interprocess::local_socket::{GenericFilePath, Stream, prelude::*};
use serde::{Deserialize, Serialize};

use crate::protocol::{Request, Response, Stats, read_frame, write_frame};
use crate::registry;

/// How long `start` waits for a new daemon to answer, and `stop` for one to
/// exit.
const PATIENCE: Duration = Duration::from_secs(10);
/// How long a control request may take before the daemon counts as hung.
const CONTROL_TIMEOUT: Duration = Duration::from_secs(5);
const POLL: Duration = Duration::from_millis(20);

/// Sends one control request to the daemon at `socket`.
pub fn request(socket: &str, request: &Request) -> std::io::Result<Response> {
    let name = socket.to_fs_name::<GenericFilePath>()?;
    let stream = Stream::connect(name)?;
    // Windows named pipes have no I/O timeouts; there a hung daemon makes the
    // request wait.
    match stream.set_recv_timeout(Some(CONTROL_TIMEOUT)) {
        Err(e) if e.kind() != std::io::ErrorKind::Unsupported => return Err(e),
        _ => {}
    }
    let mut writer = &stream;
    write_frame(&mut writer, request)?;
    read_frame(&mut BufReader::new(&stream))?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "the daemon closed the connection",
        )
    })
}

/// The `hello` of the daemon at `socket`; `None` when it does not answer.
pub fn hello(socket: &str) -> Option<DaemonRecord> {
    let request = Request::Hello {
        protocol: PROTOCOL,
        version: env!("CARGO_PKG_VERSION").to_owned(),
    };
    match self::request(socket, &request) {
        Ok(Response::Hello(record)) => Some(record),
        _ => None,
    }
}

/// Whether this codix can talk to the daemon described by `record`.
pub fn compatible(record: &DaemonRecord) -> bool {
    record.protocol == PROTOCOL && record.version == env!("CARGO_PKG_VERSION")
}

/// What `daemon start` did.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StartResponse {
    /// The daemon now running.
    pub daemon: DaemonRecord,
    /// The incompatible daemon it replaced, if any.
    pub replaced: Option<DaemonRecord>,
}

impl Outcome for StartResponse {
    fn has_results(&self) -> bool {
        true
    }
}

/// Starts a daemon for `project` by running `exe daemon run`, detached.
/// A compatible daemon already running is `daemon_running`; an incompatible
/// one is stopped first.
pub fn start(project: &Project, exe: &Path) -> Result<StartResponse, Error> {
    project.require_indexed()?;
    let mut replaced = None;
    if let Some(record) = running(project)? {
        match hello(&record.socket) {
            Some(live) if compatible(&live) => {
                return Err(Error::DaemonRunning {
                    pid: live.pid,
                    socket: live.socket,
                });
            }
            _ => {
                stop_daemon(project, &record)?;
                replaced = Some(record);
            }
        }
    }

    let log_path = project.data_dir().join(LOG_FILE);
    File::create(&log_path).map_err(|source| Error::Io {
        path: log_path.clone(),
        source,
    })?;
    let mut child = launch(exe, project.root(), &log_path)?;

    let deadline = Instant::now() + PATIENCE;
    loop {
        if let Some(record) = running(project)?
            && let Some(live) = hello(&record.socket)
        {
            if let Some(mut child) = child {
                // Reaped by a thread of its own, so a long-lived caller such
                // as `codix mcp` leaves no zombie behind.
                thread::spawn(move || child.wait().ok());
            }
            return Ok(StartResponse {
                daemon: live,
                replaced,
            });
        }
        if let Some(status) = child.as_mut().and_then(|c| c.try_wait().ok().flatten()) {
            return Err(Error::Internal {
                message: format!(
                    "the daemon exited at start ({status}): {}",
                    last_line(&log_path)
                ),
            });
        }
        if Instant::now() >= deadline {
            return Err(Error::Internal {
                message: format!(
                    "the daemon did not answer within {} s: {}",
                    PATIENCE.as_secs(),
                    last_line(&log_path)
                ),
            });
        }
        thread::sleep(POLL);
    }
}

/// What `daemon stop` stopped.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct StopResponse {
    /// Every daemon stopped.
    pub stopped: Vec<DaemonRecord>,
}

impl Outcome for StopResponse {
    fn has_results(&self) -> bool {
        !self.stopped.is_empty()
    }
}

/// Stops the daemon of `project`, if one runs.
pub fn stop(project: &Project) -> Result<StopResponse, Error> {
    let mut stopped = Vec::new();
    if let Some(record) = running(project)? {
        stop_daemon(project, &record)?;
        stopped.push(record);
    }
    Ok(StopResponse { stopped })
}

/// Stops every daemon in the registry.
pub fn stop_all() -> Result<StopResponse, Error> {
    let mut stopped = Vec::new();
    for daemon in list()?.daemons {
        let project = Project::at(&daemon.daemon.root)?;
        stop_daemon(&project, &daemon.daemon)?;
        stopped.push(daemon.daemon);
    }
    Ok(StopResponse { stopped })
}

/// A daemon found in the registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListedDaemon {
    /// Its record.
    pub daemon: DaemonRecord,
    /// What it says about itself; `None` from a daemon of another protocol.
    pub stats: Option<Stats>,
}

/// What `daemon list` found.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ListResponse {
    /// Every running daemon of the user, by project root.
    pub daemons: Vec<ListedDaemon>,
}

impl Outcome for ListResponse {
    fn has_results(&self) -> bool {
        !self.daemons.is_empty()
    }
}

/// Every daemon in the registry that answers; entries of daemons that died
/// are removed.
pub fn list() -> Result<ListResponse, Error> {
    let mut daemons = Vec::new();
    for (path, record) in registry::entries() {
        let Some(live) = record.and_then(|record| hello(&record.socket)) else {
            // Gone, or never readable: a daemon that died left it.
            std::fs::remove_file(&path).ok();
            continue;
        };
        let stats = match request(&live.socket, &Request::Stats) {
            Ok(Response::Stats(stats)) => Some(stats),
            _ => None,
        };
        daemons.push(ListedDaemon {
            daemon: live,
            stats,
        });
    }
    daemons.sort_by(|a, b| a.daemon.root.cmp(&b.daemon.root));
    Ok(ListResponse { daemons })
}

/// Asks the daemon to stop, then waits until its lock is released, which
/// happens only once the process has exited.
fn stop_daemon(project: &Project, record: &DaemonRecord) -> Result<(), Error> {
    // A daemon that does not answer may still be exiting; the lock decides.
    request(&record.socket, &Request::Stop).ok();
    let path = project.data_dir().join(DAEMON_LOCK);
    let deadline = Instant::now() + PATIENCE;
    loop {
        let free = OpenOptions::new()
            .write(true)
            .open(&path)
            .map_or(true, |file| file.try_lock().is_ok());
        if free {
            return Ok(());
        }
        if Instant::now() >= deadline {
            return Err(Error::Internal {
                message: format!(
                    "the daemon (pid {}) did not stop within {} s",
                    record.pid,
                    PATIENCE.as_secs()
                ),
            });
        }
        thread::sleep(POLL);
    }
}

/// Starts `exe daemon run` for `root` in a process group of its own, so the
/// terminal's signals do not reach it; its standard error also goes to the
/// log. Returns the child, to notice an early exit.
#[cfg(unix)]
fn launch(exe: &Path, root: &Path, log: &Path) -> Result<Option<Child>, Error> {
    use std::os::unix::process::CommandExt;
    let stderr = OpenOptions::new()
        .append(true)
        .open(log)
        .map_err(|source| Error::Io {
            path: log.to_path_buf(),
            source,
        })?;
    let child = Command::new(exe)
        .args(["daemon", "run", "--root"])
        .arg(root)
        .arg("--log")
        .arg(log)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(stderr)
        .process_group(0)
        .spawn()
        .map_err(|source| Error::Io {
            path: exe.to_path_buf(),
            source,
        })?;
    Ok(Some(child))
}

/// Starts `exe daemon run` for `root` through PowerShell's `Start-Process`.
///
/// A child created with `std::process::Command` on Windows inherits every
/// inheritable handle of its parent, including the pipes the parent's own
/// caller reads, and would hold them open for its whole life: whoever reads
/// `codix daemon start` or `codix mcp` would then never see the end of their
/// output. `Start-Process` goes through `ShellExecute`, which passes no
/// handles. The PowerShell process holds the inherited ones only until it
/// exits, an instant later. The daemon writes its log itself.
#[cfg(windows)]
fn launch(exe: &Path, root: &Path, log: &Path) -> Result<Option<Child>, Error> {
    let arguments = [
        "daemon".as_ref(),
        "run".as_ref(),
        "--root".as_ref(),
        root.as_os_str(),
        "--log".as_ref(),
        log.as_os_str(),
    ]
    .iter()
    .map(|arg| windows_quote(&arg.to_string_lossy()))
    .collect::<Vec<_>>()
    .join(" ");
    let output = Command::new("powershell.exe")
        .args([
            "-NoLogo",
            "-NoProfile",
            "-NonInteractive",
            "-Command",
            "Start-Process -FilePath $env:CODIX_DAEMON_EXE -ArgumentList $env:CODIX_DAEMON_ARGS -WindowStyle Hidden",
        ])
        .env("CODIX_DAEMON_EXE", exe)
        .env("CODIX_DAEMON_ARGS", arguments)
        .stdin(Stdio::null())
        .output()
        .map_err(|source| Error::Io {
            path: "powershell.exe".into(),
            source,
        })?;
    if !output.status.success() {
        return Err(Error::Internal {
            message: format!(
                "powershell could not start the daemon ({}): {}",
                output.status,
                String::from_utf8_lossy(&output.stderr).trim()
            ),
        });
    }
    Ok(None)
}

/// Quotes one argument for a Windows command line, as `CommandLineToArgvW`
/// reads it back: backslashes are literal except before a quote.
#[cfg(any(windows, test))]
fn windows_quote(arg: &str) -> String {
    let mut quoted = String::from('"');
    let mut backslashes = 0;
    for c in arg.chars() {
        match c {
            '\\' => backslashes += 1,
            // The backslashes already written, as many again, and one more
            // to escape the quote.
            '"' => {
                quoted.push_str(&"\\".repeat(backslashes + 1));
                backslashes = 0;
            }
            _ => backslashes = 0,
        }
        quoted.push(c);
    }
    // Doubled before the closing quote.
    quoted.push_str(&"\\".repeat(backslashes));
    quoted.push('"');
    quoted
}

fn last_line(path: &Path) -> String {
    std::fs::read_to_string(path)
        .ok()
        .and_then(|text| text.lines().last().map(str::to_owned))
        .unwrap_or_else(|| format!("see {}", path.display()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn windows_arguments_survive_command_line_parsing() {
        assert_eq!(windows_quote("run"), r#""run""#);
        assert_eq!(
            windows_quote(r"C:\Program Files\codix"),
            r#""C:\Program Files\codix""#
        );
        // A trailing backslash would escape the closing quote.
        assert_eq!(windows_quote(r"C:\"), r#""C:\\""#);
        assert_eq!(windows_quote(r#"a\"b"#), r#""a\\\"b""#);
    }
}
