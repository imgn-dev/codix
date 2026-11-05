//! `capabilities`: what this build of codix supports, per language.

use codix_provider::{ProviderKind, known, registry};
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::op::{Context, Operation, Outcome};

/// How deep support for a language goes.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LanguageSupport {
    /// Language name.
    pub name: String,
    /// Support tier.
    pub tier: Tier,
}

/// One compiled-in provider.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Provider {
    /// Provider kind.
    pub kind: ProviderKind,
    /// Provider name.
    pub name: String,
}

/// The `capabilities` operation.
pub struct CapabilitiesOp;

/// Request for [`CapabilitiesOp`]: nothing to choose.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct CapabilitiesRequest {}

/// What this build supports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capabilities {
    /// Every known language, sorted by name.
    pub languages: Vec<LanguageSupport>,
    /// Compiled-in providers, sorted by kind and name.
    pub providers: Vec<Provider>,
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

    fn run(&self, _: &Context, _: CapabilitiesRequest) -> Result<Capabilities, Error> {
        Ok(capabilities())
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
    Capabilities {
        languages,
        providers,
    }
}
