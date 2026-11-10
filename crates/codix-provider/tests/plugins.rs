//! Plugins join the registry after the compiled-in providers: a compiled-in
//! provider wins over a plugin of the same name, and between plugins the
//! first found wins.

use std::path::PathBuf;

use codix_provider::{
    Descriptor, Plugin, PluginSource, ProviderKind, Rejected, Rejection, Source, registry,
};

codix_provider::register!(Vcs, "fixture-vcs", "tests/fixtures/fixture-vcs.toml");

fn vcs(name: &str, dir: &str) -> Plugin {
    let text =
        format!("name = \"{name}\"\ndetect = [\".{name}\"]\n\n[ignore]\nstrategy = \"none\"\n");
    Plugin {
        version: "2.0.0".to_owned(),
        dir: PathBuf::from(dir),
        descriptor: Descriptor::parse(ProviderKind::Vcs, &text).expect("descriptor"),
        grammar: None,
    }
}

fn found() -> Vec<Result<Plugin, Rejected>> {
    vec![
        Ok(vcs("fixture-vcs", "/a/vcs/fixture-vcs")),
        Ok(vcs("other", "/a/vcs/other")),
        Err(Rejected {
            kind: ProviderKind::Vcs,
            name: "broken".to_owned(),
            dir: PathBuf::from("/a/vcs/broken"),
            rejection: Rejection::Invalid("plugin.toml: missing".to_owned()),
        }),
        Ok(vcs("other", "/b/vcs/other")),
    ]
}

codix_provider::inventory::submit! {
    PluginSource { discover: found }
}

#[test]
fn plugins_join_the_registry_and_conflicts_are_left_out() {
    let registry = registry();
    assert_eq!(
        registry.names(),
        [
            (ProviderKind::Vcs, "fixture-vcs"),
            (ProviderKind::Vcs, "other")
        ]
    );
    let source = |name: &str| registry.sources[&(ProviderKind::Vcs, name.to_owned())].clone();
    assert_eq!(
        source("fixture-vcs"),
        (Source::CompiledIn, env!("CARGO_PKG_VERSION").to_owned())
    );
    assert_eq!(
        source("other"),
        (
            Source::Plugin(PathBuf::from("/a/vcs/other")),
            "2.0.0".to_owned()
        )
    );
    let rejected: Vec<(&str, &Rejection)> = registry
        .rejected
        .iter()
        .map(|r| (r.dir.to_str().expect("utf-8"), &r.rejection))
        .collect();
    assert_eq!(
        rejected,
        [
            (
                "/a/vcs/fixture-vcs",
                &Rejection::LeftOut("the vcs provider fixture-vcs is compiled in".to_owned())
            ),
            (
                "/a/vcs/broken",
                &Rejection::Invalid("plugin.toml: missing".to_owned())
            ),
            (
                "/b/vcs/other",
                &Rejection::LeftOut(format!(
                    "the plugin in {} comes first",
                    PathBuf::from("/a/vcs/other").display()
                ))
            ),
        ]
    );
}
