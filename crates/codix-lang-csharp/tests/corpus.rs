//! The C# corpus: expected symbols and scopes for each case in `corpus/csharp`.

use codix_lang_csharp as _;

#[test]
fn corpus() {
    codix_testkit::check_language_corpus("csharp");
}

#[test]
fn scopes() {
    codix_testkit::check_language_scopes("csharp");
}

#[test]
fn round_trips() {
    codix_testkit::check_language_round_trips("csharp");
}
