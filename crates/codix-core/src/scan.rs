//! Change detection and incremental indexing, after git's index: compare stat
//! fields with the manifest, hash only what changed, reparse only what really
//! changed, and commit everything in one transaction.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::config::Config;
use crate::lock::WriteLock;
use crate::project::{FileId, Project};
use crate::store::{FileRow, FileStatus, FileUpdate, Store, SymbolRow};
use crate::vcs::Vcs;
use crate::walk::{ContentClass, Skip, WalkedFile, classify, walk};

/// Stored as the mtime of a racily clean file: modified so close to the scan
/// that a later change within the filesystem's timestamp granularity would
/// leave its stat unchanged. The next scan then sees a different mtime and
/// rehashes it, as git does with its index.
pub const RACY_MTIME: i64 = i64::MIN;

/// Files modified less than this before a scan started count as racy. It
/// covers filesystems with one-second timestamps.
const RACY_WINDOW: Duration = Duration::from_secs(1);

/// How often a file changed between parsing and commit is parsed again before
/// being left pending.
const RECHECKS: usize = 3;

/// The language-specific part of indexing.
pub trait Indexer: Sync {
    /// The parser version for `language`: a change forces a reparse of its
    /// files even when they did not change.
    fn parser_version(&self, language: &str) -> String;

    /// Detects the language of a file and extracts its symbols.
    fn index(&self, id: &FileId, content: &[u8]) -> Indexed;
}

/// What an [`Indexer`] makes of one file.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Indexed {
    /// Detected language.
    pub language: String,
    /// How the file is held.
    pub status: FileStatus,
    /// Its symbols.
    pub symbols: Vec<SymbolRow>,
}

/// Indexes every file as plain text, for search only.
#[derive(Debug, Clone, Copy, Default)]
pub struct TextIndexer;

impl Indexer for TextIndexer {
    fn parser_version(&self, _: &str) -> String {
        "text-1".to_owned()
    }

    fn index(&self, _: &FileId, content: &[u8]) -> Indexed {
        Indexed {
            language: "text".to_owned(),
            status: if classify(content) == ContentClass::Binary {
                FileStatus::Skipped
            } else {
                FileStatus::SearchOnly
            },
            symbols: Vec::new(),
        }
    }
}

/// What a scan did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScanReport {
    /// Files parsed: new, changed, or with a new parser version.
    pub indexed: usize,
    /// Files whose stat changed but not their content.
    pub touched: usize,
    /// Files removed from the index.
    pub deleted: usize,
    /// Files compared and found unchanged.
    pub unchanged: usize,
    /// Files that kept changing during the scan, left for the next one.
    pub pending: usize,
    /// Files left out by the walk, with the reason.
    pub skipped: Vec<(String, Skip)>,
    /// Files that could not be read, as (path, error).
    pub errors: Vec<(String, String)>,
}

/// Scans the project and brings the index up to date.
pub fn scan(
    project: &Project,
    config: &Config,
    indexer: &dyn Indexer,
) -> Result<ScanReport, Error> {
    let start_ns = now_ns();
    let walked = walk(
        project,
        &Vcs::detect(project.root()),
        config.max_file_size(),
    )?;
    fs::create_dir_all(project.data_dir()).map_err(|source| Error::Io {
        path: project.data_dir(),
        source,
    })?;
    let mut store = Store::open(&project.index_path())?;
    let (manifest, stored_dirs) = {
        let snapshot = store.snapshot()?;
        let files: HashMap<String, FileRow> = snapshot
            .files()?
            .into_iter()
            .map(|row| (row.path.clone(), row))
            .collect();
        (files, snapshot.dirs()?)
    };

    let current_dirs = dir_hashes(
        walked
            .files
            .iter()
            .map(|f| (f.id.as_str(), f.size, f.mtime_ns, f.inode)),
    );
    let mut report = ScanReport {
        skipped: walked.skipped.clone(),
        errors: walked.errors.clone(),
        ..ScanReport::default()
    };
    let mut kept: Vec<FileRow> = Vec::new();
    let mut candidates: Vec<(&WalkedFile, Option<&FileRow>)> = Vec::new();
    for file in &walked.files {
        let row = manifest.get(file.id.as_str());
        match row {
            Some(row)
                if is_current(row, indexer)
                    && (in_unchanged_dir(file, &current_dirs, &stored_dirs)
                        || same_stat(row, file)) =>
            {
                report.unchanged += 1;
                kept.push(row.clone());
            }
            _ => candidates.push((file, row)),
        }
    }

    let mut outcomes: Vec<Outcome> = candidates
        .par_iter()
        .map(|(file, row)| process(project, indexer, file, *row, start_ns))
        .collect();

    let walked_ids: HashSet<&str> = walked.files.iter().map(|f| f.id.as_str()).collect();
    let deleted: Vec<String> = manifest
        .keys()
        .filter(|path| !walked_ids.contains(path.as_str()))
        .cloned()
        .collect();
    report.deleted = deleted.len();

    let _lock = WriteLock::acquire(project)?;
    recheck(project, indexer, &mut outcomes, start_ns);
    let fresh: HashMap<String, FileRow> = {
        let snapshot = store.snapshot()?;
        snapshot
            .files()?
            .into_iter()
            .map(|row| (row.path.clone(), row))
            .collect()
    };
    let mut updates = Vec::new();
    for outcome in outcomes {
        match outcome {
            Outcome::Indexed(update) => {
                report.indexed += 1;
                kept.push(update.file.clone());
                push_unless_committed(&mut updates, update, &fresh);
            }
            Outcome::Touched(update) => {
                report.touched += 1;
                kept.push(update.file.clone());
                push_unless_committed(&mut updates, update, &fresh);
            }
            Outcome::Pending(update) => {
                report.pending += 1;
                kept.push(update.file.clone());
                updates.push(update);
            }
            Outcome::Failed(path, error) => report.errors.push((path, error)),
        }
    }
    let dirs: Vec<(String, [u8; 32])> = dir_hashes(
        kept.iter()
            .map(|row| (row.path.as_str(), row.size, row.mtime_ns, row.inode)),
    )
    .into_iter()
    .collect();
    store.commit_scan(&updates, &deleted, Some(&dirs))?;
    store.replace_build(&crate::build::discover(project, config))?;
    report.errors.sort();
    Ok(report)
}

