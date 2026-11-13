//! Pre-flight checks: what an edit plan must pass before it is stored. Each
//! failing check refuses the plan with a typed error naming its sites, and
//! the override that allows it, if one does. Overrides allow one condition
//! each, are listed in the preview, and never apply to parse errors.

use std::fs;
use std::path::Path;

use codix_provider::{LanguageDescriptor, registry};
use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::config::Config;
use crate::edit::{Edit, OverrideUse, apply};
use crate::error::Site;
use crate::op::Context;
use crate::project::{FileId, Project};
use crate::refs::Analyzed;
use crate::scope::{Binding, Defines, FileModel};

/// The name rule when a pack sets none: Unicode identifiers.
const DEFAULT_IDENTIFIER: &str = r"^[\p{XID_Start}_]\p{XID_Continue}*$";

/// Lines read at the top of a file for generated-code markers.
const HEADER_LINES: usize = 20;

/// A marker of generated code in any language.
const GENERATED_MARKER: &str = "@generated";

/// A condition a check refuses, which the user can allow, one at a time.
#[derive(
    Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize, schemars::JsonSchema,
)]
#[serde(rename_all = "kebab-case")]
pub enum Allow {
    /// The new name shadows, or is shadowed by, another definition, changing
    /// what some references mean.
    Shadowing,
    /// The new name is another method's in the same scope, making it an
    /// overload.
    Overload,
    /// The first letter of a Go name changes case, and with it whether it
    /// is exported.
    VisibilityChange,
    /// The edit changes generated or vendored files.
    Generated,
}

impl Allow {
    /// The value as given on the command line.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Shadowing => "shadowing",
            Self::Overload => "overload",
            Self::VisibilityChange => "visibility-change",
            Self::Generated => "generated",
        }
    }
}

/// The overrides an edit command takes, each allowing one condition.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Overrides {
    /// Allow a condition the checks refuse; repeatable.
    #[serde(default)]
    pub allow: Vec<Allow>,
    /// Edit the sites codix binds and leave unchanged the references it cannot bind, in other files or through other objects.
    #[serde(default)]
    pub exclude_unresolved: bool,
    /// At tier 1, where codix cannot bind names, rename every whole-word match of the name in its file.
    #[serde(default)]
    pub textual: bool,
}

impl Overrides {
    fn allows(&self, condition: Allow) -> bool {
        self.allow.contains(&condition)
    }
}

/// An override use, as previews list them.
fn used(condition: Allow, what: String, sites: Vec<Site>) -> OverrideUse {
    OverrideUse {
        flag: format!("--allow {}", condition.as_str()),
        condition: what,
        sites: sites.into_iter().map(|s| s.location).collect(),
    }
}

/// A rename to check: definitions of one file, renamed at the given sites.
pub struct Rename<'a> {
    /// The file, as it is on disk.
    pub file: &'a Analyzed,
    /// The definitions renamed, indices into the model's definitions.
    pub targets: &'a [usize],
    /// The edits: every site renamed, definitions included.
    pub edits: &'a [Edit],
    /// The new name.
    pub new_name: &'a str,
    /// References codix cannot bind that may mean the targets.
    pub unresolved: &'a [Site],
    /// Overrides given.
    pub overrides: &'a Overrides,
}

fn refused(code: &'static str, cause: String, hint: &str, sites: Vec<Site>) -> Error {
    Error::Refused {
        code,
        cause,
        hint: hint.to_owned(),
        sites,
    }
}

/// The text of a line, trimmed, as a site's note.
fn line_text(content: &str, line: u32) -> String {
    content
        .lines()
        .nth((line as usize).saturating_sub(1))
        .unwrap_or_default()
        .trim()
        .to_owned()
}

fn site_at(ctx: &Context, file: &Analyzed, line: u32, column: u32) -> Site {
    Site {
        location: ctx.location(&file.id, line, column),
        note: line_text(&file.content, line),
    }
}

