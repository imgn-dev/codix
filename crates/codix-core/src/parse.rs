//! Tier 1 extraction: parse a file with its pack's grammar and turn the
//! definitions its `tags` query captures into symbols.
//!
//! Besides `@definition.<kind>`, `@name` and `@parent`, a `tags` query may
//! capture, in a definition's match, its declared type (`@type`) and the
//! types it extends or implements (`@supertype`, in as many matches as
//! needed); and, in matches of their own, the file's package (`@package`),
//! its imports (`@import`, with `@import.alias` for the name it is imported
//! under, `(#set! import.static)` and `(#set! import.wildcard)`) and its
//! includes (`@include`, `(#set! include.system)` for `<…>`).

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::OnceLock;

use codix_provider::{Grammar, LanguageDescriptor, PluginError, registry};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor, Tree};

use crate::Error;
use crate::config::Config;
use crate::detect::Detector;
use crate::project::FileId;
use crate::scan::{Indexed, Indexer};
use crate::store::{Confidence, FileStatus, ImportKind, ImportRow, Point, Span, SymbolRow};
use crate::symbol_id;
use crate::walk::{ContentClass, classify};

/// Longest signature kept, in characters.
const MAX_SIGNATURE: usize = 300;

thread_local! {
    // One parser per thread per grammar, reused across files.
    static PARSERS: RefCell<HashMap<Language, Parser>> = RefCell::new(HashMap::new());
}

/// A pack's grammar, loaded on first use.
fn language<'g>(name: &str, grammar: &'g Grammar) -> Result<&'g Language, Error> {
    grammar.language().map_err(unusable(name))
}

/// The error for a pack whose grammar or queries cannot be used.
pub(crate) fn unusable(name: &str) -> impl Fn(PluginError) -> Error + '_ {
    move |e| Error::PluginInvalid {
        provider: name.to_owned(),
        cause: e.to_string(),
    }
}

/// A definition captured by the `tags` query. `@parent` names the enclosing
/// type of a definition written outside it, such as a Go method's receiver,
/// the type of a Rust `impl` block, or the scopes of a C++ `geo::Shape::area`.
struct Definition<'tree> {
    node: Node<'tree>,
    name: Node<'tree>,
    kind: String,
    /// `@parent` captures, outermost first, such as `geo` and `Shape` in
    /// `geo::Shape::area`.
    parents: Vec<Node<'tree>>,
    /// `@type`.
    declared_type: Option<Node<'tree>>,
    pattern: usize,
}

/// Extracts the symbols of one file.
pub fn extract(
    descriptor: &LanguageDescriptor,
    grammar: &Grammar,
    path: &str,
    content: &str,
) -> Result<Vec<SymbolRow>, Error> {
    let tree = parse(descriptor, grammar, path, content)?;
    Ok(symbols_of(descriptor, grammar, path, content, &tree)?.0)
}

/// Parses one file with its pack's grammar.
pub(crate) fn parse(
    descriptor: &LanguageDescriptor,
    grammar: &Grammar,
    path: &str,
    content: &str,
) -> Result<Tree, Error> {
    let language = language(&descriptor.name, grammar)?;
    PARSERS.with(|parsers| {
        let mut parsers = parsers.borrow_mut();
        if !parsers.contains_key(language) {
            let mut parser = Parser::new();
            parser.set_language(language).map_err(|e| Error::Internal {
                message: format!("the {} grammar does not load: {e}", descriptor.name),
            })?;
            parsers.insert(language.clone(), parser);
        }
        let parser = parsers.get_mut(language).ok_or_else(|| Error::Internal {
            message: format!("no {} parser", descriptor.name),
        })?;
        parser.parse(content, None).ok_or_else(|| Error::Internal {
            message: format!("{path}: the {} parser gave up", descriptor.name),
        })
    })
}

