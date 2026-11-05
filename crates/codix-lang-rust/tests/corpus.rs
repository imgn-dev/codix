//! The Rust corpus: expected symbols for each case in `corpus/rust`.

use codix_lang_rust as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("rust");
}
