//! A C function, its prototypes and its definitions in `#if` branches.
#![cfg(with_lang_c)]

use std::path::Path;

use codix_testkit::fixture::{codix_in, indexed};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

fn codix(root: &Path, args: &[&str]) -> codix_testkit::Run {
    codix_in(CODIX, root, root, args)
}

fn json(root: &Path, args: &[&str]) -> Value {
    let mut args = args.to_vec();
    args.push("--json");
    serde_json::from_str(&codix(root, &args).stdout).expect("JSON")
}

const HEADER: &str = "int add(int a, int b);\n";
const DEFINITION: &str = "#include \"add.h\"\n\nint add(int a, int b)\n{\n\treturn a + b;\n}\n";
const CALLER: &str = "#include \"add.h\"\n\nint twice(int a)\n{\n\treturn add(a, a);\n}\n";

fn project() -> (tempfile::TempDir, std::path::PathBuf) {
    indexed(
        CODIX,
        &[
            ("add.h", HEADER),
            ("add.c", DEFINITION),
            ("twice.c", CALLER),
        ],
    )
}

#[test]
fn a_prototype_is_a_declaration_listed_after_the_definition() {
    let (_guard, root) = project();
    let def = codix(&root, &["def", "add"]);
    assert_eq!(def.code, Some(0), "{}", def.stderr);
    let lines: Vec<&str> = def.stdout.lines().collect();
    assert!(
        lines[0].starts_with("add.c:3:5  function  add"),
        "{}",
        def.stdout
    );
    assert!(
        lines[2].starts_with("add.h:1:5  declaration  add"),
        "{}",
        def.stdout
    );
    assert_eq!(lines[4], "2 matches");
}

#[test]
fn refs_of_a_function_count_its_declaration() {
    let (_guard, root) = project();
    let found = json(&root, &["refs", "c:add.c::add"]);
    let places: Vec<String> = found["result"]["references"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|r| {
            format!(
                "{}:{} {}",
                r["path"].as_str().unwrap_or_default(),
                r["line"],
                r["context"].as_str().unwrap_or_default()
            )
        })
        .collect();
    assert_eq!(places, ["add.c:3 def", "add.h:1 def", "twice.c:5 call"]);
}

#[test]
fn a_rename_edits_the_prototype_and_the_calls_bound_through_it() {
    let (_guard, root) = project();
    // The C resolver binds the call in twice.c through the header.
    let planned = codix(&root, &["rename", "c:add.c::add", "sum"]);
    assert_eq!(planned.code, Some(0), "{}", planned.stderr);
    assert!(
        planned.stdout.contains("3 files, 3 sites"),
        "{}",
        planned.stdout
    );
    let preview = planned
        .stdout
        .split_whitespace()
        .nth(1)
        .expect("a preview ID")
        .trim_end_matches(':');
    let applied = codix(&root, &["apply", preview]);
    assert_eq!(applied.code, Some(0), "{}", applied.stderr);
    let read = |file: &str| std::fs::read_to_string(root.join(file)).expect("read");
    assert_eq!(read("add.h"), "int sum(int a, int b);\n");
    assert!(
        read("add.c").contains("int sum(int a, int b)\n"),
        "{}",
        read("add.c")
    );
    assert_eq!(read("twice.c"), CALLER.replace("add(a, a)", "sum(a, a)"));
}

#[test]
fn a_static_function_and_its_forward_declaration_are_renamed_together() {
    let source = "static int step(int x);\n\nint run(void)\n{\n\treturn step(1);\n}\n\nstatic int step(int x)\n{\n\treturn x + 1;\n}\n";
    let (_guard, root) = indexed(CODIX, &[("run.c", source)]);
    let def = json(&root, &["def", "step", "--kind", "function"]);
    let id = def["result"]["definitions"][0]["symbol_id"]
        .as_str()
        .expect("an ID")
        .to_owned();
    let planned = codix(&root, &["rename", &id, "next"]);
    assert_eq!(
        planned.code,
        Some(0),
        "{}{}",
        planned.stdout,
        planned.stderr
    );
    assert!(
        planned.stdout.contains("1 file, 3 sites"),
        "{}",
        planned.stdout
    );
}

#[test]
fn definitions_in_both_branches_of_an_if_are_both_kept() {
    let source = "#ifdef FAST\nstatic int pick(int x)\n{\n\treturn x;\n}\n#else\nstatic int pick(int x)\n{\n\treturn x + 1;\n}\n#endif\n";
    let (_guard, root) = indexed(CODIX, &[("pick.c", source)]);
    let found = json(&root, &["find", "pick", "--exact"]);
    let ids: Vec<&str> = found["result"]["results"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|r| r["symbol_id"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(ids.len(), 2, "{found}");
    assert_eq!(format!("{}~2", ids[0]), ids[1]);
    let second = json(&root, &["def", "pick.c:7:12"]);
    assert_eq!(second["result"]["definitions"][0]["line"], 7, "{second}");
    let by_id = json(&root, &["def", ids[0]]);
    assert_eq!(
        by_id["result"]["definitions"].as_array().map(Vec::len),
        Some(1),
        "{by_id}"
    );
}
