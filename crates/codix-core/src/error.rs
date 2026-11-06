//! Errors returned by operations, each with a stable code, a one-line cause, a
//! hint naming the next step, and an exit code.

use std::fmt;
use std::io;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::page::Location;

/// Process exit codes shared by every frontend.
pub mod exit {
    /// Success with results.
    pub const SUCCESS: u8 = 0;
    /// Success, no results.
    pub const NO_RESULTS: u8 = 1;
    /// Usage error: bad flag, bad value, unknown symbol syntax.
    pub const USAGE: u8 = 2;
    /// Ambiguous input; candidates listed.
    pub const AMBIGUOUS: u8 = 3;
    /// Edit refused: conflict, stale preview, tier too low.
    pub const REFUSED: u8 = 4;
    /// Internal or I/O error.
    pub const INTERNAL: u8 = 5;
    /// No project found, or project not indexed.
    pub const NO_PROJECT: u8 = 6;
}

/// An operation failure.
#[derive(Debug)]
pub enum Error {
    /// No `.codix/` or `codix.toml` in the start directory or any parent.
    NoProject {
        /// Directory the search started from.
        start: PathBuf,
    },
    /// The project has a `codix.toml` but no index yet.
    NotIndexed {
        /// Project root.
        root: PathBuf,
    },
    /// The input matches several symbols.
    AmbiguousSymbol {
        /// The input as given.
        query: String,
        /// Symbol IDs of every match.
        candidates: Vec<String>,
    },
    /// The input matches no symbol.
    UnknownSymbol {
        /// The input as given.
        query: String,
    },
    /// A configuration key or value is not valid.
    InvalidConfig {
        /// What is wrong.
        cause: String,
        /// The valid keys or values.
        hint: String,
    },
    /// The file's provider is not compiled into this build.
    NotSupportedInBuild {
        /// Provider name, such as `kotlin`.
        provider: String,
        /// The file that needs it.
        path: String,
    },
    /// The command line is not valid.
    Usage {
        /// What is wrong.
        cause: String,
        /// How to fix it.
        hint: String,
    },
    /// Reading or writing a file failed.
    Io {
        /// The file involved.
        path: PathBuf,
        /// The underlying error.
        source: io::Error,
    },
    /// A file would be overwritten that codix did not write.
    FileExists {
        /// The file.
        path: PathBuf,
    },
    /// A daemon already serves the project.
    DaemonRunning {
        /// Its process ID.
        pid: u32,
        /// Its socket.
        socket: String,
    },
    /// An edit refused by a check, with the sites that made it fail.
    Refused {
        /// Stable code, such as `name_collision`.
        code: &'static str,
        /// What is wrong.
        cause: String,
        /// The next step, such as the override that allows it.
        hint: String,
        /// Where.
        sites: Vec<Site>,
    },
    /// The symbol ID names a symbol codix renamed.
    SymbolRenamed {
        /// The ID as given.
        old: String,
        /// The symbol's ID now.
        new: String,
    },
    /// A bug in codix.
    Internal {
        /// What went wrong.
        message: String,
    },
    /// An error that happened in a daemon, as it reported it.
    Remote(Box<ErrorReport>),
}

/// An error as it crosses the daemon socket: everything a frontend shows.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ErrorReport {
    /// Stable code.
    pub code: String,
    /// One line saying what went wrong.
    pub cause: String,
    /// The next step.
    pub hint: String,
    /// Symbol IDs listed with the error.
    pub candidates: Vec<String>,
    /// Sites listed with the error, such as those refusing an edit.
    #[serde(default)]
    pub sites: Vec<Site>,
    /// Process exit code.
    pub exit_code: u8,
}

/// A place in a file an error points at.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Site {
    /// Where.
    #[serde(flatten)]
    pub location: Location,
    /// What is there, such as the line's text.
    pub note: String,
}

impl Error {
    /// The stable code, such as `ambiguous_symbol`.
    pub fn code(&self) -> &str {
        match self {
            Self::Remote(report) => &report.code,
            Self::NoProject { .. } => "no_project",
            Self::NotIndexed { .. } => "not_indexed",
            Self::AmbiguousSymbol { .. } => "ambiguous_symbol",
            Self::UnknownSymbol { .. } => "unknown_symbol",
            Self::InvalidConfig { .. } => "invalid_config",
            Self::NotSupportedInBuild { .. } => "not_supported_in_build",
            Self::Usage { .. } => "usage",
            Self::Io { .. } => "io",
            Self::FileExists { .. } => "file_exists",
            Self::DaemonRunning { .. } => "daemon_running",
            Self::Refused { code, .. } => code,
            Self::SymbolRenamed { .. } => "symbol_renamed",
            Self::Internal { .. } => "internal",
        }
    }

