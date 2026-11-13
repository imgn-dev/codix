//! What the index keeps beyond names for Java: declared types, supertypes,
//! the package and imports, receivers, their types and argument counts.

use codix_core::scope::RefContext;
use codix_core::store::{ImportKind, ImportRow};

use codix_lang_java as _;

const SOURCE: &str = "package demo;

import java.util.List;
import java.util.*;
import static java.util.Objects.requireNonNull;

public class Factory extends AbstractFactory<String> implements BeanFactory, Comparable<Factory> {
    private Registry registry;
    private List<String> names;

    Object create(String name, Registry other) {
        var found = registry.lookup(name, 2);
        other.lookup(name);
        this.registry.lookup(name, 1, 3);
        found.lookup();
        return requireNonNull(found);
    }
}

interface BeanFactory extends Base, java.io.Serializable {}
";

fn model() -> codix_core::scope::FileModel {
    let registry = codix_provider::registry();
    let descriptor = registry
        .languages
        .iter()
        .find(|d| d.name == "java")
        .expect("java");
    let grammar = &registry.grammars["java"];
    codix_core::scope::model(descriptor, grammar, "src/demo/Factory.java", SOURCE).expect("model")
}

#[test]
fn types_and_supertypes_are_kept_as_written() {
    let model = model();
    let symbol = |name: &str| {
        model
            .symbols
            .iter()
            .find(|s| s.name == name)
            .expect("symbol")
    };
    assert_eq!(
        symbol("registry").declared_type.as_deref(),
        Some("Registry")
    );
    assert_eq!(symbol("names").declared_type.as_deref(), Some("List"));
    assert_eq!(
        symbol("Factory").supertypes,
        ["AbstractFactory", "BeanFactory", "Comparable"]
    );
    assert_eq!(
        symbol("BeanFactory").supertypes,
        ["Base", "java.io.Serializable"]
    );
}

#[test]
fn the_package_and_imports_are_kept_with_their_flags() {
    let import = |kind, name: &str, is_static, wildcard, line| ImportRow {
        kind,
        name: name.to_owned(),
        alias: None,
        is_static,
        wildcard,
        system: false,
        line,
    };
    assert_eq!(
        model().imports,
        [
            import(ImportKind::Package, "demo", false, false, 1),
            import(ImportKind::Import, "java.util.List", false, false, 3),
            import(ImportKind::Import, "java.util", false, true, 4),
            import(
                ImportKind::Import,
                "java.util.Objects.requireNonNull",
                true,
                false,
                5
            ),
        ]
    );
}

#[test]
fn calls_keep_their_receiver_its_declared_type_and_their_arity() {
    let model = model();
    let calls: Vec<(Option<&str>, Option<&str>, Option<u32>)> = model
        .references
        .iter()
        .filter(|r| r.context == RefContext::Call)
        .map(|r| {
            (
                r.receiver_text.as_deref(),
                r.receiver_type.as_deref(),
                r.arity,
            )
        })
        .collect();
    assert_eq!(
        calls,
        [
            (Some("registry"), Some("Registry"), Some(2)),
            (Some("other"), Some("Registry"), Some(1)),
            (Some("this.registry"), Some("Registry"), Some(3)),
            // `var` leaves the type to inference.
            (Some("found"), None, Some(0)),
            (None, None, Some(1)),
        ]
    );
}