/// Runs every check on a rename, in order; the overrides used, with the
/// sites each allowed.
pub fn check_rename(
    ctx: &Context,
    project: &Project,
    config: &Config,
    rename: &Rename<'_>,
) -> Result<Vec<OverrideUse>, Error> {
    let descriptor = descriptor(&rename.file.language)?;
    let mut used = Vec::new();
    valid_name(descriptor, rename)?;
    no_parse_errors(ctx, rename)?;
    not_named_after_file(ctx, descriptor, rename)?;
    used.extend(no_collision(ctx, descriptor, rename)?);
    used.extend(no_shadowing(ctx, descriptor, rename)?);
    used.extend(same_visibility(ctx, descriptor, rename)?);
    used.extend(no_unresolved(rename)?);
    used.extend(not_generated(
        ctx,
        project,
        config,
        rename.file,
        rename.overrides,
    )?);
    Ok(used)
}

/// The checks for another file a rename edits, at references a resolver
/// bound to the symbol: the file must not be generated, and the new name
/// must not bind to a definition of the file there (`shadowing`).
pub fn check_other_file(
    ctx: &Context,
    project: &Project,
    config: &Config,
    file: &Analyzed,
    edits: &[Edit],
    new_name: &str,
    overrides: &Overrides,
) -> Result<Vec<OverrideUse>, Error> {
    let mut used_overrides: Vec<OverrideUse> =
        not_generated(ctx, project, config, file, overrides)?
            .into_iter()
            .collect();
    let descriptor = descriptor(&file.language)?;
    let Some(grammar) = registry().grammars.get(&descriptor.name) else {
        return Ok(used_overrides);
    };
    if grammar.locals.is_none() {
        return Ok(used_overrides);
    }
    let after = crate::scope::model(
        descriptor,
        grammar,
        file.id.as_str(),
        &apply(&file.content, edits),
    )?;
    let mut sites = Vec::new();
    for e in edits {
        let Some(r) = after.reference_at(shifted(edits, e.start)) else {
            continue;
        };
        if let Binding::Bound(_) = after.references[r].binding {
            let at = file
                .model
                .reference_at(e.start)
                .map(|i| file.model.references[i].range.start);
            if let Some(at) = at {
                let mut site = site_at(ctx, file, at.line, at.column);
                site.note = format!("would now mean a definition of its file: {}", site.note);
                sites.push(site);
            }
        }
    }
    if sites.is_empty() {
        return Ok(used_overrides);
    }
    if overrides.allows(Allow::Shadowing) {
        used_overrides.push(used(
            Allow::Shadowing,
            "references whose meaning changes".to_owned(),
            sites,
        ));
        return Ok(used_overrides);
    }
    Err(refused(
        "shadowing",
        format!(
            "`{new_name}` would change what {} reference(s) mean",
            sites.len()
        ),
        "choose another name, or pass --allow shadowing to accept it",
        sites,
    ))
}

fn descriptor(language: &str) -> Result<&'static LanguageDescriptor, Error> {
    registry()
        .languages
        .iter()
        .find(|d| d.name == language)
        .ok_or_else(|| Error::Internal {
            message: format!("no {language} pack for a file of that language"),
        })
}

/// Kinds named like types, for words reserved as type names only.
fn is_type_kind(kind: &str) -> bool {
    matches!(
        kind,
        "class" | "struct" | "interface" | "trait" | "enum" | "type_alias" | "type_parameter"
    )
}

fn target_kind(model: &FileModel, definition: usize) -> &str {
    match &model.definitions[definition].defines {
        Defines::Symbol(i) => &model.symbols[*i].kind,
        Defines::Local { kind } => kind,
    }
}

/// `invalid_name`: the new name must be an identifier and no keyword, and
/// the edited file must parse as well as before.
fn valid_name(descriptor: &LanguageDescriptor, rename: &Rename<'_>) -> Result<(), Error> {
    let names = &descriptor.names;
    let pattern = names.identifier.as_deref().unwrap_or(DEFAULT_IDENTIFIER);
    let identifier = Regex::new(pattern).map_err(|e| Error::Internal {
        message: format!("the {} name pattern does not compile: {e}", descriptor.name),
    })?;
    let name = rename.new_name;
    let kind = target_kind(&rename.file.model, rename.targets[0]);
    let cause = if !identifier.is_match(name) {
        Some(format!("`{name}` is not a valid {} name", descriptor.name))
    } else if names.keywords.iter().any(|k| k == name) {
        Some(format!("`{name}` is a {} keyword", descriptor.name))
    } else if is_type_kind(kind) && names.type_keywords.iter().any(|k| k == name) {
        Some(format!(
            "`{name}` is reserved for {} type names",
            descriptor.name
        ))
    } else {
        None
    };
    if let Some(cause) = cause {
        return Err(refused(
            "invalid_name",
            cause,
            "choose another name",
            Vec::new(),
        ));
    }
    let before = error_count(descriptor, rename.file, &rename.file.content)?;
    let after = error_count(
        descriptor,
        rename.file,
        &apply(&rename.file.content, rename.edits),
    )?;
    if after > before {
        return Err(refused(
            "invalid_name",
            format!("the file would not parse with `{name}`"),
            "choose another name",
            Vec::new(),
        ));
    }
    Ok(())
}

