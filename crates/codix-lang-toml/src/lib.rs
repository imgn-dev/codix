//! The TOML language pack.

codix_provider::register!(
    Language,
    "toml",
    "descriptor.toml",
    tree_sitter_toml_ng::LANGUAGE,
    "queries/tags.scm"
);
