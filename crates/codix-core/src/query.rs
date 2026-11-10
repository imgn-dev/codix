//! The query commands: `find`, `def`, `outline`, `show` and `grep`. They read
//! the index, keeping it fresh first: lookups recheck the files they return,
//! and `grep`, which must not miss a change anywhere, scans first.

use std::collections::{BTreeMap, HashSet};
use std::fs;

use regex::Regex;
use serde::{Deserialize, Serialize};

use crate::Error;
use crate::config::Config;
use crate::lock::QUERY_WAIT;
use crate::op::{Context, Operation, Outcome};
use crate::page::{Location, Page, Paged, Ranked, sort_results};
use crate::parse::PackIndexer;
use crate::project::{FileId, Project, ScopeRequest};
use crate::refs::DefAt;
use crate::scan::{rescan, scan_within};
use crate::store::{FileStatus, MatchMode, NameQuery, Snapshot, Store, SymbolHit};
use crate::symbol_id::{SymbolRef, resolve_overloads_where, resolve_where};

/// Longest line text shown with a `grep` match, in characters.
const MAX_LINE: usize = 200;

/// A symbol in query results.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct SymbolResult {
    /// Where its name is.
    #[serde(flatten)]
    pub location: Location,
    /// Symbol ID, accepted by every command naming a symbol.
    pub symbol_id: String,
    /// Kind from the shared vocabulary.
    pub kind: String,
    /// Simple name.
    pub name: String,
    /// Qualified name within the file.
    pub qualified_name: String,
    /// Declaration without the body, when captured.
    pub signature: Option<String>,
    /// Language of the file.
    pub language: String,
    /// How it was found: `scoped` when the name at a location binds to it
    /// within its file, `textual` when found by name, `low` in or around a
    /// parse error.
    pub confidence: String,
}

impl Ranked for SymbolResult {
    fn is_definition(&self) -> bool {
        true
    }

    fn location(&self) -> &Location {
        &self.location
    }
}

impl SymbolResult {
    fn new(ctx: &Context, hit: SymbolHit) -> Self {
        let start = hit.symbol.name_range.start;
        Self {
            location: ctx.location(&FileId::from_canonical(hit.path), start.line, start.column),
            symbol_id: hit.symbol.symbol_id,
            kind: hit.symbol.kind,
            name: hit.symbol.name,
            qualified_name: hit.symbol.qualified_name,
            signature: hit.symbol.signature,
            language: hit.language,
            confidence: match hit.symbol.confidence {
                crate::store::Confidence::Normal => "textual".to_owned(),
                crate::store::Confidence::Low => "low".to_owned(),
            },
        }
    }
}

/// A symbol ID that codix renamed, followed to the symbol's ID now.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Renamed {
    /// The ID as given.
    pub from: String,
    /// The ID the symbol has now.
    pub to: String,
}

/// Runs a query on `input`; when it names a symbol codix renamed, runs it
/// again on the symbol's ID now, and says so.
pub(crate) fn following<T>(
    input: &str,
    query: impl Fn(&str) -> Result<T, Error>,
) -> Result<(T, Option<Renamed>), Error> {
    match query(input) {
        Err(Error::SymbolRenamed { old, new }) => {
            Ok((query(&new)?, Some(Renamed { from: old, to: new })))
        }
        other => other.map(|answer| (answer, None)),
    }
}

/// The project, indexed, with its configuration.
pub(crate) fn indexed(ctx: &Context) -> Result<(&Project, Config), Error> {
    let project = ctx.project()?;
    project.require_indexed()?;
    Ok((project, Config::load(project)?))
}

