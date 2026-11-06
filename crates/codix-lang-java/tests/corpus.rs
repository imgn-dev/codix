//! The Java corpus: expected symbols and scopes for each case in
//! `corpus/java`.

use codix_lang_java as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("java");
}

#[test]
fn scopes() {
    codix_testkit::check_language_scopes("java");
}

#[test]
fn round_trips() {
    codix_testkit::check_language_round_trips("java");
}