/// Another process may have committed the same change while this one parsed;
/// rewriting identical rows would only churn the database.
fn push_unless_committed(
    updates: &mut Vec<FileUpdate>,
    update: FileUpdate,
    fresh: &HashMap<String, FileRow>,
) {
    if fresh.get(&update.file.path) != Some(&update.file) {
        updates.push(update);
    }
}

fn is_current(row: &FileRow, indexer: &dyn Indexer) -> bool {
    row.status != FileStatus::Pending && row.parser_version == indexer.parser_version(&row.language)
}

fn same_stat(row: &FileRow, file: &WalkedFile) -> bool {
    row.size == file.size && row.mtime_ns == file.mtime_ns && row.inode == file.inode
}

/// A directory whose stat hash matches the stored one holds only unchanged
/// files, so its files need no comparison of their own.
fn in_unchanged_dir(
    file: &WalkedFile,
    current: &BTreeMap<String, [u8; 32]>,
    stored: &HashMap<String, [u8; 32]>,
) -> bool {
    let parent = parent_of(file.id.as_str());
    current
        .get(parent)
        .is_some_and(|hash| stored.get(parent) == Some(hash))
}

enum Outcome {
    Indexed(FileUpdate),
    Touched(FileUpdate),
    Pending(FileUpdate),
    Failed(String, String),
}

/// Reads and hashes a file whose stat changed (or that is new, pending or due
/// for a new parser), then reparses it only if its content changed.
fn process(
    project: &Project,
    indexer: &dyn Indexer,
    file: &WalkedFile,
    row: Option<&FileRow>,
    start_ns: i64,
) -> Outcome {
    let path = project.path_of(&file.id);
    let content = match fs::read(&path) {
        Ok(content) => content,
        Err(e) => return Outcome::Failed(file.id.as_str().to_owned(), e.to_string()),
    };
    let hash = *blake3::hash(&content).as_bytes();
    let mtime_ns = stored_mtime(file.mtime_ns, start_ns);
    if let Some(row) = row
        && row.hash == hash
        && is_current(row, indexer)
    {
        return Outcome::Touched(FileUpdate {
            file: FileRow {
                size: file.size,
                mtime_ns,
                inode: file.inode,
                ..row.clone()
            },
            symbols: None,
        });
    }
    let result = indexer.index(&file.id, &content);
    Outcome::Indexed(FileUpdate {
        file: FileRow {
            path: file.id.as_str().to_owned(),
            size: file.size,
            mtime_ns,
            inode: file.inode,
            hash,
            parser_version: indexer.parser_version(&result.language),
            language: result.language,
            status: result.status,
        },
        symbols: Some(result.symbols),
    })
}

