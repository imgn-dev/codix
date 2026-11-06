//! Fails the build when a descriptor is malformed.

use std::io::Write as _;

fn main() {
    for query in [
        "queries/tags.scm",
        "queries/locals.scm",
        "queries/locals-jsx.scm",
    ] {
        writeln!(std::io::stdout(), "cargo::rerun-if-changed={query}")
            .expect("write the rerun directive for cargo");
    }
    codix_provider::check_descriptor(
        codix_provider::ProviderKind::Language,
        "typescript",
        "descriptor.toml",
    );
    codix_provider::check_descriptor(codix_provider::ProviderKind::Language, "tsx", "tsx.toml");
}
