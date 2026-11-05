//! The SQL corpus: expected symbols for each case in `corpus/sql`.

use codix_lang_sql as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("sql");
}
