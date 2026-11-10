//! Every language pack, built as a plugin, parses its corpus as the
//! compiled-in pack does: the same symbols, and at tier 2 the same scopes.

use std::path::Path;

use codix_provider::{Descriptor, Grammar};

#[test]
fn every_language_plugin_passes_its_corpus() {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("xtask-corpus");
    std::thread::scope(|scope| {
        for pack in xtask::plugins::PACKS {
            let out = &out;
            scope.spawn(move || {
                xtask::plugins::language_plugin(pack.language, out)
                    .unwrap_or_else(|e| panic!("{}: {e}", pack.language));
            });
        }
    });
    let found = codix_plugin::discover(std::slice::from_ref(&out));
    assert_eq!(found.len(), xtask::plugins::PACKS.len());
    for plugin in found {
        let plugin = plugin.unwrap_or_else(|r| panic!("{r:?}"));
        let Descriptor::Language(descriptor) = &plugin.descriptor else {
            panic!("not a language: {plugin:?}");
        };
        let grammar = Grammar::plugin(plugin.grammar.clone().expect("a grammar"));
        grammar
            .language()
            .unwrap_or_else(|e| panic!("{}: {e}", descriptor.name));
        codix_testkit::check_pack_corpus(descriptor, &grammar);
        if grammar.locals.is_some() {
            codix_testkit::check_pack_scopes(descriptor, &grammar);
        }
    }
}
