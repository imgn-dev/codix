//! The resolvers xtask packages declare the protocol and views codix speaks.

#[test]
fn resolver_versions_are_codix_s() {
    assert_eq!(xtask::plugins::RESOLVER_PROTOCOL, codix_resolver::PROTOCOL);
    assert_eq!(xtask::plugins::RESOLVER_VIEWS, codix_resolver::VIEWS);
}
