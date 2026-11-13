//! `types`: subtypes and supertypes, transitively, and how each relation
//! was found.
#![cfg(with_lang_java)]

use codix_testkit::fixture::{codix_in, indexed};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const FILES: &[(&str, &str)] = &[
    (
        "src/a/BeanPostProcessor.java",
        "package a;\npublic interface BeanPostProcessor {}\n",
    ),
    (
        "src/a/Smart.java",
        "package a;\npublic interface Smart extends BeanPostProcessor, java.io.Serializable {}\n",
    ),
    (
        "src/a/Async.java",
        "package a;
public class Async extends AbstractAdvising implements Smart {}
class AbstractAdvising implements BeanPostProcessor, Comparable<AbstractAdvising> {}
class Nested {
    interface BeanPostProcessor {}
    class Local implements BeanPostProcessor {}
}
",
    ),
];

fn codix(root: &std::path::Path, args: &[&str]) -> codix_testkit::Run {
    codix_in(CODIX, root, root, args)
}

fn rows(json: &Value) -> Vec<(String, usize, String, String)> {
    json["result"]["types"]["items"]
        .as_array()
        .expect("items")
        .iter()
        .map(|t| {
            (
                t["qualified_name"].as_str().expect("name").to_owned(),
                usize::try_from(t["depth"].as_u64().expect("depth")).expect("depth"),
                t["via"].as_str().expect("via").to_owned(),
                t["confidence"].as_str().expect("confidence").to_owned(),
            )
        })
        .collect()
}

fn row(name: &str, depth: usize, via: &str, confidence: &str) -> (String, usize, String, String) {
    (
        name.to_owned(),
        depth,
        via.to_owned(),
        confidence.to_owned(),
    )
}

#[test]
fn subtypes_are_found_transitively_by_name_or_by_scope() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["types", "BeanPostProcessor", "--json"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    let json: Value = serde_json::from_str(&out.stdout).unwrap();
    assert_eq!(
        rows(&json),
        [
            row("Async", 2, "AbstractAdvising", "scoped"),
            row("AbstractAdvising", 1, "BeanPostProcessor", "textual"),
            // The nested interface of the name binds by scope in its file.
            row("Nested.Local", 1, "Nested.BeanPostProcessor", "scoped"),
            row("Smart", 1, "BeanPostProcessor", "textual"),
        ]
    );
    assert_eq!(json["result"]["targets"].as_array().unwrap().len(), 2);
}

#[test]
fn depth_limits_the_levels() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["types", "Smart", "--depth", "1"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(
        out.stdout
            .starts_with("src/a/Async.java:2:14  class  Async  depth 1  via Smart  textual\n"),
        "{}",
        out.stdout
    );
    assert!(
        out.stdout.ends_with("1 result\n0 bound, 1 textual\n"),
        "{}",
        out.stdout
    );
}

#[test]
fn supertypes_go_up_and_name_those_outside_the_project() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["types", "Async", "--super", "--json"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    let json: Value = serde_json::from_str(&out.stdout).unwrap();
    let names: Vec<String> = rows(&json).into_iter().map(|r| r.0).collect();
    assert_eq!(
        names,
        [
            "AbstractAdvising",
            "Nested.BeanPostProcessor",
            "BeanPostProcessor",
            "Smart"
        ]
    );
    assert_eq!(
        json["result"]["outside"],
        serde_json::json!(["Comparable", "java.io.Serializable"])
    );
}

#[test]
fn a_type_without_subtypes_says_so() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["types", "Async"]);
    assert_eq!(out.code, Some(1), "{}", out.stderr);
    assert_eq!(out.stdout, "no subtypes in the project\n");
}