/// Runs a lookup, rechecks the files it returned, and when any had changed,
/// reindexes them and runs it again. The flag is set when the write lock
/// stayed busy, so changed files could not be reindexed.
fn lookup<T>(
    ctx: &Context,
    project: &Project,
    config: &Config,
    query: impl Fn(&Snapshot<'_>) -> Result<(T, Vec<String>), Error>,
) -> Result<(T, bool), Error> {
    let mut store = Store::open(&project.index_path())?;
    let (first, paths) = query(&store.snapshot()?)?;
    match rescan(
        project,
        &PackIndexer::new(config),
        ctx.gate(),
        &paths,
        QUERY_WAIT,
    )? {
        None => Ok((first, true)),
        Some(r) if r.indexed + r.touched + r.deleted + r.pending == 0 => Ok((first, false)),
        Some(_) => Ok((query(&store.snapshot()?)?.0, false)),
    }
}

/// Brings the whole index up to date before a query that must not miss a
/// change anywhere: a daemon whose watcher covers every directory has it
/// current once its pending changes are in; otherwise the whole tree is
/// scanned. Whether the write lock stayed busy, so changes may be missed.
pub(crate) fn complete(ctx: &Context, project: &Project, config: &Config) -> Result<bool, Error> {
    let watched = ctx.watch().is_some_and(crate::daemon::Watch::flush);
    Ok(!watched
        && scan_within(
            project,
            config,
            &PackIndexer::new(config),
            ctx.gate(),
            Some(QUERY_WAIT),
        )?
        .is_none())
}

fn distinct(paths: impl Iterator<Item = String>) -> Vec<String> {
    let mut seen = HashSet::new();
    paths.filter(|p| seen.insert(p.clone())).collect()
}

/// `find`: symbols by name.
pub struct FindOp;

/// Request for [`FindOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FindRequest {
    /// Name to look for.
    pub query: String,
    /// How to match: `exact`, `prefix` and `fuzzy` ignore case, `glob` matches
    /// the qualified name; by default exact, then prefix, then fuzzy.
    #[serde(default)]
    pub mode: Option<MatchMode>,
    /// Keep symbols of this kind, such as method; repeatable.
    #[serde(default, rename = "kind")]
    pub kinds: Vec<String>,
    /// Keep symbols of this language; repeatable.
    #[serde(default, rename = "lang")]
    pub languages: Vec<String>,
    /// Directories to keep.
    #[serde(flatten)]
    pub scope: ScopeRequest,
    /// Page to return.
    #[serde(flatten)]
    pub page: Page,
}

/// What [`FindOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct FindResponse {
    /// The mode that matched, or the last one tried.
    pub mode: MatchMode,
    /// Matching symbols.
    pub results: Paged<SymbolResult>,
    /// Whether another process held the write lock, so changed files could
    /// not be reindexed first.
    pub possibly_stale: bool,
}

impl Outcome for FindResponse {
    fn has_results(&self) -> bool {
        self.results.has_results()
    }
}

impl Operation for FindOp {
    const NAME: &'static str = "find";
    type Request = FindRequest;
    type Response = FindResponse;

    fn run(&self, ctx: &Context, request: FindRequest) -> Result<FindResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let scope = ctx.scope(&request.scope)?;
        let modes = match request.mode {
            Some(mode) => vec![mode],
            None => vec![MatchMode::Exact, MatchMode::Prefix, MatchMode::Fuzzy],
        };
        let keep = |hit: &SymbolHit| {
            scope.contains(&FileId::from_canonical(hit.path.as_str()))
                && (request.kinds.is_empty() || request.kinds.contains(&hit.symbol.kind))
                && (request.languages.is_empty() || request.languages.contains(&hit.language))
        };
        let ((mode, results), possibly_stale) = lookup(ctx, project, &config, |snapshot| {
            let mut last = (modes[modes.len() - 1], Vec::new());
            for &mode in &modes {
                let hits: Vec<SymbolHit> = snapshot
                    .find(&NameQuery {
                        text: request.query.clone(),
                        mode,
                    })?
                    .into_iter()
                    .filter(|hit| keep(hit))
                    .collect();
                if !hits.is_empty() {
                    last = (mode, hits);
                    break;
                }
            }
            let (mode, hits) = last;
            let mut results: Vec<SymbolResult> = hits
                .into_iter()
                .map(|hit| SymbolResult::new(ctx, hit))
                .collect();
            if mode != MatchMode::Fuzzy {
                sort_results(&mut results, None);
            }
            // Only the page is returned, so only its files need to be current.
            let page = request.page.apply(results);
            let paths = distinct(
                page.items
                    .iter()
                    .map(|r| r.location.root_path.as_str().to_owned()),
            );
            Ok(((mode, page), paths))
        })?;
        Ok(FindResponse {
            mode,
            results,
            possibly_stale,
        })
    }
}

