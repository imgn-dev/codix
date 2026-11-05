//! The Maven provider: reads `pom.xml` files declaratively, without running
//! Maven, for modules and source roots.

use std::collections::HashMap;
use std::fs;
use std::path::{Component, Path, PathBuf};

use codix_provider::{BuildModel, BuildResolver, Module, Unsupported};
use roxmltree::{Document, Node};

/// The Maven resolver.
#[derive(Debug)]
pub struct Maven;

codix_provider::register!(Build, "maven", "descriptor.toml", &Maven);

const BUILD_HELPER: &str = "build-helper-maven-plugin";

impl BuildResolver for Maven {
    fn discover(&self, root: &Path) -> BuildModel {
        let mut model = BuildModel::default();
        let mut seen = Vec::new();
        visit(root, root, None, &mut model, &mut seen);
        model
    }
}

/// A parsed POM: its coordinates and properties after inheritance.
#[derive(Debug, Clone, Default)]
struct Pom {
    artifact_id: String,
    properties: HashMap<String, String>,
}

/// Reads the POM in `dir`, records its module, and visits its `<modules>`.
fn visit(
    root: &Path,
    dir: &Path,
    parent: Option<&Pom>,
    model: &mut BuildModel,
    seen: &mut Vec<PathBuf>,
) {
    let file = dir.join("pom.xml");
    let Ok(canonical) = fs::canonicalize(&file) else {
        return;
    };
    if seen.contains(&canonical) {
        return;
    }
    seen.push(canonical);
    let shown = relative(root, &file);
    let Ok(xml) = fs::read_to_string(&file) else {
        return;
    };
    let doc = match Document::parse(&xml) {
        Ok(doc) => doc,
        Err(e) => {
            model.unsupported.push(Unsupported {
                file: shown,
                line: e.pos().row,
                construct: format!("unreadable pom.xml: {e}"),
            });
            return;
        }
    };
    let project = doc.root_element();
    let inherited = parent
        .cloned()
        .or_else(|| declared_parent(root, dir, project));
    let pom = read_pom(root, dir, project, inherited.as_ref());
    let mut notes = Vec::new();
    let mut resolve = |raw: &str| -> Option<String> {
        let value = interpolate(raw, &pom.properties, &mut notes);
        let path = normalize(&dir.join(value));
        let relative = path.strip_prefix(root).ok()?;
        Some(to_root_relative(relative))
    };

    let build = child(project, "build");
    let mut source_roots: Vec<String> = Vec::new();
    let mut test_roots: Vec<String> = Vec::new();
    let main = build
        .and_then(|b| child(b, "sourceDirectory"))
        .and_then(|n| n.text())
        .unwrap_or("src/main/java");
    let test = build
        .and_then(|b| child(b, "testSourceDirectory"))
        .and_then(|n| n.text())
        .unwrap_or("src/test/java");
    source_roots.extend(resolve(main.trim()));
    test_roots.extend(resolve(test.trim()));
    for (goal, source) in build_helper_sources(project) {
        let target = if goal == "add-test-source" {
            &mut test_roots
        } else {
            &mut source_roots
        };
        target.extend(resolve(&source));
    }
    let name = pom.artifact_id.clone();
    model.modules.push(Module {
        name: name.clone(),
        build: "maven".to_owned(),
        root: to_root_relative(dir.strip_prefix(root).unwrap_or(Path::new(""))),
        source_roots,
        test_roots,
        parent: inherited.map(|p| p.artifact_id).filter(|p| !p.is_empty()),
        notes,
    });

    for module in children(project, "modules").flat_map(|m| children(m, "module")) {
        if let Some(path) = module.text() {
            visit(root, &dir.join(path.trim()), Some(&pom), model, seen);
        }
    }
}

/// The parent POM named by `<parent><relativePath>` (default `../pom.xml`),
/// when it is inside the project and is not the module's own aggregator.
fn declared_parent(root: &Path, dir: &Path, project: Node<'_, '_>) -> Option<Pom> {
    let parent = child(project, "parent")?;
    let relative_path = child(parent, "relativePath")
        .and_then(|n| n.text())
        .map_or("../pom.xml", str::trim);
    if relative_path.is_empty() {
        return None;
    }
    let mut file = normalize(&dir.join(relative_path));
    if file.is_dir() {
        file = file.join("pom.xml");
    }
    if !file.starts_with(root) {
        return None;
    }
    let text = fs::read_to_string(&file).ok()?;
    let doc = Document::parse(&text).ok()?;
    let parent_dir = file.parent()?.to_path_buf();
    let grandparent = declared_parent(root, &parent_dir, doc.root_element());
    Some(read_pom(
        root,
        &parent_dir,
        doc.root_element(),
        grandparent.as_ref(),
    ))
}

