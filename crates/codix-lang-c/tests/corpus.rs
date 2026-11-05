//! The C corpus: expected symbols for each case in `corpus/c`.

use codix_lang_c as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("c");
}
