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
    "tsx",
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

/// Extensions of every known language, so files of a language left out of a
/// build are recognized as such. Packs declare the same in their descriptors.
pub const EXTENSIONS: &[(&str, &str)] = &[
    ("c", "c"),
    ("c++", "cpp"),
    ("cc", "cpp"),
    ("cjs", "javascript"),
    ("cpp", "cpp"),
    ("cs", "csharp"),
    ("cts", "typescript"),
    ("cxx", "cpp"),
    ("gemspec", "ruby"),
    ("go", "go"),
    ("hh", "cpp"),
    ("hpp", "cpp"),
    ("hxx", "cpp"),
    ("java", "java"),
    ("js", "javascript"),
    ("json", "json"),
    ("jsx", "javascript"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("markdown", "markdown"),
    ("md", "markdown"),
    ("mjs", "javascript"),
    ("mts", "typescript"),
    ("php", "php"),
    ("py", "python"),
    ("pyi", "python"),
    ("rake", "ruby"),
    ("rb", "ruby"),
    ("rs", "rust"),
    ("sql", "sql"),
    ("toml", "toml"),
    ("ts", "typescript"),
    ("tsx", "tsx"),
    ("txt", "text"),
    ("xml", "xml"),
    ("xsd", "xml"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
];

/// Exact file names of every known language.
pub const FILENAMES: &[(&str, &str)] = &[("Gemfile", "ruby"), ("Rakefile", "ruby")];

/// The known language for an extension, compiled in or not.
pub fn language_for_extension(extension: &str) -> Option<&'static str> {
    EXTENSIONS
        .iter()
        .find(|(e, _)| *e == extension)
        .map(|(_, language)| *language)
}

/// The known language for an exact file name, compiled in or not.
pub fn language_for_filename(name: &str) -> Option<&'static str> {
    FILENAMES
        .iter()
        .find(|(f, _)| *f == name)
        .map(|(_, language)| *language)
}
