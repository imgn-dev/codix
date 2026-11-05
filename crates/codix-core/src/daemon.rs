//! What a running daemon leaves in `.codix/`: its lock and its record, so
//! any process can tell whether one serves the project and how to reach it.

use std::fmt;
use std::fs::{self, OpenOptions, TryLockError};
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::project::Project;

/// Held by the daemon for its lifetime: one daemon per project. The OS
/// releases it when the daemon exits, however it exits.
pub const DAEMON_LOCK: &str = "daemon.lock";
/// The daemon's record, [`DaemonRecord`] as JSON.
pub const DAEMON_FILE: &str = "daemon.json";
/// The daemon's socket, when its path fits the platform's limit.
pub const SOCKET_FILE: &str = "daemon.sock";
/// The daemon's standard error.
pub const LOG_FILE: &str = "daemon.log";

/// Version of the daemon protocol; a client talks only to a daemon with the
/// same protocol and codix version.
pub const PROTOCOL: u32 = 1;

/// A running daemon, as recorded in `.codix/daemon.json` and in the runtime
/// registry.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonRecord {
    /// The project root it serves.
    pub root: PathBuf,
    /// Its socket: a file path, or a pipe name on Windows.
    pub socket: String,
    /// Its process ID.
    pub pid: u32,
    /// The codix version it runs.
    pub version: String,
    /// Its protocol version.
    pub protocol: u32,
    /// When it started, in seconds since the Unix epoch.
    pub started_at: u64,
}

/// What a daemon's file watcher covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchStatus {
    /// Whether one recursive watch covers the project (macOS, Windows), rather
    /// than one watch per directory (Linux).
    pub recursive: bool,
    /// Directories watched one by one.
    pub watched: usize,
    /// Directories the OS would not watch, such as past the inotify quota:
    /// only the periodic scan sees their changes.
    pub unwatched: usize,
    /// Seconds between two full scans.
    pub rescan_seconds: u64,
    /// How long the last batch of changes took to index, in milliseconds,
    /// from the end of its debounce to its commit.
    pub last_batch_ms: Option<u64>,
}

/// A running file watcher, as the operations of a daemon see it.
pub trait Watch: fmt::Debug + Send + Sync {
    /// Indexes the changes reported so far without waiting for their debounce;
    /// `true` when that leaves the index current, because every directory is
    /// watched.
    fn flush(&self) -> bool;

    /// What the watcher covers.
    fn status(&self) -> WatchStatus;
}

/// A running daemon, as `status` reports it.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DaemonStatus {
    /// Its record.
    #[serde(flatten)]
    pub record: DaemonRecord,
    /// Its watcher; known only to the daemon itself.
    pub watch: Option<WatchStatus>,
}

/// The daemon serving `project`, if one runs: its lock is held and its record
/// readable. A record left by a daemon that died is ignored.
pub fn running(project: &Project) -> Result<Option<DaemonRecord>, Error> {
    let path = project.data_dir().join(DAEMON_LOCK);
    let file = match OpenOptions::new().write(true).open(&path) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(source) => return Err(Error::Io { path, source }),
    };
    match file.try_lock() {
        // Free: no daemon. Dropping the file releases the lock at once.
        Ok(()) => Ok(None),
        Err(TryLockError::WouldBlock) => Ok(read_record(project)),
        Err(TryLockError::Error(source)) => Err(Error::Io { path, source }),
    }
}

/// The record in `.codix/daemon.json`; `None` while a starting daemon has not
/// written it yet.
pub fn read_record(project: &Project) -> Option<DaemonRecord> {
    let text = fs::read_to_string(project.data_dir().join(DAEMON_FILE)).ok()?;
    serde_json::from_str(&text).ok()
}
