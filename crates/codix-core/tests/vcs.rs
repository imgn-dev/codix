//! Version control detection and ignore strategies.

use std::fs;
use std::path::{Path, PathBuf};

use codix_core::project::Project;
use codix_core::vcs::Vcs;
use tempfile::TempDir;

use codix_vcs_git as _;
use codix_vcs_hg as _;

// Jujutsu is not a codix provider; a fixture stands in for a second system
// colocated with git.
codix_provider::register!(Vcs, "jujutsu", "tests/fixtures/jujutsu.toml");

fn temp() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = fs::canonicalize(dir.path()).expect("canonical temp dir");
    (dir, path)
}

fn project(root: &Path) -> Project {
    fs::create_dir_all(root.join(".codix")).expect("create .codix");
    Project::discover(root).expect("discover").expect("project")
}

#[test]
fn git_clone_is_detected_from_its_directory() {
    let (_guard, root) = temp();
    fs::create_dir(root.join(".git")).unwrap();
    let vcs = Vcs::detect(&root);
    assert_eq!(vcs.names(), ["git"]);
    assert_eq!(vcs.skip_dirs(), [".git"]);
    assert_eq!(vcs.ignore_files(), [".gitignore"]);
}

#[test]
fn git_worktree_is_detected_from_its_file() {
    let (_guard, root) = temp();
    fs::write(
        root.join(".git"),
        "gitdir: /elsewhere/.git/worktrees/feature\n",
    )
    .unwrap();
    assert_eq!(Vcs::detect(&root).names(), ["git"]);
}

#[test]
fn submodule_is_detected_from_its_file() {
    let (_guard, root) = temp();
    let module = root.join("vendor/lib");
    fs::create_dir_all(&module).unwrap();
    fs::write(module.join(".git"), "gitdir: ../../.git/modules/lib\n").unwrap();
    assert_eq!(Vcs::detect(&module).names(), ["git"]);
}

#[test]
fn project_below_the_repository_root_is_detected() {
    let (_guard, root) = temp();
    fs::create_dir(root.join(".git")).unwrap();
    fs::create_dir_all(root.join("services/billing")).unwrap();
    assert_eq!(Vcs::detect(&root.join("services/billing")).names(), ["git"]);
}

#[test]
fn colocated_systems_all_apply() {
    let (_guard, root) = temp();
    fs::create_dir(root.join(".git")).unwrap();
    fs::create_dir(root.join(".jj")).unwrap();
    let vcs = Vcs::detect(&root);
    assert_eq!(vcs.names(), ["git", "jujutsu"]);
    assert_eq!(vcs.skip_dirs(), [".git", ".jj"]);
}

#[test]
fn no_system_without_markers() {
    let (_guard, root) = temp();
    assert_eq!(Vcs::detect(&root).names(), Vec::<&str>::new());
}

#[test]
fn nested_file_strategy_writes_inside_codix_only() {
    let (_guard, root) = temp();
    fs::create_dir(root.join(".git")).unwrap();
    fs::write(root.join(".gitignore"), "target/\n").unwrap();
    let p = project(&root);
    let hints = Vcs::detect(&root).apply_ignore_strategies(&p).unwrap();
    assert_eq!(hints, Vec::<String>::new());
    assert_eq!(
        fs::read_to_string(root.join(".codix/.gitignore")).unwrap(),
        "*\n"
    );
    assert_eq!(
        fs::read_to_string(root.join(".gitignore")).unwrap(),
        "target/\n"
    );
}

#[test]
fn hint_strategy_returns_its_hint_and_writes_nothing() {
    let (_guard, root) = temp();
    fs::create_dir(root.join(".hg")).unwrap();
    let p = project(&root);
    let hints = Vcs::detect(&root).apply_ignore_strategies(&p).unwrap();
    assert_eq!(hints, ["add `^\\.codix/` to .hgignore"]);
    assert_eq!(fs::read_dir(root.join(".codix")).unwrap().count(), 0);
}
