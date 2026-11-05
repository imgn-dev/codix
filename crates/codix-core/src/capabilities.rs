//! `capabilities` and `languages`: what this build of codix supports, per
//! command and per language, and in a project, what it holds.

use std::collections::BTreeMap;
use std::path::PathBuf;

use codix_provider::{ProviderKind, known, registry};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::daemon::DaemonRecord;
use crate::op::{Context, Operation, Outcome};
use crate::store::{FileStatus, Store};

/// How deep support for a language goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Tier {
    /// Tier 1: definitions, outlines, name search, textual references.
    Symbols,
    /// Keys, headings or text, for search only.
    Search,
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
    /// Files in the index.
    pub files: usize,
    /// Files waiting to be reparsed.
    pub pending: usize,
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
                    tier: tiers.get(name.as_str()).copied().unwrap_or(Tier::Search),
                    name,
                    files,
                })
                .collect();
            languages.sort_by(|a, b| b.files.cmp(&a.files).then_with(|| a.name.cmp(&b.name)));
            capabilities.project = Some(ProjectSupport {
                root: project.root().to_path_buf(),
                files: rows.len(),
                pending: rows
                    .iter()
                    .filter(|r| r.status == FileStatus::Pending)
                    .count(),
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
                None if *name == crate::detect::TEXT => Tier::Search,
                None => Tier::NotInBuild,
                Some(d) if d.search_only || !registry.grammars.contains_key(&d.name) => {
                    Tier::Search
                }
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
        let languages = capabilities()
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
                LanguageInfo {
                    extensions: of(known::EXTENSIONS),
                    filenames: of(known::FILENAMES),
                    name: support.name,
                    tier: support.tier,
                }
            })
            .collect();
        Ok(LanguagesResponse { languages })
    }
}
