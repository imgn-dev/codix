//! The Java resolver: references across files bound to their symbol, and
//! renames that edit them.
#![cfg(with_lang_java)]

use codix_testkit::fixture::{codix_in, indexed};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const FILES: &[(&str, &str)] = &[
    (
        "src/f/Helper.java",
        "package f;\npublic class Helper {\n    public static int twice(int x) { return 2 * x; }\n}\n",
    ),
    (
        "src/f/Other.java",
        "package f;\npublic class Other {\n    public static int twice(int x) { return x + x; }\n}\n",
    ),
    (
        "src/g/Caller.java",
        "package g;\nimport f.Helper;\nimport java.util.List;\nclass Caller {\n    int go(List<String> names) {\n        names.add(\"x\");\n        return Helper.twice(2) + f.Other.twice(3);\n    }\n}\n",
    ),
    (
        "src/g/Shadow.java",
        "package g;\nimport static f.Helper.twice;\nclass Shadow {\n    int done(int y) { return y; }\n    int go() { return twice(1); }\n}\n",
    ),
];

fn codix(root: &std::path::Path, args: &[&str]) -> codix_testkit::Run {
    codix_in(CODIX, root, root, args)
}

#[test]
fn refs_lists_uses_bound_across_files_and_leaves_out_others() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["refs", "Helper.twice"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "src/f/Helper.java:3:23  def     -          scoped\n\
         src/g/Caller.java:7:23  call    Caller.go  bound\n\
         src/g/Shadow.java:2:24  import  -          bound\n\
         src/g/Shadow.java:5:23  call    Shadow.go  bound\n\
         4 results\n\
         1 definition, 0 scoped, 3 bound, 0 textual\n"
    );
}

#[test]
fn rename_edits_the_bound_references_in_other_files() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["rename", "Helper.twice", "doubled"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(
        out.stdout.contains("  3 files, 4 sites\n"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout
            .contains("+        return Helper.doubled(2) + f.Other.twice(3);\n"),
        "{}",
        out.stdout
    );
}

#[test]
fn rename_refuses_a_name_another_file_defines_at_a_bound_use() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["rename", "Helper.twice", "done"]);
    assert_eq!(out.code, Some(4), "{}", out.stdout);
    assert!(out.stderr.contains("error[shadowing]"), "{}", out.stderr);
    assert!(
        out.stderr.contains("src/g/Shadow.java:5:23"),
        "{}",
        out.stderr
    );
}
