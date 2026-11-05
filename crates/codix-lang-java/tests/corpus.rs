//! The Java corpus: expected symbols for each case in `corpus/java`.

use codix_core::parse::extract;
use codix_core::store::{Confidence, SymbolRow};
use codix_provider::registry;

use codix_lang_java as _;

fn line(symbol: &SymbolRow) -> String {
    let visibility = symbol
        .visibility
        .as_ref()
        .map(|v| format!(" [{v}]"))
        .unwrap_or_default();
    let low = if symbol.confidence == Confidence::Low {
        " (low)"
    } else {
        ""
    };
    let signature = symbol
        .signature
        .as_ref()
        .map(|s| format!(" | {s}"))
        .unwrap_or_default();
    format!(
        "{}:{} {} {}{visibility}{low}{signature}",
        symbol.name_range.start.line,
        symbol.name_range.start.column,
        symbol.kind,
        symbol.qualified_name
    )
}

#[test]
fn corpus() {
    let registry = registry();
    let descriptor = registry
        .languages
        .iter()
        .find(|d| d.name == "java")
        .unwrap();
    let grammar = registry.grammars.get("java").unwrap();
    codix_testkit::check_corpus(
        concat!(env!("CARGO_MANIFEST_DIR"), "/../../corpus/java"),
        |name, content| {
            extract(descriptor, grammar, name, content)
                .unwrap()
                .iter()
                .map(line)
                .collect()
        },
    );
}
