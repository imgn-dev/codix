//! The providers available to this binary, as the engine sees them, and what
//! a project misses when one it needs is not.

use std::path::Path;
use std::sync::OnceLock;

use codix_provider::{LanguageDescriptor, ProviderKind, Rejection, Source, known, registry};
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
/// knows but this build lacks is `not_supported_in_build`, and one whose
/// plugin cannot be used `plugin_invalid`.
pub fn require_language(language: &str, path: &str) -> Result<&'static LanguageDescriptor, Error> {
    if let Some(cause) = invalid(ProviderKind::Language, language) {
        return Err(Error::PluginInvalid {
            provider: language.to_owned(),
            cause,
        });
    }
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
    /// Why its plugin, installed, cannot be used.
    pub invalid: Option<String>,
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

    /// Why the provider is not used: its plugin is not installed or cannot
    /// be used, or it is not in this build.
    pub fn absence(&self) -> String {
        match &self.invalid {
            Some(cause) => format!("plugin invalid: {cause}"),
            None => absence().to_owned(),
        }
    }
}

/// Why the provider of this kind and name, installed as a plugin, cannot be
/// used: its plugin was rejected, its grammar does not load, or it failed at
/// its last build discovery. `None` when it is usable or not installed.
fn invalid(kind: ProviderKind, name: &str) -> Option<String> {
    let registry = registry();
    if registry.contains(kind, name) {
        return match kind {
            ProviderKind::Language => registry
                .grammars
                .get(name)
                .and_then(|g| g.language().err())
                .map(|e| e.to_string()),
            ProviderKind::Build => crate::build::failure(name),
            ProviderKind::Vcs => None,
        };
    }
    registry.rejected.iter().find_map(|r| match &r.rejection {
        Rejection::Invalid(cause) if r.kind == kind && r.name == name => Some(cause.clone()),
        _ => None,
    })
}

/// The providers the project at `root` needs and this build lacks: the
/// languages of `search_only` files, by language as the index counts them;
/// the build systems whose files are at the root; and the version control
/// systems whose markers are at the root or above. Sorted by kind and name.
pub fn missing(root: &Path, search_only: &[(String, usize)]) -> Vec<MissingProvider> {
    let registry = registry();
    let mut missing: Vec<MissingProvider> = Vec::new();
    for (language, files) in search_only {
        if !known::is_known(ProviderKind::Language, language) || language == crate::detect::TEXT {
            continue;
        }
        let invalid = invalid(ProviderKind::Language, language);
        if invalid.is_some() || !registry.contains(ProviderKind::Language, language) {
            missing.push(MissingProvider {
                kind: ProviderKind::Language,
                name: language.clone(),
                files: Some(*files),
                marker: None,
                hint: hint(ProviderKind::Language, language),
                invalid,
            });
        }
    }
    let marked = |kind, markers: &[(&'static str, &'static str)], dirs: &[&Path]| {
        let mut found: Vec<MissingProvider> = Vec::new();
        for (marker, name) in markers {
            let invalid = invalid(kind, name);
            if (invalid.is_none() && registry.contains(kind, name))
                || found.iter().any(|m| m.name == *name)
            {
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
                    invalid,
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

/// A fingerprint of the plugins this process found, used or not: their
/// directories, and the path, size and modification time of every file in
/// them. Installing, removing or updating a plugin changes it. Empty when
/// there are none.
pub fn plugins_fingerprint() -> &'static str {
    static FINGERPRINT: OnceLock<String> = OnceLock::new();
    FINGERPRINT.get_or_init(|| {
        let registry = registry();
        let mut dirs: Vec<&Path> = registry
            .sources
            .values()
            .filter_map(|(source, _)| match source {
                Source::Plugin(dir) => Some(dir.as_path()),
                Source::CompiledIn => None,
            })
            .chain(registry.rejected.iter().map(|r| r.dir.as_path()))
            .collect();
        if dirs.is_empty() {
            return String::new();
        }
        dirs.sort_unstable();
        dirs.dedup();
        let mut hasher = blake3::Hasher::new();
        for dir in dirs {
            hasher.update(dir.to_string_lossy().as_bytes());
            hasher.update(&[0]);
            hash_files(&mut hasher, dir, dir);
        }
        hasher.finalize().to_hex()[..16].to_owned()
    })
}

/// Hashes the path, size and modification time of every file under `dir`,
/// in name order.
fn hash_files(hasher: &mut blake3::Hasher, base: &Path, dir: &Path) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    let mut entries: Vec<_> = entries.filter_map(Result::ok).collect();
    entries.sort_by_key(std::fs::DirEntry::file_name);
    for entry in entries {
        let path = entry.path();
        let Ok(metadata) = std::fs::metadata(&path) else {
            continue;
        };
        if metadata.is_dir() {
            hash_files(hasher, base, &path);
            continue;
        }
        let relative = path.strip_prefix(base).unwrap_or(&path);
        let modified = metadata
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map_or(0, |d| d.as_nanos());
        hasher.update(relative.to_string_lossy().as_bytes());
        hasher.update(&metadata.len().to_le_bytes());
        hasher.update(&modified.to_le_bytes());
    }
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
            invalid: None,
        };
        assert_eq!(language.effect(), "214 files indexed as text");
        assert_eq!(language.absence(), absence());
        let invalid = MissingProvider {
            invalid: Some("symbol tree_sitter_kotlin: not found".to_owned()),
            ..language.clone()
        };
        assert_eq!(
            invalid.absence(),
            "plugin invalid: symbol tree_sitter_kotlin: not found"
        );
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