/// `def`: where a symbol is defined.
pub struct DefOp;

/// Request for [`DefOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DefRequest {
    /// Symbol ID, its path part, `file:line:col`, or a name.
    pub symbol: String,
    /// Consider symbols of this kind only; repeatable.
    #[serde(default, rename = "kind")]
    pub kinds: Vec<String>,
    /// Directories to consider, narrowing an ambiguous name.
    #[serde(flatten)]
    pub scope: ScopeRequest,
}

/// What [`DefOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct DefResponse {
    /// The definition; for overloads, the first one in its file.
    pub definition: SymbolResult,
    /// The other overloads of the same name in the same file.
    #[serde(default)]
    pub overloads: Vec<SymbolResult>,
    /// See [`FindResponse::possibly_stale`].
    pub possibly_stale: bool,
    /// When the ID given was the symbol's before codix renamed it.
    #[serde(default)]
    pub renamed: Option<Renamed>,
}

impl Outcome for DefResponse {
    fn has_results(&self) -> bool {
        true
    }
}

impl Operation for DefOp {
    const NAME: &'static str = "def";
    type Request = DefRequest;
    type Response = DefResponse;

    fn run(&self, ctx: &Context, request: DefRequest) -> Result<DefResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let scope = ctx.scope(&request.scope)?;
        // At a location, the name there decides, bound by its file's scopes;
        // a name the file does not bind is looked up by name.
        let mut name = request.symbol.clone();
        if let SymbolRef::Location { file, line, column } = SymbolRef::parse(&request.symbol)? {
            match crate::refs::def_at(ctx, project, &config, &file, line, column)? {
                Some(DefAt::Found(mut results)) => {
                    let definition = results.remove(0);
                    return Ok(DefResponse {
                        definition,
                        overloads: results,
                        possibly_stale: false,
                        renamed: None,
                    });
                }
                Some(DefAt::ByName(unbound)) => name = unbound,
                None => {}
            }
        }
        let ((hits, possibly_stale), renamed) = following(&name, |name| {
            lookup(ctx, project, &config, |snapshot| {
                let hits = resolve_overloads_where(snapshot, project, ctx.cwd(), name, &|hit| {
                    (request.kinds.is_empty() || request.kinds.contains(&hit.symbol.kind))
                        && scope.contains(&FileId::from_canonical(hit.path.as_str()))
                })?;
                let paths = distinct(hits.iter().map(|h| h.path.clone()));
                Ok((hits, paths))
            })
        })?;
        let mut results = hits.into_iter().map(|hit| SymbolResult::new(ctx, hit));
        let definition = results.next().ok_or_else(|| Error::Internal {
            message: "a lookup answered no symbol".to_owned(),
        })?;
        Ok(DefResponse {
            definition,
            overloads: results.collect(),
            possibly_stale,
            renamed,
        })
    }
}

/// `outline`: a file's or directory's symbol tree, signatures only.
pub struct OutlineOp;

/// Request for [`OutlineOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutlineRequest {
    /// File or directory.
    pub path: String,
}

/// One symbol in an outline, with the symbols it contains.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutlineNode {
    /// The symbol.
    #[serde(flatten)]
    pub symbol: SymbolResult,
    /// Symbols defined inside it.
    pub children: Vec<OutlineNode>,
}

