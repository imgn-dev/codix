//! The C# corpus: expected symbols for each case in `corpus/csharp`.

use codix_lang_csharp as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("csharp");
}
