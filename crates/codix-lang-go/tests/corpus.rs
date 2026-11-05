//! The Go corpus: expected symbols for each case in `corpus/go`.

use codix_lang_go as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("go");
}
