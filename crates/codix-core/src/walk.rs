//! The parallel walk of a project: which files are candidates for indexing,
//! with the stat fields change detection compares.

use std::collections::BTreeMap;
use std::fs::Metadata;
use std::path::Path;
use std::sync::Mutex;
use std::time::UNIX_EPOCH;

use ignore::overrides::OverrideBuilder;
use ignore::{DirEntry, WalkBuilder, WalkState};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::project::{DATA_DIR, FileId, Project};
use crate::vcs::Vcs;

/// codix's own ignore file at the project root, in gitignore syntax.
pub const IGNORE_FILE: &str = ".codixignore";

/// Directories excluded by default. A `.codixignore` cannot re-include them.
pub const DEFAULT_EXCLUDES: &[&str] = &["node_modules", "target", ".venv", "dist"];

/// Bytes read to decide whether a file is binary.
const BINARY_PROBE: usize = 8 << 10;

/// Average line length above which a file counts as minified.
const MINIFIED_LINE_LENGTH: usize = 300;

/// A file found by the walk, with its stat fields.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalkedFile {
    /// File identity.
    pub id: FileId,
    /// Size in bytes.
    pub size: u64,
    /// Modification time, in nanoseconds since the Unix epoch.
    pub mtime_ns: i64,
    /// Inode number; 0 where the platform does not expose one.
    pub inode: u64,
}

/// Why a file was left out of the walk.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Skip {
    /// Over `index.max_file_size`.
    TooLarge,
    /// A symlink whose target is outside the project root.
    LinkOutsideRoot,
    /// A name that is not valid UTF-8.
    NonUtf8Name,
}

/// The result of a walk.
#[derive(Debug, Default)]
pub struct Walk {
    /// Candidate files, sorted by identity.
    pub files: Vec<WalkedFile>,
    /// Files left out, with the reason, sorted by path.
    pub skipped: Vec<(String, Skip)>,
    /// Entries that could not be read, as (path, error), sorted by path.
    pub errors: Vec<(String, String)>,
}

/// Walks the project in parallel, honoring `.codixignore`, the ignore files of
/// detected version control systems, the default excludes and the systems'
/// skip directories, which always win.
pub fn walk(project: &Project, vcs: &Vcs, max_file_size: u64) -> Result<Walk, Error> {
    walk_where(project, vcs, max_file_size, |_, _| true)
}

/// Like [`walk`], visiting only the entries `keep` accepts, given their
/// root-relative path with `/` separators and whether they are directories.
/// Rules apply as in a full walk, since it still starts at the root.
pub fn walk_where(
    project: &Project,
    vcs: &Vcs,
    max_file_size: u64,
    keep: impl Fn(&str, bool) -> bool + Send + Sync + 'static,
) -> Result<Walk, Error> {
    let root = project.root().to_path_buf();
    let mut builder = WalkBuilder::new(&root);
    builder
        .hidden(false)
        .ignore(false)
        .git_ignore(false)
        .git_global(false)
        .git_exclude(false)
        .require_git(false)
        .follow_links(false);
    // Later names take precedence: `.codixignore` over VCS ignore files.
    for file in vcs.ignore_files() {
        builder.add_custom_ignore_filename(file);
    }
    builder.add_custom_ignore_filename(IGNORE_FILE);

    let mut overrides = OverrideBuilder::new(&root);
    for dir in DEFAULT_EXCLUDES {
        overrides
            .add(&format!("!{dir}/"))
            .map_err(|e| internal(&format!("default exclude {dir}: {e}")))?;
    }
    builder.overrides(
        overrides
            .build()
            .map_err(|e| internal(&format!("default excludes: {e}")))?,
    );

    let skip_dirs: Vec<String> = vcs
        .skip_dirs()
        .into_iter()
        .map(str::to_owned)
        .chain([DATA_DIR.to_owned()])
        .collect();
    let filter_root = root.clone();
    builder.filter_entry(move |entry| {
        let is_dir = entry.file_type().is_some_and(|t| t.is_dir());
        if is_dir
            && entry
                .file_name()
                .to_str()
                .is_some_and(|n| skip_dirs.iter().any(|d| d == n))
        {
            return false;
        }
        // The root itself is always walked; a name that is not UTF-8 is
        // reported by the visit.
        match walked_id(&filter_root, entry.path()) {
            Some(id) if !id.as_str().is_empty() => keep(id.as_str(), is_dir),
            _ => true,
        }
    });

    let found: Mutex<Found> = Mutex::new(Found::default());
    builder.build_parallel().run(|| {
        let found = &found;
        let project = &*project;
        Box::new(move |entry| {
            let outcome = visit(project, entry, max_file_size);
            let mut found = found
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match outcome {
                Visit::File(file) => {
                    found.files.insert(file.id.clone(), file);
                }
                Visit::Skipped(path, reason) => found.skipped.push((path, reason)),
                Visit::Error(path, error) => found.errors.push((path, error)),
                Visit::Nothing => {}
            }
            WalkState::Continue
        })
    });

    let found = found
        .into_inner()
        .unwrap_or_else(std::sync::PoisonError::into_inner);
    let mut walk = Walk {
        files: found.files.into_values().collect(),
        skipped: found.skipped,
        errors: found.errors,
    };
    walk.skipped.sort();
    walk.errors.sort();
    Ok(walk)
}

