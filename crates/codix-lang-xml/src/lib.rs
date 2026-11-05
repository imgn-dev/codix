//! The XML language pack.

codix_provider::register!(
    Language,
    "xml",
    "descriptor.toml",
    tree_sitter_xml::LANGUAGE_XML,
    "queries/tags.scm"
);
