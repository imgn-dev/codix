//! Change detection and incremental indexing, after git's index: compare stat
//! fields with the manifest, hash only what changed, reparse only what really
//! changed, and commit everything in one transaction.

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fs;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rayon::prelude::*;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::config::Config;
use crate::lock::WriteGate;
use crate::op::Warning;
use crate::project::{FileId, Project};
use crate::providers::{MissingProvider, missing};
use crate::store::{FileRow, FileStatus, FileUpdate, ImportRow, RefRow, Store, SymbolRow};
use crate::vcs::Vcs;
use crate::walk::{ContentClass, Skip, WalkedFile, classify, walk, walk_where};

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
    /// Its references that can leave its local scopes (tier 2).
    pub refs: Vec<RefRow>,
    /// Its package, imports and includes.
    pub imports: Vec<ImportRow>,
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
            refs: Vec::new(),
            imports: Vec::new(),
        }
    }
}

/// What a scan did.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
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
    /// Providers the project needs and this build lacks.
    pub missing: Vec<MissingProvider>,
    /// What went wrong without stopping the scan, such as a resolver that
    /// failed.
    pub warnings: Vec<Warning>,
}

/// Scans the project and brings the index up to date, waiting for the write
/// lock as long as needed.
pub fn scan(
    project: &Project,
    config: &Config,
    indexer: &dyn Indexer,
    gate: &dyn WriteGate,
) -> Result<ScanReport, Error> {
    scan_within(project, config, indexer, gate, None)?.ok_or_else(|| Error::Internal {
        message: "the write lock was given up without a timeout".to_owned(),
    })
}

/// Like [`scan`], giving up when the write lock stays busy longer than
/// `wait`: `None` then means another process is writing and the index may be
/// stale.
pub fn scan_within(
    project: &Project,
    config: &Config,
    indexer: &dyn Indexer,
    gate: &dyn WriteGate,
    wait: Option<Duration>,
) -> Result<Option<ScanReport>, Error> {
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
        (rows_by_path(snapshot.files()?), snapshot.dirs()?)
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

    let Some(_access) = gate.enter(project, wait)? else {
        return Ok(None);
    };
    // The first writer after a crash mid-apply rolls it back.
    crate::apply::recover(project, &mut store)?;
    recheck(project, indexer, &mut outcomes, start_ns);
    let fresh = rows_by_path(store.snapshot()?.files()?);
    let updates = tally(outcomes, &fresh, &mut report, &mut kept);
    let dirs: Vec<(String, [u8; 32])> = dir_hashes(
        kept.iter()
            .map(|row| (row.path.as_str(), row.size, row.mtime_ns, row.inode)),
    )
    .into_iter()
    .collect();
    report.warnings = commit_and_resolve(project, &mut store, &updates, &deleted, Some(&dirs))?;
    store.replace_build(&crate::build::discover(project, config))?;
    report.missing = missing(
        project.root(),
        &store.snapshot()?.search_only_by_language()?,
    );
    report.errors.sort();
    Ok(Some(report))
}

/// The directories a partial scan covers, root-relative (`""` is the root):
/// some for the files directly in them, some for their whole subtree.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Targets {
    dirs: BTreeSet<String>,
    trees: BTreeSet<String>,
}

impl Targets {
    /// Covers the files directly in `dir`.
    pub fn add_dir(&mut self, dir: &str) {
        self.dirs.insert(dir.to_owned());
    }

    /// Covers every file under `dir`.
    pub fn add_tree(&mut self, dir: &str) {
        self.trees.insert(dir.to_owned());
    }

    /// Whether nothing is covered.
    pub fn is_empty(&self) -> bool {
        self.dirs.is_empty() && self.trees.is_empty()
    }

    fn covers_file(&self, path: &str) -> bool {
        self.dirs.contains(parent_of(path)) || self.trees.iter().any(|t| within(path, t))
    }

    /// Whether the walk must enter `dir`: it is covered, or leads to a
    /// covered directory.
    fn enters(&self, dir: &str) -> bool {
        self.trees.iter().any(|t| within(dir, t) || within(t, dir))
            || self.dirs.iter().any(|d| d == dir || within(d, dir))
    }
}

