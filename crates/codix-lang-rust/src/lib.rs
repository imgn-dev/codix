//! The Rust language pack.

codix_provider::register!(
    Language,
    "rust",
    "descriptor.toml",
    tree_sitter_rust::LANGUAGE,
    "queries/tags.scm"
);
