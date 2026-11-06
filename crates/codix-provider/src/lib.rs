//! Provider traits, descriptor schema and registration for language packs,
//! build systems and version control systems.
//!
//! A provider is a crate that registers one [`ProviderEntry`] at link time with
//! [`register!`]. Its descriptor is TOML embedded in the binary and validated
//! by the crate's build script ([`check_descriptor`]), so a malformed
//! descriptor fails the build rather than a run.

pub mod build;
pub mod descriptor;
pub mod known;
mod registry;

pub use build::{BuildModel, BuildResolver, Module, Unsupported};
pub use descriptor::{
    BuildDescriptor, Descriptor, Detect, IgnoreStrategy, LanguageDescriptor, ProviderKind, Scopes,
    VcsDescriptor,
};
pub use registry::{Grammar, ProviderEntry, Registry, registry};

#[doc(hidden)]
pub use inventory;

/// Registers a provider: `register!(Vcs, "git", "descriptor.toml")`, with
/// paths relative to the calling crate's manifest directory. A language pack
/// that parses adds its grammar and `tags` query, and for tier 2 its `locals`
/// query, which may be several files concatenated:
/// `register!(Language, "java", "descriptor.toml", tree_sitter_java::LANGUAGE, "queries/tags.scm", "queries/locals.scm")`;
/// a build system adds its resolver: `register!(Build, "maven", "descriptor.toml", &Maven)`.
#[macro_export]
macro_rules! register {
    ($kind:ident, $name:literal, $path:literal) => {
        $crate::inventory::submit! {
            $crate::ProviderEntry {
                kind: $crate::ProviderKind::$kind,
                name: $name,
                descriptor: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)),
                grammar: None,
                resolver: None,
            }
        }
    };
    (Build, $name:literal, $path:literal, $resolver:expr) => {
        $crate::inventory::submit! {
            $crate::ProviderEntry {
                kind: $crate::ProviderKind::Build,
                name: $name,
                descriptor: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)),
                grammar: None,
                resolver: Some($resolver),
            }
        }
    };
    (Language, $name:literal, $path:literal, $language:expr, $tags:literal) => {
        $crate::inventory::submit! {
            $crate::ProviderEntry {
                kind: $crate::ProviderKind::Language,
                name: $name,
                descriptor: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)),
                grammar: Some($crate::Grammar {
                    language: $language,
                    tags: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $tags)),
                    locals: None,
                }),
                resolver: None,
            }
        }
    };
    (Language, $name:literal, $path:literal, $language:expr, $tags:literal, $($locals:literal),+) => {
        $crate::inventory::submit! {
            $crate::ProviderEntry {
                kind: $crate::ProviderKind::Language,
                name: $name,
                descriptor: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)),
                grammar: Some($crate::Grammar {
                    language: $language,
                    tags: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $tags)),
                    locals: Some(concat!($(include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $locals))),+)),
                }),
                resolver: None,
            }
        }
    };
}

/// For provider build scripts: parses the descriptor at `path` as `kind` and
/// panics, failing the build, when it is malformed or misnamed.
///
/// # Panics
///
/// When the file cannot be read, does not parse, or names another provider.
pub fn check_descriptor(kind: ProviderKind, name: &str, path: &str) {
    use std::io::Write as _;
    writeln!(std::io::stdout(), "cargo::rerun-if-changed={path}")
        .expect("write the rerun directive for cargo");
    let text = std::fs::read_to_string(path)
        .unwrap_or_else(|e| panic!("cannot read provider descriptor {path}: {e}"));
    let descriptor = Descriptor::parse(kind, &text)
        .unwrap_or_else(|e| panic!("invalid provider descriptor {path}: {e}"));
    assert!(
        descriptor.name() == name,
        "provider descriptor {path} names `{}`, expected `{name}`",
        descriptor.name()
    );
}