/// The outline of one file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutlineFile {
    /// Path relative to the cwd.
    pub path: String,
    /// Root-relative path.
    pub root_path: FileId,
    /// Language of the file.
    pub language: String,
    /// Top-level symbols, in source order.
    pub symbols: Vec<OutlineNode>,
}

/// What [`OutlineOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct OutlineResponse {
    /// Files with symbols, sorted by path.
    pub files: Vec<OutlineFile>,
    /// See [`FindResponse::possibly_stale`].
    pub possibly_stale: bool,
}

impl Outcome for OutlineResponse {
    fn has_results(&self) -> bool {
        !self.files.is_empty()
    }
}

impl Operation for OutlineOp {
    const NAME: &'static str = "outline";
    type Request = OutlineRequest;
    type Response = OutlineResponse;

    fn run(&self, ctx: &Context, request: OutlineRequest) -> Result<OutlineResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let target = project.file_id(ctx.cwd(), &request.path)?;
        {
            // A file of a language this build leaves out, or whose plugin
            // cannot be used, has no outline here.
            let mut store = Store::open(&project.index_path())?;
            if let Some(row) = store.snapshot()?.file(target.as_str())?
                && row.language != crate::detect::TEXT
                && row.status != FileStatus::Indexed
                && let Err(e @ (Error::NotSupportedInBuild { .. } | Error::PluginInvalid { .. })) =
                    crate::providers::require_language(&row.language, &request.path)
            {
                return Err(e);
            }
        }
        let (files, possibly_stale) = lookup(ctx, project, &config, |snapshot| {
            let paths: Vec<(String, String)> = snapshot
                .files()?
                .into_iter()
                .filter(|row| FileId::from_canonical(row.path.as_str()).is_within(&target))
                .filter(|row| row.status == FileStatus::Indexed)
                .map(|row| (row.path, row.language))
                .collect();
            let mut files = Vec::new();
            for (path, language) in &paths {
                let hits = snapshot.symbols_in(path)?;
                if hits.is_empty() {
                    continue;
                }
                files.push((path.clone(), language.clone(), hits));
            }
            let returned = paths.into_iter().map(|(p, _)| p).collect();
            Ok((files, returned))
        })?;
        let files = files
            .into_iter()
            .map(|(path, language, hits)| {
                let id = FileId::from_canonical(path);
                OutlineFile {
                    path: ctx.location(&id, 1, 1).path,
                    root_path: id,
                    language,
                    symbols: tree(ctx, hits),
                }
            })
            .collect();
        Ok(OutlineResponse {
            files,
            possibly_stale,
        })
    }
}

/// Symbols by parent ID, each with its position in source order.
type Children = BTreeMap<Option<String>, Vec<(usize, SymbolHit)>>;

/// Nests symbols under their parents, keeping source order.
fn tree(ctx: &Context, hits: Vec<SymbolHit>) -> Vec<OutlineNode> {
    let mut children = Children::new();
    for (i, hit) in hits.into_iter().enumerate() {
        children
            .entry(hit.symbol.parent.clone())
            .or_default()
            .push((i, hit));
    }
    level(ctx, None, &mut children)
}

fn level(ctx: &Context, parent: Option<&str>, children: &mut Children) -> Vec<OutlineNode> {
    let Some(mut nodes) = children.remove(&parent.map(str::to_owned)) else {
        return Vec::new();
    };
    nodes.sort_by_key(|(i, _)| *i);
    nodes
        .into_iter()
        .map(|(_, hit)| {
            let id = hit.symbol.symbol_id.clone();
            OutlineNode {
                symbol: SymbolResult::new(ctx, hit),
                children: level(ctx, Some(&id), children),
            }
        })
        .collect()
}

/// `show`: a symbol's source, optionally with lines around it.
pub struct ShowOp;

