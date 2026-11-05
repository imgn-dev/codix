//! The PHP language pack.

codix_provider::register!(
    Language,
    "php",
    "descriptor.toml",
    tree_sitter_php::LANGUAGE_PHP,
    "queries/tags.scm"
);
