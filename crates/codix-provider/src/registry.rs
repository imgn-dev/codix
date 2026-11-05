//! Providers registered at link time.

use std::sync::OnceLock;

use crate::descriptor::{
    BuildDescriptor, Descriptor, LanguageDescriptor, ProviderKind, VcsDescriptor,
};

/// One registered provider: built by [`crate::register!`].
#[derive(Debug)]
pub struct ProviderEntry {
    /// Provider kind.
    pub kind: ProviderKind,
    /// Provider name.
    pub name: &'static str,
    /// Descriptor TOML, validated by the provider's build script.
    pub descriptor: &'static str,
    /// A language pack's grammar, if it parses.
    pub grammar: Option<Grammar>,
    /// A build system's logic.
    pub resolver: Option<&'static dyn crate::BuildResolver>,
}

/// A compiled-in tree-sitter grammar and the pack's queries for it.
#[derive(Clone, Copy)]
pub struct Grammar {
    /// The grammar.
    pub language: tree_sitter_language::LanguageFn,
    /// The pack's `tags` query.
    pub tags: &'static str,
}

impl std::fmt::Debug for Grammar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grammar").finish_non_exhaustive()
    }
}

inventory::collect!(ProviderEntry);

/// The descriptors of every provider compiled into this binary, sorted by name.
#[derive(Debug, Default)]
pub struct Registry {
    /// Language packs.
    pub languages: Vec<LanguageDescriptor>,
    /// Build systems.
    pub builds: Vec<BuildDescriptor>,
    /// Version control systems.
    pub vcs: Vec<VcsDescriptor>,
    /// Grammars of language packs, by language name.
    pub grammars: std::collections::BTreeMap<String, Grammar>,
    /// Resolvers of build systems, by build system name.
    pub resolvers: std::collections::BTreeMap<String, &'static dyn crate::BuildResolver>,
}

impl Registry {
    /// Whether a provider of this kind and name is compiled in.
    pub fn contains(&self, kind: ProviderKind, name: &str) -> bool {
        match kind {
            ProviderKind::Language => self.languages.iter().any(|d| d.name == name),
            ProviderKind::Build => self.builds.iter().any(|d| d.name == name),
            ProviderKind::Vcs => self.vcs.iter().any(|d| d.name == name),
        }
    }

    /// Every compiled-in provider as (kind, name), sorted.
    pub fn names(&self) -> Vec<(ProviderKind, &str)> {
        let mut names: Vec<(ProviderKind, &str)> = self
            .languages
            .iter()
            .map(|d| (ProviderKind::Language, d.name.as_str()))
            .chain(
                self.builds
                    .iter()
                    .map(|d| (ProviderKind::Build, d.name.as_str())),
            )
            .chain(
                self.vcs
                    .iter()
                    .map(|d| (ProviderKind::Vcs, d.name.as_str())),
            )
            .collect();
        names.sort_unstable();
        names
    }
}

/// The registry of this binary, parsed once.
///
/// # Panics
///
/// Never for descriptors registered with [`crate::register!`], which their
/// build scripts validated.
pub fn registry() -> &'static Registry {
    static REGISTRY: OnceLock<Registry> = OnceLock::new();
    REGISTRY.get_or_init(|| {
        let mut registry = Registry::default();
        for entry in inventory::iter::<ProviderEntry> {
            let descriptor = Descriptor::parse(entry.kind, entry.descriptor).unwrap_or_else(|e| {
                panic!(
                    "descriptor of {} provider `{}` was not validated at build time: {e}",
                    entry.kind, entry.name
                )
            });
            match descriptor {
                Descriptor::Language(d) => {
                    if let Some(grammar) = entry.grammar {
                        registry.grammars.insert(d.name.clone(), grammar);
                    }
                    registry.languages.push(d);
                }
                Descriptor::Build(d) => {
                    if let Some(resolver) = entry.resolver {
                        registry.resolvers.insert(d.name.clone(), resolver);
                    }
                    registry.builds.push(d);
                }
                Descriptor::Vcs(d) => registry.vcs.push(d),
            }
        }
        registry.languages.sort_by(|a, b| a.name.cmp(&b.name));
        registry.builds.sort_by(|a, b| a.name.cmp(&b.name));
        registry.vcs.sort_by(|a, b| a.name.cmp(&b.name));
        registry
    })
}
