//! The project write lock, and the network filesystem check that tells users
//! when that lock cannot be trusted.

use std::fs::{File, OpenOptions, TryLockError};
use std::path::Path;
use std::thread;
use std::time::{Duration, Instant};

use crate::Error;
use crate::op::Warning;
use crate::project::Project;

/// Lock file inside `.codix/`.
pub const LOCK_FILE: &str = "write.lock";

/// How long a query waits for a busy lock before answering from the current
/// snapshot.
pub const QUERY_WAIT: Duration = Duration::from_millis(200);

const POLL: Duration = Duration::from_millis(10);

/// Forces the network filesystem warning, for tests only.
pub const ASSUME_NETWORK_FS_VAR: &str = "CODIX_TEST_ASSUME_NETWORK_FS";

/// The held write lock: an OS advisory lock on `.codix/write.lock`, released
/// on drop and by the OS when the process exits, however it exits.
#[derive(Debug)]
pub struct WriteLock {
    // Held for its lock; the OS releases it with the descriptor.
    _file: File,
}

fn open_lock_file(project: &Project) -> Result<File, Error> {
    let path = project.data_dir().join(LOCK_FILE);
    OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .open(&path)
        .map_err(|source| Error::Io { path, source })
}

impl WriteLock {
    /// Waits as long as needed (`codix index`).
    pub fn acquire(project: &Project) -> Result<Self, Error> {
        let file = open_lock_file(project)?;
        file.lock().map_err(|source| Error::Io {
            path: project.data_dir().join(LOCK_FILE),
            source,
        })?;
        Ok(Self { _file: file })
    }

    /// Waits up to `timeout`; `None` when another process still holds the lock.
    pub fn try_acquire_for(project: &Project, timeout: Duration) -> Result<Option<Self>, Error> {
        let file = open_lock_file(project)?;
        let deadline = Instant::now() + timeout;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Some(Self { _file: file })),
                Err(TryLockError::WouldBlock) if Instant::now() < deadline => thread::sleep(POLL),
                Err(TryLockError::WouldBlock) => return Ok(None),
                Err(TryLockError::Error(source)) => {
                    return Err(Error::Io {
                        path: project.data_dir().join(LOCK_FILE),
                        source,
                    });
                }
            }
        }
    }
}

/// The warning for an index on a network filesystem, where advisory locks are
/// unreliable; `None` on a local disk.
pub fn network_filesystem_warning(data_dir: &Path) -> Option<Warning> {
    let forced = std::env::var_os(ASSUME_NETWORK_FS_VAR).is_some_and(|v| v == "1");
    (forced || !is_local(data_dir)).then(|| Warning {
        code: "network_filesystem".to_owned(),
        message: format!(
            "{} is not on a local disk; file locks may not work there, so using this project from several machines at once can corrupt the index or lose edits",
            data_dir.display()
        ),
    })
}

/// sysinfo lists local disks only (it leaves out NFS, SMB and other network
/// mounts on every platform), so a path on none of them is remote.
fn is_local(path: &Path) -> bool {
    let Ok(disks) =
        sysinfo::Disks::new_with_refreshed_list_specifics(sysinfo::DiskRefreshKind::nothing())
    else {
        return true;
    };
    let mounts: Vec<&Path> = disks.iter().map(sysinfo::Disk::mount_point).collect();
    on_local_disk(path, &mounts)
}

/// Compares devices: a path's device is that of one of the local mount points.
/// Path prefixes would mislead on macOS, where firmlinks put `/Users` on the
/// data volume while `/` is the system volume.
#[cfg(unix)]
fn on_local_disk(path: &Path, mounts: &[&Path]) -> bool {
    use std::os::unix::fs::MetadataExt;
    let Ok(device) = std::fs::metadata(path).map(|m| m.dev()) else {
        return true;
    };
    mounts
        .iter()
        .any(|mount| std::fs::metadata(mount).is_ok_and(|m| m.dev() == device))
}

/// UNC paths are shares; otherwise the drive must be a listed local one.
#[cfg(windows)]
fn on_local_disk(path: &Path, mounts: &[&Path]) -> bool {
    use std::path::{Component, Prefix};
    let Ok(canonical) = std::fs::canonicalize(path) else {
        return true;
    };
    match canonical.components().next() {
        Some(Component::Prefix(prefix)) => match prefix.kind() {
            Prefix::UNC(..) | Prefix::VerbatimUNC(..) => false,
            Prefix::Disk(letter) | Prefix::VerbatimDisk(letter) => mounts.iter().any(|mount| {
                mount
                    .to_str()
                    .and_then(|m| m.bytes().next())
                    .is_some_and(|m| m.eq_ignore_ascii_case(&letter))
            }),
            _ => true,
        },
        _ => true,
    }
}

#[cfg(not(any(unix, windows)))]
fn on_local_disk(_: &Path, _: &[&Path]) -> bool {
    true
}
