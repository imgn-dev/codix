//! The TOML corpus: expected symbols for each case in `corpus/toml`.

use codix_lang_toml as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("toml");
}
