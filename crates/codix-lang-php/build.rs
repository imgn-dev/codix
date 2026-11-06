//! Fails the build when the descriptor is malformed.

use std::io::Write as _;

fn main() {
    for query in ["queries/tags.scm", "queries/locals.scm"] {
        writeln!(std::io::stdout(), "cargo::rerun-if-changed={query}")
            .expect("write the rerun directive for cargo");
    }
    codix_provider::check_descriptor(
        codix_provider::ProviderKind::Language,
        "php",
        "descriptor.toml",
    );
}