/// The symbols of a parsed file, and its package, imports and includes.
pub(crate) fn symbols_of(
    descriptor: &LanguageDescriptor,
    grammar: &Grammar,
    path: &str,
    content: &str,
    tree: &Tree,
) -> Result<(Vec<SymbolRow>, Vec<ImportRow>), Error> {
    let query = grammar.tags_query().map_err(unusable(&descriptor.name))?;
    let Found {
        definitions,
        supertypes,
        imports,
    } = definitions(descriptor, query, tree.root_node(), content);
    let (mut rows, parents) = symbols(descriptor, path, content, &definitions);
    for (row, definition) in rows.iter_mut().zip(&definitions) {
        if let Some(names) = supertypes.get(&definition.node.id()) {
            row.supertypes.clone_from(names);
        }
    }
    symbol_id::disambiguate(&mut rows, &descriptor.id_prefix, path);
    // Parents by position, now that every ID is final.
    let ids: Vec<String> = rows.iter().map(|r| r.symbol_id.clone()).collect();
    for (row, parent) in rows.iter_mut().zip(parents) {
        row.parent = parent.map(|p| ids[p].clone());
    }
    Ok((rows, imports))
}

/// What the `tags` query found in a file.
struct Found<'tree> {
    definitions: Vec<Definition<'tree>>,
    /// Supertypes by definition node, in file order, from every match.
    supertypes: HashMap<usize, Vec<String>>,
    imports: Vec<ImportRow>,
}

fn definitions<'tree>(
    descriptor: &LanguageDescriptor,
    query: &Query,
    root: Node<'tree>,
    content: &str,
) -> Found<'tree> {
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, content.as_bytes());
    let mut found = Vec::new();
    let mut supertypes: HashMap<usize, Vec<(usize, String)>> = HashMap::new();
    let mut imports: Vec<(usize, ImportRow)> = Vec::new();
    while let Some(m) = matches.next() {
        // One match may name several symbols, as `const a, b = 1, 2` does.
        let mut declared = Vec::new();
        let mut definition = None;
        let mut parents = Vec::new();
        let mut declared_type = None;
        let mut supers = Vec::new();
        let mut alias = None;
        for capture in m.captures() {
            let capture_name = names[capture.index as usize];
            match capture_name {
                "name" => declared.push(capture.node),
                "parent" => parents.push(capture.node),
                "type" => declared_type = Some(capture.node),
                "supertype" => supers.push(capture.node),
                "import.alias" => alias = Some(text(content, capture.node).to_owned()),
                _ => {
                    if let Some(kind) = capture_name.strip_prefix("definition.") {
                        definition = Some((capture.node, kind));
                    }
                }
            }
        }
        imports.extend(imports_of(query, m, content, alias.as_ref()));
        if let Some((node, kind)) = definition
            && let Some(kind) = descriptor.symbol_kind(kind)
        {
            let known = supertypes.entry(node.id()).or_default();
            for s in supers {
                let entry = (s.start_byte(), written_type(text(content, s)));
                if !known.contains(&entry) {
                    known.push(entry);
                }
            }
            for name in declared {
                found.push(Definition {
                    node,
                    name,
                    kind: kind.to_owned(),
                    parents: parents.clone(),
                    declared_type,
                    pattern: m.pattern_index,
                });
            }
        }
    }
    let supertypes = supertypes
        .into_iter()
        .map(|(node, mut names)| {
            names.sort();
            (node, names.into_iter().map(|(_, name)| name).collect())
        })
        .collect();
    // When several patterns capture the same definition, the one written last
    // in the query wins: packs follow upstream's general patterns with more
    // specific ones, such as a function inside a class being a method.
    found.sort_by_key(|d| {
        (
            d.node.start_byte(),
            std::cmp::Reverse(d.node.end_byte()),
            d.name.start_byte(),
            std::cmp::Reverse(d.pattern),
        )
    });
    found.dedup_by_key(|d| (d.node.id(), d.name.id()));
    Found {
        definitions: found,
        supertypes,
        imports: merged(imports),
    }
}

