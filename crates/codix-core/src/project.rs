//! Project discovery and path identity.

use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use crate::Error;

/// Directory holding the index and local configuration, at the project root.
pub const DATA_DIR: &str = ".codix";
/// Shared configuration file at the project root.
pub const SHARED_CONFIG: &str = "codix.toml";
/// Index database inside [`DATA_DIR`].
pub const INDEX_FILE: &str = "index.db";

/// A project: a canonical root and what is known about its filesystem.
#[derive(Debug, Clone)]
pub struct Project {
    root: PathBuf,
    case_insensitive: bool,
}

/// A file or directory identity: its root-relative path with `/` separators,
/// the case found on disk, and symlinks resolved. The root itself is `""`.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct FileId(String);

impl FileId {
    /// The root-relative path.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    /// Wraps a root-relative path that is already canonical, such as one read
    /// from the index or produced by the walker.
    pub fn from_canonical(path: impl Into<String>) -> Self {
        Self(path.into())
    }

    /// Whether `self` is `dir` or lies under it.
    pub fn is_within(&self, dir: &FileId) -> bool {
        dir.0.is_empty()
            || self.0 == dir.0
            || (self.0.starts_with(&dir.0) && self.0.as_bytes().get(dir.0.len()) == Some(&b'/'))
    }
}

impl Project {
    /// Finds the project containing `start`: the nearest directory, `start`
    /// included, that holds [`DATA_DIR`] or [`SHARED_CONFIG`].
    pub fn discover(start: &Path) -> Result<Option<Self>, Error> {
        let start = canonical(start)?;
        for dir in start.ancestors() {
            if dir.join(DATA_DIR).is_dir() || dir.join(SHARED_CONFIG).is_file() {
                return Ok(Some(Self::open(dir.to_path_buf())));
            }
        }
        Ok(None)
    }

    /// The project rooted at `root`, without searching (`--root`).
    pub fn at(root: &Path) -> Result<Self, Error> {
        let root = canonical(root)?;
        if !root.is_dir() {
            return Err(Error::Usage {
                cause: format!("{} is not a directory", root.display()),
                hint: "pass the project root directory to --root".to_owned(),
            });
        }
        Ok(Self::open(root))
    }

    fn open(root: PathBuf) -> Self {
        let case_insensitive = detect_case_insensitive(&root);
        Self {
            root,
            case_insensitive,
        }
    }

    /// The canonical project root.
    pub fn root(&self) -> &Path {
        &self.root
    }

    /// The [`DATA_DIR`] directory.
    pub fn data_dir(&self) -> PathBuf {
        self.root.join(DATA_DIR)
    }

    /// Whether paths differing only in case name the same file here.
    pub fn is_case_insensitive(&self) -> bool {
        self.case_insensitive
    }

    /// Fails with `not_indexed` unless the project has an index.
    pub fn require_indexed(&self) -> Result<(), Error> {
        if self.data_dir().join(INDEX_FILE).is_file() {
            Ok(())
        } else {
            Err(Error::NotIndexed {
                root: self.root.clone(),
            })
        }
    }

    /// The identity of `input`, a path relative to `cwd`, relative to the
    /// root, or absolute. The cwd-relative reading is tried first.
    pub fn file_id(&self, cwd: &Path, input: &str) -> Result<FileId, Error> {
        let given = Path::new(input);
        let candidates = if given.is_absolute() {
            vec![given.to_path_buf()]
        } else {
            vec![cwd.join(given), self.root.join(given)]
        };
        let mut last_error = None;
        for candidate in candidates {
            match fs::canonicalize(&candidate) {
                Ok(path) => return self.id_of_canonical(&path, input),
                Err(e) if e.kind() == io::ErrorKind::NotFound => last_error = Some(e),
                Err(source) => {
                    return Err(Error::Io {
                        path: input.into(),
                        source,
                    });
                }
            }
        }
        Err(Error::Io {
            path: input.into(),
            source: last_error.unwrap_or_else(|| io::ErrorKind::NotFound.into()),
        })
    }

    /// The identity of a canonical absolute path, or `None` when it lies outside
    /// the root (for example through a symlink).
    pub fn id_of(&self, canonical_path: &Path) -> Option<FileId> {
        let relative = canonical_path.strip_prefix(&self.root).ok()?;
        let mut parts = Vec::new();
        let mut dir = self.root.clone();
        for component in relative.components() {
            let Component::Normal(name) = component else {
                return None;
            };
            let name = name.to_str()?;
            let on_disk = if self.case_insensitive {
                on_disk_name(&dir, name).unwrap_or_else(|| name.to_owned())
            } else {
                name.to_owned()
            };
            dir.push(&on_disk);
            parts.push(on_disk);
        }
        Some(FileId(parts.join("/")))
    }

    fn id_of_canonical(&self, path: &Path, input: &str) -> Result<FileId, Error> {
        self.id_of(path).ok_or_else(|| Error::Usage {
            cause: format!(
                "{input} is outside the project root {}",
                self.root.display()
            ),
            hint: "pass a path inside the project, or select another project with --root"
                .to_owned(),
        })
    }

    /// The absolute path of `id`.
    pub fn path_of(&self, id: &FileId) -> PathBuf {
        let mut path = self.root.clone();
        path.extend(id.0.split('/').filter(|part| !part.is_empty()));
        path
    }

