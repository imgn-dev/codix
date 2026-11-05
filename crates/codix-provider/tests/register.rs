//! A provider registered with `register!` shows up in the registry.

use codix_provider::{IgnoreStrategy, ProviderKind, registry};

codix_provider::register!(Vcs, "fixture-vcs", "tests/fixtures/fixture-vcs.toml");

#[test]
fn registered_provider_is_in_the_registry() {
    let registry = registry();
    assert!(registry.contains(ProviderKind::Vcs, "fixture-vcs"));
    assert_eq!(registry.names(), [(ProviderKind::Vcs, "fixture-vcs")]);
    assert!(matches!(
        registry.vcs[0].ignore,
        IgnoreStrategy::Hint { .. }
    ));
}
