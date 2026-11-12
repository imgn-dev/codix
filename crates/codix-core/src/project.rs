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
#[derive(
    Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize, schemars::JsonSchema,
)]
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
        Ok(find_root(start)?.map(Self::open))
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

    /// The index database.
    pub fn index_path(&self) -> PathBuf {
        self.data_dir().join(INDEX_FILE)
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
            match canonicalize(&candidate) {
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

/// [`fs::canonicalize`], returning an ordinary path where Windows would give a
/// verbatim one: `C:\x` rather than `\\?\C:\x`, and `\\server\share`
/// rather than `\\?\UNC\server\share`. Every canonical path codix shows or
/// compares comes from here, so they all have the same form.
pub fn canonicalize(path: impl AsRef<Path>) -> io::Result<PathBuf> {
    let canonical = fs::canonicalize(path)?;
    if cfg!(windows)
        && let Some(ordinary) = canonical.to_str().and_then(without_verbatim_prefix)
    {
        return Ok(PathBuf::from(ordinary));
    }
    Ok(canonical)
}

/// A Windows verbatim path written without its `\\?\` prefix, when that
/// means the same path: the result must not exceed the classic length limit,
/// and no component may be one Windows rewrites in ordinary paths (a trailing
/// dot or space, a device name such as `NUL`) or contain a character only
/// verbatim paths allow.
pub fn without_verbatim_prefix(path: &str) -> Option<String> {
    const MAX_PATH: usize = 260;
    const DEVICES: &[&str] = &["CON", "PRN", "AUX", "NUL", "CONIN$", "CONOUT$"];
    let (ordinary, rest) = if let Some(rest) = path.strip_prefix(r"\\?\UNC\") {
        (format!(r"\\{rest}"), rest)
    } else {
        let rest = path.strip_prefix(r"\\?\")?;
        let drive = rest.as_bytes();
        if drive.len() < 3
            || !drive[0].is_ascii_alphabetic()
            || drive[1] != b':'
            || drive[2] != b'\\'
        {
            return None;
        }
        (rest.to_owned(), &rest[3..])
    };
    let sound = |part: &str| {
        let stem = part.split('.').next().unwrap_or(part).to_ascii_uppercase();
        let device = DEVICES.contains(&stem.as_str())
            || (stem.len() == 4
                && (stem.starts_with("COM") || stem.starts_with("LPT"))
                && stem.as_bytes()[3].is_ascii_digit());
        !part.ends_with(['.', ' '])
            && !device
            && !part.contains(['/', '<', '>', ':', '"', '|', '?', '*'])
    };
    let parts_sound = rest.split('\\').filter(|p| !p.is_empty()).all(sound);
    (ordinary.len() < MAX_PATH && parts_sound).then_some(ordinary)
}

/// The root of the project containing `start`: the nearest directory, `start`
/// included, holding `.codix/` or `codix.toml`.
pub fn find_root(start: &Path) -> Result<Option<PathBuf>, Error> {
    let start = canonical(start)?;
    Ok(start
        .ancestors()
        .find(|dir| dir.join(DATA_DIR).is_dir() || dir.join(SHARED_CONFIG).is_file())
        .map(Path::to_path_buf))
}

fn canonical(path: &Path) -> Result<PathBuf, Error> {
    canonicalize(path).map_err(|source| Error::Io {
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

/// Where results may be: `--here`, `--path`, `--exclude-path` and
/// `--sources`, as given.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ScopeRequest {
    /// Limit results to the current directory's subtree.
    #[serde(default)]
    pub here: bool,
    /// Limit results to this subtree; repeatable.
    #[serde(default, rename = "path")]
    pub paths: Vec<String>,
    /// Leave out this subtree; repeatable.
    #[serde(default, rename = "exclude_path")]
    pub excluded: Vec<String>,
    /// Keep main or test sources only, as the build systems declare them.
    #[serde(default)]
    pub sources: Option<Sources>,
}

impl ScopeRequest {
    /// Whether the request limits results at all.
    pub fn is_empty(&self) -> bool {
        !self.here && self.paths.is_empty() && self.excluded.is_empty() && self.sources.is_none()
    }
}

/// Which sources a build system declares.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Sources {
    /// The main sources.
    Main,
    /// The test sources.
    Test,
}

/// Resolved scope: results must lie within one of `dirs` (anywhere when
/// there are none), outside every one of `excluded`, and within one of
/// `roots` when the sources are chosen.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Scope {
    dirs: Vec<FileId>,
    excluded: Vec<FileId>,
    roots: Option<Vec<FileId>>,
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
        let excluded = request
            .excluded
            .iter()
            .map(|path| project.file_id(cwd, path))
            .collect::<Result<_, _>>()?;
        Ok(Self {
            dirs,
            excluded,
            roots: None,
        })
    }

    /// The same scope, within `roots` only.
    #[must_use]
    pub fn within(self, roots: Vec<FileId>) -> Self {
        Self {
            roots: Some(roots),
            ..self
        }
    }

    /// Whether `id` is in scope.
    pub fn contains(&self, id: &FileId) -> bool {
        (self.dirs.is_empty() || self.dirs.iter().any(|dir| id.is_within(dir)))
            && !self.excluded.iter().any(|dir| id.is_within(dir))
            && self
                .roots
                .as_ref()
                .is_none_or(|roots| roots.iter().any(|root| id.is_within(root)))
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
    fn verbatim_prefixes_are_dropped_when_safe() {
        let ordinary = |p: &str| without_verbatim_prefix(p);
        assert_eq!(
            ordinary(r"\\?\C:\work\shop").as_deref(),
            Some(r"C:\work\shop")
        );
        assert_eq!(ordinary(r"\\?\C:\").as_deref(), Some(r"C:\"));
        assert_eq!(
            ordinary(r"\\?\UNC\server\share\x").as_deref(),
            Some(r"\\server\share\x")
        );
        assert_eq!(ordinary(r"C:\work"), None, "not verbatim");
        assert_eq!(ordinary(r"\\?\Volume{0b2c}\x"), None, "no drive letter");
        assert_eq!(ordinary(r"\\?\C:\work\trailing."), None);
        assert_eq!(ordinary(r"\\?\C:\work\NUL.txt"), None);
        assert_eq!(ordinary(r"\\?\C:\work\com1"), None);
        assert_eq!(ordinary(r"\\?\C:\a?b"), None);
        let long = format!(r"\\?\C:\{}", "x".repeat(300));
        assert_eq!(ordinary(&long), None, "over the length limit");
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
