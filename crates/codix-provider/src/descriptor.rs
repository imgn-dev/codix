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
    /// How files of the language are recognized.
    #[serde(default)]
    pub detect: Detect,
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
    /// Parses descriptor TOML of the given kind.
    pub fn parse(kind: ProviderKind, text: &str) -> Result<Self, toml::de::Error> {
        Ok(match kind {
            ProviderKind::Language => Self::Language(toml::from_str(text)?),
            ProviderKind::Build => Self::Build(toml::from_str(text)?),
            ProviderKind::Vcs => Self::Vcs(toml::from_str(text)?),
        })
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
        let text = "name = \"java\"\nversion = 1\nextensionz = [\"java\"]\n";
        let error = Descriptor::parse(ProviderKind::Language, text).unwrap_err();
        assert!(error.to_string().contains("extensionz"), "{error}");
    }
}