/// The package, imports and includes one match captures, by first byte.
fn imports_of(
    query: &Query,
    m: &tree_sitter::QueryMatch<'_, '_>,
    content: &str,
    alias: Option<&String>,
) -> Vec<(usize, ImportRow)> {
    let names = query.capture_names();
    let settings = query.property_settings(m.pattern_index);
    let set = |key: &str| settings.iter().any(|p| &*p.key == key);
    let mut found = Vec::new();
    for capture in m.captures() {
        let kind = match names[capture.index as usize] {
            "package" => ImportKind::Package,
            "import" => ImportKind::Import,
            "include" => ImportKind::Include,
            _ => continue,
        };
        let node = capture.node;
        let name = match kind {
            ImportKind::Include => text(content, node)
                .trim()
                .trim_matches(['"', '<', '>'])
                .to_owned(),
            _ => written_type(text(content, node)),
        };
        found.push((
            node.start_byte(),
            ImportRow {
                kind,
                name,
                alias: alias.cloned(),
                is_static: set("import.static"),
                wildcard: set("import.wildcard"),
                system: set("include.system"),
                line: u32::try_from(node.start_position().row + 1).unwrap_or(u32::MAX),
            },
        ));
    }
    found
}

/// One row per import, in file order: several patterns may capture one, and
/// the flags of all of them hold.
fn merged(mut imports: Vec<(usize, ImportRow)>) -> Vec<ImportRow> {
    imports.sort_by_key(|(start, i)| (*start, i.kind));
    let mut merged: Vec<(usize, ImportRow)> = Vec::new();
    for (start, import) in imports {
        match merged.last_mut() {
            Some((s, last)) if *s == start && last.kind == import.kind => {
                last.is_static |= import.is_static;
                last.wildcard |= import.wildcard;
                last.system |= import.system;
                if last.alias.is_none() {
                    last.alias = import.alias;
                }
            }
            _ => merged.push((start, import)),
        }
    }
    merged.into_iter().map(|(_, i)| i).collect()
}

/// A type or a name as written, whitespace and type arguments removed:
/// `Map<String, List<T>>` gives `Map`, `java.util . List` gives
/// `java.util.List`.
pub(crate) fn written_type(text: &str) -> String {
    let mut out = String::new();
    let mut depth = 0usize;
    for c in text.chars() {
        match c {
            '<' => depth += 1,
            '>' => depth = depth.saturating_sub(1),
            c if depth == 0 && !c.is_whitespace() => out.push(c),
            _ => {}
        }
    }
    out
}

