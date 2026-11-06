//! The JavaScript corpus: expected symbols and scopes for each case in `corpus/javascript`.

use codix_lang_javascript as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("javascript");
}

#[test]
fn scopes() {
    codix_testkit::check_language_scopes("javascript");
}

#[test]
fn round_trips() {
    codix_testkit::check_language_round_trips("javascript");
}
