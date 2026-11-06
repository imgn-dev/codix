//! `rename` within a file: the definition and the references its file's
//! scopes bind to it, checked before anything is stored. References codix
//! cannot bind, in other files or through other objects, refuse the plan
//! unless `--exclude-unresolved` leaves them unchanged.

use std::collections::HashMap;
use std::fs;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::config::Config;
use crate::edit::{Edit, EditSet, ExcludedSite, Exclusion, PreviewResponse, diff, plan_file};
use crate::error::Site;
use crate::op::{Context, Operation};
use crate::preflight::{Overrides, Rename, check_rename};
use crate::project::{FileId, Project, ScopeRequest};
use crate::query::{complete, indexed};
use crate::refs::{Analyzed, Target, target};
use crate::render::since_epoch;
use crate::scope::{Binding, Defines, Receiver};
use crate::store::{FileStatus, PreviewRow, RefState, Snapshot, Store, SymbolHit};

/// `rename`: plans renaming a symbol or a local within its file.
pub struct RenameOp;

/// Request for [`RenameOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct RenameRequest {
    /// Symbol ID, its path part, `file:line:col`, or a name; a local by `file:line:col` only.
    pub symbol: String,
    /// The new name.
    pub new_name: String,
    /// Consider symbols of this kind only; repeatable.
    #[serde(default, rename = "kind")]
    pub kinds: Vec<String>,
    /// Directories to consider, narrowing an ambiguous name.
    #[serde(flatten)]
    pub scope: ScopeRequest,
    /// Rename every overload of the name together.
    #[serde(default)]
    pub all_overloads: bool,
    /// Overrides of the checks.
    #[serde(flatten)]
    pub overrides: Overrides,
}

/// A planned rename, before it is stored.
pub(crate) struct Plan {
    /// The edits.
    pub edits: EditSet,
    /// The file, as planning read it.
    pub file: Analyzed,
    /// What planning answers, without an ID yet.
    pub response: PreviewResponse,
}

impl Operation for RenameOp {
    const NAME: &'static str = "rename";
    type Request = RenameRequest;
    type Response = PreviewResponse;

    fn run(&self, ctx: &Context, request: RenameRequest) -> Result<PreviewResponse, Error> {
        let (project, config) = indexed(ctx)?;
        complete(ctx, project, &config)?;
        let mut store = Store::open(&project.index_path())?;
        let plan = {
            let snapshot = store.snapshot()?;
            plan(ctx, project, &config, &snapshot, &request)?
        };
        prune(&mut store, &config)?;
        let mut row = PreviewRow {
            created: since_epoch(),
            operation: Self::NAME.to_owned(),
            request: serde_json::to_string(&request).map_err(|e| Error::Internal {
                message: format!("cannot store the rename request: {e}"),
            })?,
            files: vec![plan_file(
                plan.file.id.as_str(),
                &plan.file.content,
                &plan.edits.files[0].edits,
            )],
            edits: plan.edits,
            response: plan.response,
        };
        store.insert_preview(&mut row)?;
        Ok(row.response)
    }
}

/// Removes expired previews.
fn prune(store: &mut Store, config: &Config) -> Result<(), Error> {
    let now = since_epoch();
    for row in store.previews()? {
        if crate::previews::expired(&row, config.preview_expiry(), now) {
            store.delete_preview(&row.response.id)?;
        }
    }
    Ok(())
}

fn refused(code: &'static str, cause: String, hint: &str) -> Error {
    Error::Refused {
        code,
        cause,
        hint: hint.to_owned(),
        sites: Vec::new(),
    }
}

