//! `capabilities` and `languages`: what this build of codix supports, per
//! command and per language, and in a project, what it holds.

use std::collections::BTreeMap;
use std::path::PathBuf;

use codix_provider::{ProviderKind, Rejection, Source, known, registry};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::daemon::DaemonRecord;
use crate::detect::AMBIGUOUS;
use crate::indexing::FileCounts;
use crate::op::{Context, Operation, Outcome};
use crate::store::Store;

/// How deep support for a language goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Tier 1: definitions, outlines, name search, textual references.
    Symbols,
    /// Tier 2: tier 1, and scope-aware references and renames within a file.
    Scopes,
    /// Text without symbols: files codix keeps track of, nothing more.
    Text,
    /// Known to codix but left out of this build.
    NotInBuild,
}

/// One language's support.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LanguageSupport {
    /// Language name.
    pub name: String,
    /// Support tier.
    pub tier: Tier,
}

/// One compiled-in provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Provider {
    /// Provider kind.
    pub kind: ProviderKind,
    /// Provider name.
    pub name: String,
}

/// The `capabilities` operation.
pub struct CapabilitiesOp;

/// Request for [`CapabilitiesOp`]: nothing to choose.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CapabilitiesRequest {}

/// A command, with the language support it needs.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct CommandSupport {
    /// Command name.
    pub name: String,
    /// The tier it needs; `None` when it works on any file.
    pub tier: Option<Tier>,
}

/// A language present in the project.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectLanguage {
    /// Language name.
    pub name: String,
    /// Indexed files of that language.
    pub files: usize,
    /// Support tier.
    pub tier: Tier,
}

/// What the index of the current project holds.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProjectSupport {
    /// The project root.
    pub root: PathBuf,
    /// Files in the index, by how it holds them, as `status` counts them.
    pub files: FileCounts,
    /// The languages of its files, most files first.
    pub languages: Vec<ProjectLanguage>,
    /// The daemon serving it, if one runs.
    pub daemon: Option<DaemonRecord>,
}

/// What this build supports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Capabilities {
    /// Every command, as `--help` lists them.
    pub commands: Vec<CommandSupport>,
    /// Every known language, sorted by name.
    pub languages: Vec<LanguageSupport>,
    /// Compiled-in providers, sorted by kind and name.
    pub providers: Vec<Provider>,
    /// The current project's index; `None` outside an indexed project.
    pub project: Option<ProjectSupport>,
}

impl Outcome for Capabilities {
    fn has_results(&self) -> bool {
        true
    }
}

impl Operation for CapabilitiesOp {
    const NAME: &'static str = "capabilities";
    type Request = CapabilitiesRequest;
    type Response = Capabilities;

    fn run(&self, ctx: &Context, _: CapabilitiesRequest) -> Result<Capabilities, Error> {
        let mut capabilities = capabilities();
        if let Ok(project) = ctx.project()
            && project.require_indexed().is_ok()
        {
            let tiers: BTreeMap<&str, Tier> = capabilities
                .languages
                .iter()
                .map(|l| (l.name.as_str(), l.tier))
                .collect();
            let mut store = Store::open(&project.index_path())?;
            let rows = store.snapshot()?.files()?;
            let mut counts: BTreeMap<String, usize> = BTreeMap::new();
            for row in &rows {
                *counts.entry(row.language.clone()).or_default() += 1;
            }
            let mut languages: Vec<ProjectLanguage> = counts
                .into_iter()
                .map(|(name, files)| ProjectLanguage {
                    tier: tiers.get(name.as_str()).copied().unwrap_or(Tier::Text),
                    name,
                    files,
                })
                .collect();
            languages.sort_by(|a, b| b.files.cmp(&a.files).then_with(|| a.name.cmp(&b.name)));
            capabilities.project = Some(ProjectSupport {
                root: project.root().to_path_buf(),
                files: FileCounts::of(rows.iter().map(|r| &r.status)),
                languages,
                daemon: crate::daemon::running(project)?,
            });
        }
        Ok(capabilities)
    }
}