/// The symbols of the definitions, each with the index of its parent; parent
/// IDs are filled in by the caller.
fn symbols(
    descriptor: &LanguageDescriptor,
    path: &str,
    content: &str,
    definitions: &[Definition<'_>],
) -> (Vec<SymbolRow>, Vec<Option<usize>>) {
    let mut rows: Vec<SymbolRow> = Vec::with_capacity(definitions.len());
    let mut parents: Vec<Option<usize>> = Vec::with_capacity(definitions.len());
    // Indices into `rows` of the definitions enclosing the current one.
    let mut open: Vec<usize> = Vec::new();
    for (i, definition) in definitions.iter().enumerate() {
        let (start, end) = (definition.node.start_byte(), definition.node.end_byte());
        while let Some(&top) = open.last() {
            let enclosing = &definitions[top].node;
            let contains = enclosing.start_byte() <= start
                && end <= enclosing.end_byte()
                && (enclosing.start_byte(), enclosing.end_byte()) != (start, end);
            if contains {
                break;
            }
            open.pop();
        }
        let name = text(content, definition.name).trim();
        let parent_index = open.last().copied();
        let parent = parent_index.map(|p| &rows[p]);
        let separator = &descriptor.symbols.separator;
        let qualified_name = [
            parent.map(|p| p.qualified_name.clone()),
            (!definition.parents.is_empty()).then(|| {
                definition
                    .parents
                    .iter()
                    .map(|p| text(content, *p))
                    .collect::<Vec<_>>()
                    .join(separator)
            }),
            Some(name.to_owned()),
        ]
        .into_iter()
        .flatten()
        .collect::<Vec<_>>()
        .join(separator);
        let signature = signature(descriptor, content, definition);
        let symbol_id = symbol_id::format(
            &descriptor.id_prefix,
            path,
            &qualified_name,
            &definition.kind,
            signature.as_deref(),
        );
        rows.push(SymbolRow {
            symbol_id,
            name: name.to_owned(),
            kind: definition.kind.clone(),
            qualified_name,
            parent: None,
            range: span(content, definition.node),
            name_range: span(content, definition.name),
            signature,
            visibility: visibility(descriptor, content, definition.node),
            confidence: if in_error(descriptor, definition) {
                Confidence::Low
            } else {
                Confidence::Normal
            },
            declared_type: definition
                .declared_type
                .map(|t| written_type(text(content, t))),
            supertypes: Vec::new(),
        });
        parents.push(parent_index);
        open.push(i);
    }
    (rows, parents)
}

fn text<'a>(content: &'a str, node: Node<'_>) -> &'a str {
    content
        .get(node.start_byte()..node.end_byte())
        .unwrap_or_default()
}

/// Where a definition's header ends: at its body, or for a declarator such
/// as one variable of `int a = 1, b;`, at its value.
fn header_end(descriptor: &LanguageDescriptor, definition: &Definition<'_>) -> usize {
    let body_start = |node: Node<'_>| {
        descriptor
            .symbols
            .body_fields
            .iter()
            .find_map(|field| node.child_by_field_name(field))
            .map(|body| body.start_byte())
    };
    let body_node = |node: Node<'_>| {
        let mut cursor = node.walk();
        node.children(&mut cursor)
            .find(|child| {
                descriptor
                    .symbols
                    .body_nodes
                    .iter()
                    .any(|k| k == child.kind())
            })
            .map(|body| body.start_byte())
    };
    body_start(definition.node)
        .or_else(|| declarator(definition).and_then(body_start))
        .or_else(|| body_node(definition.node))
        .or_else(|| body_token(descriptor, definition))
        .unwrap_or(definition.node.end_byte())
}

/// The first body-opening token before the name, when the name follows the
/// body, as in a C `typedef`.
fn body_token_before_name(
    descriptor: &LanguageDescriptor,
    definition: &Definition<'_>,
) -> Option<usize> {
    let tokens = &descriptor.symbols.body_tokens;
    let name_start = definition.name.start_byte();
    let mut cursor = definition.node.walk();
    loop {
        let node = cursor.node();
        if node.start_byte() >= name_start {
            return None;
        }
        if !node.is_named() && tokens.iter().any(|t| t == node.kind()) {
            return Some(node.start_byte());
        }
        if cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return None;
            }
        }
    }
}

/// The first body-opening token after the name, anywhere in the definition.
fn body_token(descriptor: &LanguageDescriptor, definition: &Definition<'_>) -> Option<usize> {
    let tokens = &descriptor.symbols.body_tokens;
    if tokens.is_empty() {
        return None;
    }
    let after = definition.name.end_byte();
    let mut cursor = definition.node.walk();
    let mut found = None;
    'walk: loop {
        let node = cursor.node();
        if node.start_byte() >= after && !node.is_named() && tokens.iter().any(|t| t == node.kind())
        {
            found = Some(node.start_byte());
            break;
        }
        if node.end_byte() > after && cursor.goto_first_child() {
            continue;
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                break 'walk;
            }
        }
    }
    found
}

/// The node declaring the name when the definition declares several, such as
/// a `variable_declarator` in a field declaration.
fn declarator<'tree>(definition: &Definition<'tree>) -> Option<Node<'tree>> {
    definition.name.parent().filter(|p| {
        p.id() != definition.node.id()
            && p.parent().is_some_and(|pp| pp.id() == definition.node.id())
    })
}

