//! The Gradle provider. Gradle build files are code, so this reads only the
//! common declarative patterns, in the Groovy and Kotlin DSLs alike: projects
//! included from `settings.gradle(.kts)` and `sourceSets` blocks. Anything
//! else that could change the layout is reported, never guessed.

use std::fs;
use std::path::Path;

use codix_provider::{BuildModel, BuildResolver, Module, Unsupported};

/// The Gradle resolver.
#[derive(Debug)]
pub struct Gradle;

codix_provider::register!(Build, "gradle", "descriptor.toml", &Gradle);

const SETTINGS: &[&str] = &["settings.gradle.kts", "settings.gradle"];
const BUILD_FILES: &[&str] = &["build.gradle.kts", "build.gradle"];

impl BuildResolver for Gradle {
    fn discover(&self, root: &Path) -> BuildModel {
        let mut model = BuildModel::default();
        let mut root_name = root
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default();
        let mut included: Vec<String> = Vec::new();
        if let Some((file, text)) = first_existing(root, "", SETTINGS) {
            let settings = read_settings(&tokens(&text));
            if let Some(name) = settings.root_name {
                root_name = name;
            }
            included = settings.included;
            model
                .unsupported
                .extend(
                    settings
                        .unsupported
                        .into_iter()
                        .map(|(line, construct)| Unsupported {
                            file: file.clone(),
                            line,
                            construct,
                        }),
                );
        }
        let mut projects = vec![(root_name.clone(), String::new())];
        projects.extend(included.into_iter().map(|path| {
            let dir = path.trim_start_matches(':').replace(':', "/");
            (format!(":{}", path.trim_start_matches(':')), dir)
        }));
        for (name, dir) in projects {
            let Some((file, text)) = first_existing(root, &dir, BUILD_FILES) else {
                model
                    .modules
                    .push(module(&name, &dir, &root_name, &SourceSets::default()));
                continue;
            };
            let (sets, unsupported) = read_source_sets(&tokens(&text));
            model
                .unsupported
                .extend(
                    unsupported
                        .into_iter()
                        .map(|(line, construct)| Unsupported {
                            file: file.clone(),
                            line,
                            construct,
                        }),
                );
            model.modules.push(module(&name, &dir, &root_name, &sets));
        }
        model
    }
}

fn module(name: &str, dir: &str, root_name: &str, sets: &SourceSets) -> Module {
    let under = |path: &str| {
        if dir.is_empty() {
            path.trim_end_matches('/').to_owned()
        } else {
            format!("{dir}/{}", path.trim_end_matches('/'))
        }
    };
    Module {
        name: name.to_owned(),
        build: "gradle".to_owned(),
        root: dir.to_owned(),
        source_roots: sets.main.iter().map(|p| under(p)).collect(),
        test_roots: sets.test.iter().map(|p| under(p)).collect(),
        parent: (!dir.is_empty()).then(|| root_name.to_owned()),
        notes: Vec::new(),
    }
}

/// The first of `names` that exists in `dir`, as (root-relative path, text).
fn first_existing(root: &Path, dir: &str, names: &[&str]) -> Option<(String, String)> {
    names.iter().find_map(|name| {
        let relative = if dir.is_empty() {
            (*name).to_owned()
        } else {
            format!("{dir}/{name}")
        };
        fs::read_to_string(root.join(&relative))
            .ok()
            .map(|text| (relative, text))
    })
}

/// A token of either DSL, with its 1-based line.
#[derive(Debug, Clone, PartialEq, Eq)]
enum Token {
    /// A name, dots included: `rootProject.name`, `java.srcDir`.
    Word(String),
    /// A string literal's content.
    Str(String),
    /// Any other character, or `+=`.
    Punct(String),
}

