//! A language plugin built from the Java pack parses the Java corpus as the
//! compiled-in pack does: the same symbols, scopes and references.

use std::path::Path;

use codix_lang_java as _;
use codix_provider::{Descriptor, Grammar, registry};

#[test]
fn the_java_plugin_gives_the_compiled_in_results() {
    let out = Path::new(env!("CARGO_TARGET_TMPDIR")).join("plugin-language");
    let dir = codix_testkit::plugin::language_plugin("java", &out);
    let found = codix_plugin::discover(&[out]);
    let [Ok(plugin)] = found.as_slice() else {
        panic!("{found:?}");
    };
    assert_eq!(plugin.dir, dir);
    let Descriptor::Language(descriptor) = &plugin.descriptor else {
        panic!("not a language: {plugin:?}");
    };
    let grammar = Grammar::plugin(plugin.grammar.clone().expect("a grammar"));
    let compiled = &registry().grammars["java"];
    assert_ne!(
        grammar.language().expect("the plugin loads"),
        compiled.language().expect("the pack loads"),
        "the plugin's grammar is its own"
    );
    codix_testkit::check_pack_corpus(descriptor, &grammar);
    codix_testkit::check_pack_scopes(descriptor, &grammar);
}
