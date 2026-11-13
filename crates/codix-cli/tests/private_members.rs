//! A private member's uses in its file that its scopes cannot bind: through
//! another instance and from an anonymous class, which the Java resolver
//! binds, and through a chained call, which nothing binds.
#![cfg(with_lang_java)]

use codix_testkit::fixture::{codix_in, indexed};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const FILES: &[(&str, &str)] = &[(
    "src/Factory.java",
    "package demo;

public class Factory {
    private Object resolveBean(String name) {
        return name;
    }

    Object direct(String n) {
        return resolveBean(n);
    }

    Object other(Factory f, String n) {
        return f.resolveBean(n);
    }

    Runnable anonymous(String n) {
        return new Runnable() {
            public void run() {
                resolveBean(n);
            }
        };
    }

    Factory self() {
        return this;
    }

    Object chained(String n) {
        return self().resolveBean(n);
    }
}
",
)];

fn codix(root: &std::path::Path, args: &[&str]) -> codix_testkit::Run {
    codix_in(CODIX, root, root, args)
}

#[test]
fn refs_lists_them_bound_or_textual() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["refs", "resolveBean"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    for line in [
        "src/Factory.java:9:16   call  Factory.direct         scoped\n",
        "src/Factory.java:13:18  call  Factory.other          bound\n",
        "src/Factory.java:19:17  call  Factory.anonymous.run  bound\n",
        "src/Factory.java:29:23  call  Factory.chained        textual\n",
    ] {
        assert!(out.stdout.contains(line), "{line}in\n{}", out.stdout);
    }
    assert!(
        out.stdout
            .ends_with("1 definition, 1 scoped, 2 bound, 1 textual\n"),
        "{}",
        out.stdout
    );
}

#[test]
fn rename_refuses_unless_they_are_excluded() {
    let (_guard, root) = indexed(CODIX, FILES);
    let out = codix(&root, &["rename", "resolveBean", "lookup"]);
    assert_eq!(out.code, Some(4), "{}", out.stdout);
    assert!(
        out.stderr
            .contains("  src/Factory.java:29:23  return self().resolveBean(n);\n"),
        "{}",
        out.stderr
    );
    let out = codix(
        &root,
        &["rename", "resolveBean", "lookup", "--exclude-unresolved"],
    );
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(out.stdout.contains("1 unresolved site:"), "{}", out.stdout);
    assert!(out.stdout.contains("  1 file, 4 sites\n"), "{}", out.stdout);
}

#[test]
fn rename_lists_mentions_in_other_files_without_changing_them() {
    let mut files = FILES.to_vec();
    files.extend([
        (
            "src/User.java",
            "package demo;\n/** Calls {@link Factory#resolveBean}. */\nclass User {\n    String key = \"resolveBean\";\n    int resolveBean = 1;\n}\n",
        ),
        ("conf/app.properties", "bean.method=resolveBean\n"),
    ]);
    let (_guard, root) = indexed(CODIX, &files);
    let out = codix(
        &root,
        &[
            "rename",
            "Factory.resolveBean",
            "lookup",
            "--exclude-unresolved",
        ],
    );
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert!(
        out.stdout.contains(
            "  not changed, 3 mentions elsewhere:\n    conf/app.properties:1:13  bean.method=resolveBean\n    src/User.java:2:26        /** Calls {@link Factory#resolveBean}. */\n    src/User.java:4:19        String key = \"resolveBean\";\n"
        ),
        "{}",
        out.stdout
    );
    assert!(out.stdout.contains("  1 file, 4 sites\n"), "{}", out.stdout);
}