/// Request for [`ShowOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShowRequest {
    /// Symbol ID, its path part, `file:line:col`, or a name.
    pub symbol: String,
    /// Consider symbols of this kind only; repeatable.
    #[serde(default, rename = "kind")]
    pub kinds: Vec<String>,
    /// Directories to consider, narrowing an ambiguous name.
    #[serde(flatten)]
    pub scope: ScopeRequest,
    /// Lines shown before and after the symbol.
    #[serde(default)]
    pub context: usize,
}

/// What [`ShowOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct ShowResponse {
    /// The symbol.
    pub symbol: SymbolResult,
    /// 1-based number of the first line in `lines`.
    pub first_line: u32,
    /// The symbol's lines, with the context asked for.
    pub lines: Vec<String>,
    /// See [`FindResponse::possibly_stale`].
    pub possibly_stale: bool,
    /// When the ID given was the symbol's before codix renamed it.
    #[serde(default)]
    pub renamed: Option<Renamed>,
}

impl Outcome for ShowResponse {
    fn has_results(&self) -> bool {
        true
    }
}

impl Operation for ShowOp {
    const NAME: &'static str = "show";
    type Request = ShowRequest;
    type Response = ShowResponse;

    fn run(&self, ctx: &Context, request: ShowRequest) -> Result<ShowResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let scope = ctx.scope(&request.scope)?;
        let ((hit, possibly_stale), renamed) = following(&request.symbol, |input| {
            lookup(ctx, project, &config, |snapshot| {
                let hit = resolve_where(snapshot, project, ctx.cwd(), input, &|hit| {
                    (request.kinds.is_empty() || request.kinds.contains(&hit.symbol.kind))
                        && scope.contains(&FileId::from_canonical(hit.path.as_str()))
                })?;
                let path = hit.path.clone();
                Ok((hit, vec![path]))
            })
        })?;
        let path = project.path_of(&FileId::from_canonical(hit.path.as_str()));
        let text = fs::read_to_string(&path).map_err(|source| Error::Io { path, source })?;
        let all: Vec<&str> = text.lines().collect();
        let range = hit.symbol.range;
        let first = (range.start.line as usize)
            .saturating_sub(request.context)
            .max(1);
        // A range ending at column 1 stops before that line.
        let end_line = if range.end.column == 1 && range.end.line > range.start.line {
            range.end.line - 1
        } else {
            range.end.line
        } as usize;
        let last = (end_line + request.context).min(all.len());
        let lines = all
            .get(first - 1..last)
            .unwrap_or_default()
            .iter()
            .map(|l| (*l).to_owned())
            .collect();
        Ok(ShowResponse {
            symbol: SymbolResult::new(ctx, hit),
            first_line: u32::try_from(first).unwrap_or(u32::MAX),
            lines,
            possibly_stale,
            renamed,
        })
    }
}

/// `grep`: a regular expression over the indexed files, with the enclosing
/// symbol of each match.
pub struct GrepOp;

/// Request for [`GrepOp`].
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GrepRequest {
    /// Regular expression, matched line by line.
    pub pattern: String,
    /// Search files of this language; repeatable.
    #[serde(default, rename = "lang")]
    pub languages: Vec<String>,
    /// Keep matches inside a symbol of this kind; repeatable.
    #[serde(default, rename = "kind")]
    pub kinds: Vec<String>,
    /// Directories to search.
    #[serde(flatten)]
    pub scope: ScopeRequest,
    /// Page to return.
    #[serde(flatten)]
    pub page: Page,
}

/// One match.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GrepMatch {
    /// Where the match starts.
    #[serde(flatten)]
    pub location: Location,
    /// The line, cut to 200 characters.
    pub text: String,
    /// Qualified name of the innermost symbol around the match.
    pub enclosing: Option<String>,
    /// Kind of that symbol.
    pub enclosing_kind: Option<String>,
}

impl Ranked for GrepMatch {
    fn is_definition(&self) -> bool {
        false
    }