/// Error and missing nodes in a parse of `content`.
fn error_count(
    descriptor: &LanguageDescriptor,
    file: &Analyzed,
    content: &str,
) -> Result<usize, Error> {
    let grammar = registry()
        .grammars
        .get(&descriptor.name)
        .ok_or_else(|| Error::Internal {
            message: format!("no {} grammar", descriptor.name),
        })?;
    let tree = crate::parse::parse(descriptor, grammar, file.id.as_str(), content)?;
    let mut count = 0;
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if node.is_error() || node.is_missing() {
            count += 1;
        }
        if node.has_error() && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return Ok(count);
            }
        }
    }
}

/// `parse_error_region`: no site may be in or around a parse error, and no
/// override allows one.
fn no_parse_errors(ctx: &Context, rename: &Rename<'_>) -> Result<(), Error> {
    let model = &rename.file.model;
    let mut sites = Vec::new();
    for &d in rename.targets {
        let def = &model.definitions[d];
        if def.low {
            let at = def.name_range.start;
            sites.push(site_at(ctx, rename.file, at.line, at.column));
        }
    }
    for r in &model.references {
        if r.low && matches!(r.binding, Binding::Bound(d) if rename.targets.contains(&d)) {
            sites.push(site_at(
                ctx,
                rename.file,
                r.range.start.line,
                r.range.start.column,
            ));
        }
    }
    if sites.is_empty() {
        return Ok(());
    }
    Err(refused(
        "parse_error_region",
        format!("{} site(s) are in or around a parse error", sites.len()),
        "fix the syntax error first; no override applies to parse errors",
        sites,
    ))
}

/// `renames_file`: a name the file's name must match cannot change here.
fn not_named_after_file(
    ctx: &Context,
    descriptor: &LanguageDescriptor,
    rename: &Rename<'_>,
) -> Result<(), Error> {
    let model = &rename.file.model;
    for &d in rename.targets {
        let Defines::Symbol(i) = model.definitions[d].defines else {
            continue;
        };
        let symbol = &model.symbols[i];
        let bound = descriptor.names.file_named_after.iter().any(|rule| {
            rule.kinds.contains(&symbol.kind)
                && rule
                    .visibility
                    .as_ref()
                    .is_none_or(|v| symbol.visibility.as_ref() == Some(v))
                && (!rule.top_level || symbol.parent.is_none())
                && (!rule.without_body
                    || rename.file.content
                        [symbol.range.start.byte as usize..symbol.range.end.byte as usize]
                        .trim_end()
                        .ends_with(';'))
        });
        if bound {
            let at = symbol.name_range.start;
            return Err(refused(
                "renames_file",
                format!("the file's name must match `{}`", symbol.name),
                "renaming it means moving the file, which codix does not do yet",
                vec![site_at(ctx, rename.file, at.line, at.column)],
            ));
        }
    }
    Ok(())
}

fn is_callable(kind: &str) -> bool {
    matches!(kind, "method" | "function" | "constructor")
}

