//! The XML corpus: expected symbols for each case in `corpus/xml`.

use codix_lang_xml as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("xml");
}