    fn location(&self) -> &Location {
        &self.location
    }
}

/// What [`GrepOp`] answers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct GrepResponse {
    /// Matches, by path and position.
    pub matches: Paged<GrepMatch>,
    /// Whether another process held the write lock, so the scan before the
    /// search could not run.
    pub possibly_stale: bool,
}

impl Outcome for GrepResponse {
    fn has_results(&self) -> bool {
        self.matches.has_results()
    }
}

impl Operation for GrepOp {
    const NAME: &'static str = "grep";
    type Request = GrepRequest;
    type Response = GrepResponse;

    fn run(&self, ctx: &Context, request: GrepRequest) -> Result<GrepResponse, Error> {
        let (project, config) = indexed(ctx)?;
        let regex = Regex::new(&request.pattern).map_err(|e| Error::Usage {
            cause: format!(
                "invalid regular expression: {}",
                e.to_string().lines().last().unwrap_or_default()
            ),
            hint: "grep takes Rust regex syntax; escape special characters with \\".to_owned(),
        })?;
        let scope = ctx.scope(&request.scope)?;
        let possibly_stale = complete(ctx, project, &config)?;
        let mut store = Store::open(&project.index_path())?;
        let snapshot = store.snapshot()?;
        let mut matches = Vec::new();
        for row in snapshot.files()? {
            let id = FileId::from_canonical(row.path.as_str());
            let searchable = matches!(row.status, FileStatus::Indexed | FileStatus::SearchOnly);
            if !searchable
                || !scope.contains(&id)
                || !(request.languages.is_empty() || request.languages.contains(&row.language))
            {
                continue;
            }
            let Ok(text) = fs::read_to_string(project.path_of(&id)) else {
                continue;
            };
            let found = search(ctx, &id, &text, &regex);
            if found.is_empty() {
                continue;
            }
            let symbols = snapshot.symbols_in(&row.path)?;
            for (mut m, byte) in found {
                if let Some(enclosing) = enclosing(&symbols, byte) {
                    m.enclosing = Some(enclosing.symbol.qualified_name.clone());
                    m.enclosing_kind = Some(enclosing.symbol.kind.clone());
                }
                if request.kinds.is_empty()
                    || m.enclosing_kind
                        .as_ref()
                        .is_some_and(|k| request.kinds.contains(k))
                {
                    matches.push(m);
                }
            }
        }
        sort_results(&mut matches, None);
        Ok(GrepResponse {
            matches: request.page.apply(matches),
            possibly_stale,
        })
    }
}

/// Every match in `text`, with its byte offset.
fn search(ctx: &Context, id: &FileId, text: &str, regex: &Regex) -> Vec<(GrepMatch, usize)> {
    let mut found = Vec::new();
    let mut offset = 0;
    for (number, line) in text.split_inclusive('\n').enumerate() {
        let content = line.trim_end_matches(['\n', '\r']);
        for m in regex.find_iter(content) {
            let column = content[..m.start()].chars().count() + 1;
            let shown: String = content.chars().take(MAX_LINE).collect();
            found.push((
                GrepMatch {
                    location: ctx.location(
                        id,
                        u32::try_from(number + 1).unwrap_or(u32::MAX),
                        u32::try_from(column).unwrap_or(u32::MAX),
                    ),
                    text: shown,
                    enclosing: None,
                    enclosing_kind: None,
                },
                offset + m.start(),
            ));
        }
        offset += line.len();
    }
    found
}

/// The innermost symbol whose range holds `byte`.
fn enclosing(symbols: &[SymbolHit], byte: usize) -> Option<&SymbolHit> {
    symbols
        .iter()
        .filter(|hit| {
            let r = hit.symbol.range;
            (r.start.byte as usize) <= byte && byte < r.end.byte as usize
        })
        .min_by_key(|hit| hit.symbol.range.end.byte - hit.symbol.range.start.byte)
}
