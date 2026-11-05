//! The SQL language pack.

codix_provider::register!(
    Language,
    "sql",
    "descriptor.toml",
    tree_sitter_sequel::LANGUAGE,
    "queries/tags.scm"
);