/// Whether `path` is `dir` or under it (`""` holds everything).
fn within(path: &str, dir: &str) -> bool {
    dir.is_empty()
        || path == dir
        || path
            .strip_prefix(dir)
            .is_some_and(|rest| rest.starts_with('/'))
}

/// Brings the files `targets` covers up to date, as a full scan would: what
/// a file watcher reports changed. `None` when the write lock stays busy
/// longer than `wait`.
pub fn scan_dirs(
    project: &Project,
    config: &Config,
    indexer: &dyn Indexer,
    gate: &dyn WriteGate,
    targets: &Targets,
    wait: Option<Duration>,
) -> Result<Option<ScanReport>, Error> {
    let start_ns = now_ns();
    let filter = targets.clone();
    let walked = walk_where(
        project,
        &Vcs::detect(project.root()),
        config.max_file_size(),
        move |path, is_dir| {
            if is_dir {
                filter.enters(path)
            } else {
                filter.covers_file(path)
            }
        },
    )?;
    let mut store = Store::open(&project.index_path())?;
    let manifest: HashMap<String, FileRow> = rows_by_path(store.snapshot()?.files()?)
        .into_iter()
        .filter(|(path, _)| targets.covers_file(path))
        .collect();

    let mut report = ScanReport {
        skipped: walked.skipped.clone(),
        errors: walked.errors.clone(),
        ..ScanReport::default()
    };
    let mut candidates: Vec<(&WalkedFile, Option<&FileRow>)> = Vec::new();
    for file in &walked.files {
        match manifest.get(file.id.as_str()) {
            Some(row) if is_current(row, indexer) && same_stat(row, file) => report.unchanged += 1,
            row => candidates.push((file, row)),
        }
    }
    let walked_ids: HashSet<&str> = walked.files.iter().map(|f| f.id.as_str()).collect();
    let deleted: Vec<String> = manifest
        .keys()
        .filter(|path| !walked_ids.contains(path.as_str()))
        .cloned()
        .collect();
    report.deleted = deleted.len();
    if candidates.is_empty() && deleted.is_empty() {
        return Ok(Some(report));
    }

    let mut outcomes: Vec<Outcome> = candidates
        .par_iter()
        .map(|(file, row)| process(project, indexer, file, *row, start_ns))
        .collect();
    let Some(_access) = gate.enter(project, wait)? else {
        return Ok(None);
    };
    // The first writer after a crash mid-apply rolls it back.
    crate::apply::recover(project, &mut store)?;
    recheck(project, indexer, &mut outcomes, start_ns);
    let fresh = rows_by_path(store.snapshot()?.files()?);
    let updates = tally(outcomes, &fresh, &mut report, &mut Vec::new());
    report.warnings = commit_and_resolve(project, &mut store, &updates, &deleted, None)?;
    let build_changed = updates
        .iter()
        .map(|u| u.file.path.as_str())
        .chain(deleted.iter().map(String::as_str))
        .any(|path| {
            crate::build::is_build_file(path[parent_of(path).len()..].trim_start_matches('/'))
        });
    if build_changed {
        store.replace_build(&crate::build::discover(project, config))?;
    }
    report.errors.sort();
    Ok(Some(report))
}

/// Brings only `paths` (root-relative) up to date: the files a lookup is
/// about to answer from. Takes the write lock only when one changed; `None`
/// when it stays busy longer than `wait`.
pub fn rescan(
    project: &Project,
    indexer: &dyn Indexer,
    gate: &dyn WriteGate,
    paths: &[String],
    wait: Duration,
) -> Result<Option<ScanReport>, Error> {
    let start_ns = now_ns();
    let mut store = Store::open(&project.index_path())?;
    let mut report = ScanReport::default();
    let mut candidates: Vec<(WalkedFile, Option<FileRow>)> = Vec::new();
    let mut deleted = Vec::new();
    {
        let snapshot = store.snapshot()?;
        for path in paths {
            let row = snapshot.file(path)?;
            let id = FileId::from_canonical(path.clone());
            match (stat(project, &id), row) {
                (Some(file), Some(row)) if is_current(&row, indexer) && same_stat(&row, &file) => {
                    report.unchanged += 1;
                }
                (Some(file), row) => candidates.push((file, row)),
                (None, Some(_)) => deleted.push(path.clone()),
                (None, None) => {}
            }
        }
    }
    if candidates.is_empty() && deleted.is_empty() {
        return Ok(Some(report));
    }
    let mut outcomes: Vec<Outcome> = candidates
        .iter()
        .map(|(file, row)| process(project, indexer, file, row.as_ref(), start_ns))
        .collect();
    let Some(_access) = gate.enter(project, Some(wait))? else {
        return Ok(None);
    };
    // The first writer after a crash mid-apply rolls it back.
    crate::apply::recover(project, &mut store)?;
    recheck(project, indexer, &mut outcomes, start_ns);
    let fresh = rows_by_path(store.snapshot()?.files()?);
    let updates = tally(outcomes, &fresh, &mut report, &mut Vec::new());
    report.deleted = deleted.len();
    report.warnings = commit_and_resolve(project, &mut store, &updates, &deleted, None)?;
    report.errors.sort();
    Ok(Some(report))
}

