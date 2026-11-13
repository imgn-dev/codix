//! What the index keeps beyond names for C: includes, receivers and
//! argument counts.

use codix_core::store::{ImportKind, ImportRow};

use codix_lang_c as _;

const SOURCE: &str = "#include <linux/sched.h>
#include \"local.h\"

static int f(int a, int b) { return a + b; }
int g(struct ops *o) { return f(1, 2) + o->read(o, 3); }
";

fn model() -> codix_core::scope::FileModel {
    let registry = codix_provider::registry();
    let descriptor = registry
        .languages
        .iter()
        .find(|d| d.name == "c")
        .expect("c");
    let grammar = &registry.grammars["c"];
    codix_core::scope::model(descriptor, grammar, "src/x.c", SOURCE).expect("model")
}

#[test]
fn includes_are_kept_as_written() {
    let include = |name: &str, system, line| ImportRow {
        kind: ImportKind::Include,
        name: name.to_owned(),
        alias: None,
        is_static: false,
        wildcard: false,
        system,
        line,
    };
    assert_eq!(
        model().imports,
        [
            include("linux/sched.h", true, 1),
            include("local.h", false, 2)
        ]
    );
}

#[test]
fn calls_keep_their_receiver_and_arity() {
    let model = model();
    let calls: Vec<(&str, Option<&str>, Option<u32>)> = model
        .references
        .iter()
        .filter(|r| r.arity.is_some())
        .map(|r| (r.name.as_str(), r.receiver_text.as_deref(), r.arity))
        .collect();
    assert_eq!(calls, [("f", None, Some(2)), ("read", Some("o"), Some(2))]);
}
