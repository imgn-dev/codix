//! The Kotlin language pack.

codix_provider::register!(
    Language,
    "kotlin",
    "descriptor.toml",
    tree_sitter_kotlin_ng::LANGUAGE,
    "queries/tags.scm"
);
