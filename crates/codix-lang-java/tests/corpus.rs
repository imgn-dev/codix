//! The Java corpus: expected symbols for each case in `corpus/java`.

use codix_lang_java as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("java");
}
