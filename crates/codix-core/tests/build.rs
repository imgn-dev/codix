//! Scans store the build's modules; `sources.roots` overrides them.

use std::fs;
use std::path::{Path, PathBuf};

use codix_core::config::Config;
use codix_core::project::Project;
use codix_core::scan::{TextIndexer, scan};
use codix_core::store::Store;
use codix_provider::BuildModel;

use codix_build_gradle as _;
use codix_build_maven as _;

fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = codix_core::project::canonicalize(dir.path()).expect("canonical temp dir");
    fs::create_dir_all(root.join(".codix")).expect("create .codix");
    for (path, content) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        fs::write(path, content).expect("write file");
    }
    (dir, root)
}

fn scanned(root: &Path) -> BuildModel {
    let project = Project::discover(root).expect("discover").expect("project");
    let config = Config::load(&project).expect("config");
    scan(&project, &config, &TextIndexer).expect("scan");
    let mut store = Store::open(&project.index_path()).expect("open index");
    store.snapshot().expect("snapshot").build().expect("build")
}

#[test]
fn a_scan_stores_maven_modules() {
    let (_guard, root) = project(&[("pom.xml", "<project><artifactId>app</artifactId></project>")]);
    let model = scanned(&root);
    assert_eq!(model.modules.len(), 1);
    assert_eq!(model.modules[0].name, "app");
    assert_eq!(model.modules[0].source_roots, ["src/main/java"]);
}

#[test]
fn a_scan_stores_gradle_issues() {
    let (_guard, root) = project(&[("settings.gradle.kts", "includeBuild(\"plugins\")\n")]);
    let model = scanned(&root);
    assert_eq!(model.unsupported.len(), 1);
    assert_eq!(model.unsupported[0].file, "settings.gradle.kts");
    assert_eq!(model.unsupported[0].line, 1);
}

#[test]
fn source_roots_in_config_override_the_build() {
    let (_guard, root) = project(&[
        ("pom.xml", "<project><artifactId>app</artifactId></project>"),
        ("codix.toml", "[sources]\nroots = [\"java\", \"gen\"]\n"),
    ]);
    let model = scanned(&root);
    assert_eq!(model.modules.len(), 1);
    assert_eq!(model.modules[0].source_roots, ["java", "gen"]);
}

#[test]
fn no_build_files_means_no_modules() {
    let (_guard, root) = project(&[("README", "hello\n")]);
    assert_eq!(scanned(&root), BuildModel::default());
}
