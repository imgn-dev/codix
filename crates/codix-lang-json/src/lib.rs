//! The JSON language pack.

codix_provider::register!(
    Language,
    "json",
    "descriptor.toml",
    tree_sitter_json::LANGUAGE,
    "queries/tags.scm"
);
