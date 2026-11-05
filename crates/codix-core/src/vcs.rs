//! Version control systems: which apply to a project, what they skip, which
//! ignore files they bring, and how `.codix/` is kept out of them.

use std::fs;
use std::path::Path;

use codix_provider::{IgnoreStrategy, VcsDescriptor, registry};

use crate::Error;
use crate::project::Project;

/// The systems detected for a project. Every matching descriptor applies, so
/// Jujutsu colocated with git gets both.
#[derive(Debug, Clone, Default)]
pub struct Vcs {
    detected: Vec<&'static VcsDescriptor>,
}

impl Vcs {
    /// Detects systems from their markers, a directory or a file (git
    /// worktrees and submodules have a `.git` file), at the project root or
    /// any directory above it.
    pub fn detect(root: &Path) -> Self {
        Self::detect_among(root, &registry().vcs)
    }

    fn detect_among(root: &Path, descriptors: &'static [VcsDescriptor]) -> Self {
        let detected = descriptors
            .iter()
            .filter(|d| {
                root.ancestors().any(|dir| {
                    d.detect
                        .iter()
                        .any(|marker| dir.join(marker).symlink_metadata().is_ok())
                })
            })
            .collect();
        Self { detected }
    }

    /// Names of the detected systems.
    pub fn names(&self) -> Vec<&str> {
        self.detected.iter().map(|d| d.name.as_str()).collect()
    }

    /// Directories always skipped, from every detected system.
    pub fn skip_dirs(&self) -> Vec<&str> {
        union(self.detected.iter().flat_map(|d| &d.skip_dirs))
    }

    /// Ignore files honored, from every detected system.
    pub fn ignore_files(&self) -> Vec<&str> {
        union(self.detected.iter().flat_map(|d| &d.honor_ignore_files))
    }

    /// Applies each detected system's ignore strategy. Writes only inside
    /// `.codix/`, never a project's own ignore files. Returns the hints of
    /// `hint` strategies, one per system, for `codix init` to print.
    pub fn apply_ignore_strategies(&self, project: &Project) -> Result<Vec<String>, Error> {
        let mut hints = Vec::new();
        for descriptor in &self.detected {
            match &descriptor.ignore {
                IgnoreStrategy::NestedFile { file, content } => {
                    let path = project.data_dir().join(file);
                    fs::create_dir_all(project.data_dir())
                        .and_then(|()| fs::write(&path, content))
                        .map_err(|source| Error::Io { path, source })?;
                }
                IgnoreStrategy::Hint { hint } => {
                    if !hints.contains(hint) {
                        hints.push(hint.clone());
                    }
                }
                IgnoreStrategy::None => {}
            }
        }
        Ok(hints)
    }
}

fn union<'a>(names: impl Iterator<Item = &'a String>) -> Vec<&'a str> {
    let mut all: Vec<&str> = names.map(String::as_str).collect();
    all.sort_unstable();
    all.dedup();
    all
}
