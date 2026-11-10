//! Descriptor schema: what each kind of provider declares. Descriptors choose
//! among behaviors the engine implements and carry no code.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The three kinds of provider.
#[derive(
    Debug,
    Clone,
    Copy,
    PartialEq,
    Eq,
    Hash,
    PartialOrd,
    Ord,
    Serialize,
    Deserialize,
    schemars::JsonSchema,
)]
#[serde(rename_all = "snake_case")]
pub enum ProviderKind {
    /// A language pack.
    Language,
    /// A build system.
    Build,
    /// A version control system.
    Vcs,
}

impl fmt::Display for ProviderKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.pad(match self {
            Self::Language => "language",
            Self::Build => "build",
            Self::Vcs => "vcs",
        })
    }
}

/// A language pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct LanguageDescriptor {
    /// Language name, as in [`crate::known::LANGUAGES`].
    pub name: String,
    /// Pack version; part of the parser version stored per file.
    pub version: u32,
    /// Prefix of the language's symbol IDs, such as `py` in
    /// `py:src/billing/invoice.py::Invoice.total#a3f9`.
    pub id_prefix: String,
    /// Whether symbols are extracted for search only, as keys of a data file
    /// or headings of a document, rather than as code definitions.
    #[serde(default)]
    pub search_only: bool,
    /// How files of the language are recognized.
    #[serde(default)]
    pub detect: Detect,
    /// How definitions become symbols; packs without a grammar have none.
    #[serde(default)]
    pub symbols: Symbols,
    /// How references bind to definitions within a file, for packs with a
    /// `locals` query.
    #[serde(default)]
    pub scopes: Scopes,
    /// What a valid name is, for renames.
    #[serde(default)]
    pub names: Names,
    /// How the language's generated and vendored files are marked.
    #[serde(default)]
    pub generated: Generated,
}

/// What a valid name is, for the pre-flight checks of renames.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Names {
    /// A regular expression a name must match entirely; by default letters,
    /// digits and `_`, not starting with a digit (Unicode identifiers).
    pub identifier: Option<String>,
    /// Reserved words.
    #[serde(default)]
    pub keywords: Vec<String>,
    /// Words reserved as type names only, such as Java's `var` and `record`.
    #[serde(default)]
    pub type_keywords: Vec<String>,
    /// Whether methods can share a name, told apart by their parameters.
    #[serde(default)]
    pub overloading: bool,
    /// Whether the case of a name's first letter sets its visibility, as in
    /// Go, where a capital exports it.
    #[serde(default)]
    pub visibility_by_case: bool,
    /// Whether a type's constructors (and destructors) carry its name.
    #[serde(default)]
    pub constructors_named_after_type: bool,
    /// Definitions whose name the file's name must match.
    #[serde(default)]
    pub file_named_after: Vec<FileNamed>,
}

/// Definitions whose name the file's name must match, such as a Java public
/// top-level class.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct FileNamed {
    /// Symbol kinds.
    pub kinds: Vec<String>,
    /// Only with this visibility.
    pub visibility: Option<String>,
    /// Only outside any other symbol.
    #[serde(default)]
    pub top_level: bool,
    /// Only declarations without a body, such as Rust's `mod x;`.
    #[serde(default)]
    pub without_body: bool,
}

/// How a language marks generated and vendored files.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Generated {
    /// Regular expressions matched against each of a file's first lines,
    /// such as Go's `^// Code generated .* DO NOT EDIT\.$`.
    #[serde(default)]
    pub headers: Vec<String>,
    /// Gitignore-style patterns of vendored directories, such as
    /// `vendor/`.
    #[serde(default)]
    pub paths: Vec<String>,
}

/// The contexts a reference can appear in, as `@local.reference.<context>`
/// captures name them.
pub const REFERENCE_CONTEXTS: &[&str] = &["call", "type", "import", "read", "write"];

/// The namespace of definitions and contexts a pack does not list.
pub const DEFAULT_NAMESPACE: &str = "value";

