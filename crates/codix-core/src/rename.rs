//! `rename` within a file: the definition and the references its file's
//! scopes bind to it, checked before anything is stored. References codix
//! cannot bind, in other files or through other objects, refuse the plan
//! unless `--exclude-unresolved` leaves them unchanged.

use std::collections::{HashMap, HashSet};
use std::fs;

use serde::{Deserialize, Serialize};

use crate::Error;
use crate::config::Config;
use crate::edit::{
    Edit, EditSet, ExcludedSite, Exclusion, OverrideUse, PreviewResponse, diff, plan_file,
};
use crate::error::Site;
use crate::op::{Context, Operation};
use crate::preflight::{Overrides, Rename, check_rename};
use crate::project::{FileId, Project, ScopeRequest};
use crate::query::{complete, indexed};
use crate::refs::{Analyzed, Target, target};
use crate::render::since_epoch;
use crate::scope::{Binding, Defines, Receiver};
use crate::store::{FileStatus, PreviewRow, RefState, Resolved, Snapshot, Store, SymbolHit};
use crate::symbol_id::{DECLARATION, same_function};

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
    /// Every file edited, as planning read it: the file first, then those
    /// holding the declarations of the same C function.
    pub contents: Vec<(FileId, String)>,
    /// What planning answers, without an ID yet.
    pub response: PreviewResponse,
    /// The target as stored with the preview, to plan it again: a symbol's
    /// ID, or a local's root-relative `file:line:col`.
    pub target: String,
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
            request: serde_json::to_string(&RenameRequest {
                symbol: plan.target.clone(),
                ..request
            })
            .map_err(|e| Error::Internal {
                message: format!("cannot store the rename request: {e}"),
            })?,
            files: plan
                .edits
                .files
                .iter()
                .map(|f| plan_file(&f.path, content_of(&plan.contents, &f.path), &f.edits))
                .collect(),
            edits: plan.edits,
            response: plan.response,
        };
        store.insert_preview(&mut row)?;
        Ok(row.response)
    }
}

/// The content planning read for `path`.
pub(crate) fn content_of<'a>(contents: &'a [(FileId, String)], path: &str) -> &'a str {
    contents
        .iter()
        .find(|(id, _)| id.as_str() == path)
        .map_or("", |(_, content)| content.as_str())
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
    check_names(&file, &old_name, &request.new_name)?;
    // A C prototype is the function it declares, not an overload of it.
    let overloads = definitions
        .iter()
        .filter(|&&d| symbol_of(&file, d).is_none_or(|s| s.kind != DECLARATION))
        .count();
    if overloads > 1 && !request.all_overloads {
        return Err(refused(
            "tier_too_low",
            format!(
                "`{old_name}` has {overloads} overloads, and references to one cannot be told from references to the others without types"
            ),
            "pass --all-overloads to rename them together",
        ));
    }
    let others = same_function_sites(
        project,
        snapshot,
        &file,
        &mut definitions,
        &request.new_name,
    )?;
    let Sites {
        mut edits,
        bound,
        unresolved,
        mentions,
    } = sites(ctx, project, snapshot, request, &file, &mut definitions)?;
    let mut others = others;
    add_bound(
        project,
        &file,
        bound,
        &request.new_name,
        &mut edits,
        &mut others,
    )?;
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
        .map(|s| excluded_site(s, Exclusion::Unresolved))
        .chain(
            mentions
                .into_iter()
                .map(|s| excluded_site(s, Exclusion::Mention)),
        )
        .collect();
    if symbol_of(&file, definitions[0]).is_some() {
        let elsewhere = elsewhere_sites(
            ctx,
            project,
            snapshot,
            &file,
            definitions[0],
            &others,
            &excluded,
        )?;
        excluded.extend(elsewhere);
    }
    let mut overrides = overrides;
    overrides.extend(check_others(ctx, project, config, &others, request)?);
    let description = description(ctx, &file, definitions[0], &request.new_name);
    let target = crate::refs::symbol_result(ctx, &file, definitions[0], "scoped").symbol_id;
    assemble(
        &file,
        edits,
        others,
        description,
        overrides,
        excluded,
        target,
    )
}

