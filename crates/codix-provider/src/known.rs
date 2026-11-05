//! Every provider codix knows, compiled in or not, so a file whose provider is
//! left out of a build reports `not_supported_in_build` rather than an unknown
//! language.

use crate::ProviderKind;

/// Language names, as used in configuration and detection. `text` is plain
/// text, indexed for search only, and needs no pack.
pub const LANGUAGES: &[&str] = &[
    "c",
    "cpp",
    "csharp",
    "go",
    "java",
    "javascript",
    "json",
    "kotlin",
    "markdown",
    "php",
    "python",
    "ruby",
    "rust",
    "sql",
    "text",
    "toml",
    "typescript",
    "xml",
    "yaml",
];

/// Build system names.
pub const BUILDS: &[&str] = &["gradle", "maven"];

/// Version control system names.
pub const VCS: &[&str] = &["git", "mercurial", "subversion"];

/// Whether codix knows a provider of this kind and name, compiled in or not.
pub fn is_known(kind: ProviderKind, name: &str) -> bool {
    let names = match kind {
        ProviderKind::Language => LANGUAGES,
        ProviderKind::Build => BUILDS,
        ProviderKind::Vcs => VCS,
    };
    names.contains(&name)
}