/// Records files the caller just wrote under the write access it holds:
/// their new content and stats, so the next scan and the watcher find them
/// current.
pub(crate) fn record_written(
    project: &Project,
    indexer: &dyn Indexer,
    store: &mut Store,
    ids: &[FileId],
) -> Result<(), Error> {
    let start_ns = now_ns();
    let rows = rows_by_path(store.snapshot()?.files()?);
    let mut updates = Vec::new();
    for id in ids {
        let Some(file) = stat(project, id) else {
            continue;
        };
        match process(project, indexer, &file, rows.get(id.as_str()), start_ns) {
            Outcome::Indexed(update) | Outcome::Touched(update) | Outcome::Pending(update) => {
                updates.push(update);
            }
            Outcome::Failed(path, error) => {
                return Err(Error::Internal {
                    message: format!("{path} was written but cannot be read back: {error}"),
                });
            }
        }
    }
    // Warnings are for scans: the caller reports what it wrote.
    commit_and_resolve(project, store, &updates, &[], None).map(|_| ())
}

/// Commits a scan's changes, then resolves the files whose references
/// changed; returns the resolvers' warnings.
fn commit_and_resolve(
    project: &Project,
    store: &mut Store,
    updates: &[FileUpdate],
    deleted: &[String],
    dirs: Option<&[(String, [u8; 32])]>,
) -> Result<Vec<Warning>, Error> {
    let mut changed: BTreeMap<String, Vec<String>> = BTreeMap::new();
    {
        let snapshot = store.snapshot()?;
        for path in deleted {
            if let Some(row) = snapshot.file(path)? {
                changed.entry(row.language).or_default().push(path.clone());
            }
        }
    }
    for update in updates.iter().filter(|u| u.symbols.is_some()) {
        changed
            .entry(update.file.language.clone())
            .or_default()
            .push(update.file.path.clone());
    }
    store.commit_scan(updates, deleted, dirs)?;
    crate::resolve::run(project, store, &changed)
}

fn rows_by_path(rows: Vec<FileRow>) -> HashMap<String, FileRow> {
    rows.into_iter()
        .map(|row| (row.path.clone(), row))
        .collect()
}

/// Counts the outcomes into `report`, records the rows kept, and returns the
/// updates to commit.
fn tally(
    outcomes: Vec<Outcome>,
    fresh: &HashMap<String, FileRow>,
    report: &mut ScanReport,
    kept: &mut Vec<FileRow>,
) -> Vec<FileUpdate> {
    let mut updates = Vec::new();
    for outcome in outcomes {
        match outcome {
            Outcome::Indexed(update) => {
                report.indexed += 1;
                kept.push(update.file.clone());
                push_unless_committed(&mut updates, update, fresh);
            }
            Outcome::Touched(update) => {
                report.touched += 1;
                kept.push(update.file.clone());
                push_unless_committed(&mut updates, update, fresh);
            }
            Outcome::Pending(update) => {
                report.pending += 1;
                kept.push(update.file.clone());
                updates.push(update);
            }
            Outcome::Failed(path, error) => report.errors.push((path, error)),
        }
    }
    updates
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
            refs: Vec::new(),
            imports: Vec::new(),
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
        refs: result.refs,
        imports: result.imports,
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
                    refs: Vec::new(),
                    imports: Vec::new(),
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
