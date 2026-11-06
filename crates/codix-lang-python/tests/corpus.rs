//! The Python corpus: expected symbols and scopes for each case in `corpus/python`.

use codix_lang_python as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("python");
}

#[test]
fn scopes() {
    codix_testkit::check_language_scopes("python");
}

#[test]
fn round_trips() {
    codix_testkit::check_language_round_trips("python");
}
