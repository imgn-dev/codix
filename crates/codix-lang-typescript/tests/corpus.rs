//! The TypeScript corpus: expected symbols for each case in `corpus/typescript`.

use codix_lang_typescript as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("typescript");
}

#[test]
fn tsx_corpus() {
    codix_testkit::check_language_corpus("tsx");
}
