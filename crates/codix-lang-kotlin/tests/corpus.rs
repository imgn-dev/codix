//! The Kotlin corpus: expected symbols for each case in `corpus/kotlin`.

use codix_lang_kotlin as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("kotlin");
}