fn tokens(text: &str) -> Vec<(u32, Token)> {
    let chars: Vec<char> = text.chars().collect();
    let mut out = Vec::new();
    let (mut i, mut line) = (0, 1u32);
    while i < chars.len() {
        let c = chars[i];
        let next = chars.get(i + 1).copied();
        match c {
            '\n' => {
                line += 1;
                i += 1;
            }
            c if c.is_whitespace() || c == ';' => i += 1,
            '/' if next == Some('/') => {
                while i < chars.len() && chars[i] != '\n' {
                    i += 1;
                }
            }
            '/' if next == Some('*') => {
                i += 2;
                while i + 1 < chars.len() && !(chars[i] == '*' && chars[i + 1] == '/') {
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    i += 1;
                }
                i += 2;
            }
            '"' | '\'' => {
                let quote = c;
                let start_line = line;
                let mut value = String::new();
                i += 1;
                while i < chars.len() && chars[i] != quote {
                    if chars[i] == '\\' && i + 1 < chars.len() {
                        i += 1;
                    }
                    if chars[i] == '\n' {
                        line += 1;
                    }
                    value.push(chars[i]);
                    i += 1;
                }
                i += 1;
                out.push((start_line, Token::Str(value)));
            }
            '+' if next == Some('=') => {
                out.push((line, Token::Punct("+=".to_owned())));
                i += 2;
            }
            c if c.is_alphanumeric() || c == '_' || c == '.' || c == ':' => {
                let mut word = String::new();
                while i < chars.len()
                    && (chars[i].is_alphanumeric() || matches!(chars[i], '_' | '.' | ':'))
                {
                    word.push(chars[i]);
                    i += 1;
                }
                out.push((line, Token::Word(word)));
            }
            c => {
                out.push((line, Token::Punct(c.to_string())));
                i += 1;
            }
        }
    }
    out
}

fn is(token: Option<&(u32, Token)>, punct: &str) -> bool {
    matches!(token, Some((_, Token::Punct(p))) if p == punct)
}

fn word(token: Option<&(u32, Token)>) -> Option<&str> {
    match token {
        Some((_, Token::Word(w))) => Some(w),
        _ => None,
    }
}

/// The text of the tokens from `start` to the end of its line, for reports.
fn construct(tokens: &[(u32, Token)], start: usize) -> String {
    let line = tokens[start].0;
    tokens[start..]
        .iter()
        .take_while(|(l, _)| *l == line)
        .map(|(_, t)| match t {
            Token::Word(w) | Token::Punct(w) => w.clone(),
            Token::Str(s) => format!("\"{s}\""),
        })
        .collect::<Vec<_>>()
        .join(" ")
}

#[derive(Debug, Default)]
struct Settings {
    root_name: Option<String>,
    included: Vec<String>,
    unsupported: Vec<(u32, String)>,
}

fn read_settings(tokens: &[(u32, Token)]) -> Settings {
    let mut settings = Settings::default();
    let mut i = 0;
    while i < tokens.len() {
        // One report per line is enough to point at the construct.
        if settings
            .unsupported
            .last()
            .is_some_and(|(line, _)| *line == tokens[i].0)
        {
            i += 1;
            continue;
        }
        match word(tokens.get(i)) {
            Some("rootProject.name") if is(tokens.get(i + 1), "=") => {
                if let Some((_, Token::Str(name))) = tokens.get(i + 2) {
                    settings.root_name = Some(name.clone());
                    i += 3;
                    continue;
                }
                settings
                    .unsupported
                    .push((tokens[i].0, construct(tokens, i)));
            }
            Some("include") => {
                let (strings, end) = arguments(tokens, i + 1);
                match strings {
                    Some(paths) if !paths.iter().any(|p| p.contains('$')) => {
                        settings.included.extend(paths);
                    }
                    _ => settings
                        .unsupported
                        .push((tokens[i].0, construct(tokens, i))),
                }
                i = end;
                continue;
            }
            Some(w)
                if w == "includeBuild" || w.contains("projectDir") || w.starts_with("project(") =>
            {
                settings
                    .unsupported
                    .push((tokens[i].0, construct(tokens, i)));
            }
            Some("project") if is(tokens.get(i + 1), "(") => {
                settings
                    .unsupported
                    .push((tokens[i].0, construct(tokens, i)));
            }
            _ => {}
        }
        i += 1;
    }
    settings
}

