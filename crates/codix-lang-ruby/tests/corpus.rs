//! The Ruby corpus: expected symbols for each case in `corpus/ruby`.

use codix_lang_ruby as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("ruby");
}
