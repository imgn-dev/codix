//! Provider traits, descriptor schema and registration for language packs,
//! build systems and version control systems.
//!
//! A provider is a crate that registers one [`ProviderEntry`] at link time with
//! [`register!`]. Its descriptor is TOML embedded in the binary and validated
//! by the crate's build script ([`check_descriptor`]), so a malformed
//! descriptor fails the build rather than a run.

pub mod descriptor;
pub mod known;
mod registry;

pub use descriptor::{
    BuildDescriptor, Descriptor, IgnoreStrategy, LanguageDescriptor, ProviderKind, VcsDescriptor,
};
pub use registry::{ProviderEntry, Registry, registry};

#[doc(hidden)]
pub use inventory;

/// Registers a provider: `register!(Vcs, "git", "descriptor.toml")`, with the
/// descriptor path relative to the calling crate's manifest directory.
#[macro_export]
macro_rules! register {
    ($kind:ident, $name:literal, $path:literal) => {
        $crate::inventory::submit! {
            $crate::ProviderEntry {
                kind: $crate::ProviderKind::$kind,
                name: $name,
                descriptor: include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/", $path)),
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