/// String arguments after a call name: `("a", "b")`, `'a', 'b'`,
/// `(listOf("a"))`, `= ["a"]`, `= listOf("a")`. `None` when any argument is
/// not a string literal. Returns the index after the arguments.
fn arguments(tokens: &[(u32, Token)], mut i: usize) -> (Option<Vec<String>>, usize) {
    let line = tokens.get(i.saturating_sub(1)).map_or(0, |t| t.0);
    if is(tokens.get(i), "=") || is(tokens.get(i), "+=") {
        i += 1;
    }
    if matches!(
        word(tokens.get(i)),
        Some("listOf" | "setOf" | "files" | "mutableListOf")
    ) {
        i += 1;
    }
    let close = match tokens.get(i) {
        Some((_, Token::Punct(p))) if p == "(" => Some(")"),
        Some((_, Token::Punct(p))) if p == "[" => Some("]"),
        _ => None,
    };
    if close.is_some() {
        i += 1;
        if matches!(word(tokens.get(i)), Some("listOf" | "setOf" | "files"))
            && is(tokens.get(i + 1), "(")
        {
            let (inner, end) = arguments(tokens, i);
            return (
                inner,
                if close.is_some_and(|c| is(tokens.get(end), c)) {
                    end + 1
                } else {
                    end
                },
            );
        }
    }
    let mut values = Vec::new();
    let mut all_strings = true;
    while let Some((l, token)) = tokens.get(i) {
        match (close, token) {
            (Some(c), Token::Punct(p)) if p == c => {
                return ((all_strings && !values.is_empty()).then_some(values), i + 1);
            }
            (None, _) if *l != line => break,
            (_, Token::Str(s)) => values.push(s.clone()),
            (_, Token::Punct(p)) if p == "," => {}
            (None, Token::Punct(_) | Token::Word(_)) => break,
            _ => all_strings = false,
        }
        i += 1;
    }
    ((all_strings && !values.is_empty()).then_some(values), i)
}

/// Java source directories of the `main` and `test` source sets.
#[derive(Debug, Clone)]
struct SourceSets {
    main: Vec<String>,
    test: Vec<String>,
}

impl Default for SourceSets {
    fn default() -> Self {
        Self {
            main: vec!["src/main/java".to_owned()],
            test: vec!["src/test/java".to_owned()],
        }
    }
}

fn read_source_sets(tokens: &[(u32, Token)]) -> (SourceSets, Vec<(u32, String)>) {
    let mut sets = SourceSets::default();
    let mut unsupported = Vec::new();
    let mut i = 0;
    while i < tokens.len() {
        match word(tokens.get(i)) {
            Some("sourceSets") if is(tokens.get(i + 1), "{") => {
                i = source_sets_block(tokens, i + 2, &mut sets, &mut unsupported);
                continue;
            }
            Some(w) if w.starts_with("sourceSets") => {
                unsupported.push((tokens[i].0, construct(tokens, i)));
            }
            _ => {}
        }
        i += 1;
    }
    (sets, unsupported)
}

/// Inside `sourceSets { … }`: `main { … }`, `test { … }`, `named("main") { … }`.
fn source_sets_block(
    tokens: &[(u32, Token)],
    mut i: usize,
    sets: &mut SourceSets,
    unsupported: &mut Vec<(u32, String)>,
) -> usize {
    while i < tokens.len() {
        if is(tokens.get(i), "}") {
            return i + 1;
        }
        let set = match (word(tokens.get(i)), tokens.get(i + 2)) {
            (Some(name @ ("main" | "test")), _) if is(tokens.get(i + 1), "{") => {
                i += 2;
                Some(name.to_owned())
            }
            (Some("named" | "getByName"), Some((_, Token::Str(name))))
                if is(tokens.get(i + 1), "(")
                    && is(tokens.get(i + 3), ")")
                    && is(tokens.get(i + 4), "{") =>
            {
                i += 5;
                Some(name.clone())
            }
            _ => None,
        };
        match set {
            Some(name) if name == "main" || name == "test" => {
                let target = if name == "main" {
                    &mut sets.main
                } else {
                    &mut sets.test
                };
                i = set_block(tokens, i, target, unsupported);
            }
            Some(_) => i = skip_block(tokens, i),
            None => {
                unsupported.push((tokens[i].0, construct(tokens, i)));
                i = skip_statement(tokens, i);
            }
        }
    }
    i
}

