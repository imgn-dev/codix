//! The YAML corpus: expected symbols for each case in `corpus/yaml`.

use codix_lang_yaml as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("yaml");
}