/// The capabilities of this build.
pub fn capabilities() -> Capabilities {
    let registry = registry();
    let languages = known::LANGUAGES
        .iter()
        .map(|name| {
            let descriptor = registry.languages.iter().find(|d| d.name == *name);
            let tier = match descriptor {
                None if *name == crate::detect::TEXT => Tier::Text,
                None => Tier::NotInBuild,
                Some(d) if d.search_only || !registry.grammars.contains_key(&d.name) => Tier::Text,
                Some(d) if registry.grammars[&d.name].locals.is_some() => Tier::Scopes,
                Some(_) => Tier::Symbols,
            };
            LanguageSupport {
                name: (*name).to_owned(),
                tier,
            }
        })
        .collect();
    let providers = registry
        .names()
        .into_iter()
        .map(|(kind, name)| Provider {
            kind,
            name: name.to_owned(),
        })
        .collect();
    let commands = crate::describe::COMMANDS
        .iter()
        .map(|d| CommandSupport {
            name: d.name.to_owned(),
            tier: d.tier,
        })
        .collect();
    Capabilities {
        commands,
        languages,
        providers,
        project: None,
    }
}

/// The `languages` operation: every known language and how it is handled.
pub struct LanguagesOp;

/// Request for [`LanguagesOp`]: nothing to choose.
#[derive(Debug, Clone, Default, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LanguagesRequest {}

/// One language and the files it covers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LanguageInfo {
    /// Language name.
    pub name: String,
    /// Support tier; `not_in_build` when this build leaves it out.
    pub tier: Tier,
    /// File extensions, without the dot.
    pub extensions: Vec<String>,
    /// Exact file names, such as `Gemfile`.
    pub filenames: Vec<String>,
}

/// What `languages` lists.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct LanguagesResponse {
    /// Every known language, sorted by name.
    pub languages: Vec<LanguageInfo>,
    /// The extensions several languages use, and how codix chooses.
    pub ambiguous: Vec<AmbiguousExtension>,
    /// Every provider known, available or found as a plugin, sorted by kind
    /// and name; one name may be listed more than once, used or not.
    pub providers: Vec<ProviderInfo>,
}

/// An extension several languages use.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct AmbiguousExtension {
    /// The extension, without the dot.
    pub extension: String,
    /// The languages it may be, the default first.
    pub languages: Vec<String>,
    /// How the file's content decides.
    pub rule: String,
}

/// One provider: where it comes from and whether it is used.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ProviderInfo {
    /// Provider kind.
    pub kind: ProviderKind,
    /// Provider name.
    pub name: String,
    /// Its version, when it is used.
    pub version: Option<String>,
    /// Where it comes from; none when it is not there.
    pub source: Option<ProviderSource>,
    /// Whether it is used.
    pub status: ProviderStatus,
    /// Why it is not used, when it was found.
    pub reason: Option<String>,
}

/// Where a provider comes from.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderSource {
    /// Compiled into this binary.
    CompiledIn,
    /// A plugin, in this directory.
    Plugin(PathBuf),
}

/// Whether a provider is used.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum ProviderStatus {
    /// Used.
    Available,
    /// Known to codix, but no plugin provides it.
    NotInstalled,
    /// Known to codix, but left out of this build.
    NotInBuild,
    /// Found as a plugin that cannot be used.
    Invalid,
    /// Found as a plugin, but another provider of the same name is used.
    LeftOut,
}

