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
    pub grammar: Option<CompiledGrammar>,
    /// A build system's logic.
    pub resolver: Option<&'static dyn crate::BuildResolver>,
}

/// A grammar compiled into this binary and the pack's queries for it, as
/// [`crate::register!`] records them.
#[derive(Clone, Copy)]
pub struct CompiledGrammar {
    /// The grammar.
    pub language: tree_sitter_language::LanguageFn,
    /// The pack's `tags` query.
    pub tags: &'static str,
    /// The pack's `locals` query.
    pub locals: Option<&'static str>,
}

impl std::fmt::Debug for CompiledGrammar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CompiledGrammar").finish_non_exhaustive()
    }
}

/// A language pack's tree-sitter grammar and its queries. The grammar is
/// loaded on first use, once per process.
pub struct Grammar {
    source: GrammarSource,
    language: OnceLock<Result<tree_sitter::Language, PluginError>>,
    /// The pack's `tags` query.
    pub tags: &'static str,
    /// The pack's `locals` query, for tier 2: scopes, local definitions and
    /// references.
    pub locals: Option<&'static str>,
}

/// Where a grammar comes from.
#[derive(Clone, Copy)]
enum GrammarSource {
    /// Compiled into this binary.
    Compiled(tree_sitter_language::LanguageFn),
}

impl Grammar {
    fn compiled(grammar: CompiledGrammar) -> Self {
        Self {
            source: GrammarSource::Compiled(grammar.language),
            language: OnceLock::new(),
            tags: grammar.tags,
            locals: grammar.locals,
        }
    }

    /// The grammar, loaded on the first call; an error when it cannot be
    /// loaded or its ABI is not one this tree-sitter reads.
    pub fn language(&self) -> Result<&tree_sitter::Language, PluginError> {
        self.language
            .get_or_init(|| {
                let language = match self.source {
                    GrammarSource::Compiled(function) => tree_sitter::Language::new(function),
                };
                let abi = language.abi_version();
                let supported =
                    tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION;
                if supported.contains(&abi) {
                    Ok(language)
                } else {
                    Err(PluginError(format!(
                        "grammar ABI {abi}, this build reads {} to {}",
                        supported.start(),
                        supported.end()
                    )))
                }
            })
            .as_ref()
            .map_err(Clone::clone)
    }
}

impl std::fmt::Debug for Grammar {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Grammar").finish_non_exhaustive()
    }
}

/// Why a provider cannot be used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginError(String);

impl std::fmt::Display for PluginError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for PluginError {}

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
                        registry
                            .grammars
                            .insert(d.name.clone(), Grammar::compiled(grammar));
                    }
                    registry.languages.push(*d);
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