/// `name_collision`: no other definition of the new name in the targets'
/// scope and namespace; with overloading, another method allowed by
/// `--allow overload`.
fn no_collision(
    ctx: &Context,
    descriptor: &LanguageDescriptor,
    rename: &Rename<'_>,
) -> Result<Option<OverrideUse>, Error> {
    let model = &rename.file.model;
    let first = &model.definitions[rename.targets[0]];
    let colliding: Vec<usize> = model
        .definitions
        .iter()
        .enumerate()
        .filter(|(i, d)| {
            !rename.targets.contains(i)
                && d.scope == first.scope
                && d.namespace == first.namespace
                && d.name == rename.new_name
        })
        .map(|(i, _)| i)
        .collect();
    if colliding.is_empty() {
        return Ok(None);
    }
    let sites: Vec<Site> = colliding
        .iter()
        .map(|&d| {
            let at = model.definitions[d].name_range.start;
            site_at(ctx, rename.file, at.line, at.column)
        })
        .collect();
    let overloads = descriptor.names.overloading
        && is_callable(target_kind(model, rename.targets[0]))
        && colliding
            .iter()
            .all(|&d| is_callable(target_kind(model, d)));
    if overloads && rename.overrides.allows(Allow::Overload) {
        return Ok(Some(used(
            Allow::Overload,
            format!(
                "`{}` becomes an overload of an existing method",
                rename.new_name
            ),
            sites,
        )));
    }
    let hint = if overloads {
        "choose another name, or pass --allow overload to make it an overload"
    } else {
        "choose another name"
    };
    Err(refused(
        "name_collision",
        format!("`{}` is already defined in the same scope", rename.new_name),
        hint,
        sites,
    ))
}

/// Where a position moves once the edits, sorted, are applied; a position
/// inside an edit moves to its start.
fn shifted(edits: &[Edit], byte: usize) -> usize {
    let mut delta: isize = 0;
    for e in edits {
        if byte < e.start {
            break;
        }
        if byte < e.end {
            return e.start.saturating_add_signed(delta);
        }
        delta += isize::try_from(e.replacement.len()).unwrap_or(isize::MAX)
            - isize::try_from(e.end - e.start).unwrap_or(isize::MAX);
    }
    byte.saturating_add_signed(delta)
}

/// `shadowing`: once renamed, every reference must bind as before. Applies
/// the edits in memory, binds the file again, and compares.
fn no_shadowing(
    ctx: &Context,
    descriptor: &LanguageDescriptor,
    rename: &Rename<'_>,
) -> Result<Option<OverrideUse>, Error> {
    let file = rename.file;
    let Some(grammar) = registry().grammars.get(&descriptor.name) else {
        return Ok(None);
    };
    if grammar.locals.is_none() {
        // Without scopes there is nothing to compare.
        return Ok(None);
    }
    let content = apply(&file.content, rename.edits);
    let after = crate::scope::model(descriptor, grammar, file.id.as_str(), &content)?;
    let before = &file.model;
    let renamed: Vec<usize> = rename
        .targets
        .iter()
        .map(|&d| {
            shifted(
                rename.edits,
                before.definitions[d].name_range.start.byte as usize,
            )
        })
        .collect();
    let target_of = |model: &FileModel, binding: Binding| match binding {
        Binding::Bound(d) => Some(model.definitions[d].name_range.start.byte as usize),
        Binding::Free | Binding::Uncertain => None,
    };
    let mut sites = Vec::new();
    for r in &before.references {
        let start = r.range.start.byte as usize;
        let Some(new) = after.reference_at(shifted(rename.edits, start)) else {
            continue;
        };
        let expected = target_of(before, r.binding).map(|b| shifted(rename.edits, b));
        let actual = target_of(&after, after.references[new].binding);
        if expected == actual {
            continue;
        }
        let note = if actual.is_some_and(|a| renamed.contains(&a)) {
            "would now mean the renamed definition"
        } else if expected.is_some_and(|e| renamed.contains(&e)) {
            "would no longer mean the renamed definition"
        } else {
            "would change meaning"
        };
        let mut site = site_at(ctx, file, r.range.start.line, r.range.start.column);
        site.note = format!("{note}: {}", site.note);
        sites.push(site);
    }
    if sites.is_empty() {
        return Ok(None);
    }
    if rename.overrides.allows(Allow::Shadowing) {
        return Ok(Some(used(
            Allow::Shadowing,
            "references whose meaning changes".to_owned(),
            sites,
        )));
    }
    Err(refused(
        "shadowing",
        format!(
            "`{}` would change what {} reference(s) mean",
            rename.new_name,
            sites.len()
        ),
        "choose another name, or pass --allow shadowing to accept it",
        sites,
    ))
}

