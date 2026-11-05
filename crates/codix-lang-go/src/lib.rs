//! The Go language pack.

codix_provider::register!(
    Language,
    "go",
    "descriptor.toml",
    tree_sitter_go::LANGUAGE,
    "queries/tags.scm"
);