/// Plans a rename: its edits and its preview, checked.
pub(crate) fn plan(
    ctx: &Context,
    project: &Project,
    config: &Config,
    snapshot: &Snapshot<'_>,
    request: &RenameRequest,
) -> Result<Plan, Error> {
    let keep =
        |hit: &SymbolHit| request.kinds.is_empty() || request.kinds.contains(&hit.symbol.kind);
    let Target::Definitions {
        file,
        mut definitions,
    } = target(ctx, project, config, snapshot, &request.symbol, &keep)?;
    let constructors_named = codix_provider::registry()
        .languages
        .iter()
        .find(|d| d.name == file.language)
        .is_some_and(|pack| pack.names.constructors_named_after_type);
    if constructors_named {
        to_type(&file, &mut definitions);
    }
    let old_name = file.model.definitions[definitions[0]].name.clone();
    if request.new_name == old_name {
        return Err(Error::Usage {
            cause: format!("`{old_name}` already has that name"),
            hint: "pass the new name".to_owned(),
        });
    }
    if definitions.len() > 1 && !request.all_overloads {
        return Err(refused(
            "tier_too_low",
            format!(
                "`{old_name}` has {} overloads, and references to one cannot be told from references to the others without types",
                definitions.len()
            ),
            "pass --all-overloads to rename them together",
        ));
    }
    let Sites {
        edits,
        unresolved,
        mentions,
    } = sites(ctx, project, snapshot, request, &file, &mut definitions)?;
    let overrides = check_rename(
        ctx,
        project,
        config,
        &Rename {
            file: &file,
            targets: &definitions,
            edits: &edits,
            new_name: &request.new_name,
            unresolved: &unresolved,
            overrides: &request.overrides,
        },
    )?;
    let mut excluded: Vec<ExcludedSite> = unresolved
        .into_iter()
        .map(|s| ExcludedSite {
            location: s.location,
            reason: Exclusion::Unresolved,
            text: s.note,
        })
        .collect();
    excluded.extend(mentions.into_iter().map(|s| ExcludedSite {
        location: s.location,
        reason: Exclusion::Mention,
        text: s.note,
    }));
    let response = PreviewResponse {
        id: String::new(),
        description: description(ctx, &file, definitions[0], &request.new_name),
        files: 1,
        sites: edits.len(),
        overrides,
        excluded,
        ambiguous: Vec::new(),
        diff: diff(file.id.as_str(), &file.content, &edits),
    };
    let edits = EditSet::new(edits.into_iter().map(|e| (file.id.as_str().to_owned(), e)))?;
    Ok(Plan {
        edits,
        file,
        response,
    })
}

/// What a rename touches.
struct Sites {
    /// The sites it edits.
    edits: Vec<Edit>,
    /// The references it cannot bind.
    unresolved: Vec<Site>,
    /// The mentions of the name it leaves alone.
    mentions: Vec<Site>,
}

/// The sites a rename edits, the references it cannot bind, and the
/// mentions of the name it leaves alone.
fn sites(
    ctx: &Context,
    project: &Project,
    snapshot: &Snapshot<'_>,
    request: &RenameRequest,
    file: &Analyzed,
    definitions: &mut Vec<usize>,
) -> Result<Sites, Error> {
    let old_name = file.model.definitions[definitions[0]].name.as_str();
    let constructors_named = codix_provider::registry()
        .languages
        .iter()
        .find(|d| d.name == file.language)
        .is_some_and(|pack| pack.names.constructors_named_after_type);
    let registry = codix_provider::registry();
    let scoped = registry
        .grammars
        .get(&file.language)
        .is_some_and(|g| g.locals.is_some());
    let edits;
    let mut unresolved = Vec::new();
    let mut mentions = Vec::new();
    if scoped {
        if request.overrides.textual {
            return Err(Error::Usage {
                cause: format!(
                    "{} has scopes; --textual is for tier 1 languages",
                    file.language
                ),
                hint: "drop --textual".to_owned(),
            });
        }
        if constructors_named {
            add_constructors(file, definitions);
        }
        edits = scoped_edits(file, definitions, &request.new_name);
        if visible_elsewhere(file, definitions) {
            unresolved = unresolved_sites(ctx, project, snapshot, file, old_name)?;
        }
        mentions = mention_sites(ctx, file, old_name, &edits);
    } else {
        if !request.overrides.textual {
            return Err(refused(
                "tier_too_low",
                format!(
                    "{} has no scopes in codix (tier 1): references cannot be told from other uses of the name",
                    file.language
                ),
                "pass --textual to rename every whole-word match of the name in its file",
            ));
        }
        edits = word_edits(&file.content, old_name, &request.new_name);
    }
    Ok(Sites {
        edits,
        unresolved,
        mentions,
    })
}