/// `visibility_change`: in Go, the case of a first letter decides export.
fn same_visibility(
    ctx: &Context,
    descriptor: &LanguageDescriptor,
    rename: &Rename<'_>,
) -> Result<Option<OverrideUse>, Error> {
    let model = &rename.file.model;
    let first = &model.definitions[rename.targets[0]];
    let upper = |name: &str| name.chars().next().is_some_and(char::is_uppercase);
    let changes = descriptor.names.visibility_by_case
        && matches!(first.defines, Defines::Symbol(_))
        && upper(&first.name) != upper(rename.new_name);
    if !changes {
        return Ok(None);
    }
    let at = first.name_range.start;
    let site = site_at(ctx, rename.file, at.line, at.column);
    if rename.overrides.allows(Allow::VisibilityChange) {
        return Ok(Some(used(
            Allow::VisibilityChange,
            "the name's first letter changes case, and with it whether it is exported".to_owned(),
            vec![site],
        )));
    }
    Err(refused(
        "visibility_change",
        format!(
            "`{}` to `{}` changes whether it is exported",
            first.name, rename.new_name
        ),
        "keep the case of the first letter, or pass --allow visibility-change",
        vec![site],
    ))
}

/// `unresolved_sites`: references codix cannot bind may mean the symbol;
/// leaving them would break the code unless the user says to.
fn no_unresolved(rename: &Rename<'_>) -> Result<Option<OverrideUse>, Error> {
    if rename.unresolved.is_empty() {
        return Ok(None);
    }
    if rename.overrides.exclude_unresolved {
        return Ok(Some(OverrideUse {
            flag: "--exclude-unresolved".to_owned(),
            condition: "references codix cannot bind, left unchanged".to_owned(),
            sites: rename
                .unresolved
                .iter()
                .map(|s| s.location.clone())
                .collect(),
        }));
    }
    Err(refused(
        "unresolved_sites",
        format!(
            "{} reference(s) may mean it, but codix cannot bind them",
            rename.unresolved.len()
        ),
        "check them; pass --exclude-unresolved to rename the bound sites only and leave these unchanged",
        rename.unresolved.to_vec(),
    ))
}

/// `generated_code`: generated and vendored files are not edited by hand,
/// so not by codix either, unless `--allow generated` says so.
pub fn not_generated(
    ctx: &Context,
    project: &Project,
    config: &Config,
    file: &Analyzed,
    overrides: &Overrides,
) -> Result<Option<OverrideUse>, Error> {
    let Some(reason) = generated(project, config, &file.id, &file.language, &file.content) else {
        return Ok(None);
    };
    let location = ctx.location(&file.id, 1, 1);
    if overrides.allows(Allow::Generated) {
        return Ok(Some(OverrideUse {
            flag: format!("--allow {}", Allow::Generated.as_str()),
            condition: format!("edits a generated or vendored file ({reason})"),
            sites: vec![location],
        }));
    }
    Err(refused(
        "generated_code",
        format!("{} is generated or vendored ({reason})", location.path),
        "edit what generates it instead, or pass --allow generated",
        vec![Site {
            location,
            note: reason,
        }],
    ))
}

/// Why a file counts as generated or vendored, if it does.
pub fn generated(
    project: &Project,
    config: &Config,
    id: &FileId,
    language: &str,
    content: &str,
) -> Option<String> {
    let registry = registry();
    let pack = registry.languages.iter().find(|d| d.name == language);
    let header_patterns: Vec<Regex> = pack
        .map(|p| {
            p.generated
                .headers
                .iter()
                .filter_map(|h| Regex::new(h).ok())
                .collect()
        })
        .unwrap_or_default();
    for line in content.lines().take(HEADER_LINES) {
        if line.contains(GENERATED_MARKER) {
            return Some(format!("its header says `{GENERATED_MARKER}`"));
        }
        if header_patterns.iter().any(|p| p.is_match(line)) {
            return Some(format!("its header says `{}`", line.trim()));
        }
    }
    let root = project.root();
    let path = Path::new(id.as_str());
    let matches = |patterns: &[String]| matched(root, path, patterns);
    if let Some(pack) = pack
        && let Some(p) = matches(&pack.generated.paths)
    {
        return Some(format!("vendored, `{p}`"));
    }
    for build in &registry.builds {
        if let Some(p) = matches(&build.generated) {
            return Some(format!("generated by {}, `{p}`", build.name));
        }
    }
    if let Some(p) = matches(&config.generated_paths()) {
        return Some(format!("edits.generated `{p}` in codix.toml"));
    }
    for vcs in &registry.vcs {
        if vcs.generated_attributes.is_empty() || !root.join(vcs.detect.first()?).exists() {
            continue;
        }
        if let Some(attribute) = attribute_set(root, id, &vcs.generated_attributes) {
            return Some(format!("{attribute} in .gitattributes"));
        }
    }
    None
}

