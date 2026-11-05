//! The JavaScript language pack.

codix_provider::register!(
    Language,
    "javascript",
    "descriptor.toml",
    tree_sitter_javascript::LANGUAGE,
    "queries/tags.scm"
);