/// The plan of edits in `file` and in `others`.
fn assemble(
    file: &Analyzed,
    edits: Vec<Edit>,
    others: Vec<(FileId, String, Vec<Edit>)>,
    description: String,
    overrides: Vec<OverrideUse>,
    excluded: Vec<ExcludedSite>,
    target: String,
) -> Result<Plan, Error> {
    let mut text = diff(file.id.as_str(), &file.content, &edits);
    for (id, content, edits) in &others {
        text.push_str(&diff(id.as_str(), content, edits));
    }
    let response = PreviewResponse {
        id: String::new(),
        description,
        files: 1 + others.len(),
        sites: edits.len() + others.iter().map(|(_, _, e)| e.len()).sum::<usize>(),
        overrides,
        excluded,
        ambiguous: Vec::new(),
        diff: text,
    };
    let mut contents = vec![(file.id.clone(), file.content.clone())];
    let mut all: Vec<(String, Edit)> = edits
        .into_iter()
        .map(|e| (file.id.as_str().to_owned(), e))
        .collect();
    for (id, content, edits) in others {
        all.extend(edits.into_iter().map(|e| (id.as_str().to_owned(), e)));
        contents.push((id, content));
    }
    Ok(Plan {
        edits: EditSet::new(all)?,
        contents,
        response,
        target,
    })
}

/// The declarations of the same C or C++ function as the renamed one: those
/// in its file join `definitions`, so their uses are renamed too; those in
/// other files are edited where their name is, file by file.
fn same_function_sites(
    project: &Project,
    snapshot: &Snapshot<'_>,
    file: &Analyzed,
    definitions: &mut Vec<usize>,
    new_name: &str,
) -> Result<Vec<(FileId, String, Vec<Edit>)>, Error> {
    let Some(symbol) = symbol_of(file, definitions[0]) else {
        return Ok(Vec::new());
    };
    let Some(hit) = snapshot
        .symbols_with_id_prefix(&symbol.symbol_id)?
        .into_iter()
        .find(|h| h.path == file.id.as_str() && h.symbol.symbol_id == symbol.symbol_id)
    else {
        return Ok(Vec::new());
    };
    let mut others: Vec<(FileId, String, Vec<Edit>)> = Vec::new();
    for member in same_function(snapshot, &hit)? {
        if member.path == file.id.as_str() {
            let d = file.model.definitions.iter().position(|d| {
                matches!(d.defines, Defines::Symbol(i)
                    if file.model.symbols[i].symbol_id == member.symbol.symbol_id)
            });
            if let Some(d) = d
                && !definitions.contains(&d)
            {
                definitions.push(d);
            }
            continue;
        }
        let id = FileId::from_canonical(member.path.as_str());
        if !others.iter().any(|(other, _, _)| *other == id) {
            let path = project.path_of(&id);
            let content = fs::read_to_string(&path).map_err(|source| Error::Io { path, source })?;
            others.push((id.clone(), content, Vec::new()));
        }
        let (_, content, edits) = others
            .iter_mut()
            .find(|(other, _, _)| *other == id)
            .ok_or_else(|| Error::Internal {
                message: "a file read for a rename went missing".to_owned(),
            })?;
        let range = member.symbol.name_range;
        let (start, end) = (range.start.byte as usize, range.end.byte as usize);
        // The index describes the file as it was scanned; check it still is.
        if content.get(start..end) == Some(member.symbol.name.as_str()) {
            edits.push(Edit {
                start,
                end,
                replacement: new_name.to_owned(),
            });
        }
    }
    others.retain(|(_, _, edits)| !edits.is_empty());
    for (_, _, edits) in &mut others {
        edits.sort_by_key(|e| e.start);
    }
    definitions.sort_unstable();
    Ok(others)
}