/// The first of `patterns`, gitignore-style and relative to the root, that
/// matches `path` or a directory above it.
fn matched(root: &Path, path: &Path, patterns: &[String]) -> Option<String> {
    patterns
        .iter()
        .find(|pattern| {
            let mut builder = ignore::gitignore::GitignoreBuilder::new(root);
            builder.add_line(None, pattern).is_ok()
                && builder.build().is_ok_and(|matcher| {
                    matcher
                        .matched_path_or_any_parents(root.join(path), false)
                        .is_ignore()
                })
        })
        .cloned()
}

/// The first of `attributes` set for `id` by the `.gitattributes` files of
/// its directories, the deepest file winning.
fn attribute_set(root: &Path, id: &FileId, attributes: &[String]) -> Option<String> {
    let mut found: Option<(String, bool)> = None;
    let mut dir = String::new();
    let parts: Vec<&str> = id.as_str().split('/').collect();
    for depth in 0..parts.len() {
        if depth > 0 {
            if !dir.is_empty() {
                dir.push('/');
            }
            dir.push_str(parts[depth - 1]);
        }
        let base = root.join(&dir);
        let Ok(text) = fs::read_to_string(base.join(".gitattributes")) else {
            continue;
        };
        let rest = parts[depth..].join("/");
        for line in text.lines() {
            let mut words = line.split_whitespace();
            let Some(pattern) = words.next().filter(|p| !p.starts_with('#')) else {
                continue;
            };
            let applies = matched(&base, Path::new(&rest), &[pattern.to_owned()]).is_some();
            if !applies {
                continue;
            }
            for word in words {
                let (name, set) = if let Some(name) = word.strip_prefix('-') {
                    (name, false)
                } else if let Some(name) = word.strip_suffix("=false") {
                    (name, false)
                } else {
                    (word.strip_suffix("=true").unwrap_or(word), true)
                };
                if attributes.iter().any(|a| a == name) {
                    found = Some((name.to_owned(), set));
                }
            }
        }
    }
    found.and_then(|(name, set)| set.then_some(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn edit(start: usize, end: usize, replacement: &str) -> Edit {
        Edit {
            start,
            end,
            replacement: replacement.to_owned(),
        }
    }

    #[test]
    fn positions_shift_by_the_edits_before_them() {
        let edits = [edit(2, 5, "x"), edit(10, 11, "long")];
        assert_eq!(shifted(&edits, 0), 0);
        assert_eq!(shifted(&edits, 2), 2);
        assert_eq!(shifted(&edits, 3), 2);
        assert_eq!(shifted(&edits, 5), 3);
        assert_eq!(shifted(&edits, 10), 8);
        assert_eq!(shifted(&edits, 12), 13);
    }

    #[test]
    fn gitignore_patterns_match_files_and_their_directories() {
        let root = Path::new("/p");
        let patterns = ["**/target/generated-sources/".to_owned()];
        assert!(
            matched(
                root,
                Path::new("m/target/generated-sources/A.java"),
                &patterns
            )
            .is_some()
        );
        assert!(matched(root, Path::new("m/src/A.java"), &patterns).is_none());
        assert!(matched(root, Path::new("vendor/x.go"), &["vendor/".to_owned()]).is_some());
    }

    #[test]
    fn attributes_follow_the_deepest_gitattributes() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("gen/keep")).unwrap();
        fs::write(root.join(".gitattributes"), "gen/** linguist-generated\n").unwrap();
        fs::write(
            root.join("gen/keep/.gitattributes"),
            "*.java -linguist-generated\n",
        )
        .unwrap();
        let attributes = ["linguist-generated".to_owned()];
        let set = |path: &str| attribute_set(root, &FileId::from_canonical(path), &attributes);
        assert_eq!(set("gen/A.java"), Some("linguist-generated".to_owned()));
        assert_eq!(set("gen/keep/B.java"), None);
        assert_eq!(set("src/C.java"), None);
    }
}
