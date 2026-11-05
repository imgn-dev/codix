//! Every provider codix knows, compiled in or not, so a file whose provider is
//! left out of a build reports `not_supported_in_build` rather than an unknown
//! language.

/// Language names, as used in configuration and detection. `text` is plain
/// text, indexed for search only.
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
