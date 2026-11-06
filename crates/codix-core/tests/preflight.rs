//! Pre-flight checks of renames, on Java files.

use std::fs;
use std::path::PathBuf;

use codix_core::Error;
use codix_core::config::Config;
use codix_core::edit::Edit;
use codix_core::error::Site;
use codix_core::op::Context;
use codix_core::preflight::{Allow, Overrides, Rename, check_rename};
use codix_core::project::{FileId, Project};
use codix_core::refs::{Analyzed, analyze};
use codix_core::scope::Binding;

use codix_lang_go as _;
use codix_lang_java as _;
use codix_vcs_git as _;

struct Fixture {
    _dir: tempfile::TempDir,
    root: PathBuf,
    project: Project,
}

fn fixture(files: &[(&str, &str)]) -> Fixture {
    let dir = tempfile::tempdir().expect("test setup");
    let root = codix_core::project::canonicalize(dir.path()).expect("test setup");
    fs::create_dir_all(root.join(".codix")).expect("test setup");
    for (path, content) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("test setup")).expect("test setup");
        fs::write(path, content).expect("test setup");
    }
    let project = Project::discover(&root)
        .expect("test setup")
        .expect("test setup");
    Fixture {
        _dir: dir,
        root,
        project,
    }
}

/// The definition whose name is the `nth` occurrence of `name`, and the
/// edits renaming it and the references bound to it.
fn target(file: &Analyzed, name: &str, nth: usize, new_name: &str) -> (usize, Vec<Edit>) {
    let byte = file
        .content
        .match_indices(name)
        .nth(nth)
        .expect("test setup")
        .0;
    let d = file.model.definition_at(byte).expect("a definition there");
    let mut edits: Vec<Edit> = std::iter::once(&file.model.definitions[d].name_range)
        .chain(
            file.model
                .references
                .iter()
                .filter(|r| r.binding == Binding::Bound(d))
                .map(|r| &r.range),
        )
        .map(|range| Edit {
            start: range.start.byte as usize,
            end: range.end.byte as usize,
            replacement: new_name.to_owned(),
        })
        .collect();
    edits.sort_by_key(|e| e.start);
    (d, edits)
}

fn check(
    f: &Fixture,
    path: &str,
    name: &str,
    nth: usize,
    new_name: &str,
    overrides: &Overrides,
) -> Result<Vec<String>, Error> {
    let config = Config::load(&f.project).expect("test setup");
    let file = analyze(&f.project, &config, &FileId::from_canonical(path))
        .expect("test setup")
        .expect("test setup");
    let (d, edits) = target(&file, name, nth, new_name);
    let ctx = Context::discover(&f.root, None).expect("test setup");
    check_rename(
        &ctx,
        &f.project,
        &config,
        &Rename {
            file: &file,
            targets: &[d],
            edits: &edits,
            new_name,
            unresolved: &[],
            overrides,
        },
    )
    .map(|used| used.into_iter().map(|u| u.flag).collect())
}

fn code(result: Result<Vec<String>, Error>) -> String {
    result.map_or_else(|e| e.code().to_owned(), |_| "ok".to_owned())
}

const INVOICE: &str = "class Invoice {\n    long total;\n\n    long sum(long tax) {\n        long base = total;\n        long rate = 2;\n        return base + tax * rate;\n    }\n}\n";

#[test]
fn a_clean_rename_passes() {
    let f = fixture(&[("Invoice.java", INVOICE)]);
    assert_eq!(
        code(check(
            &f,
            "Invoice.java",
            "base",
            0,
            "net",
            &Overrides::default()
        )),
        "ok"
    );
}

#[test]
fn a_name_that_is_not_an_identifier_or_breaks_the_parse_is_refused() {
    let f = fixture(&[("Invoice.java", INVOICE)]);
    let none = Overrides::default();
    assert_eq!(
        code(check(&f, "Invoice.java", "base", 0, "1st", &none)),
        "invalid_name"
    );
    assert_eq!(
        code(check(&f, "Invoice.java", "base", 0, "class", &none)),
        "invalid_name"
    );
}

#[test]
fn a_name_already_in_the_scope_collides() {
    let f = fixture(&[("Invoice.java", INVOICE)]);
    assert_eq!(
        code(check(
            &f,
            "Invoice.java",
            "base",
            0,
            "rate",
            &Overrides::default()
        )),
        "name_collision"
    );
}

#[test]
fn shadowing_is_refused_in_both_directions_unless_allowed() {
    let f = fixture(&[("Invoice.java", INVOICE)]);
    let none = Overrides::default();
    // `base` renamed to `total` would capture the read of the field.
    let error = check(&f, "Invoice.java", "base", 0, "total", &none).unwrap_err();
    assert_eq!(error.code(), "shadowing");
    assert_eq!(error.sites()[0].location.line, 5, "{:?}", error.sites());
    // The field renamed to `base` would be hidden by the local.
    assert_eq!(
        code(check(&f, "Invoice.java", "total", 0, "base", &none)),
        "shadowing"
    );
    let allowed = Overrides {
        allow: vec![Allow::Shadowing],
        ..Overrides::default()
    };
    assert_eq!(
        check(&f, "Invoice.java", "total", 0, "base", &allowed).unwrap(),
        ["--allow shadowing"]
    );
}

