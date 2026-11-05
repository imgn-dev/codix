//! The operation layer: one typed request and response per command, shared by
//! every frontend so they cannot behave differently.

use std::path::{Path, PathBuf};

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::Error;

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

/// What every operation runs against.
#[derive(Debug, Clone)]
pub struct Context {
    cwd: PathBuf,
}

impl Context {
    /// A context for a command run from `cwd`.
    pub fn new(cwd: PathBuf) -> Self {
        Self { cwd }
    }

    /// The directory the command runs from.
    pub fn cwd(&self) -> &Path {
        &self.cwd
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
