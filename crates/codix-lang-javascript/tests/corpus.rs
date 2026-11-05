//! The JavaScript corpus: expected symbols for each case in `corpus/javascript`.

use codix_lang_javascript as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("javascript");
}