/// Binding rules that a `locals` query's captures do not carry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Scopes {
    /// The namespace of each definition kind, a symbol kind or a local kind
    /// such as `parameter`; kinds not listed are in `value`. Java keeps
    /// types, methods and values apart; Python has one namespace.
    #[serde(default)]
    pub namespaces: std::collections::BTreeMap<String, String>,
    /// The namespaces each reference context reads, in order; contexts not
    /// listed read `value`. In Java a name read in an expression is a
    /// variable if one is in scope, else a type.
    #[serde(default)]
    pub contexts: std::collections::BTreeMap<String, Vec<String>>,
    /// Scope kinds whose definitions are visible before their position, such
    /// as class bodies; in other scopes a reference sees only earlier
    /// definitions.
    #[serde(default)]
    pub hoisted: Vec<String>,
    /// Receivers meaning the enclosing type's own instance, such as `this`:
    /// a member reached through one binds to the members of that type.
    #[serde(default)]
    pub self_receivers: Vec<String>,
    /// Names that scopes of a kind have without declaring them, by scope
    /// kind: Java classes inherit `toString` and the other methods of
    /// `Object`.
    #[serde(default)]
    pub implicit_members: std::collections::BTreeMap<String, Vec<String>>,
    /// The namespaces each context reads for a member reached through a self
    /// receiver, when they differ from `contexts`: in JavaScript, `this.x`
    /// reads class members, which no plain name reads.
    #[serde(default)]
    pub member_contexts: std::collections::BTreeMap<String, Vec<String>>,
    /// Scope kinds that code in a nested function does not see, such as a
    /// Python class body from its methods.
    #[serde(default)]
    pub hidden_from_functions: Vec<String>,
    /// Node kinds whose child names a member rather than a variable: a symbol
    /// whose name is the property in `obj.f = function () {}` or the key in
    /// `{ f: function () {} }` is in the `member` namespace.
    #[serde(default)]
    pub member_name_parents: Vec<String>,
    /// Whether the symbols of the `tags` query are visible in their whole
    /// scope, before their position, as Rust items are in a block.
    #[serde(default)]
    pub symbols_hoisted: bool,
    /// Scope kinds that names of some namespaces do not cross, by kind: a
    /// Rust `mod` does not see the names around it, and a Ruby `def` does
    /// not see the local variables around it.
    #[serde(default)]
    pub closed: std::collections::BTreeMap<String, Vec<String>>,
}

/// The namespace of members reached only through a receiver.
pub const MEMBER_NAMESPACE: &str = "member";

impl Scopes {
    /// The namespace of a definition kind.
    pub fn namespace_of(&self, kind: &str) -> &str {
        self.namespaces
            .get(kind)
            .map_or(DEFAULT_NAMESPACE, String::as_str)
    }

    /// The namespaces a context reads for a member reached through a self
    /// receiver, in order.
    pub fn members_read_by(&self, context: &str) -> Vec<&str> {
        match self.member_contexts.get(context) {
            Some(namespaces) => namespaces.iter().map(String::as_str).collect(),
            None => self.read_by(context),
        }
    }

    /// The namespaces a reference context reads, in order.
    pub fn read_by(&self, context: &str) -> Vec<&str> {
        self.contexts.get(context).map_or_else(
            || vec![DEFAULT_NAMESPACE],
            |namespaces| namespaces.iter().map(String::as_str).collect(),
        )
    }
}

/// The kinds every language maps its definitions onto.
pub const SYMBOL_KINDS: &[&str] = &[
    "module",
    "namespace",
    "class",
    "struct",
    "interface",
    "trait",
    "enum",
    "enum_member",
    "function",
    "method",
    "constructor",
    "field",
    "property",
    "constant",
    "variable",
    "type_alias",
    "macro",
];

/// How a pack's `tags` query captures become symbols.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Symbols {
    /// Between a parent's and a child's name in qualified names.
    #[serde(default = "default_separator")]
    pub separator: String,
    /// Fields holding a definition's body; its signature is the text before.
    #[serde(default)]
    pub body_fields: Vec<String>,
    /// Kinds of child nodes holding a body that no field names, such as
    /// Kotlin's `class_body`; the signature is the text before.
    #[serde(default)]
    pub body_nodes: Vec<String>,
    /// Tokens opening a body that no field or node holds, such as `{` for a
    /// Go struct: the signature ends at the first one after the name.
    #[serde(default)]
    pub body_tokens: Vec<String>,
    /// `@definition.<kind>` capture kinds mapped onto [`SYMBOL_KINDS`]; a kind
    /// already in the vocabulary maps to itself.
    #[serde(default)]
    pub kinds: std::collections::BTreeMap<String, String>,
    /// Where visibility is written, if the language has it.
    pub visibility: Option<Visibility>,
}

impl Default for Symbols {
    fn default() -> Self {
        Self {
            separator: default_separator(),
            body_fields: Vec::new(),
            body_nodes: Vec::new(),
            body_tokens: Vec::new(),
            kinds: std::collections::BTreeMap::new(),
            visibility: None,
        }
    }
}

fn default_separator() -> String {
    ".".to_owned()
}