/// What a rename touches.
struct Sites {
    /// The sites it edits.
    edits: Vec<Edit>,
    /// The references a resolver bound to it, by path and bytes: in its file
    /// and in others.
    bound: Vec<(String, usize, usize)>,
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
    let mut bound = Vec::new();
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
        if definitions
            .iter()
            .any(|&d| matches!(file.model.definitions[d].defines, Defines::Symbol(_)))
        {
            let elsewhere = visible_elsewhere(file, definitions);
            unresolved = unresolved_sites(
                ctx,
                project,
                snapshot,
                file,
                definitions,
                elsewhere,
                &mut bound,
            )?;
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
        bound,
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
/// through another object or from a class that may inherit the name in their
/// file, and, when `elsewhere`, by name in other files.
fn unresolved_sites(
    ctx: &Context,
    project: &Project,
    snapshot: &Snapshot<'_>,
    file: &Analyzed,
    definitions: &[usize],
    elsewhere: bool,
    bound: &mut Vec<(String, usize, usize)>,
) -> Result<Vec<Site>, Error> {
    let mut lines = Lines {
        project,
        files: HashMap::new(),
    };
    let name = file.model.definitions[definitions[0]].name.as_str();
    let ids = crate::refs::with_definitions(
        snapshot,
        &definitions
            .iter()
            .filter_map(|&d| symbol_of(file, d).map(|s| s.symbol_id.clone()))
            .collect(),
    )?;
    let resolved: HashMap<u32, Resolved> = snapshot
        .refs_in(file.id.as_str())?
        .into_iter()
        .filter_map(|r| Some((r.start.byte, r.resolved?)))
        .collect();
    let mut sites = Vec::new();
    for r in &file.model.references {
        let unbound = matches!(r.binding, Binding::Free | Binding::Uncertain);
        if r.name == name
            && unbound
            && (r.receiver != Receiver::None || r.binding == Binding::Uncertain)
        {
            let at = r.range.start;
            match resolved.get(&at.byte) {
                Some(Resolved::Symbol(id)) if ids.contains(id) => {
                    bound.push((
                        file.id.as_str().to_owned(),
                        at.byte as usize,
                        r.range.end.byte as usize,
                    ));
                }
                Some(_) => {}
                None => sites.push(Site {
                    location: ctx.location(&file.id, at.line, at.column),
                    note: lines.line(file.id.as_str(), at.line),
                }),
            }
        }
    }
    if !elsewhere {
        return Ok(sites);
    }
    for r in snapshot.refs_named(name)? {
        if r.path == file.id.as_str() || r.state == RefState::Bound {
            continue;
        }
        match &r.resolved {
            // A resolver bound it to the symbol: a site like any other.
            Some(Resolved::Symbol(id)) if ids.contains(id) => {
                bound.push((r.path.clone(), r.start.byte as usize, r.end_byte as usize));
                continue;
            }
            Some(_) => continue,
            None => {}
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

/// The mentions of the definition's name in other files, but those already
/// `excluded` and the edits `others` make.
fn elsewhere_sites(
    ctx: &Context,
    project: &Project,
    snapshot: &Snapshot<'_>,
    file: &Analyzed,
    definition: usize,
    others: &[(FileId, String, Vec<Edit>)],
    excluded: &[ExcludedSite],
) -> Result<Vec<ExcludedSite>, Error> {
    let listed: HashSet<(String, u32, u32)> = excluded.iter().map(|e| place(&e.location)).collect();
    let edited: HashMap<&str, Vec<usize>> = others
        .iter()
        .map(|(id, _, edits)| (id.as_str(), edits.iter().map(|e| e.start).collect()))
        .collect();
    let name = &file.model.definitions[definition].name;
    Ok(
        mentions_elsewhere(ctx, project, snapshot, file, name, &edited)?
            .into_iter()
            .filter(|s| !listed.contains(&place(&s.location)))
            .map(|s| excluded_site(s, Exclusion::MentionElsewhere))
            .collect(),
    )
}

/// Refuses to rename a keyword, or to the name it already has.
fn check_names(file: &Analyzed, old_name: &str, new_name: &str) -> Result<(), Error> {
    let keyword = codix_provider::registry()
        .languages
        .iter()
        .find(|d| d.name == file.language)
        .is_some_and(|pack| pack.names.keywords.iter().any(|k| k == old_name));
    if keyword {
        // Such as a Kotlin secondary constructor, declared by its keyword.
        return Err(refused(
            "invalid_name",
            format!(
                "`{old_name}` is a {} keyword, not a name to rename",
                file.language
            ),
            "rename what it belongs to instead",
        ));
    }
    if new_name == old_name {
        return Err(Error::Usage {
            cause: format!("`{old_name}` already has that name"),
            hint: "pass the new name".to_owned(),
        });
    }
    Ok(())
}

/// The checks of the other files a rename edits.
fn check_others(
    ctx: &Context,
    project: &Project,
    config: &Config,
    others: &[(FileId, String, Vec<Edit>)],
    request: &RenameRequest,
) -> Result<Vec<OverrideUse>, Error> {
    let mut used = Vec::new();
    for (id, _, list) in others {
        if let Some(other) = crate::refs::analyze(project, config, id)? {
            used.extend(crate::preflight::check_other_file(
                ctx,
                project,
                config,
                &other,
                list,
                &request.new_name,
                &request.overrides,
            )?);
        }
    }
    Ok(used)
}

/// Adds the references a resolver bound to the symbol to the edits: of its
/// file, or of the other file they are in.
fn add_bound(
    project: &Project,
    file: &Analyzed,
    bound: Vec<(String, usize, usize)>,
    new_name: &str,
    edits: &mut Vec<Edit>,
    others: &mut Vec<(FileId, String, Vec<Edit>)>,
) -> Result<(), Error> {
    for (path, start, end) in bound {
        let edit = Edit {
            start,
            end,
            replacement: new_name.to_owned(),
        };
        if path == file.id.as_str() {
            edits.push(edit);
            continue;
        }
        if let Some((_, _, list)) = others.iter_mut().find(|(id, _, _)| id.as_str() == path) {
            list.push(edit);
            continue;
        }
        let id = FileId::from_canonical(path);
        let full = project.path_of(&id);
        let content =
            fs::read_to_string(&full).map_err(|source| Error::Io { path: full, source })?;
        others.push((id, content, vec![edit]));
    }
    edits.sort_by_key(|e| e.start);
    edits.dedup_by_key(|e| e.start);
    for (_, _, list) in others.iter_mut() {
        list.sort_by_key(|e| e.start);
        list.dedup_by_key(|e| e.start);
    }
    others.sort_by(|a, b| a.0.as_str().cmp(b.0.as_str()));
    Ok(())
}

fn excluded_site(site: Site, reason: Exclusion) -> ExcludedSite {
    ExcludedSite {
        location: site.location,
        reason,
        text: site.note,
    }
}

fn place(location: &crate::page::Location) -> (String, u32, u32) {
    (
        location.root_path.as_str().to_owned(),
        location.line,
        location.column,
    )
}

/// Whole-word mentions of the name in the other indexed files: in comments
/// and strings of code with scopes, anywhere in other files, but at the
/// edits other files get (`edited`, by path and first byte). Files are read
/// in parallel.
fn mentions_elsewhere(
    ctx: &Context,
    project: &Project,
    snapshot: &Snapshot<'_>,
    file: &Analyzed,
    name: &str,
    edited: &HashMap<&str, Vec<usize>>,
) -> Result<Vec<Site>, Error> {
    use rayon::prelude::*;
    let rows: Vec<_> = snapshot
        .files()?
        .into_iter()
        .filter(|row| {
            matches!(row.status, FileStatus::Indexed | FileStatus::SearchOnly)
                && row.path != file.id.as_str()
        })
        .collect();
    let registry = codix_provider::registry();
    let mut sites: Vec<Site> = rows
        .par_iter()
        .flat_map_iter(|row| {
            let id = FileId::from_canonical(row.path.as_str());
            let Ok(text) = fs::read_to_string(project.path_of(&id)) else {
                return Vec::new();
            };
            let mut starts = words(&text, name);
            if let Some(skip) = edited.get(row.path.as_str()) {
                starts.retain(|s| !skip.contains(s));
            }
            if starts.is_empty() {
                return Vec::new();
            }
            // In code with scopes, only comments and strings mention it:
            // its other uses are references.
            let scoped = registry
                .languages
                .iter()
                .find(|d| d.name == row.language)
                .zip(registry.grammars.get(&row.language))
                .filter(|(_, g)| g.locals.is_some());
            if let Some((descriptor, grammar)) = scoped {
                let Ok(tree) = crate::parse::parse(descriptor, grammar, &row.path, &text) else {
                    return Vec::new();
                };
                starts.retain(|&s| in_comment_or_string(&tree, s, s + name.len()));
            }
            starts
                .into_iter()
                .map(|start| {
                    let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
                    let line_end = text[start..].find('\n').map_or(text.len(), |i| start + i);
                    let number = text[..start].matches('\n').count() + 1;
                    let column = text[line_start..start].chars().count() + 1;
                    Site {
                        location: ctx.location(
                            &id,
                            u32::try_from(number).unwrap_or(u32::MAX),
                            u32::try_from(column).unwrap_or(u32::MAX),
                        ),
                        note: text[line_start..line_end].trim().to_owned(),
                    }
                })
                .collect()
        })
        .collect();
    sites.sort_by_key(|s| place(&s.location));
    Ok(sites)
}

/// Whether the bytes `start..end` lie in a comment or a string.
fn in_comment_or_string(tree: &tree_sitter::Tree, start: usize, end: usize) -> bool {
    let mut node = tree.root_node().descendant_for_byte_range(start, end);
    while let Some(n) = node {
        let kind = n.kind();
        if kind.contains("comment") || kind.contains("string") {
            return true;
        }
        node = n.parent();
    }
    false
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
