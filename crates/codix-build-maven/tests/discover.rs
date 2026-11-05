//! Maven discovery on fixture builds.

use std::fs;
use std::path::{Path, PathBuf};

use codix_build_maven::Maven;
use codix_provider::{BuildModel, BuildResolver, Module};

fn project(files: &[(&str, &str)]) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = fs::canonicalize(dir.path()).expect("canonical temp dir");
    for (path, content) in files {
        let path = root.join(path);
        fs::create_dir_all(path.parent().expect("parent")).expect("create dirs");
        fs::write(path, content).expect("write file");
    }
    (dir, root)
}

fn module<'a>(model: &'a BuildModel, name: &str) -> &'a Module {
    model
        .modules
        .iter()
        .find(|m| m.name == name)
        .expect("module")
}

fn discover(root: &Path) -> BuildModel {
    Maven.discover(root)
}

const PARENT: &str = r"<project>
  <groupId>com.acme</groupId>
  <artifactId>acme-parent</artifactId>
  <version>1.0</version>
  <packaging>pom</packaging>
  <properties>
    <generated.dir>target/generated-sources/acme</generated.dir>
  </properties>
  <modules>
    <module>core</module>
    <module>app</module>
  </modules>
</project>";

#[test]
fn multi_module_builds_with_a_parent_and_inherited_properties() {
    let (_guard, root) = project(&[
        ("pom.xml", PARENT),
        (
            "core/pom.xml",
            r"<project>
              <parent><groupId>com.acme</groupId><artifactId>acme-parent</artifactId><version>1.0</version></parent>
              <artifactId>acme-core</artifactId>
              <build><sourceDirectory>${project.basedir}/src/java</sourceDirectory></build>
            </project>",
        ),
        (
            "app/pom.xml",
            r"<project>
              <parent><artifactId>acme-parent</artifactId></parent>
              <artifactId>acme-app</artifactId>
              <build><plugins><plugin>
                <groupId>org.codehaus.mojo</groupId>
                <artifactId>build-helper-maven-plugin</artifactId>
                <executions>
                  <execution><goals><goal>add-source</goal></goals>
                    <configuration><sources><source>${generated.dir}</source></sources></configuration></execution>
                  <execution><goals><goal>add-test-source</goal></goals>
                    <configuration><sources><source>src/it/java</source></sources></configuration></execution>
                </executions>
              </plugin></plugins></build>
            </project>",
        ),
    ]);
    let model = discover(&root);
    let names: Vec<&str> = model.modules.iter().map(|m| m.name.as_str()).collect();
    assert_eq!(names, ["acme-parent", "acme-core", "acme-app"]);

    let core = module(&model, "acme-core");
    assert_eq!(core.root, "core");
    assert_eq!(
        core.source_roots,
        ["core/src/java"],
        "project.basedir interpolated"
    );
    assert_eq!(
        core.test_roots,
        ["core/src/test/java"],
        "conventional test root"
    );
    assert_eq!(core.parent.as_deref(), Some("acme-parent"));

    let app = module(&model, "acme-app");
    assert_eq!(
        app.source_roots,
        ["app/src/main/java", "app/target/generated-sources/acme"],
        "build-helper source with a property inherited from the parent"
    );
    assert_eq!(app.test_roots, ["app/src/test/java", "app/src/it/java"]);
    assert_eq!(app.notes, Vec::<String>::new());
}

#[test]
fn a_single_pom_falls_back_to_the_conventional_layout() {
    let (_guard, root) = project(&[(
        "pom.xml",
        "<project><artifactId>solo</artifactId></project>",
    )]);
    let model = discover(&root);
    assert_eq!(
        model.modules,
        [Module {
            name: "solo".to_owned(),
            build: "maven".to_owned(),
            root: String::new(),
            source_roots: vec!["src/main/java".to_owned()],
            test_roots: vec!["src/test/java".to_owned()],
            parent: None,
            notes: vec![],
        }]
    );
}

#[test]
fn undefined_properties_are_noted_not_guessed() {
    let (_guard, root) = project(&[(
        "pom.xml",
        "<project><artifactId>x</artifactId><build><sourceDirectory>${nowhere}/java</sourceDirectory></build></project>",
    )]);
    let model = discover(&root);
    assert_eq!(model.modules[0].notes, ["undefined property ${nowhere}"]);
}

#[test]
fn an_unreadable_pom_is_reported() {
    let (_guard, root) = project(&[("pom.xml", "<project><artifactId>x</project>")]);
    let model = discover(&root);
    assert_eq!(model.modules, vec![]);
    assert_eq!(model.unsupported.len(), 1);
    assert_eq!(model.unsupported[0].file, "pom.xml");
}
