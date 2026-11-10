//! Providers a project needs and this build lacks are reported once each,
//! with what is lost and how to get them, by `init`, `index` and `status`.

use codix_testkit::fixture::{codix_in, files_only};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

/// The end of a missing provider's line: why it is missing, and the hint.
#[cfg(any(
    not(feature = "lang-kotlin"),
    not(feature = "build-maven"),
    not(feature = "vcs-hg")
))]
fn absent(kind: &str, name: &str) -> String {
    let (why, hint) = codix_testkit::missing_provider(cfg!(feature = "plugins"), kind, name);
    format!("{why} ({hint})")
}

/// `init`, `index` and `status` output, in that order.
fn reports(files: &[(&str, &str)], dirs: &[&str]) -> [String; 3] {
    let (_guard, root) = files_only(files);
    for dir in dirs {
        std::fs::create_dir(root.join(dir)).expect("create dir");
    }
    let out = |args: &[&str]| {
        let out = codix_in(CODIX, &root, &root, args);
        assert_eq!(out.code, Some(0), "{args:?}: {}", out.stderr);
        out.stdout
    };
    [out(&["init"]), out(&["index"]), out(&["status"])]
}

#[cfg(not(feature = "lang-kotlin"))]
#[test]
fn a_language_without_its_pack_is_indexed_as_text_and_reported() {
    let files = [
        ("src/Main.kt", "fun main() {}\n"),
        ("src/Util.kt", "fun util() {}\n"),
    ];
    let line = format!(
        "kotlin: 2 files indexed as text; {}\n",
        absent("lang", "kotlin")
    );
    for report in reports(&files, &[]) {
        assert_eq!(report.matches(&line).count(), 1, "{report}");
    }

    let (_guard, root) = files_only(&files);
    codix_in(CODIX, &root, &root, &["init"]);
    let out = codix_in(CODIX, &root, &root, &["index", "--json"]);
    let json: serde_json::Value = serde_json::from_str(&out.stdout).expect("JSON");
    assert_eq!(
        json["result"]["missing"],
        serde_json::json!([{
            "kind": "language",
            "name": "kotlin",
            "files": 2,
            "marker": null,
            "hint": codix_testkit::missing_provider(cfg!(feature = "plugins"), "lang", "kotlin").1,
            "invalid": null,
        }])
    );
}

#[cfg(not(feature = "build-maven"))]
#[test]
fn a_build_system_without_its_provider_is_reported() {
    let line = format!(
        "maven: pom.xml found, modules not read; {}\n",
        absent("build", "maven")
    );
    for report in reports(&[("pom.xml", "<project/>\n")], &[]) {
        assert!(report.contains(&line), "{report}");
    }
}

#[cfg(not(feature = "vcs-hg"))]
#[test]
fn a_version_control_system_without_its_provider_is_reported() {
    let line = format!(
        "mercurial: .hg found, ignore rules not applied; {}\n",
        absent("vcs", "mercurial")
    );
    for report in reports(&[("notes.txt", "hello\n")], &[".hg"]) {
        assert!(report.contains(&line), "{report}");
    }
}

#[test]
fn nothing_is_reported_when_every_provider_is_there() {
    for report in reports(&[("notes.txt", "hello\n")], &[]) {
        assert!(!report.contains(" found, "), "{report}");
        assert!(!report.contains(" indexed as text; "), "{report}");
    }
}
