//! The Python language pack.

codix_provider::register!(
    Language,
    "python",
    "descriptor.toml",
    tree_sitter_python::LANGUAGE,
    "queries/tags.scm",
    "queries/locals.scm"
);
