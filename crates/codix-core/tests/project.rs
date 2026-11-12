//! Project discovery, file identity, path rendering and scopes.

use std::fs;
use std::path::{Path, PathBuf};

use codix_core::op::Context;
use codix_core::project::{FileId, Project, Scope, ScopeRequest};
use tempfile::TempDir;

/// A temporary directory, canonicalized so comparisons with discovered roots
/// hold where the temp directory is behind a symlink (`/var` on macOS).
fn temp() -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("test setup");
    let path = codix_core::project::canonicalize(dir.path()).expect("test setup");
    (dir, path)
}

fn touch(path: &Path) {
    fs::create_dir_all(path.parent().expect("test setup")).expect("test setup");
    fs::write(path, "").expect("test setup");
}

fn project(root: &Path) -> Project {
    fs::create_dir_all(root.join(".codix")).expect("test setup");
    Project::discover(root)
        .expect("test setup")
        .expect("test setup")
}

#[test]
fn discovery_walks_up_to_the_nearest_marker() {
    let (_guard, root) = temp();
    fs::create_dir_all(root.join(".codix")).unwrap();
    touch(&root.join("inner/codix.toml"));
    fs::create_dir_all(root.join("inner/deep/er")).unwrap();
    fs::create_dir_all(root.join("other")).unwrap();

    let found = |dir: &str| {
        Project::discover(&root.join(dir))
            .unwrap()
            .unwrap()
            .root()
            .to_path_buf()
    };
    assert_eq!(found("inner/deep/er"), root.join("inner"));
    assert_eq!(found("inner"), root.join("inner"));
    assert_eq!(found("other"), root);
    assert_eq!(found(""), root);
}

#[test]
fn discovery_without_marker_finds_nothing() {
    let (_guard, root) = temp();
    fs::create_dir_all(root.join("a/b")).unwrap();
    assert!(Project::discover(&root.join("a/b")).unwrap().is_none());

    let ctx = Context::discover(&root.join("a/b"), None).unwrap();
    assert_eq!(ctx.project().unwrap_err().code(), "no_project");
}

#[test]
fn shared_config_without_index_is_not_indexed() {
    let (_guard, root) = temp();
    touch(&root.join("codix.toml"));
    let found = Project::discover(&root).unwrap().unwrap();
    assert_eq!(found.require_indexed().unwrap_err().code(), "not_indexed");

    touch(&root.join(".codix/index.db"));
    found.require_indexed().unwrap();
}

#[test]
fn root_override_needs_no_marker() {
    let (_guard, root) = temp();
    fs::create_dir_all(root.join("plain/sub")).unwrap();
    let ctx = Context::discover(&root.join("plain/sub"), Some(Path::new(".."))).unwrap();
    assert_eq!(ctx.project().unwrap().root(), root.join("plain"));

    let missing = Context::discover(&root, Some(Path::new("missing"))).unwrap_err();
    assert_eq!(missing.code(), "io");
}

#[test]
fn input_paths_resolve_from_cwd_then_root() {
    let (_guard, root) = temp();
    let p = project(&root);
    touch(&root.join("src/main.rs"));
    touch(&root.join("src/src/main.rs"));
    touch(&root.join("lib.rs"));
    let src = root.join("src");

    let id = |input: &str| p.file_id(&src, input).unwrap();
    assert_eq!(id("main.rs").as_str(), "src/main.rs");
    assert_eq!(
        id("src/main.rs").as_str(),
        "src/src/main.rs",
        "cwd-relative first"
    );
    assert_eq!(id("lib.rs").as_str(), "lib.rs", "root-relative second");
    assert_eq!(id("../lib.rs").as_str(), "lib.rs");
    assert_eq!(id(root.join("lib.rs").to_str().unwrap()).as_str(), "lib.rs");
    assert_eq!(id(".").as_str(), "src");

    assert_eq!(p.file_id(&src, "nope.rs").unwrap_err().code(), "io");
}

#[test]
fn paths_outside_the_root_are_refused() {
    let (_guard, base) = temp();
    let root = base.join("project");
    let p = project(&root);
    touch(&base.join("outside.rs"));
    let error = p.file_id(&root, "../outside.rs").unwrap_err();
    assert_eq!(error.code(), "usage");
    assert!(error.cause().contains("outside the project root"));
}

#[cfg(unix)]
#[test]
fn symlinks_resolve_inside_the_root() {
    let (_guard, root) = temp();
    let p = project(&root);
    touch(&root.join("src/real.rs"));
    std::os::unix::fs::symlink("src/real.rs", root.join("link.rs")).unwrap();
    assert_eq!(p.file_id(&root, "link.rs").unwrap().as_str(), "src/real.rs");
}

