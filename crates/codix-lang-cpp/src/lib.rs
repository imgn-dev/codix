//! The C++ language pack.

codix_provider::register!(
    Language,
    "cpp",
    "descriptor.toml",
    tree_sitter_cpp::LANGUAGE,
    "queries/tags.scm"
);
