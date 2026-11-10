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
/// build are recognized as such. Compiled-in packs declare the same in their
/// descriptors.
pub const EXTENSIONS: &[(&str, &str)] = &[
    ("c", "c"),
    ("c++", "cpp"),
    ("cc", "cpp"),
    ("cjs", "javascript"),
    ("cpp", "cpp"),
    ("cs", "csharp"),
    ("csproj", "xml"),
    ("csx", "csharp"),
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
    ("jsonc", "json"),
    ("jsx", "javascript"),
    ("kt", "kotlin"),
    ("kts", "kotlin"),
    ("markdown", "markdown"),
    ("md", "markdown"),
    ("mjs", "javascript"),
    ("mts", "typescript"),
    ("php", "php"),
    ("phtml", "php"),
    ("plist", "xml"),
    ("pom", "xml"),
    ("props", "xml"),
    ("py", "python"),
    ("pyi", "python"),
    ("pyw", "python"),
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
    ("xsl", "xml"),
    ("xslt", "xml"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
];

/// Exact file names of every known language.
pub const FILENAMES: &[(&str, &str)] = &[
    ("Cargo.lock", "toml"),
    ("Gemfile", "ruby"),
    ("Pipfile", "toml"),
    ("Rakefile", "ruby"),
];

/// Interpreters of `#!` lines, for every known language.
pub const SHEBANGS: &[(&str, &str)] = &[
    ("bun", "typescript"),
    ("deno", "typescript"),
    ("dotnet-script", "csharp"),
    ("jruby", "ruby"),
    ("kotlin", "kotlin"),
    ("kotlinc", "kotlin"),
    ("node", "javascript"),
    ("nodejs", "javascript"),
    ("php", "php"),
    ("pypy", "python"),
    ("pypy3", "python"),
    ("python", "python"),
    ("python2", "python"),
    ("python3", "python"),
    ("ruby", "ruby"),
    ("ts-node", "typescript"),
    ("tsx", "typescript"),
];

/// Names in Vim and Emacs modelines, for every known language.
pub const MODELINES: &[(&str, &str)] = &[
    ("c", "c"),
    ("c++", "cpp"),
    ("cpp", "cpp"),
    ("cs", "csharp"),
    ("csharp", "csharp"),
    ("go", "go"),
    ("java", "java"),
    ("javascript", "javascript"),
    ("js", "javascript"),
    ("json", "json"),
    ("kotlin", "kotlin"),
    ("markdown", "markdown"),
    ("php", "php"),
    ("python", "python"),
    ("ruby", "ruby"),
    ("rust", "rust"),
    ("sql", "sql"),
    ("toml", "toml"),
    ("ts", "typescript"),
    ("tsx", "tsx"),
    ("typescript", "typescript"),
    ("typescriptreact", "tsx"),
    ("xml", "xml"),
    ("yaml", "yaml"),
];

/// Files that mark a module of every known build system, at the project root.
pub const BUILD_MARKERS: &[(&str, &str)] = &[
    ("build.gradle", "gradle"),
    ("build.gradle.kts", "gradle"),
    ("pom.xml", "maven"),
    ("settings.gradle", "gradle"),
    ("settings.gradle.kts", "gradle"),
];

/// Names that mark every known version control system, at the project root
/// or above.
pub const VCS_MARKERS: &[(&str, &str)] = &[
    (".git", "git"),
    (".hg", "mercurial"),
    (".svn", "subversion"),
];

/// The known language for an extension, compiled in or not.
pub fn language_for_extension(extension: &str) -> Option<&'static str> {
    lookup(EXTENSIONS, extension)
}

/// The known language for an exact file name, compiled in or not.
pub fn language_for_filename(name: &str) -> Option<&'static str> {
    lookup(FILENAMES, name)
}

/// The known language for a `#!` interpreter, compiled in or not.
pub fn language_for_shebang(interpreter: &str) -> Option<&'static str> {
    lookup(SHEBANGS, interpreter)
}

/// The known language for a modeline name, compiled in or not.
pub fn language_for_modeline(name: &str) -> Option<&'static str> {
    lookup(MODELINES, name)
}

/// The values of `table` that map to `name`, such as the extensions of one
/// language.
pub fn values_of(table: &[(&'static str, &str)], name: &str) -> Vec<&'static str> {
    table
        .iter()
        .filter(|(_, n)| *n == name)
        .map(|(value, _)| *value)
        .collect()
}

fn lookup(table: &[(&str, &'static str)], value: &str) -> Option<&'static str> {
    table
        .iter()
        .find(|(v, _)| *v == value)
        .map(|(_, name)| *name)
}
