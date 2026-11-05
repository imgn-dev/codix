//! Fails the build when the descriptor is malformed.

fn main() {
    codix_provider::check_descriptor(
        codix_provider::ProviderKind::Vcs,
        "mercurial",
        "descriptor.toml",
    );
}
