//! The providers available to this binary, as the engine sees them, and what
//! a project misses when one it needs is not.

use std::path::Path;

use codix_provider::{LanguageDescriptor, ProviderKind, known, registry};
use serde::{Deserialize, Serialize};

use crate::Error;

/// Whether this build finds providers as plugins, rather than having every
/// provider it can use compiled in.
pub(crate) const MODULAR: bool = cfg!(feature = "plugins");

/// The command a packager sets to install a plugin, with `{kind}` and
/// `{name}` placeholders, such as `brew install codix-{kind}-{name}`.
const INSTALL_HINT: Option<&str> = option_env!("CODIX_INSTALL_HINT");

/// Where plugins are explained when no packager set an install command.
const PLUGINS_URL: &str = "https://github.com/imgn-dev/codix#plugins";

/// The language pack named `language`, needed for `path`. A language codix
/// knows but this build lacks is `not_supported_in_build`.
pub fn require_language(language: &str, path: &str) -> Result<&'static LanguageDescriptor, Error> {
    if let Some(descriptor) = registry().languages.iter().find(|d| d.name == language) {
        return Ok(descriptor);
    }
    if known::is_known(ProviderKind::Language, language) {
        Err(Error::NotSupportedInBuild {
            provider: language.to_owned(),
            path: path.to_owned(),
        })
    } else {
        Err(Error::Internal {
            message: format!("unknown language `{language}` for {path}"),
        })
    }
}

/// A provider the project needs and this build lacks, reported once.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct MissingProvider {
    /// Provider kind.
    pub kind: ProviderKind,
    /// Provider name, such as `kotlin`.
    pub name: String,
    /// For a language, its files, indexed as text.
    pub files: Option<usize>,
    /// For a build system or version control system, the file or directory
    /// that shows it is in use, such as `pom.xml`.
    pub marker: Option<String>,
    /// How to get the provider.
    pub hint: String,
}

impl MissingProvider {
    /// What codix does without the provider.
    pub fn effect(&self) -> String {
        match (&self.marker, self.files.unwrap_or(0)) {
            (Some(marker), _) if self.kind == ProviderKind::Build => {
                format!("{marker} found, modules not read")
            }
            (Some(marker), _) => format!("{marker} found, ignore rules not applied"),
            (None, 1) => "1 file indexed as text".to_owned(),
            (None, files) => format!("{files} files indexed as text"),
        }
    }
}

/// The providers the project at `root` needs and this build lacks: the
/// languages of `search_only` files, by language as the index counts them;
/// the build systems whose files are at the root; and the version control
/// systems whose markers are at the root or above. Sorted by kind and name.
pub fn missing(root: &Path, search_only: &[(String, usize)]) -> Vec<MissingProvider> {
    let registry = registry();
    let mut missing: Vec<MissingProvider> = Vec::new();
    for (language, files) in search_only {
        if known::is_known(ProviderKind::Language, language)
            && language != crate::detect::TEXT
            && !registry.contains(ProviderKind::Language, language)
        {
            missing.push(MissingProvider {
                kind: ProviderKind::Language,
                name: language.clone(),
                files: Some(*files),
                marker: None,
                hint: hint(ProviderKind::Language, language),
            });
        }
    }
    let marked = |kind, markers: &[(&'static str, &'static str)], dirs: &[&Path]| {
        let mut found: Vec<MissingProvider> = Vec::new();
        for (marker, name) in markers {
            if registry.contains(kind, name) || found.iter().any(|m| m.name == *name) {
                continue;
            }
            if dirs
                .iter()
                .any(|dir| dir.join(marker).symlink_metadata().is_ok())
            {
                found.push(MissingProvider {
                    kind,
                    name: (*name).to_owned(),
                    files: None,
                    marker: Some((*marker).to_owned()),
                    hint: hint(kind, name),
                });
            }
        }
        found
    };
    missing.extend(marked(ProviderKind::Build, known::BUILD_MARKERS, &[root]));
    let ancestors: Vec<&Path> = root.ancestors().collect();
    missing.extend(marked(ProviderKind::Vcs, known::VCS_MARKERS, &ancestors));
    missing.sort_by(|a, b| (a.kind, &a.name).cmp(&(b.kind, &b.name)));
    missing
}

