//! The YAML language pack.

codix_provider::register!(
    Language,
    "yaml",
    "descriptor.toml",
    tree_sitter_yaml::LANGUAGE,
    "queries/tags.scm"
);