fn description(ctx: &Context, file: &Analyzed, definition: usize, new_name: &str) -> String {
    let d = &file.model.definitions[definition];
    match &d.defines {
        Defines::Symbol(i) => {
            let s = &file.model.symbols[*i];
            format!("rename {} {} to {new_name}", s.kind, s.qualified_name)
        }
        Defines::Local { kind } => {
            let at = d.name_range.start;
            let location = ctx.location(&file.id, at.line, at.column);
            format!(
                "rename {kind} {} at {}:{}:{} to {new_name}",
                d.name, location.path, location.line, location.column
            )
        }
    }
}

fn symbol_of(file: &Analyzed, d: usize) -> Option<&crate::store::SymbolRow> {
    match file.model.definitions[d].defines {
        Defines::Symbol(i) => Some(&file.model.symbols[i]),
        Defines::Local { .. } => None,
    }
}

/// A constructor named after its type means the type: renaming it renames
/// the type and every constructor.
fn to_type(file: &Analyzed, definitions: &mut Vec<usize>) {
    if let Some(constructor) = symbol_of(file, definitions[0]).filter(|s| s.kind == "constructor")
        && let Some(parent) = &constructor.parent
        && let Some(ty) = (0..file.model.definitions.len())
            .find(|&d| symbol_of(file, d).is_some_and(|s| &s.symbol_id == parent))
    {
        *definitions = vec![ty];
    }
}

/// A type renamed renames the constructors named after it.
fn add_constructors(file: &Analyzed, definitions: &mut Vec<usize>) {
    let Some(ty) = symbol_of(file, definitions[0]) else {
        return;
    };
    let constructors: Vec<usize> = (0..file.model.definitions.len())
        .filter(|&d| {
            symbol_of(file, d).is_some_and(|s| {
                s.kind == "constructor"
                    && s.name == ty.name
                    && s.parent.as_deref() == Some(ty.symbol_id.as_str())
            })
        })
        .collect();
    definitions.extend(constructors);
    definitions.sort_unstable();
    definitions.dedup();
}

/// The names of the definitions and of the references bound to them.
fn scoped_edits(file: &Analyzed, definitions: &[usize], new_name: &str) -> Vec<Edit> {
    let model = &file.model;
    let mut edits: Vec<Edit> = definitions
        .iter()
        .map(|&d| &model.definitions[d].name_range)
        .chain(
            model
                .references
                .iter()
                .filter(|r| matches!(r.binding, Binding::Bound(d) if definitions.contains(&d)))
                .map(|r| &r.range),
        )
        .map(|range| Edit {
            start: range.start.byte as usize,
            end: range.end.byte as usize,
            replacement: new_name.to_owned(),
        })
        .collect();
    edits.sort_by_key(|e| e.start);
    edits.dedup();
    edits
}

/// Whether other files may refer to the definitions: symbols that are not
/// private. Locals never are.
fn visible_elsewhere(file: &Analyzed, definitions: &[usize]) -> bool {
    definitions
        .iter()
        .any(|&d| match file.model.definitions[d].defines {
            Defines::Symbol(i) => file.model.symbols[i].visibility.as_deref() != Some("private"),
            Defines::Local { .. } => false,
        })
}

/// Lines of files, read once each.
struct Lines<'a> {
    project: &'a Project,
    files: HashMap<String, Vec<String>>,
}

impl Lines<'_> {
    fn line(&mut self, path: &str, line: u32) -> String {
        let project = self.project;
        let lines = self.files.entry(path.to_owned()).or_insert_with(|| {
            fs::read_to_string(project.path_of(&FileId::from_canonical(path)))
                .map(|text| text.lines().map(str::to_owned).collect())
                .unwrap_or_default()
        });
        lines
            .get((line as usize).saturating_sub(1))
            .map(|l| l.trim().to_owned())
            .unwrap_or_default()
    }
}

