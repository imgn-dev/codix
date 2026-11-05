//! The Markdown language pack.

codix_provider::register!(
    Language,
    "markdown",
    "descriptor.toml",
    tree_sitter_md::LANGUAGE,
    "queries/tags.scm"
);
