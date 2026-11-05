//! The Java language pack.

codix_provider::register!(
    Language,
    "java",
    "descriptor.toml",
    tree_sitter_java::LANGUAGE,
    "queries/tags.scm"
);
