//! The C# language pack.

codix_provider::register!(
    Language,
    "csharp",
    "descriptor.toml",
    tree_sitter_c_sharp::LANGUAGE,
    "queries/tags.scm",
    "queries/locals.scm"
);