/// The definition's header, whitespace collapsed. For one declarator among
/// several, the shared part (`static int`) followed by that declarator.
fn signature(
    descriptor: &LanguageDescriptor,
    content: &str,
    definition: &Definition<'_>,
) -> Option<String> {
    let node = definition.node;
    let end = header_end(descriptor, definition);
    // `typedef struct { … } Name;`: the body comes before the name, so the
    // signature is the text before the body followed by the name.
    if end == node.end_byte()
        && let Some(token) = body_token_before_name(descriptor, definition)
    {
        let before = content.get(node.start_byte()..token)?;
        let collapsed = format!(
            "{} {}",
            before.split_whitespace().collect::<Vec<_>>().join(" "),
            text(content, definition.name)
        );
        return Some(collapsed);
    }
    let raw = match declarator(definition) {
        Some(own) => {
            let mut cursor = node.walk();
            let first = node
                .children(&mut cursor)
                .find(|c| c.kind() == own.kind())?;
            let shared = content.get(node.start_byte()..first.start_byte())?;
            let declared = content.get(own.start_byte()..end.min(own.end_byte()))?;
            format!("{shared}{declared}")
        }
        None => content.get(node.start_byte()..end)?.to_owned(),
    };
    let collapsed = raw.split_whitespace().collect::<Vec<_>>().join(" ");
    let trimmed = collapsed
        .trim_end_matches(['=', '{', ':', ';', ',', ' '])
        .to_owned();
    if trimmed.is_empty() {
        return None;
    }
    Some(if trimmed.chars().count() > MAX_SIGNATURE {
        let mut cut: String = trimmed.chars().take(MAX_SIGNATURE).collect();
        cut.push('…');
        cut
    } else {
        trimmed
    })
}

fn visibility(descriptor: &LanguageDescriptor, content: &str, node: Node<'_>) -> Option<String> {
    let rule = descriptor.symbols.visibility.as_ref()?;
    let mut cursor = node.walk();
    let modifiers = node
        .children(&mut cursor)
        .find(|child| child.kind() == rule.node)?;
    text(content, modifiers)
        .split(|c: char| !c.is_alphanumeric() && c != '_')
        .find(|word| rule.values.iter().any(|v| v == word))
        .map(str::to_owned)
}

/// Inside or around a parse error: the definition lies inside an `ERROR`
/// node, or its header (everything before the body) contains an `ERROR` or
/// `MISSING` node. Errors inside the body leave the definition itself sound.
fn in_error(descriptor: &LanguageDescriptor, definition: &Definition<'_>) -> bool {
    let header_end = header_end(descriptor, definition);
    let mut cursor = definition.node.walk();
    let header_broken = definition.node.is_error()
        || definition
            .node
            .children(&mut cursor)
            .take_while(|child| child.start_byte() < header_end)
            .any(|child| child.has_error());
    if header_broken {
        return true;
    }
    let mut current = definition.node.parent();
    while let Some(ancestor) = current {
        if ancestor.is_error() {
            return true;
        }
        current = ancestor.parent();
    }
    false
}

pub(crate) fn span(content: &str, node: Node<'_>) -> Span {
    Span {
        start: point(content, node.start_byte(), node.start_position()),
        end: point(content, node.end_byte(), node.end_position()),
    }
}

/// tree-sitter counts columns in bytes; codix in Unicode scalar values.
fn point(content: &str, byte: usize, position: tree_sitter::Point) -> Point {
    let line_start = byte - position.column;
    let column = content
        .get(line_start..byte)
        .map_or(position.column, |prefix| prefix.chars().count());
    Point {
        line: u32::try_from(position.row + 1).unwrap_or(u32::MAX),
        column: u32::try_from(column + 1).unwrap_or(u32::MAX),
        byte: u32::try_from(byte).unwrap_or(u32::MAX),
    }
}

/// Detects each file's language and extracts symbols with the compiled-in
/// packs; files of languages without a grammar are searched as text.
#[derive(Debug)]
pub struct PackIndexer {
    detector: Detector,
    /// Parser versions by language, each computed on first use: it loads the
    /// grammar.
    versions: HashMap<String, OnceLock<String>>,
}

