//! The C++ corpus: expected symbols for each case in `corpus/cpp`.

use codix_lang_cpp as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("cpp");
}
