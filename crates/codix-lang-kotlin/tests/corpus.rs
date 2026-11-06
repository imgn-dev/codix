//! The Kotlin corpus: expected symbols and scopes for each case in `corpus/kotlin`.

use codix_lang_kotlin as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("kotlin");
}

#[test]
fn scopes() {
    codix_testkit::check_language_scopes("kotlin");
}

#[test]
fn round_trips() {
    codix_testkit::check_language_round_trips("kotlin");
}