    /// `id` as shown to a user in `cwd`: relative to it, with `../` for files
    /// outside it and `/` separators.
    pub fn display(&self, cwd: &Path, id: &FileId) -> String {
        relative(cwd, &self.path_of(id))
    }
}

fn canonical(path: &Path) -> Result<PathBuf, Error> {
    fs::canonicalize(path).map_err(|source| Error::Io {
        path: path.to_path_buf(),
        source,
    })
}

/// Detects case-insensitivity without writing: a marker such as `.codix`,
/// looked up with its case swapped, resolves only on a case-insensitive
/// filesystem, unless a separate entry with that exact name exists.
fn detect_case_insensitive(root: &Path) -> bool {
    let Ok(entries) = fs::read_dir(root) else {
        return false;
    };
    let names: Vec<String> = entries
        .filter_map(Result::ok)
        .filter_map(|entry| entry.file_name().into_string().ok())
        .collect();
    let probe = [DATA_DIR, SHARED_CONFIG]
        .into_iter()
        .find(|marker| names.iter().any(|name| name == marker))
        .map(str::to_owned)
        .or_else(|| names.iter().find(|name| swap_case(name) != **name).cloned());
    let Some(probe) = probe else {
        return false;
    };
    let swapped = swap_case(&probe);
    !names.contains(&swapped) && root.join(&swapped).symlink_metadata().is_ok()
}

fn swap_case(name: &str) -> String {
    name.chars()
        .flat_map(|c| {
            if c.is_lowercase() {
                c.to_uppercase().collect::<Vec<_>>()
            } else {
                c.to_lowercase().collect::<Vec<_>>()
            }
        })
        .collect()
}

/// The name as listed in `dir` that matches `name` ignoring case.
fn on_disk_name(dir: &Path, name: &str) -> Option<String> {
    let lower = name.to_lowercase();
    let mut fallback = None;
    for entry in fs::read_dir(dir).ok()?.filter_map(Result::ok) {
        let Ok(listed) = entry.file_name().into_string() else {
            continue;
        };
        if listed == name {
            return Some(listed);
        }
        if fallback.is_none() && listed.to_lowercase() == lower {
            fallback = Some(listed);
        }
    }
    fallback
}

/// `to` relative to `from`, both canonical absolute paths, with `/` separators.
fn relative(from: &Path, to: &Path) -> String {
    let from: Vec<Component<'_>> = from.components().collect();
    let to_parts: Vec<Component<'_>> = to.components().collect();
    let common = from
        .iter()
        .zip(&to_parts)
        .take_while(|(a, b)| a == b)
        .count();
    if common == 0 {
        return to.to_string_lossy().replace('\\', "/");
    }
    let mut parts: Vec<String> = vec!["..".to_owned(); from.len() - common];
    parts.extend(
        to_parts[common..]
            .iter()
            .map(|c| c.as_os_str().to_string_lossy().into_owned()),
    );
    if parts.is_empty() {
        ".".to_owned()
    } else {
        parts.join("/")
    }
}

/// Directories results are limited to: `--here` and `--path`, as given.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct ScopeRequest {
    /// Limit to the current directory's subtree.
    #[serde(default)]
    pub here: bool,
    /// Limit to these subtrees; paths relative to the cwd or the root.
    #[serde(default)]
    pub paths: Vec<String>,
}

/// Resolved scope: results must lie within one of `dirs`, or anywhere when
/// `dirs` is empty.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    dirs: Vec<FileId>,
}

impl Scope {
    /// Resolves a request against a project and cwd.
    pub fn resolve(project: &Project, cwd: &Path, request: &ScopeRequest) -> Result<Self, Error> {
        let mut dirs = Vec::new();
        if request.here {
            let here = project.id_of(cwd).ok_or_else(|| Error::Usage {
                cause: format!(
                    "--here: {} is outside the project root {}",
                    cwd.display(),
                    project.root().display()
                ),
                hint: "run from inside the project, or use --path".to_owned(),
            })?;
            dirs.push(here);
        }
        for path in &request.paths {
            dirs.push(project.file_id(cwd, path)?);
        }
        Ok(Self { dirs })
    }

    /// Whether `id` is in scope.
    pub fn contains(&self, id: &FileId) -> bool {
        self.dirs.is_empty() || self.dirs.iter().any(|dir| id.is_within(dir))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn within_requires_a_path_boundary() {
        let id = FileId::from_canonical("src/main.rs");
        assert!(id.is_within(&FileId::from_canonical("")));
        assert!(id.is_within(&FileId::from_canonical("src")));
        assert!(id.is_within(&FileId::from_canonical("src/main.rs")));
        assert!(!id.is_within(&FileId::from_canonical("sr")));
        assert!(!id.is_within(&FileId::from_canonical("src/main")));
    }

    #[test]
    fn relative_paths_climb_with_dot_dot() {
        let root = Path::new("/w/p");
        assert_eq!(relative(&root.join("src"), &root.join("src/a.rs")), "a.rs");
        assert_eq!(
            relative(&root.join("docs"), &root.join("src/a.rs")),
            "../src/a.rs"
        );
        assert_eq!(relative(root, &root.join("src/a.rs")), "src/a.rs");
        assert_eq!(relative(&root.join("src"), &root.join("src")), ".");
    }
}
