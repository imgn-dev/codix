//! The Rust corpus: expected symbols and scopes for each case in `corpus/rust`.

use codix_lang_rust as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("rust");
}

#[test]
fn scopes() {
    codix_testkit::check_language_scopes("rust");
}