/// Visibility: the first of `values` among the words of a definition's child
/// node of type `node`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Visibility {
    /// Node type holding modifiers, such as `modifiers`.
    pub node: String,
    /// Visibility keywords, such as `public`.
    pub values: Vec<String>,
}

impl LanguageDescriptor {
    /// The vocabulary kind of a `@definition.<kind>` capture.
    pub fn symbol_kind(&self, capture_kind: &str) -> Option<&str> {
        match self.symbols.kinds.get(capture_kind) {
            Some(kind) => Some(kind),
            None => SYMBOL_KINDS.iter().find(|k| **k == capture_kind).copied(),
        }
    }
}

/// Detection rules of a language. Content heuristics for ambiguous
/// extensions (`.h`, `.m`, `.pl`) are engine behaviors, not rules.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct Detect {
    /// Extensions without the dot, such as `java`.
    #[serde(default)]
    pub extensions: Vec<String>,
    /// Exact file names, such as `Rakefile`.
    #[serde(default)]
    pub filenames: Vec<String>,
    /// Interpreter names in a `#!` line, such as `python3`.
    #[serde(default)]
    pub shebangs: Vec<String>,
    /// Names in Vim (`ft=`) and Emacs (`mode:`) modelines.
    #[serde(default)]
    pub modelines: Vec<String>,
}

/// A build system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct BuildDescriptor {
    /// Build system name.
    pub name: String,
    /// Files whose presence in a directory marks a module, such as `pom.xml`.
    pub detect: Vec<String>,
    /// Gitignore-style patterns of the files the build generates, such as
    /// `**/target/generated-sources/`.
    #[serde(default)]
    pub generated: Vec<String>,
}

/// A version control system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct VcsDescriptor {
    /// System name.
    pub name: String,
    /// Names at the project root whose presence, as a directory or a file,
    /// means the system is in use.
    pub detect: Vec<String>,
    /// Directories always skipped, whatever ignore files say.
    #[serde(default)]
    pub skip_dirs: Vec<String>,
    /// Ignore files honored when the system is detected, such as `.gitignore`.
    #[serde(default)]
    pub honor_ignore_files: Vec<String>,
    /// Attributes, set in the system's attribute files, that mark a file as
    /// generated or vendored, such as git's `linguist-generated`.
    #[serde(default)]
    pub generated_attributes: Vec<String>,
    /// How to keep `.codix/` out of the system.
    pub ignore: IgnoreStrategy,
}

/// How a version control system is kept from tracking `.codix/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "strategy", rename_all = "kebab-case", deny_unknown_fields)]
pub enum IgnoreStrategy {
    /// Write an ignore file inside `.codix/` that ignores the directory itself.
    NestedFile {
        /// File name inside `.codix/`, such as `.gitignore`.
        file: String,
        /// File content.
        content: String,
    },
    /// Print a one-line suggestion at `codix init`; never edit tracked files.
    Hint {
        /// The suggestion.
        hint: String,
    },
    /// Only skip the system's directories.
    None,
}

/// A TOML error on one line, with the line it is on, such as
/// ``line 3: unknown field `x` ``.
pub fn toml_error(text: &str, error: &toml::de::Error) -> String {
    match error.span() {
        Some(span) => {
            let line = text[..span.start.min(text.len())].matches('\n').count() + 1;
            format!("line {line}: {}", error.message())
        }
        None => error.message().to_owned(),
    }
}

/// A descriptor of any kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Descriptor {
    /// A language pack, boxed: it is much larger than the others.
    Language(Box<LanguageDescriptor>),
    /// A build system.
    Build(BuildDescriptor),
    /// A version control system.
    Vcs(VcsDescriptor),
}

impl Descriptor {
    /// Parses descriptor TOML of the given kind, and checks what the schema
    /// cannot express.
    pub fn parse(kind: ProviderKind, text: &str) -> Result<Self, String> {
        let error = |e| toml_error(text, &e);
        let descriptor = match kind {
            ProviderKind::Language => {
                Self::Language(Box::new(toml::from_str(text).map_err(error)?))
            }
            ProviderKind::Build => Self::Build(toml::from_str(text).map_err(error)?),
            ProviderKind::Vcs => Self::Vcs(toml::from_str(text).map_err(error)?),
        };
        if let Self::Language(language) = &descriptor {
            for (capture, kind) in &language.symbols.kinds {
                if !SYMBOL_KINDS.contains(&kind.as_str()) {
                    return Err(format!(
                        "symbols.kinds.{capture} = \"{kind}\" is not one of: {}",
                        SYMBOL_KINDS.join(", ")
                    ));
                }
            }
            let patterns = language
                .names
                .identifier
                .iter()
                .chain(&language.generated.headers);
            for pattern in patterns {
                if let Err(e) = regex::Regex::new(pattern) {
                    return Err(format!(
                        "`{pattern}` is not a valid regular expression: {e}"
                    ));
                }
            }
            for context in language
                .scopes
                .contexts
                .keys()
                .chain(language.scopes.member_contexts.keys())
            {
                if !REFERENCE_CONTEXTS.contains(&context.as_str()) {
                    return Err(format!(
                        "scopes.contexts.{context} is not one of: {}",
                        REFERENCE_CONTEXTS.join(", ")
                    ));
                }
            }
        }
        Ok(descriptor)
    }