impl PackIndexer {
    /// An indexer for a project's configuration.
    pub fn new(config: &Config) -> Self {
        let versions = registry()
            .languages
            .iter()
            .map(|d| (d.name.clone(), OnceLock::new()))
            .collect();
        Self {
            detector: Detector::new(config),
            versions,
        }
    }
}

/// The parser version of a compiled-in language: its name, its pack's
/// version and its grammar's fingerprint. A grammar that does not load gives
/// the version of text, which changes when it loads again.
fn parser_version(language: &str) -> String {
    let registry = registry();
    let Some(descriptor) = registry.languages.iter().find(|d| d.name == language) else {
        return TEXT_VERSION.to_owned();
    };
    match registry.grammars.get(language) {
        Some(grammar) => match grammar_fingerprint(grammar) {
            Some(fingerprint) => format!("{language}-{}-{fingerprint}", descriptor.version),
            None => TEXT_VERSION.to_owned(),
        },
        None => format!("{language}-{}", descriptor.version),
    }
}

/// The parser version of files searched as text.
const TEXT_VERSION: &str = "text-1";

/// Changes whenever the grammar or the query does: a hash of the query text,
/// of what the grammar exposes about itself, and of a plugin's library.
/// `None` when the grammar does not load.
fn grammar_fingerprint(grammar: &Grammar) -> Option<String> {
    let language = grammar.language().ok()?;
    let mut hasher = blake3::Hasher::new();
    if let Some(library) = grammar.library() {
        hasher
            .update_reader(std::fs::File::open(library).ok()?)
            .ok()?;
    }
    hasher.update(grammar.tags.as_bytes());
    if let Some(locals) = &grammar.locals {
        hasher.update(&[0]);
        hasher.update(locals.as_bytes());
    }
    hasher.update(&language.abi_version().to_le_bytes());
    hasher.update(&language.node_kind_count().to_le_bytes());
    hasher.update(&language.field_count().to_le_bytes());
    if let Some(m) = language.metadata() {
        hasher.update(&[m.major_version, m.minor_version, m.patch_version]);
    }
    Some(hasher.finalize().to_hex()[..8].to_owned())
}

impl Indexer for PackIndexer {
    fn parser_version(&self, language: &str) -> String {
        match self.versions.get(language) {
            Some(version) => version.get_or_init(|| parser_version(language)).clone(),
            None => TEXT_VERSION.to_owned(),
        }
    }

    fn index(&self, id: &FileId, content: &[u8]) -> Indexed {
        let detection = self.detector.detect(id.as_str(), content);
        let search_only = |status| Indexed {
            language: detection.language.clone(),
            status,
            symbols: Vec::new(),
            refs: Vec::new(),
            imports: Vec::new(),
        };
        match classify(content) {
            ContentClass::Binary => return search_only(FileStatus::Skipped),
            ContentClass::NonUtf8 | ContentClass::Minified => {
                return search_only(FileStatus::SearchOnly);
            }
            ContentClass::Indexed => {}
        }
        let registry = registry();
        let (Some(descriptor), Some(grammar)) = (
            registry
                .languages
                .iter()
                .find(|d| d.name == detection.language),
            registry.grammars.get(&detection.language),
        ) else {
            return search_only(FileStatus::SearchOnly);
        };
        let Ok(text) = std::str::from_utf8(content) else {
            return search_only(FileStatus::SearchOnly);
        };
        if grammar.language().is_err() {
            return search_only(FileStatus::SearchOnly);
        }
        match crate::scope::model(descriptor, grammar, id.as_str(), text) {
            Ok(model) => Indexed {
                language: detection.language,
                status: FileStatus::Indexed,
                refs: model.stored_refs(),
                imports: model.imports,
                symbols: model.symbols,
            },
            Err(_) => search_only(FileStatus::Error),
        }
    }
}
