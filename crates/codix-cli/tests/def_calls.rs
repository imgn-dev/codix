//! `def` on a call: the methods of the type it is made on and of its
//! supertypes that take as many arguments come first.
#![cfg(with_lang_java)]

use codix_testkit::fixture::{codix_in, indexed};
use serde_json::Value;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const FILES: &[(&str, &str)] = &[
    (
        "src/f/AbstractBeanFactory.java",
        "package f;
public abstract class AbstractBeanFactory {
    public Object getBean(String name, Class<?> type, Object... args) { return null; }
    public Object getBean(String name) { return null; }
}
",
    ),
    (
        "src/f/Listable.java",
        "package f;
public class Listable extends AbstractBeanFactory {
    public <T> T getBean(Class<T> type) { return null; }

    Object resolve(String beanName, Object[] args, Other other) {
        other.getBean(beanName);
        return getBean(beanName, null, args);
    }
}
class Other {
    Object getBean(String a) { return null; }
}
",
    ),
];

fn def(root: &std::path::Path, place: &str) -> (Vec<String>, Option<u64>) {
    let out = codix_in(CODIX, root, root, &["def", place, "--json"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    let json: Value = serde_json::from_str(&out.stdout).expect("json");
    let names = json["result"]["definitions"]
        .as_array()
        .expect("definitions")
        .iter()
        .map(|d| d["signature"].as_str().expect("signature").to_owned())
        .collect();
    (names, json["result"]["fitting"].as_u64())
}

#[test]
fn an_inherited_overload_taking_as_many_arguments_comes_first() {
    let (_guard, root) = indexed(CODIX, FILES);
    let (signatures, fitting) = def(&root, "src/f/Listable.java:7:16");
    assert_eq!(fitting, Some(1), "{signatures:?}");
    assert_eq!(
        signatures[0],
        "public Object getBean(String name, Class<?> type, Object... args)"
    );
}

#[test]
fn a_call_on_a_declared_receiver_prefers_its_type() {
    let (_guard, root) = indexed(CODIX, FILES);
    let (signatures, fitting) = def(&root, "src/f/Listable.java:6:15");
    assert_eq!(fitting, Some(1), "{signatures:?}");
    assert_eq!(signatures[0], "Object getBean(String a)");
    let out = codix_in(CODIX, &root, &root, &["def", "src/f/Listable.java:6:15"]);
    assert!(
        out.stdout
            .contains("3 other symbols of the name, which do not fit the call:\n"),
        "{}",
        out.stdout
    );
}