    /// The kind of provider it describes.
    pub fn kind(&self) -> ProviderKind {
        match self {
            Self::Language(_) => ProviderKind::Language,
            Self::Build(_) => ProviderKind::Build,
            Self::Vcs(_) => ProviderKind::Vcs,
        }
    }

    /// The provider name the descriptor declares.
    pub fn name(&self) -> &str {
        match self {
            Self::Language(d) => &d.name,
            Self::Build(d) => &d.name,
            Self::Vcs(d) => &d.name,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn vcs_descriptor_from_the_design() {
        let text = r#"
            name = "git"
            detect = [".git"]
            skip_dirs = [".git"]
            honor_ignore_files = [".gitignore"]
            [ignore]
            strategy = "nested-file"
            file = ".gitignore"
            content = "*\n"
        "#;
        let Descriptor::Vcs(git) = Descriptor::parse(ProviderKind::Vcs, text).unwrap() else {
            panic!("not a vcs descriptor");
        };
        assert_eq!(
            git.ignore,
            IgnoreStrategy::NestedFile {
                file: ".gitignore".to_owned(),
                content: "*\n".to_owned()
            }
        );
    }

    #[test]
    fn unknown_fields_are_refused() {
        let text = "name = \"java\"\nversion = 1\nid_prefix = \"java\"\nextensionz = [\"java\"]\n";
        let error = Descriptor::parse(ProviderKind::Language, text).unwrap_err();
        assert!(error.contains("extensionz"), "{error}");
    }
}

#[cfg(test)]
mod symbol_tests {
    use super::*;

    #[test]
    fn kinds_outside_the_vocabulary_are_refused() {
        let text =
            "name = \"x\"\nversion = 1\nid_prefix = \"x\"\n[symbols.kinds]\nclass = \"klass\"\n";
        let error = Descriptor::parse(ProviderKind::Language, text).unwrap_err();
        assert!(error.contains("klass"), "{error}");
    }

    #[test]
    fn unknown_reference_contexts_are_refused() {
        let text = "name = \"x\"\nversion = 1\nid_prefix = \"x\"\n[scopes.contexts]\ninvoke = [\"method\"]\n";
        let error = Descriptor::parse(ProviderKind::Language, text).unwrap_err();
        assert!(error.contains("invoke"), "{error}");
    }

    #[test]
    fn invalid_name_patterns_are_refused() {
        let text = "name = \"x\"\nversion = 1\nid_prefix = \"x\"\n[names]\nidentifier = \"[a-\"\n";
        let error = Descriptor::parse(ProviderKind::Language, text).unwrap_err();
        assert!(error.contains("regular expression"), "{error}");
    }

    #[test]
    fn unlisted_kinds_and_contexts_use_the_value_namespace() {
        let text = "name = \"x\"\nversion = 1\nid_prefix = \"x\"\n[scopes.namespaces]\nclass = \"type\"\n[scopes.contexts]\nread = [\"value\", \"type\"]\n";
        let Descriptor::Language(d) = Descriptor::parse(ProviderKind::Language, text).unwrap()
        else {
            panic!("not a language");
        };
        assert_eq!(d.scopes.namespace_of("class"), "type");
        assert_eq!(d.scopes.namespace_of("variable"), "value");
        assert_eq!(d.scopes.read_by("read"), ["value", "type"]);
        assert_eq!(d.scopes.read_by("call"), ["value"]);
    }

    #[test]
    fn vocabulary_kinds_map_to_themselves() {
        let text =
            "name = \"x\"\nversion = 1\nid_prefix = \"x\"\n[symbols.kinds]\nrecord = \"class\"\n";
        let Descriptor::Language(d) = Descriptor::parse(ProviderKind::Language, text).unwrap()
        else {
            panic!("not a language");
        };
        assert_eq!(d.symbol_kind("record"), Some("class"));
        assert_eq!(d.symbol_kind("method"), Some("method"));
        assert_eq!(d.symbol_kind("call"), None);
    }
}
