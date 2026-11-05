//! Modules and source roots from the build systems a project uses, or from
//! `sources.roots` in `codix.toml`, which overrides them.

use codix_provider::{BuildModel, Module, registry};

use crate::config::Config;
use crate::project::Project;

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
            model.modules.extend(found.modules);
            model.unsupported.extend(found.unsupported);
        }
    }
    model
}
