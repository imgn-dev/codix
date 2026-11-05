//! The operation layer: one typed request and response per command, shared by
//! every frontend so they cannot behave differently.

use std::fs;
use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::lock::network_filesystem_warning;
use crate::page::Location;
use crate::project::{FileId, Project, Scope, ScopeRequest};

/// A command of the operation layer.
pub trait Operation {
    /// The command name, as used by the CLI and MCP.
    const NAME: &'static str;
    /// The typed request.
    type Request: Serialize + DeserializeOwned;
    /// The typed response.
    type Response: Serialize + DeserializeOwned + Outcome;

    /// Runs the operation.
    fn run(&self, ctx: &Context, request: Self::Request) -> Result<Self::Response, Error>;
}

/// What a successful response says about its results.
pub trait Outcome {
    /// Whether the response holds any result; decides between exit codes 0 and 1.
    fn has_results(&self) -> bool;
}

/// What every operation runs against: the cwd and, when one was found, the
/// project.
#[derive(Debug, Clone)]
pub struct Context {
    cwd: PathBuf,
    project: Option<Project>,
    root_override: bool,
    warnings: Vec<Warning>,
}

impl Context {
    /// Discovers the project for a command run from `cwd`, or opens the one at
    /// `root` (`--root`, relative to `cwd`) without searching.
    pub fn discover(cwd: &Path, root: Option<&Path>) -> Result<Self, Error> {
        let cwd = fs::canonicalize(cwd).map_err(|source| Error::Io {
            path: cwd.to_path_buf(),
            source,
        })?;
        let project = match root {
            Some(root) => Some(Project::at(&cwd.join(root))?),
            None => Project::discover(&cwd)?,
        };
        let warnings = project
            .iter()
            .filter_map(|p| {
                let dir = p.data_dir();
                network_filesystem_warning(if dir.is_dir() { &dir } else { p.root() })
            })
            .collect();
        Ok(Self {
            cwd,
            project,
            root_override: root.is_some(),
            warnings,
        })
    }

    /// The canonical directory the command runs from.
    pub fn cwd(&self) -> &Path {
        &self.cwd
    }

    /// Warnings about the project, for every response.
    pub fn warnings(&self) -> &[Warning] {
        &self.warnings
    }

    /// Where `init` creates a project: the `--root` directory, or the cwd.
    pub fn init_root(&self) -> PathBuf {
        match &self.project {
            Some(project) if self.root_override => project.root().to_path_buf(),
            _ => self.cwd.clone(),
        }
    }

    /// The project, or `no_project`.
    pub fn project(&self) -> Result<&Project, Error> {
        self.project.as_ref().ok_or_else(|| Error::NoProject {
            start: self.cwd.clone(),
        })
    }

    /// A location for `file`, with its path as shown from the cwd.
    pub fn location(&self, file: &FileId, line: u32, column: u32) -> Location {
        let path = match &self.project {
            Some(project) => project.display(&self.cwd, file),
            None => file.as_str().to_owned(),
        };
        Location {
            path,
            root_path: file.clone(),
            line,
            column,
        }
    }

    /// Resolves `--here` and `--path`; without a project, only an empty request
    /// is valid.
    pub fn scope(&self, request: &ScopeRequest) -> Result<Scope, Error> {
        if !request.here && request.paths.is_empty() {
            return Ok(Scope::default());
        }
        Scope::resolve(self.project()?, &self.cwd, request)
    }
}

/// Something the user should know about a response that does not stop it,
/// such as an index on a network filesystem.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Warning {
    /// Stable code, such as `network_filesystem`.
    pub code: String,
    /// One line describing the risk.
    pub message: String,
}