/// Coordinates and properties, inherited ones first so the POM's own win.
fn read_pom(root: &Path, dir: &Path, project: Node<'_, '_>, parent: Option<&Pom>) -> Pom {
    let mut properties = parent.map(|p| p.properties.clone()).unwrap_or_default();
    let parent_element = child(project, "parent");
    let inherited = |field: &str| {
        child(project, field)
            .or_else(|| parent_element.and_then(|p| child(p, field)))
            .and_then(|n| n.text())
            .map(|t| t.trim().to_owned())
    };
    let artifact_id = inherited("artifactId").unwrap_or_default();
    for (key, value) in [
        ("project.artifactId", Some(artifact_id.clone())),
        ("project.groupId", inherited("groupId")),
        ("project.version", inherited("version")),
        ("project.basedir", Some(dir.to_string_lossy().into_owned())),
        ("basedir", Some(dir.to_string_lossy().into_owned())),
        (
            "project.build.directory",
            Some(dir.join("target").to_string_lossy().into_owned()),
        ),
        ("project.rootdir", Some(root.to_string_lossy().into_owned())),
    ] {
        if let Some(value) = value {
            properties.insert(key.to_owned(), value);
        }
    }
    for property in
        children(project, "properties").flat_map(|p| p.children().filter(Node::is_element))
    {
        properties.insert(
            property.tag_name().name().to_owned(),
            property.text().unwrap_or_default().trim().to_owned(),
        );
    }
    Pom {
        artifact_id,
        properties,
    }
}

/// `(goal, source)` pairs from `build-helper-maven-plugin` executions.
fn build_helper_sources(project: Node<'_, '_>) -> Vec<(String, String)> {
    let plugins = children(project, "build")
        .flat_map(|b| children(b, "plugins"))
        .flat_map(|p| children(p, "plugin"));
    let mut found = Vec::new();
    for plugin in plugins.filter(|p| {
        child(*p, "artifactId")
            .and_then(|a| a.text())
            .map(str::trim)
            == Some(BUILD_HELPER)
    }) {
        for execution in children(plugin, "executions").flat_map(|e| children(e, "execution")) {
            let goals: Vec<String> = children(execution, "goals")
                .flat_map(|g| children(g, "goal"))
                .filter_map(|g| g.text().map(|t| t.trim().to_owned()))
                .collect();
            let goal = if goals.iter().any(|g| g == "add-test-source") {
                "add-test-source"
            } else if goals.iter().any(|g| g == "add-source") {
                "add-source"
            } else {
                continue;
            };
            for source in children(execution, "configuration")
                .flat_map(|c| children(c, "sources"))
                .flat_map(|s| children(s, "source"))
                .filter_map(|s| s.text())
            {
                found.push((goal.to_owned(), source.trim().to_owned()));
            }
        }
    }
    found
}

/// Replaces `${name}` with property values, repeatedly for values that refer
/// to other properties; undefined ones are kept and noted.
fn interpolate(raw: &str, properties: &HashMap<String, String>, notes: &mut Vec<String>) -> String {
    let mut value = raw.to_owned();
    for _ in 0..10 {
        let Some(start) = value.find("${") else {
            return value;
        };
        let Some(len) = value[start..].find('}') else {
            return value;
        };
        let name = &value[start + 2..start + len];
        let Some(replacement) = properties.get(name) else {
            let note = format!("undefined property ${{{name}}}");
            if !notes.contains(&note) {
                notes.push(note);
            }
            return value;
        };
        value = format!(
            "{}{replacement}{}",
            &value[..start],
            &value[start + len + 1..]
        );
    }
    value
}

fn child<'a, 'input>(node: Node<'a, 'input>, name: &str) -> Option<Node<'a, 'input>> {
    node.children()
        .find(|n| n.is_element() && n.tag_name().name() == name)
}

fn children<'a, 'input>(
    node: Node<'a, 'input>,
    name: &'static str,
) -> impl Iterator<Item = Node<'a, 'input>> {
    node.children()
        .filter(move |n| n.is_element() && n.tag_name().name() == name)
}

/// Resolves `.` and `..` without touching the filesystem.
fn normalize(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for component in path.components() {
        match component {
            Component::ParentDir => {
                out.pop();
            }
            Component::CurDir => {}
            other => out.push(other),
        }
    }
    out
}

fn to_root_relative(path: &Path) -> String {
    path.iter()
        .map(|part| part.to_string_lossy())
        .collect::<Vec<_>>()
        .join("/")
}

fn relative(root: &Path, path: &Path) -> String {
    to_root_relative(path.strip_prefix(root).unwrap_or(path))
}
