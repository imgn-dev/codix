//! The Ruby language pack.

codix_provider::register!(
    Language,
    "ruby",
    "descriptor.toml",
    tree_sitter_ruby::LANGUAGE,
    "queries/tags.scm",
    "queries/locals.scm"
);
