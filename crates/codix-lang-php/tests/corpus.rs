//! The PHP corpus: expected symbols for each case in `corpus/php`.

use codix_lang_php as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("php");
}
