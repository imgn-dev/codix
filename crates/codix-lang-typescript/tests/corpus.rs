//! The TypeScript corpus: expected symbols and scopes for each case in
//! `corpus/typescript` and `corpus/tsx`.

use codix_lang_typescript as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("typescript");
}

#[test]
fn tsx_corpus() {
    codix_testkit::check_language_corpus("tsx");
}

#[test]
fn scopes() {
    codix_testkit::check_language_scopes("typescript");
}

#[test]
fn tsx_scopes() {
    codix_testkit::check_language_scopes("tsx");
}
