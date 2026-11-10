//! The protocol between the core and a resolver plugin: one JSON object per
//! line, requests on the resolver's stdin and replies on its stdout. The core
//! sends `hello` first and refuses a resolver that replies with another
//! protocol or views version.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

/// What the core asks.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Request {
    /// The versions the core speaks.
    Hello {
        /// [`crate::PROTOCOL`].
        protocol: u32,
        /// [`crate::VIEWS`].
        views: u32,
    },
    /// Resolve these files and their dependents.
    Resolve {
        /// The index, to open read-only.
        index: PathBuf,
        /// The scratch database to write into, created by the core.
        scratch: PathBuf,
        /// Root-relative paths of the files that changed.
        files: Vec<String>,
    },
    /// Exit.
    Shutdown,
}

/// What the resolver answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Reply {
    /// The versions the resolver speaks.
    Hello {
        /// Its protocol version.
        protocol: u32,
        /// Its views version.
        views: u32,
    },
    /// Done: the scratch database holds the results.
    Resolved {
        /// The other files it resolved, as dependents of the changed ones.
        dependents: Vec<String>,
    },
    /// The request failed.
    Error {
        /// Why.
        message: String,
    },
}