/// References that may mean the definitions but that codix cannot bind:
/// through another object in their file, and by name in other files.
fn unresolved_sites(
    ctx: &Context,
    project: &Project,
    snapshot: &Snapshot<'_>,
    file: &Analyzed,
    name: &str,
) -> Result<Vec<Site>, Error> {
    let mut lines = Lines {
        project,
        files: HashMap::new(),
    };
    let mut sites = Vec::new();
    for r in &file.model.references {
        let unbound = matches!(r.binding, Binding::Free | Binding::Uncertain);
        if r.name == name
            && unbound
            && (r.receiver != Receiver::None || r.binding == Binding::Uncertain)
        {
            let at = r.range.start;
            sites.push(Site {
                location: ctx.location(&file.id, at.line, at.column),
                note: lines.line(file.id.as_str(), at.line),
            });
        }
    }
    for r in snapshot.refs_named(name)? {
        if r.path == file.id.as_str() || r.state == RefState::Bound {
            continue;
        }
        sites.push(Site {
            location: ctx.location(
                &FileId::from_canonical(r.path.as_str()),
                r.start.line,
                r.start.column,
            ),
            note: lines.line(&r.path, r.start.line),
        });
    }
    // Files without scopes: every whole-word match.
    let registry = codix_provider::registry();
    for row in snapshot.files()? {
        let tier_1 = registry
            .grammars
            .get(&row.language)
            .is_some_and(|g| g.locals.is_none());
        if !tier_1 || row.status != FileStatus::Indexed || row.path == file.id.as_str() {
            continue;
        }
        let id = FileId::from_canonical(row.path.as_str());
        let Ok(text) = fs::read_to_string(project.path_of(&id)) else {
            continue;
        };
        for (number, line) in text.lines().enumerate() {
            for start in words(line, name) {
                let column = line[..start].chars().count() + 1;
                sites.push(Site {
                    location: ctx.location(
                        &id,
                        u32::try_from(number + 1).unwrap_or(u32::MAX),
                        u32::try_from(column).unwrap_or(u32::MAX),
                    ),
                    note: line.trim().to_owned(),
                });
            }
        }
    }
    Ok(sites)
}

/// Byte offsets of `name` in `text` as a whole word: not preceded or
/// followed by a character names are made of.
fn words(text: &str, name: &str) -> Vec<usize> {
    let part = |c: char| c.is_alphanumeric() || c == '_' || c == '$';
    text.match_indices(name)
        .filter(|(i, _)| {
            !text[..*i].chars().next_back().is_some_and(part)
                && !text[i + name.len()..].chars().next().is_some_and(part)
        })
        .map(|(i, _)| i)
        .collect()
}

/// Whole-word matches of the old name in the file that are no name codix
/// knows: in comments, strings and macro bodies. Listed, never edited.
fn mention_sites(ctx: &Context, file: &Analyzed, name: &str, edits: &[Edit]) -> Vec<Site> {
    let model = &file.model;
    let known = |start: usize| {
        edits.iter().any(|e| e.start == start)
            || model
                .definitions
                .iter()
                .any(|d| d.name_range.start.byte as usize == start)
            || model
                .references
                .iter()
                .any(|r| r.range.start.byte as usize == start)
    };
    let mut sites = Vec::new();
    let mut offset = 0;
    for (number, line) in file.content.split_inclusive('\n').enumerate() {
        for start in words(line, name) {
            if known(offset + start) {
                continue;
            }
            let column = line[..start].chars().count() + 1;
            sites.push(Site {
                location: ctx.location(
                    &file.id,
                    u32::try_from(number + 1).unwrap_or(u32::MAX),
                    u32::try_from(column).unwrap_or(u32::MAX),
                ),
                note: line.trim().to_owned(),
            });
        }
        offset += line.len();
    }
    sites
}

/// Every whole-word match of the old name, for `--textual` at tier 1.
fn word_edits(content: &str, old: &str, new: &str) -> Vec<Edit> {
    words(content, old)
        .into_iter()
        .map(|start| Edit {
            start,
            end: start + old.len(),
            replacement: new.to_owned(),
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn whole_words_include_names_made_of_dollar_signs() {
        assert_eq!(words("total subtotal total_x total", "total"), [0, 23]);
        assert_eq!(words("$el, $elements, a$el", "$el"), [0]);
        assert_eq!(words("é total", "total"), [3]);
    }
}
