//! Gradle discovery on Groovy and Kotlin DSL fixtures, compiled in and as a
//! plugin.

use std::fs;
use std::path::{Path, PathBuf};

use codix_build_gradle::Gradle;
use codix_provider::{BuildModel, BuildResolver};

fn discover(files: &[(&str, &str)]) -> BuildModel {
    let dir = tempfile::tempdir().expect("temp dir");
    let root: PathBuf = fs::canonicalize(dir.path()).expect("canonical temp dir");
    for (path, content) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        fs::write(path, content).expect("write file");
    }
    let model = Gradle.discover(&root);
    // The plugin form gives the same.
    let plugin = Path::new(env!("CARGO_BIN_EXE_codix-build-gradle"));
    assert_eq!(
        codix_plugin::build::run_within(plugin, &root, codix_plugin::build::TIMEOUT),
        Ok(model.clone()),
        "through the plugin"
    );
    model
}

/// Name, root, source roots, test roots and parent of a module.
type Summary = (String, String, Vec<String>, Vec<String>, Option<String>);

fn summary(model: &BuildModel) -> Vec<Summary> {
    model
        .modules
        .iter()
        .map(|m| {
            (
                m.name.clone(),
                m.root.clone(),
                m.source_roots.clone(),
                m.test_roots.clone(),
                m.parent.clone(),
            )
        })
        .collect()
}

fn strings(values: &[&str]) -> Vec<String> {
    values.iter().map(|v| (*v).to_owned()).collect()
}

fn expected() -> Vec<Summary> {
    vec![
        (
            "shop".to_owned(),
            String::new(),
            strings(&["src/main/java", "src/generated/java"]),
            strings(&["src/test/java"]),
            None,
        ),
        (
            ":core".to_owned(),
            "core".to_owned(),
            strings(&["core/src/java"]),
            strings(&["core/src/test/java", "core/src/it/java"]),
            Some("shop".to_owned()),
        ),
        (
            ":web:api".to_owned(),
            "web/api".to_owned(),
            strings(&["web/api/src/main/java"]),
            strings(&["web/api/src/test/java"]),
            Some("shop".to_owned()),
        ),
    ]
}

#[test]
fn groovy_dsl() {
    let model = discover(&[
        (
            "settings.gradle",
            "rootProject.name = 'shop'\ninclude 'core', ':web:api'\n",
        ),
        (
            "build.gradle",
            "plugins { id 'java' }\nsourceSets {\n  main {\n    java { srcDir 'src/generated/java' }\n  }\n}\n",
        ),
        (
            "core/build.gradle",
            "sourceSets {\n  main { java { srcDirs = ['src/java'] } }\n  test {\n    java {\n      srcDirs 'src/it/java' // integration\n    }\n    resources { srcDir 'src/test/res' }\n  }\n}\n",
        ),
    ]);
    assert_eq!(summary(&model), expected());
    assert_eq!(model.unsupported, vec![]);
}

#[test]
fn kotlin_dsl() {
    let model = discover(&[
        (
            "settings.gradle.kts",
            "rootProject.name = \"shop\"\ninclude(\"core\", \":web:api\")\n",
        ),
        (
            "build.gradle.kts",
            "plugins { java }\nsourceSets {\n    main {\n        java.srcDir(\"src/generated/java\")\n    }\n}\n",
        ),
        (
            "core/build.gradle.kts",
            "sourceSets {\n    main { java { setSrcDirs(listOf(\"src/java\")) } }\n    named(\"test\") {\n        java { srcDirs(\"src/it/java\") }\n    }\n}\n",
        ),
        (
            "web/api/build.gradle.kts",
            "dependencies { implementation(project(\":core\")) }\n",
        ),
    ]);
    assert_eq!(summary(&model), expected());
    assert_eq!(model.unsupported, vec![]);
}

#[test]
fn a_project_without_build_files_uses_the_conventional_layout() {
    let model = discover(&[("settings.gradle.kts", "include(\"lib\")\n")]);
    assert_eq!(model.modules[1].source_roots, ["lib/src/main/java"]);
}

#[test]
fn constructs_it_cannot_read_are_reported() {
    let model = discover(&[
        (
            "settings.gradle.kts",
            "rootProject.name = \"shop\"\nval extra = listOf(\"x\")\ninclude(extra)\nproject(\":core\").projectDir = file(\"modules/core\")\n",
        ),
        (
            "build.gradle.kts",
            "sourceSets {\n    main {\n        java.srcDir(\"$buildDir/gen\")\n    }\n}\nsourceSets[\"test\"].java.srcDir(\"x\")\n",
        ),
    ]);
    let reported: Vec<(String, u32)> = model
        .unsupported
        .iter()
        .map(|u| (u.file.clone(), u.line))
        .collect();
    assert_eq!(
        reported,
        [
            ("settings.gradle.kts".to_owned(), 3),
            ("settings.gradle.kts".to_owned(), 4),
            ("build.gradle.kts".to_owned(), 3),
            ("build.gradle.kts".to_owned(), 6),
        ]
    );
    assert_eq!(
        model.modules[0].source_roots,
        ["src/main/java"],
        "nothing guessed"
    );
}
