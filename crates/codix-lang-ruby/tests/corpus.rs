//! The Ruby corpus: expected symbols and scopes for each case in `corpus/ruby`.

use codix_lang_ruby as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("ruby");
}

#[test]
fn scopes() {
    codix_testkit::check_language_scopes("ruby");
}

#[test]
fn round_trips() {
    codix_testkit::check_language_round_trips("ruby");
}