/// Under the lock: a file whose stat changed since the walk was modified while
/// being parsed; parse it again, a bounded number of times, then leave it
/// pending for the next scan.
fn recheck(project: &Project, indexer: &dyn Indexer, outcomes: &mut [Outcome], start_ns: i64) {
    for outcome in outcomes.iter_mut() {
        for attempt in 0..=RECHECKS {
            let (Outcome::Indexed(update) | Outcome::Touched(update)) = &*outcome else {
                break;
            };
            let id = FileId::from_canonical(update.file.path.clone());
            let Some(now) = stat(project, &id) else {
                break;
            };
            let expected = (update.file.size, update.file.mtime_ns, update.file.inode);
            if (now.size, stored_mtime(now.mtime_ns, start_ns), now.inode) == expected {
                break;
            }
            if attempt == RECHECKS {
                let mut file = update.file.clone();
                file.status = FileStatus::Pending;
                *outcome = Outcome::Pending(FileUpdate {
                    file,
                    symbols: None,
                });
                break;
            }
            *outcome = process(project, indexer, &now, None, start_ns);
        }
    }
}

fn stat(project: &Project, id: &FileId) -> Option<WalkedFile> {
    let metadata = fs::metadata(project.path_of(id)).ok()?;
    Some(WalkedFile {
        id: id.clone(),
        size: metadata.len(),
        mtime_ns: crate::walk::mtime_ns(&metadata),
        inode: crate::walk::inode(&metadata),
    })
}

fn stored_mtime(mtime_ns: i64, start_ns: i64) -> i64 {
    let window = i64::try_from(RACY_WINDOW.as_nanos()).unwrap_or(i64::MAX);
    if mtime_ns > start_ns.saturating_sub(window) {
        RACY_MTIME
    } else {
        mtime_ns
    }
}

fn now_ns() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_nanos()).unwrap_or(i64::MAX))
}

fn parent_of(path: &str) -> &str {
    path.rfind('/').map_or("", |i| &path[..i])
}

/// Merkle hashes of every directory holding files: each over its files' stat
/// fields and its subdirectories' hashes, sorted by name.
fn dir_hashes<'a>(
    files: impl Iterator<Item = (&'a str, u64, i64, u64)>,
) -> BTreeMap<String, [u8; 32]> {
    let mut children: BTreeMap<String, Vec<(String, Vec<u8>)>> = BTreeMap::new();
    for (path, size, mtime_ns, inode) in files {
        let parent = parent_of(path);
        let name = &path[parent.len()..].trim_start_matches('/');
        let mut entry = vec![b'f'];
        entry.extend(size.to_le_bytes());
        entry.extend(mtime_ns.to_le_bytes());
        entry.extend(inode.to_le_bytes());
        children
            .entry(parent.to_owned())
            .or_default()
            .push(((*name).to_owned(), entry));
        let mut dir = parent;
        while !dir.is_empty() {
            let up = parent_of(dir);
            children.entry(up.to_owned()).or_default();
            dir = up;
        }
    }
    let mut dirs: Vec<String> = children.keys().cloned().collect();
    // Deepest first, so a directory's subdirectories are hashed before it.
    dirs.sort_by_key(|d| {
        std::cmp::Reverse(if d.is_empty() {
            0
        } else {
            d.matches('/').count() + 1
        })
    });
    let mut hashes = BTreeMap::new();
    for dir in dirs {
        let mut entries = children.remove(&dir).unwrap_or_default();
        entries.sort();
        let mut merkle = blake3::Hasher::new();
        for (name, entry) in &entries {
            merkle.update(name.as_bytes());
            merkle.update(&[0]);
            merkle.update(entry);
        }
        let hash = *merkle.finalize().as_bytes();
        if !dir.is_empty() {
            let parent = parent_of(&dir).to_owned();
            let name = dir[parent.len()..].trim_start_matches('/').to_owned();
            let mut entry = vec![b'd'];
            entry.extend(hash);
            children.entry(parent).or_default().push((name, entry));
        }
        hashes.insert(dir, hash);
    }
    hashes
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn directory_hashes_change_with_any_file_below() {
        let base = dir_hashes([("a/b/c.rs", 1, 2, 3), ("a/d.rs", 1, 2, 3)].into_iter());
        let changed = dir_hashes([("a/b/c.rs", 1, 9, 3), ("a/d.rs", 1, 2, 3)].into_iter());
        assert_eq!(base.keys().collect::<Vec<_>>(), ["", "a", "a/b"]);
        assert_ne!(base["a/b"], changed["a/b"]);
        assert_ne!(base["a"], changed["a"]);
        assert_ne!(base[""], changed[""]);
        let sibling = dir_hashes([("a/b/c.rs", 1, 2, 3), ("a/d.rs", 5, 2, 3)].into_iter());
        assert_eq!(base["a/b"], sibling["a/b"]);
    }

    #[test]
    fn recent_mtimes_are_stored_as_racy() {
        let start = 10_000_000_000;
        assert_eq!(
            stored_mtime(start - 2_000_000_000, start),
            start - 2_000_000_000
        );
        assert_eq!(stored_mtime(start - 10, start), RACY_MTIME);
        assert_eq!(stored_mtime(start + 10, start), RACY_MTIME);
    }
}
