//! Descriptor schema: what each kind of provider declares. Descriptors choose
//! among behaviors the engine implements and carry no code.

use std::fmt;

use serde::{Deserialize, Serialize};

/// The three kinds of provider.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
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
        f.write_str(match self {
            Self::Language => "language",
            Self::Build => "build",
            Self::Vcs => "vcs",
        })
    }
}

/// A language pack.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LanguageDescriptor {
    /// Language name, as in [`crate::known::LANGUAGES`].
    pub name: String,
    /// Pack version; part of the parser version stored per file.
    pub version: u32,
    /// Prefix of the language's symbol IDs, such as `py` in
    /// `py:src/billing/invoice.py::Invoice.total#a3f9`.
    pub id_prefix: String,
    /// How files of the language are recognized.
    #[serde(default)]
    pub detect: Detect,
    /// How definitions become symbols; packs without a grammar have none.
    #[serde(default)]
    pub symbols: Symbols,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Symbols {
    /// Between a parent's and a child's name in qualified names.
    #[serde(default = "default_separator")]
    pub separator: String,
    /// Fields holding a definition's body; its signature is the text before.
    #[serde(default)]
    pub body_fields: Vec<String>,
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
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
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct BuildDescriptor {
    /// Build system name.
    pub name: String,
    /// Files whose presence in a directory marks a module, such as `pom.xml`.
    pub detect: Vec<String>,
}

/// A version control system.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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
    /// How to keep `.codix/` out of the system.
    pub ignore: IgnoreStrategy,
}

/// How a version control system is kept from tracking `.codix/`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
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

/// A descriptor of any kind.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Descriptor {
    /// A language pack.
    Language(LanguageDescriptor),
    /// A build system.
    Build(BuildDescriptor),
    /// A version control system.
    Vcs(VcsDescriptor),
}

impl Descriptor {
    /// Parses descriptor TOML of the given kind, and checks what the schema
    /// cannot express.
    pub fn parse(kind: ProviderKind, text: &str) -> Result<Self, String> {
        let descriptor = match kind {
            ProviderKind::Language => {
                Self::Language(toml::from_str(text).map_err(|e| e.to_string())?)
            }
            ProviderKind::Build => Self::Build(toml::from_str(text).map_err(|e| e.to_string())?),
            ProviderKind::Vcs => Self::Vcs(toml::from_str(text).map_err(|e| e.to_string())?),
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
        }
        Ok(descriptor)
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