/// Every provider: those available, those known but not there, and the
/// plugins found but not used.
pub fn providers() -> Vec<ProviderInfo> {
    let registry = registry();
    let mut providers: Vec<ProviderInfo> = registry
        .sources
        .iter()
        .map(|((kind, name), (source, version))| {
            // A plugin's grammar is only known to be usable once loaded.
            let unusable = registry
                .grammars
                .get(name)
                .filter(|_| *kind == ProviderKind::Language && *source != Source::CompiledIn)
                .and_then(|g| g.language().err());
            ProviderInfo {
                kind: *kind,
                name: name.clone(),
                version: Some(version.clone()),
                source: Some(match source {
                    Source::CompiledIn => ProviderSource::CompiledIn,
                    Source::Plugin(dir) => ProviderSource::Plugin(dir.clone()),
                }),
                status: if unusable.is_some() {
                    ProviderStatus::Invalid
                } else {
                    ProviderStatus::Available
                },
                reason: unusable.map(|e| e.to_string()),
            }
        })
        .collect();
    let absent = if crate::providers::MODULAR {
        ProviderStatus::NotInstalled
    } else {
        ProviderStatus::NotInBuild
    };
    for (kind, names) in [
        (ProviderKind::Language, known::LANGUAGES),
        (ProviderKind::Build, known::BUILDS),
        (ProviderKind::Vcs, known::VCS),
    ] {
        for name in names {
            if *name != crate::detect::TEXT && !registry.contains(kind, name) {
                providers.push(ProviderInfo {
                    kind,
                    name: (*name).to_owned(),
                    version: None,
                    source: None,
                    status: absent,
                    reason: None,
                });
            }
        }
    }
    for rejected in &registry.rejected {
        let (status, reason) = match &rejected.rejection {
            Rejection::Invalid(reason) => (ProviderStatus::Invalid, reason),
            Rejection::LeftOut(reason) => (ProviderStatus::LeftOut, reason),
        };
        providers.push(ProviderInfo {
            kind: rejected.kind,
            name: rejected.name.clone(),
            version: None,
            source: Some(ProviderSource::Plugin(rejected.dir.clone())),
            status,
            reason: Some(reason.clone()),
        });
    }
    // Stable: for one name, the provider used comes first, then the plugins
    // in search order.
    providers.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    providers
}

impl Outcome for LanguagesResponse {
    fn has_results(&self) -> bool {
        true
    }
}

impl Operation for LanguagesOp {
    const NAME: &'static str = "languages";
    type Request = LanguagesRequest;
    type Response = LanguagesResponse;

    fn run(&self, _: &Context, _: LanguagesRequest) -> Result<LanguagesResponse, Error> {
        let ambiguous_of = |name: &str| -> Vec<String> {
            AMBIGUOUS
                .iter()
                .filter(|a| a.languages.contains(&name))
                .map(|a| a.extension.to_owned())
                .collect()
        };
        let mut languages: Vec<LanguageInfo> = capabilities()
            .languages
            .into_iter()
            .map(|support| {
                let of = |table: &[(&str, &str)]| -> Vec<String> {
                    table
                        .iter()
                        .filter(|(_, language)| *language == support.name)
                        .map(|(key, _)| (*key).to_owned())
                        .collect()
                };
                let mut extensions = of(known::EXTENSIONS);
                extensions.extend(ambiguous_of(&support.name));
                LanguageInfo {
                    extensions,
                    filenames: of(known::FILENAMES),
                    name: support.name,
                    tier: support.tier,
                }
            })
            .collect();
        // Languages an ambiguous extension can be, which no provider parses:
        // their files are text.
        for name in AMBIGUOUS.iter().flat_map(|a| a.languages.iter()) {
            if !languages.iter().any(|l| l.name == *name) {
                languages.push(LanguageInfo {
                    name: (*name).to_owned(),
                    tier: Tier::Text,
                    extensions: ambiguous_of(name),
                    filenames: Vec::new(),
                });
            }
        }
        languages.sort_by(|a, b| a.name.cmp(&b.name));
        Ok(LanguagesResponse {
            languages,
            ambiguous: AMBIGUOUS
                .iter()
                .map(|a| AmbiguousExtension {
                    extension: a.extension.to_owned(),
                    languages: a.languages.iter().map(|l| (*l).to_owned()).collect(),
                    rule: a.rule.to_owned(),
                })
                .collect(),
            providers: providers(),
        })
    }
}