#[test]
fn sites_in_a_parse_error_are_refused_whatever_the_overrides() {
    let f = fixture(&[(
        "Broken.java",
        "class Broken {\n    void f() {\n        int x = ;\n    }\n}\n",
    )]);
    let all = Overrides {
        allow: vec![Allow::Shadowing, Allow::Overload, Allow::Generated],
        exclude_unresolved: true,
        textual: true,
    };
    assert_eq!(
        code(check(&f, "Broken.java", "x", 0, "y", &all)),
        "parse_error_region"
    );
}

#[test]
fn unresolved_sites_refuse_unless_excluded() {
    let f = fixture(&[("Invoice.java", INVOICE)]);
    let config = Config::load(&f.project).unwrap();
    let file = analyze(&f.project, &config, &FileId::from_canonical("Invoice.java"))
        .unwrap()
        .unwrap();
    let (d, edits) = target(&file, "total", 0, "amount");
    let ctx = Context::discover(&f.root, None).unwrap();
    let unresolved = [Site {
        location: ctx.location(&FileId::from_canonical("Report.java"), 3, 20),
        note: "return invoice.total;".to_owned(),
    }];
    let run = |overrides: &Overrides| {
        check_rename(
            &ctx,
            &f.project,
            &config,
            &Rename {
                file: &file,
                targets: &[d],
                edits: &edits,
                new_name: "amount",
                unresolved: &unresolved,
                overrides,
            },
        )
    };
    let error = run(&Overrides::default()).unwrap_err();
    assert_eq!((error.code(), error.sites().len()), ("unresolved_sites", 1));
    let excluded = run(&Overrides {
        exclude_unresolved: true,
        ..Overrides::default()
    })
    .unwrap();
    assert_eq!(excluded[0].flag, "--exclude-unresolved");
    assert_eq!(excluded[0].sites[0].path, "Report.java");
}

#[test]
fn generated_files_are_refused_unless_allowed() {
    let generated = "// @generated by a tool\nclass Gen {\n    int x;\n}\n";
    let f = fixture(&[
        ("Gen.java", generated),
        ("src/Attr.java", "class Attr {\n    int x;\n}\n"),
        ("out/Conf.java", "class Conf {\n    int x;\n}\n"),
        (".gitattributes", "src/Attr.java linguist-generated\n"),
        ("codix.toml", "[edits]\ngenerated = [\"out\"]\n"),
        (".git/HEAD", "ref: refs/heads/main\n"),
    ]);
    let none = Overrides::default();
    for path in ["Gen.java", "src/Attr.java", "out/Conf.java"] {
        assert_eq!(
            code(check(&f, path, "x", 0, "y", &none)),
            "generated_code",
            "{path}"
        );
    }
    let allowed = Overrides {
        allow: vec![Allow::Generated],
        ..Overrides::default()
    };
    assert_eq!(
        check(&f, "Gen.java", "x", 0, "y", &allowed).unwrap(),
        ["--allow generated"]
    );
}

#[test]
fn type_names_and_public_types_named_after_their_file() {
    let f = fixture(&[
        (
            "Invoice.java",
            "public class Invoice {\n    static class Line {}\n}\n",
        ),
        ("Other.java", "class Other {}\n"),
    ]);
    let none = Overrides::default();
    assert_eq!(
        code(check(&f, "Invoice.java", "Invoice", 0, "Bill", &none)),
        "renames_file"
    );
    assert_eq!(
        code(check(&f, "Invoice.java", "Line", 0, "var", &none)),
        "invalid_name"
    );
    assert_eq!(
        code(check(&f, "Invoice.java", "Line", 0, "Row", &none)),
        "ok"
    );
    assert_eq!(
        code(check(&f, "Other.java", "Other", 0, "Another", &none)),
        "ok"
    );
}

#[test]
fn a_method_renamed_to_another_methods_name_is_an_overload_only_if_allowed() {
    let f = fixture(&[(
        "Invoice.java",
        "class Invoice {\n    long total() { return 0; }\n    long sum(long x) { return x; }\n}\n",
    )]);
    let none = Overrides::default();
    let error = check(&f, "Invoice.java", "total", 0, "sum", &none).unwrap_err();
    assert_eq!(error.code(), "name_collision");
    assert!(
        error.hint().contains("--allow overload"),
        "{}",
        error.hint()
    );
    let allowed = Overrides {
        allow: vec![Allow::Overload],
        ..Overrides::default()
    };
    assert_eq!(
        check(&f, "Invoice.java", "total", 0, "sum", &allowed).unwrap(),
        ["--allow overload"]
    );
}

#[test]
fn go_names_keep_their_exported_case_and_generated_files_are_marked() {
    let f = fixture(&[
        ("geo.go", "package geo\n\nfunc area() int { return 1 }\n"),
        (
            "gen.go",
            "// Code generated by stringer; DO NOT EDIT.\n\npackage geo\n\nfunc kind() int { return 1 }\n",
        ),
        (
            "vendor/lib/lib.go",
            "package lib\n\nfunc helper() int { return 1 }\n",
        ),
    ]);
    let none = Overrides::default();
    assert_eq!(
        code(check(&f, "geo.go", "area", 0, "Area", &none)),
        "visibility_change"
    );
    assert_eq!(code(check(&f, "geo.go", "area", 0, "surface", &none)), "ok");
    assert_eq!(
        code(check(&f, "geo.go", "area", 0, "func", &none)),
        "invalid_name"
    );
    assert_eq!(
        code(check(&f, "gen.go", "kind", 0, "sort", &none)),
        "generated_code"
    );
    assert_eq!(
        code(check(&f, "vendor/lib/lib.go", "helper", 0, "aid", &none)),
        "generated_code"
    );
}
