//! The walk: ignore precedence, default excludes, skip directories, size limit
//! and symlinks.

use std::fs;
use std::path::{Path, PathBuf};

use codix_core::project::Project;
use codix_core::vcs::Vcs;
use codix_core::walk::{Skip, Walk, walk};
use tempfile::TempDir;

use codix_vcs_git as _;

const LIMIT: u64 = 2 << 20;

fn temp() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = fs::canonicalize(dir.path()).expect("canonical temp dir");
    fs::create_dir_all(path.join(".codix")).expect("create .codix");
    (dir, path)
}

fn write(root: &Path, path: &str, content: &str) {
    let path = root.join(path);
    fs::create_dir_all(path.parent().expect("parent")).expect("create parent");
    fs::write(path, content).expect("write file");
}

fn walk_project(root: &Path) -> Walk {
    let project = Project::discover(root).expect("discover").expect("project");
    walk(&project, &Vcs::detect(root), LIMIT).expect("walk")
}

fn ids(walk: &Walk) -> Vec<&str> {
    walk.files.iter().map(|f| f.id.as_str()).collect()
}

#[test]
fn gitignore_applies_only_when_git_is_detected() {
    let (_guard, root) = temp();
    write(&root, ".gitignore", "build/\n");
    write(&root, "build/out.rs", "");
    write(&root, "src/main.rs", "");
    assert_eq!(
        ids(&walk_project(&root)),
        [".gitignore", "build/out.rs", "src/main.rs"]
    );

    fs::create_dir(root.join(".git")).unwrap();
    assert_eq!(ids(&walk_project(&root)), [".gitignore", "src/main.rs"]);
}

#[test]
fn codixignore_applies_without_version_control() {
    let (_guard, root) = temp();
    write(&root, ".codixignore", "*.gen.rs\n");
    write(&root, "src/a.rs", "");
    write(&root, "src/a.gen.rs", "");
    assert_eq!(ids(&walk_project(&root)), [".codixignore", "src/a.rs"]);
}

#[test]
fn codixignore_overrides_gitignore() {
    let (_guard, root) = temp();
    fs::create_dir(root.join(".git")).unwrap();
    write(&root, ".gitignore", "gen/\n");
    write(&root, ".codixignore", "!gen/\n");
    write(&root, "gen/parser.rs", "");
    assert!(ids(&walk_project(&root)).contains(&"gen/parser.rs"));
}

#[test]
fn skip_directories_always_win() {
    let (_guard, root) = temp();
    fs::create_dir(root.join(".git")).unwrap();
    write(&root, ".git/config", "");
    write(&root, ".codixignore", "!.git/\n!.git/config\n");
    write(&root, "vendor/lib/.git/HEAD", "");
    write(&root, "a.rs", "");
    assert_eq!(ids(&walk_project(&root)), [".codixignore", "a.rs"]);
}

#[test]
fn codix_data_and_default_heavy_directories_are_excluded() {
    let (_guard, root) = temp();
    write(&root, ".codix/index.db", "");
    write(&root, "web/node_modules/x/index.js", "");
    write(&root, "target/debug/out", "");
    write(&root, ".venv/lib/site.py", "");
    write(&root, "web/dist/app.js", "");
    write(&root, ".codixignore", "!dist/\n");
    write(&root, ".github/workflows/ci.yml", "");
    assert_eq!(
        ids(&walk_project(&root)),
        [".codixignore", ".github/workflows/ci.yml"],
        "hidden files are walked; defaults cannot be re-included"
    );
}

#[test]
fn files_over_the_size_limit_are_skipped() {
    let (_guard, root) = temp();
    write(
        &root,
        "big.json",
        &"x".repeat(usize::try_from(LIMIT).unwrap() + 1),
    );
    write(&root, "small.json", "{}");
    let walked = walk_project(&root);
    assert_eq!(ids(&walked), ["small.json"]);
    assert_eq!(walked.skipped, [("big.json".to_owned(), Skip::TooLarge)]);
}

#[test]
fn stat_fields_are_recorded() {
    let (_guard, root) = temp();
    write(&root, "a.rs", "fn a() {}\n");
    let walked = walk_project(&root);
    let file = &walked.files[0];
    assert_eq!(file.size, 10);
    assert!(file.mtime_ns > 0);
    if cfg!(unix) {
        assert_ne!(file.inode, 0);
    }
}

#[cfg(unix)]
#[test]
fn symlinks_inside_the_root_name_their_target_once() {
    let (_guard, root) = temp();
    write(&root, "src/real.rs", "");
    std::os::unix::fs::symlink("src/real.rs", root.join("alias.rs")).unwrap();
    std::os::unix::fs::symlink("src", root.join("src-link")).unwrap();
    assert_eq!(ids(&walk_project(&root)), ["src/real.rs"]);
}

#[cfg(unix)]
#[test]
fn symlinks_leaving_the_root_and_loops_are_skipped() {
    let (_guard, root) = temp();
    let outside = tempfile::tempdir().unwrap();
    fs::write(outside.path().join("secret.rs"), "").unwrap();
    std::os::unix::fs::symlink(outside.path().join("secret.rs"), root.join("secret.rs")).unwrap();
    std::os::unix::fs::symlink(outside.path(), root.join("elsewhere")).unwrap();
    std::os::unix::fs::symlink("loop-b", root.join("loop-a")).unwrap();
    std::os::unix::fs::symlink("loop-a", root.join("loop-b")).unwrap();
    write(&root, "a.rs", "");

    let walked = walk_project(&root);
    assert_eq!(ids(&walked), ["a.rs"]);
    assert!(
        walked
            .skipped
            .contains(&("secret.rs".to_owned(), Skip::LinkOutsideRoot))
    );
    let errors: Vec<&str> = walked.errors.iter().map(|(p, _)| p.as_str()).collect();
    assert_eq!(errors, ["loop-a", "loop-b"]);
}
