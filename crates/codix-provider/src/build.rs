//! What build system providers find: modules and their source roots, read
//! from build files without running the build.

use std::fmt::Debug;
use std::path::Path;

use serde::{Deserialize, Serialize};

/// A build system provider's logic.
pub trait BuildResolver: Sync + Debug {
    /// Reads the build files under `root` (the canonical project root).
    fn discover(&self, root: &Path) -> BuildModel;
}

/// The modules of a build, and the constructs that could not be interpreted.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct BuildModel {
    /// Modules, root first.
    pub modules: Vec<Module>,
    /// Constructs reported rather than guessed.
    pub unsupported: Vec<Unsupported>,
}

/// One module of a build.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Module {
    /// Module name, such as a Maven `artifactId` or a Gradle project path.
    pub name: String,
    /// Build system, such as `maven`.
    pub build: String,
    /// Module directory, root-relative; `""` for the root.
    pub root: String,
    /// Main source directories, root-relative.
    pub source_roots: Vec<String>,
    /// Test source directories, root-relative.
    pub test_roots: Vec<String>,
    /// Name of the parent module, if any.
    pub parent: Option<String>,
    /// What could not be resolved, such as an undefined `${property}`.
    pub notes: Vec<String>,
}

/// A build construct codix does not interpret.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Unsupported {
    /// Build file, root-relative.
    pub file: String,
    /// 1-based line.
    pub line: u32,
    /// The construct, as written.
    pub construct: String,
}
