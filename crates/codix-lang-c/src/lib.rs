//! The C language pack.

codix_provider::register!(
    Language,
    "c",
    "descriptor.toml",
    tree_sitter_c::LANGUAGE,
    "queries/tags.scm",
    "queries/locals.scm"
);
