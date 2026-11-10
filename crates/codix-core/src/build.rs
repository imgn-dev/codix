//! Modules and source roots from the build systems a project uses, or from
//! `sources.roots` in `codix.toml`, which overrides them.

use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock, PoisonError};

use codix_provider::{BuildModel, Module, registry};

use crate::config::Config;
use crate::project::Project;

/// Whether a file named `name` is a build file a compiled-in build system
/// reads, such as `pom.xml`.
pub fn is_build_file(name: &str) -> bool {
    registry()
        .builds
        .iter()
        .any(|descriptor| descriptor.detect.iter().any(|d| d == name))
}

/// The modules of the project: from `sources.roots` when set, otherwise from
/// every build system whose files are at the root.
pub fn discover(project: &Project, config: &Config) -> BuildModel {
    let roots = config.source_roots();
    if !roots.is_empty() {
        return BuildModel {
            modules: vec![Module {
                name: "codix.toml".to_owned(),
                build: "codix.toml".to_owned(),
                root: String::new(),
                source_roots: roots,
                test_roots: Vec::new(),
                parent: None,
                notes: Vec::new(),
            }],
            unsupported: Vec::new(),
        };
    }
    let registry = registry();
    let mut model = BuildModel::default();
    for descriptor in &registry.builds {
        let Some(resolver) = registry.resolvers.get(&descriptor.name) else {
            continue;
        };
        if descriptor
            .detect
            .iter()
            .any(|marker| project.root().join(marker).is_file())
        {
            let found = resolver.discover(project.root());
            let mut failures = failures().lock().unwrap_or_else(PoisonError::into_inner);
            match found {
                Ok(found) => {
                    failures.remove(&descriptor.name);
                    model.modules.extend(found.modules);
                    model.unsupported.extend(found.unsupported);
                }
                Err(e) => {
                    failures.insert(descriptor.name.clone(), e.to_string());
                }
            }
        }
    }
    model
}

/// Why the build system named `name` gave no modules at its last discovery
/// in this process: its plugin failed or answered badly.
pub(crate) fn failure(name: &str) -> Option<String> {
    failures()
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .get(name)
        .cloned()
}

fn failures() -> &'static Mutex<BTreeMap<String, String>> {
    static FAILURES: OnceLock<Mutex<BTreeMap<String, String>>> = OnceLock::new();
    FAILURES.get_or_init(Mutex::default)
}
