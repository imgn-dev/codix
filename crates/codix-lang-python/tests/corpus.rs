//! The Python corpus: expected symbols for each case in `corpus/python`.

use codix_lang_python as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("python");
}
