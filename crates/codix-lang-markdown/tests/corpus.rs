//! The Markdown corpus: expected symbols for each case in `corpus/markdown`.

use codix_lang_markdown as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("markdown");
}
