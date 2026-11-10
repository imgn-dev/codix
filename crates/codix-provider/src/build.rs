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

/// The build plugin protocol this codix speaks.
pub const PROTOCOL: u32 = 1;

/// What a build plugin prints on stdout for
/// `<executable> discover --protocol 1 <root>`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Answer {
    /// The protocol the answer follows: 1.
    pub protocol: u32,
    /// The build's modules.
    pub model: BuildModel,
}

/// The `main` of a build plugin's executable: answers
/// `discover --protocol 1 <root>` with the resolver's model as JSON on
/// stdout. Anything else is a usage error, exit code 2.
pub fn serve(resolver: &dyn BuildResolver) -> std::process::ExitCode {
    use std::io::Write as _;

    let args: Vec<std::ffi::OsString> = std::env::args_os().skip(1).collect();
    let protocol = PROTOCOL.to_string();
    let root = match args.as_slice() {
        [command, flag, version, root]
            if command == "discover" && flag == "--protocol" && *version == *protocol =>
        {
            Path::new(root)
        }
        _ => {
            // Nothing to do if stderr is gone: the exit code says it.
            let _ = writeln!(
                std::io::stderr(),
                "usage: discover --protocol {PROTOCOL} <root>"
            );
            return std::process::ExitCode::from(2);
        }
    };
    let answer = Answer {
        protocol: PROTOCOL,
        model: resolver.discover(root),
    };
    let mut stdout = std::io::stdout().lock();
    match serde_json::to_writer(&mut stdout, &answer)
        .map_err(std::io::Error::from)
        .and_then(|()| writeln!(stdout))
    {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(_) => std::process::ExitCode::FAILURE,
    }
}
