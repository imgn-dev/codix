//! The runtime registry: one file per running daemon in the user runtime
//! directory, so `daemon list` and `daemon stop --all` find every daemon of
//! the user without knowing their projects.

use std::fs;
use std::path::{Path, PathBuf};

use codix_core::Error;
use codix_core::daemon::DaemonRecord;

use crate::paths::{create_private_dir, root_hash, runtime_dir};

fn registry_dir() -> PathBuf {
    runtime_dir().join("daemons")
}

fn entry_path(root: &Path) -> PathBuf {
    registry_dir().join(format!("{}.json", root_hash(root)))
}

/// Records `record` in the registry.
pub fn register(record: &DaemonRecord) -> Result<(), Error> {
    let dir = registry_dir();
    create_private_dir(&dir)?;
    write_json(&entry_path(&record.root), record)
}

/// Removes the entry of the daemon serving `root`, if any.
pub fn unregister(root: &Path) {
    // A missing entry is already gone; nothing else can go wrong that matters
    // to a daemon on its way out.
    fs::remove_file(entry_path(root)).ok();
}

/// Every entry, readable or not; an unreadable one is returned as `None`
/// so the caller can remove it.
pub fn entries() -> Vec<(PathBuf, Option<DaemonRecord>)> {
    let Ok(dir) = fs::read_dir(registry_dir()) else {
        return Vec::new();
    };
    let mut entries: Vec<(PathBuf, Option<DaemonRecord>)> = dir
        .filter_map(Result::ok)
        .map(|entry| entry.path())
        .filter(|path| path.extension().is_some_and(|e| e == "json"))
        .map(|path| {
            let record = fs::read_to_string(&path)
                .ok()
                .and_then(|text| serde_json::from_str(&text).ok());
            (path, record)
        })
        .collect();
    entries.sort_by(|a, b| a.0.cmp(&b.0));
    entries
}

/// Writes `value` as JSON to `path` through a temporary file and a rename,
/// so a reader never sees half a record.
pub(crate) fn write_json(path: &Path, value: &DaemonRecord) -> Result<(), Error> {
    let text = serde_json::to_string(value).map_err(|e| Error::Internal {
        message: format!("cannot serialize the daemon record: {e}"),
    })?;
    let temporary = path.with_extension("json.tmp");
    fs::write(&temporary, text)
        .and_then(|()| fs::rename(&temporary, path))
        .map_err(|source| Error::Io {
            path: path.to_path_buf(),
            source,
        })
}
