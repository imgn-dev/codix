//! Grammars are loaded on first use, once per process, and parser versions
//! are computed from them on first use.

use std::fs;

use codix_core::Error;
use codix_core::config::Config;
use codix_core::error::exit;
use codix_core::parse::PackIndexer;
use codix_core::project::Project;
use codix_core::scan::Indexer as _;
use codix_provider::registry;

use codix_lang_java as _;

#[test]
fn a_grammar_loads_once() {
    let grammar = registry().grammars.get("java").expect("java grammar");
    let first = grammar.language().expect("loads");
    let second = grammar.language().expect("loads");
    assert!(std::ptr::eq(first, second));
    assert!(
        (tree_sitter::MIN_COMPATIBLE_LANGUAGE_VERSION..=tree_sitter::LANGUAGE_VERSION)
            .contains(&first.abi_version())
    );
}

#[test]
fn parser_versions_name_the_pack_and_fingerprint_its_grammar() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = codix_core::project::canonicalize(dir.path()).expect("canonical");
    fs::create_dir_all(root.join(".codix")).expect("data dir");
    let project = Project::discover(&root)
        .expect("discover")
        .expect("project");
    let config = Config::load(&project).expect("config");
    let indexer = PackIndexer::new(&config);

    let version = indexer.parser_version("java");
    let descriptor = registry()
        .languages
        .iter()
        .find(|d| d.name == "java")
        .expect("java pack");
    let prefix = format!("java-{}-", descriptor.version);
    let fingerprint = version.strip_prefix(&prefix).expect("pack and version");
    assert_eq!(fingerprint.len(), 8, "{version}");
    assert_eq!(indexer.parser_version("java"), version);
    assert_eq!(indexer.parser_version("cobol"), "text-1");
}

#[test]
fn an_unusable_provider_is_plugin_invalid() {
    let error = Error::PluginInvalid {
        provider: "kotlin".to_owned(),
        cause: "grammar ABI 16, this build reads 13 to 15".to_owned(),
    };
    assert_eq!(error.code(), "plugin_invalid");
    assert_eq!(
        error.cause(),
        "the kotlin provider cannot be used: grammar ABI 16, this build reads 13 to 15"
    );
    assert_eq!(error.exit_code(), exit::INTERNAL);
}
