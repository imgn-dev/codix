//! The JSON corpus: expected symbols for each case in `corpus/json`.

use codix_lang_json as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("json");
}