#[cfg(unix)]
#[test]
fn symlinks_leaving_the_root_are_refused() {
    let (_guard, base) = temp();
    let root = base.join("project");
    let p = project(&root);
    touch(&base.join("secret.rs"));
    std::os::unix::fs::symlink(base.join("secret.rs"), root.join("link.rs")).unwrap();
    assert_eq!(p.file_id(&root, "link.rs").unwrap_err().code(), "usage");
    assert_eq!(
        p.id_of(&codix_core::project::canonicalize(root.join("link.rs")).unwrap()),
        None
    );
}

#[cfg(unix)]
#[test]
fn symlink_loops_are_detected() {
    let (_guard, root) = temp();
    let p = project(&root);
    std::os::unix::fs::symlink("b", root.join("a")).unwrap();
    std::os::unix::fs::symlink("a", root.join("b")).unwrap();
    assert_eq!(p.file_id(&root, "a").unwrap_err().code(), "io");
}

/// Whether the temp directory's filesystem ignores case, checked directly.
fn filesystem_ignores_case(dir: &Path) -> bool {
    let probe = dir.join("case-probe");
    fs::write(&probe, "").expect("test setup");
    let ignores = dir.join("CASE-PROBE").exists();
    fs::remove_file(probe).expect("test setup");
    ignores
}

#[test]
fn case_sensitivity_is_detected() {
    let (_guard, root) = temp();
    let ignores_case = filesystem_ignores_case(&root);
    assert_eq!(project(&root).is_case_insensitive(), ignores_case);
}

#[test]
fn case_only_differences_name_the_on_disk_file() {
    let (_guard, root) = temp();
    if !filesystem_ignores_case(&root) {
        eprintln!("skipped: the temp filesystem is case-sensitive");
        return;
    }
    let p = project(&root);
    touch(&root.join("Src/Main.rs"));
    assert_eq!(
        p.file_id(&root, "src/MAIN.RS").unwrap().as_str(),
        "Src/Main.rs"
    );
    assert_eq!(p.file_id(&root, "SRC").unwrap().as_str(), "Src");
}

#[test]
fn display_is_relative_to_the_cwd() {
    let (_guard, root) = temp();
    let p = project(&root);
    let file = FileId::from_canonical("src/billing/invoice.rs");
    assert_eq!(p.display(&root, &file), "src/billing/invoice.rs");
    assert_eq!(p.display(&root.join("src"), &file), "billing/invoice.rs");
    assert_eq!(
        p.display(&root.join("docs/api"), &file),
        "../../src/billing/invoice.rs"
    );
}

#[test]
fn context_locations_carry_both_paths() {
    let (_guard, root) = temp();
    fs::create_dir_all(root.join(".codix")).unwrap();
    fs::create_dir_all(root.join("docs")).unwrap();
    let ctx = Context::discover(&root.join("docs"), None).unwrap();
    let location = ctx.location(&FileId::from_canonical("src/a.rs"), 3, 4);
    assert_eq!(location.path, "../src/a.rs");
    assert_eq!(location.root_path.as_str(), "src/a.rs");
}

#[test]
fn here_and_path_scopes() {
    let (_guard, root) = temp();
    let p = project(&root);
    fs::create_dir_all(root.join("src/billing")).unwrap();
    fs::create_dir_all(root.join("tests")).unwrap();
    let in_src = FileId::from_canonical("src/billing/a.rs");
    let in_tests = FileId::from_canonical("tests/a.rs");

    let here = Scope::resolve(
        &p,
        &root.join("src"),
        &ScopeRequest {
            here: true,
            paths: vec![],
            ..ScopeRequest::default()
        },
    )
    .unwrap();
    assert!(here.contains(&in_src));
    assert!(!here.contains(&in_tests));

    let both = Scope::resolve(
        &p,
        &root.join("src"),
        &ScopeRequest {
            here: false,
            paths: vec!["billing".to_owned(), "tests".to_owned()],
            ..ScopeRequest::default()
        },
    )
    .unwrap();
    assert!(both.contains(&in_src));
    assert!(both.contains(&in_tests));

    let but_tests = Scope::resolve(
        &p,
        &root,
        &ScopeRequest {
            excluded: vec!["tests".to_owned()],
            ..ScopeRequest::default()
        },
    )
    .unwrap();
    assert!(but_tests.contains(&in_src));
    assert!(!but_tests.contains(&in_tests));
    assert!(!both.contains(&FileId::from_canonical("src/other.rs")));
}

#[cfg(windows)]
#[test]
fn windows_paths_have_no_verbatim_prefix() {
    let dir = tempfile::tempdir().unwrap();
    fs::create_dir(dir.path().join(".codix")).unwrap();
    let ctx = Context::discover(dir.path(), None).unwrap();
    for shown in [ctx.cwd(), ctx.project().unwrap().root()] {
        assert!(
            !shown.display().to_string().starts_with(r"\\?\"),
            "{}",
            shown.display()
        );
    }
}
