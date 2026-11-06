//! The PHP corpus: expected symbols and scopes for each case in `corpus/php`.

use codix_lang_php as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("php");
}

#[test]
fn scopes() {
    codix_testkit::check_language_scopes("php");
}
