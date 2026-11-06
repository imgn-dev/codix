//! The TypeScript language pack: TypeScript and TSX, one grammar each, sharing
//! one `tags` and one `locals` query, which TSX extends for JSX.

codix_provider::register!(
    Language,
    "typescript",
    "descriptor.toml",
    tree_sitter_typescript::LANGUAGE_TYPESCRIPT,
    "queries/tags.scm",
    "queries/locals.scm"
);
codix_provider::register!(
    Language,
    "tsx",
    "tsx.toml",
    tree_sitter_typescript::LANGUAGE_TSX,
    "queries/tags.scm",
    "queries/locals.scm",
    "queries/locals-jsx.scm"
);
