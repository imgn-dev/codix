//! The Go corpus: expected symbols and scopes for each case in `corpus/go`.

use codix_lang_go as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("go");
}

#[test]
fn scopes() {
    codix_testkit::check_language_scopes("go");
}