fn internal(message: &str) -> Error {
    Error::Internal {
        message: message.to_owned(),
    }
}

#[derive(Default)]
struct Found {
    // Keyed by identity: a symlink to a file inside the root yields the same
    // identity as the file itself.
    files: BTreeMap<FileId, WalkedFile>,
    skipped: Vec<(String, Skip)>,
    errors: Vec<(String, String)>,
}

enum Visit {
    File(WalkedFile),
    Skipped(String, Skip),
    Error(String, String),
    Nothing,
}

fn visit(project: &Project, entry: Result<DirEntry, ignore::Error>, max_file_size: u64) -> Visit {
    let entry = match entry {
        Ok(entry) => entry,
        Err(e) => return Visit::Error(String::new(), e.to_string()),
    };
    let path = entry.path();
    let shown = relative(project.root(), path);
    let Some(file_type) = entry.file_type() else {
        return Visit::Nothing;
    };
    if file_type.is_dir() {
        return Visit::Nothing;
    }
    let (id, metadata) = if file_type.is_symlink() {
        // Links are not descended: a link to a directory inside the root
        // reaches files the walk visits anyway, and one outside is not
        // followed. A link to a file inside the root names that file.
        let Ok(target) = crate::project::canonicalize(path) else {
            return Visit::Error(shown, "broken symlink".to_owned());
        };
        let Some(id) = project.id_of(&target) else {
            return Visit::Skipped(shown, Skip::LinkOutsideRoot);
        };
        match std::fs::metadata(&target) {
            Ok(m) if m.is_file() => (id, m),
            Ok(_) => return Visit::Nothing,
            Err(e) => return Visit::Error(shown, e.to_string()),
        }
    } else {
        let Some(id) = walked_id(project.root(), path) else {
            return Visit::Skipped(shown, Skip::NonUtf8Name);
        };
        match entry.metadata() {
            Ok(m) => (id, m),
            Err(e) => return Visit::Error(shown, e.to_string()),
        }
    };
    if metadata.len() > max_file_size {
        return Visit::Skipped(id.as_str().to_owned(), Skip::TooLarge);
    }
    Visit::File(WalkedFile {
        id,
        size: metadata.len(),
        mtime_ns: mtime_ns(&metadata),
        inode: inode(&metadata),
    })
}

/// The identity of a path the walk produced: under the canonical root, with
/// names as listed on disk, so no case lookup is needed.
fn walked_id(root: &Path, path: &Path) -> Option<FileId> {
    let relative = path.strip_prefix(root).ok()?;
    let parts: Option<Vec<&str>> = relative.iter().map(|part| part.to_str()).collect();
    Some(FileId::from_canonical(parts?.join("/")))
}

fn relative(root: &Path, path: &Path) -> String {
    path.strip_prefix(root)
        .unwrap_or(path)
        .to_string_lossy()
        .replace('\\', "/")
}

/// Nanoseconds since the Unix epoch; negative before it.
pub fn mtime_ns(metadata: &Metadata) -> i64 {
    let Ok(modified) = metadata.modified() else {
        return 0;
    };
    match modified.duration_since(UNIX_EPOCH) {
        Ok(after) => i64::try_from(after.as_nanos()).unwrap_or(i64::MAX),
        Err(before) => i64::try_from(before.duration().as_nanos()).map_or(i64::MIN, |n| -n),
    }
}

/// Inode number; 0 where the platform does not expose one.
#[cfg(unix)]
pub fn inode(metadata: &Metadata) -> u64 {
    std::os::unix::fs::MetadataExt::ino(metadata)
}

/// Inode number; 0 where the platform does not expose one.
#[cfg(not(unix))]
pub fn inode(_: &Metadata) -> u64 {
    0
}

/// How a file's content is indexed.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ContentClass {
    /// Parsed and indexed for symbols.
    Indexed,
    /// Not valid UTF-8: searched as text only.
    NonUtf8,
    /// Minified: searched as text only.
    Minified,
    /// Binary: skipped.
    Binary,
}

/// Classifies file content: binary when a NUL byte appears in the first 8 KiB,
/// search only when not UTF-8 or minified (average line length over 300
/// characters).
pub fn classify(content: &[u8]) -> ContentClass {
    if content[..content.len().min(BINARY_PROBE)].contains(&0) {
        return ContentClass::Binary;
    }
    let Ok(text) = std::str::from_utf8(content) else {
        return ContentClass::NonUtf8;
    };
    let lines = text.lines().count().max(1);
    if text.chars().count() / lines > MINIFIED_LINE_LENGTH {
        ContentClass::Minified
    } else {
        ContentClass::Indexed
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classification() {
        assert_eq!(classify(b"fn main() {}\n"), ContentClass::Indexed);
        assert_eq!(classify(b""), ContentClass::Indexed);
        assert_eq!(classify(b"PK\x03\x04\0\0"), ContentClass::Binary);
        assert_eq!(classify(b"caf\xe9\n"), ContentClass::NonUtf8);
        let minified = format!("{}\n", "a=1;".repeat(100));
        assert_eq!(classify(minified.as_bytes()), ContentClass::Minified);
        let long_but_normal = format!("{}\n{}", "x".repeat(400), "y\n".repeat(10));
        assert_eq!(classify(long_but_normal.as_bytes()), ContentClass::Indexed);
    }
}
