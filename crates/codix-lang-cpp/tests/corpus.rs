//! The C++ corpus: expected symbols and scopes for each case in `corpus/cpp`.

use codix_lang_cpp as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("cpp");
}

#[test]
fn scopes() {
    codix_testkit::check_language_scopes("cpp");
}

#[test]
fn round_trips() {
    codix_testkit::check_language_round_trips("cpp");
}