/// Inside one source set: `java { … }` and the dotted `java.srcDir(…)` forms;
/// other languages' and resources' blocks are left alone.
fn set_block(
    tokens: &[(u32, Token)],
    mut i: usize,
    dirs: &mut Vec<String>,
    unsupported: &mut Vec<(u32, String)>,
) -> usize {
    while i < tokens.len() {
        if is(tokens.get(i), "}") {
            return i + 1;
        }
        match word(tokens.get(i)) {
            Some("java") if is(tokens.get(i + 1), "{") => {
                i += 2;
                while i < tokens.len() && !is(tokens.get(i), "}") {
                    i = dir_statement(tokens, i, dirs, unsupported);
                }
                i += 1;
            }
            Some(w) if w.starts_with("java.") => {
                let call = w.trim_start_matches("java.").to_owned();
                i = dir_call(tokens, i, &call, dirs, unsupported);
            }
            Some(_) if is(tokens.get(i + 1), "{") => i = skip_block(tokens, i + 2),
            _ => i = skip_statement(tokens, i),
        }
    }
    i
}

fn dir_statement(
    tokens: &[(u32, Token)],
    i: usize,
    dirs: &mut Vec<String>,
    unsupported: &mut Vec<(u32, String)>,
) -> usize {
    if let Some(call) = word(tokens.get(i)) {
        let call = call.to_owned();
        dir_call(tokens, i, &call, dirs, unsupported)
    } else {
        unsupported.push((tokens[i].0, construct(tokens, i)));
        skip_statement(tokens, i)
    }
}

/// `srcDir`, `srcDirs` (add) and `srcDirs =`, `setSrcDirs` (replace).
fn dir_call(
    tokens: &[(u32, Token)],
    i: usize,
    call: &str,
    dirs: &mut Vec<String>,
    unsupported: &mut Vec<(u32, String)>,
) -> usize {
    let replaces = call == "setSrcDirs" || (call == "srcDirs" && is(tokens.get(i + 1), "="));
    if !matches!(call, "srcDir" | "srcDirs" | "setSrcDirs") {
        unsupported.push((tokens[i].0, construct(tokens, i)));
        return skip_statement(tokens, i);
    }
    let (values, end) = arguments(tokens, i + 1);
    match values {
        Some(values) if !values.iter().any(|v| v.contains('$')) => {
            if replaces {
                dirs.clear();
            }
            dirs.extend(values);
        }
        _ => unsupported.push((tokens[i].0, construct(tokens, i))),
    }
    end
}

/// Skips to the end of the current line, past any block it opens.
fn skip_statement(tokens: &[(u32, Token)], i: usize) -> usize {
    let line = tokens[i].0;
    let mut j = i;
    while j < tokens.len() && tokens[j].0 == line {
        if is(tokens.get(j), "{") {
            return skip_block(tokens, j + 1);
        }
        if is(tokens.get(j), "}") && j > i {
            return j;
        }
        j += 1;
    }
    j.max(i + 1)
}

/// Skips past the `}` closing a block whose `{` is just before `i`.
fn skip_block(tokens: &[(u32, Token)], mut i: usize) -> usize {
    let mut depth = 1;
    while i < tokens.len() && depth > 0 {
        if is(tokens.get(i), "{") {
            depth += 1;
        } else if is(tokens.get(i), "}") {
            depth -= 1;
        }
        i += 1;
    }
    i
}