/// Why a provider is not there: `plugin not installed` or `not in this build`.
pub fn absence() -> &'static str {
    absence_in(MODULAR)
}

/// How to get the provider.
pub fn hint(kind: ProviderKind, name: &str) -> String {
    hint_in(MODULAR, INSTALL_HINT, kind, name)
}

/// The cause of `not_supported_in_build` for `path`.
pub(crate) fn not_supported_cause(provider: &str, path: &str) -> String {
    not_supported_cause_in(MODULAR, provider, path)
}

fn absence_in(modular: bool) -> &'static str {
    if modular {
        "plugin not installed"
    } else {
        "not in this build"
    }
}

fn hint_in(modular: bool, template: Option<&str>, kind: ProviderKind, name: &str) -> String {
    if !modular {
        return "use the full codix build, which includes every provider".to_owned();
    }
    let kind = match kind {
        ProviderKind::Language => "lang",
        ProviderKind::Build => "build",
        ProviderKind::Vcs => "vcs",
    };
    match template {
        Some(template) => template.replace("{kind}", kind).replace("{name}", name),
        None => format!("install the codix-{kind}-{name} plugin: see {PLUGINS_URL}"),
    }
}

fn not_supported_cause_in(modular: bool, provider: &str, path: &str) -> String {
    if modular {
        format!("{path}: the {provider} plugin is not installed")
    } else {
        format!("{path}: {provider} is not supported in this build")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_known_language_left_out_of_the_build_is_not_supported() {
        let error = require_language("kotlin", "src/Main.kt").unwrap_err();
        assert_eq!(error.code(), "not_supported_in_build");
        assert_eq!(error.cause(), not_supported_cause("kotlin", "src/Main.kt"));
    }

    #[test]
    fn an_unknown_language_is_a_bug() {
        assert_eq!(
            require_language("cobol", "x.cob").unwrap_err().code(),
            "internal"
        );
    }

    #[test]
    fn the_monolithic_build_points_to_the_full_build() {
        assert_eq!(absence_in(false), "not in this build");
        assert_eq!(
            not_supported_cause_in(false, "kotlin", "src/Main.kt"),
            "src/Main.kt: kotlin is not supported in this build"
        );
        assert_eq!(
            hint_in(
                false,
                Some("brew install codix-{kind}-{name}"),
                ProviderKind::Language,
                "kotlin"
            ),
            "use the full codix build, which includes every provider"
        );
    }

    #[test]
    fn the_modular_build_names_the_plugin_to_install() {
        assert_eq!(absence_in(true), "plugin not installed");
        assert_eq!(
            not_supported_cause_in(true, "kotlin", "src/Main.kt"),
            "src/Main.kt: the kotlin plugin is not installed"
        );
        let template = Some("brew install codix-{kind}-{name}");
        assert_eq!(
            hint_in(true, template, ProviderKind::Language, "kotlin"),
            "brew install codix-lang-kotlin"
        );
        assert_eq!(
            hint_in(true, template, ProviderKind::Build, "maven"),
            "brew install codix-build-maven"
        );
        assert_eq!(
            hint_in(true, None, ProviderKind::Vcs, "mercurial"),
            "install the codix-vcs-mercurial plugin: see https://github.com/imgn-dev/codix#plugins"
        );
    }

    #[test]
    fn missing_providers_say_what_they_cost() {
        let language = MissingProvider {
            kind: ProviderKind::Language,
            name: "kotlin".to_owned(),
            files: Some(214),
            marker: None,
            hint: String::new(),
        };
        assert_eq!(language.effect(), "214 files indexed as text");
        let build = MissingProvider {
            kind: ProviderKind::Build,
            files: None,
            marker: Some("pom.xml".to_owned()),
            ..language.clone()
        };
        assert_eq!(build.effect(), "pom.xml found, modules not read");
        let vcs = MissingProvider {
            kind: ProviderKind::Vcs,
            ..build.clone()
        };
        assert_eq!(vcs.effect(), "pom.xml found, ignore rules not applied");
    }
}