    /// One line saying what went wrong.
    pub fn cause(&self) -> String {
        match self {
            Self::Remote(report) => report.cause.clone(),
            Self::NoProject { start } => format!(
                "no codix project in {} or any parent directory",
                start.display()
            ),
            Self::NotIndexed { root } => {
                format!("the project at {} is not indexed", root.display())
            }
            Self::AmbiguousSymbol { query, candidates } => {
                format!("\"{query}\" matches {} symbols", candidates.len())
            }
            Self::UnknownSymbol { query } => format!("no symbol matches \"{query}\""),
            Self::InvalidConfig { cause, .. }
            | Self::Usage { cause, .. }
            | Self::Refused { cause, .. } => cause.clone(),
            Self::NotSupportedInBuild { provider, path } => {
                format!("{path}: {provider} is not supported in this build")
            }
            Self::Io { path, source } => format!("{}: {source}", path.display()),
            Self::FileExists { path } => {
                format!("{} exists and was not written by codix", path.display())
            }
            Self::DaemonRunning { pid, socket } => {
                format!("a daemon already serves this project (pid {pid}, socket {socket})")
            }
            Self::SymbolRenamed { old, new } => format!("`{old}` was renamed to `{new}`"),
            Self::Internal { message } => message.clone(),
        }
    }

    /// The next step to take.
    pub fn hint(&self) -> String {
        match self {
            Self::Remote(report) => report.hint.clone(),
            Self::NoProject { .. } | Self::NotIndexed { .. } => {
                "run `codix init` at the project root".to_owned()
            }
            Self::AmbiguousSymbol { .. } => {
                "pass a full symbol ID, or narrow with --kind or --path".to_owned()
            }
            Self::UnknownSymbol { .. } => {
                "check the name with `codix find`, or pass a full symbol ID".to_owned()
            }
            Self::InvalidConfig { hint, .. }
            | Self::Usage { hint, .. }
            | Self::Refused { hint, .. } => hint.clone(),
            Self::NotSupportedInBuild { .. } => {
                "use the full codix build, which includes every provider".to_owned()
            }
            Self::Io { source, .. } if source.kind() == io::ErrorKind::NotFound => {
                "paths are relative to the current directory or the project root; find a file with `codix find <name>`".to_owned()
            }
            Self::Io { .. } => "check that the file exists and is accessible".to_owned(),
            Self::DaemonRunning { .. } => {
                "use it as it is, or stop it first with `codix daemon stop`".to_owned()
            }
            Self::FileExists { .. } => {
                "pass another path, or move the file out of the way".to_owned()
            }
            Self::SymbolRenamed { new, .. } => {
                format!("look at `{new}` before editing it")
            }
            Self::Internal { .. } => "this is a bug in codix; please report it".to_owned(),
        }
    }

    /// Symbol IDs listed with the error; empty for most errors.
    pub fn candidates(&self) -> &[String] {
        match self {
            Self::AmbiguousSymbol { candidates, .. } => candidates,
            Self::Remote(report) => &report.candidates,
            _ => &[],
        }
    }

    /// Sites listed with the error; empty for most errors.
    pub fn sites(&self) -> &[Site] {
        match self {
            Self::Refused { sites, .. } => sites,
            Self::Remote(report) => &report.sites,
            _ => &[],
        }
    }

    /// The process exit code for this error.
    pub fn exit_code(&self) -> u8 {
        match self {
            Self::Remote(report) => report.exit_code,
            Self::NoProject { .. } | Self::NotIndexed { .. } => exit::NO_PROJECT,
            Self::AmbiguousSymbol { .. } => exit::AMBIGUOUS,
            Self::UnknownSymbol { .. } => exit::NO_RESULTS,
            Self::InvalidConfig { .. } | Self::NotSupportedInBuild { .. } | Self::Usage { .. } => {
                exit::USAGE
            }
            Self::DaemonRunning { .. }
            | Self::FileExists { .. }
            | Self::Refused { .. }
            | Self::SymbolRenamed { .. } => exit::REFUSED,
            Self::Io { .. } | Self::Internal { .. } => exit::INTERNAL,
        }
    }
}

impl Error {
    /// The error as a frontend shows it, to send across the daemon socket.
    pub fn report(&self) -> ErrorReport {
        ErrorReport {
            code: self.code().to_owned(),
            cause: self.cause(),
            hint: self.hint(),
            candidates: self.candidates().to_vec(),
            sites: self.sites().to_vec(),
            exit_code: self.exit_code(),
        }
    }
}

impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code(), self.cause())
    }
}

impl std::error::Error for Error {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        match self {
            Self::Io { source, .. } => Some(source),
            _ => None,
        }
    }
}
