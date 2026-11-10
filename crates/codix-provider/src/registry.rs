//! Providers registered at link time.

use std::collections::BTreeMap;
use std::path::PathBuf;
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
    /// Version of the crate that registers it.
    pub version: &'static str,
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

/// A provider found as a plugin, its manifest and descriptor checked.
#[derive(Debug, Clone)]
pub struct Plugin {
    /// Its version, from its manifest.
    pub version: String,
    /// Its directory.
    pub dir: PathBuf,
    /// Its descriptor.
    pub descriptor: Descriptor,
}

/// A plugin the registry does not use, and why.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rejected {
    /// Provider kind, from the plugin's directory.
    pub kind: ProviderKind,
    /// Provider name, from the plugin's directory.
    pub name: String,
    /// Its directory.
    pub dir: PathBuf,
    /// Why it is not used.
    pub rejection: Rejection,
}

/// Why a plugin is not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Rejection {
    /// It cannot be used: its manifest or descriptor is wrong.
    Invalid(String),
    /// Another provider of the same kind and name is used instead.
    LeftOut(String),
}

/// Finds plugins, in search order: registered by `codix-plugin` with
/// [`inventory::submit!`], and called once, when the registry is built.
pub struct PluginSource {
    /// Every plugin found, used or not.
    pub discover: fn() -> Vec<Result<Plugin, Rejected>>,
}

inventory::collect!(PluginSource);

/// Where a provider comes from.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Source {
    /// Compiled into this binary.
    CompiledIn,
    /// A plugin, in this directory.
    Plugin(PathBuf),
}

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
    pub grammars: BTreeMap<String, Grammar>,
    /// Resolvers of build systems, by build system name.
    pub resolvers: BTreeMap<String, &'static dyn crate::BuildResolver>,
    /// Where each provider comes from, and its version.
    pub sources: BTreeMap<(ProviderKind, String), (Source, String)>,
    /// Plugins found but not used, in search order.
    pub rejected: Vec<Rejected>,
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

/// The registry of this binary, built once: the compiled-in providers, then
/// the plugins found. A compiled-in provider wins over a plugin of the same
/// kind and name; between plugins, the first found wins.
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
            if let (Descriptor::Language(d), Some(grammar)) = (&descriptor, entry.grammar) {
                registry
                    .grammars
                    .insert(d.name.clone(), Grammar::compiled(grammar));
            }
            if let (Descriptor::Build(d), Some(resolver)) = (&descriptor, entry.resolver) {
                registry.resolvers.insert(d.name.clone(), resolver);
            }
            registry.add(descriptor, Source::CompiledIn, entry.version.to_owned());
        }
        for source in inventory::iter::<PluginSource> {
            for found in (source.discover)() {
                match found {
                    Ok(plugin) => registry.add_plugin(plugin),
                    Err(rejected) => registry.rejected.push(rejected),
                }
            }
        }
        registry.languages.sort_by(|a, b| a.name.cmp(&b.name));
        registry.builds.sort_by(|a, b| a.name.cmp(&b.name));
        registry.vcs.sort_by(|a, b| a.name.cmp(&b.name));
        registry
    })
}

impl Registry {
    fn add(&mut self, descriptor: Descriptor, source: Source, version: String) {
        let key = (descriptor.kind(), descriptor.name().to_owned());
        match descriptor {
            Descriptor::Language(d) => self.languages.push(*d),
            Descriptor::Build(d) => self.builds.push(d),
            Descriptor::Vcs(d) => self.vcs.push(d),
        }
        self.sources.insert(key, (source, version));
    }

    fn add_plugin(&mut self, plugin: Plugin) {
        let kind = plugin.descriptor.kind();
        let name = plugin.descriptor.name().to_owned();
        let Some((used, _)) = self.sources.get(&(kind, name.clone())) else {
            self.add(
                plugin.descriptor,
                Source::Plugin(plugin.dir),
                plugin.version,
            );
            return;
        };
        let reason = match used {
            Source::CompiledIn => format!("the {kind} provider {name} is compiled in"),
            Source::Plugin(dir) => format!("the plugin in {} comes first", dir.display()),
        };
        self.rejected.push(Rejected {
            kind,
            name,
            dir: plugin.dir,
            rejection: Rejection::LeftOut(reason),
        });
    }
}
