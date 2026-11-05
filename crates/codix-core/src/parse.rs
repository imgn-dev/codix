//! Tier 1 extraction: parse a file with its pack's grammar and turn the
//! definitions its `tags` query captures into symbols.

use std::cell::RefCell;
use std::collections::HashMap;
use std::sync::{Arc, Mutex, OnceLock, PoisonError};

use codix_provider::{Grammar, LanguageDescriptor, registry};
use streaming_iterator::StreamingIterator;
use tree_sitter::{Language, Node, Parser, Query, QueryCursor};

use crate::Error;
use crate::config::Config;
use crate::detect::Detector;
use crate::project::FileId;
use crate::scan::{Indexed, Indexer};
use crate::store::{Confidence, FileStatus, Point, Span, SymbolRow};
use crate::symbol_id;
use crate::walk::{ContentClass, classify};

/// Longest signature kept, in characters.
const MAX_SIGNATURE: usize = 300;

thread_local! {
    // One parser per thread per language, reused across files.
    static PARSERS: RefCell<HashMap<String, Parser>> = RefCell::new(HashMap::new());
}

fn queries() -> &'static Mutex<HashMap<String, Arc<Query>>> {
    static QUERIES: OnceLock<Mutex<HashMap<String, Arc<Query>>>> = OnceLock::new();
    QUERIES.get_or_init(|| Mutex::new(HashMap::new()))
}

/// The pack's `tags` query, compiled once per process.
fn tags_query(name: &str, grammar: &Grammar) -> Result<Arc<Query>, Error> {
    let mut cache = queries().lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(query) = cache.get(name) {
        return Ok(Arc::clone(query));
    }
    let query = Query::new(&Language::new(grammar.language), grammar.tags).map_err(|e| {
        Error::Internal {
            message: format!("the {name} tags query does not compile: {e}"),
        }
    })?;
    let query = Arc::new(query);
    cache.insert(name.to_owned(), Arc::clone(&query));
    Ok(query)
}

/// A definition captured by the `tags` query.
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
    pattern: usize,
}

/// Extracts the symbols of one file.
pub fn extract(
    descriptor: &LanguageDescriptor,
    grammar: &Grammar,
    path: &str,
    content: &str,
) -> Result<Vec<SymbolRow>, Error> {
    let tree = PARSERS.with(|parsers| {
        let mut parsers = parsers.borrow_mut();
        if !parsers.contains_key(&descriptor.name) {
            let mut parser = Parser::new();
            parser
                .set_language(&Language::new(grammar.language))
                .map_err(|e| Error::Internal {
                    message: format!("the {} grammar does not load: {e}", descriptor.name),
                })?;
            parsers.insert(descriptor.name.clone(), parser);
        }
        let parser = parsers
            .get_mut(&descriptor.name)
            .ok_or_else(|| Error::Internal {
                message: format!("no {} parser", descriptor.name),
            })?;
        parser.parse(content, None).ok_or_else(|| Error::Internal {
            message: format!("{path}: the {} parser gave up", descriptor.name),
        })
    })?;
    let query = tags_query(&descriptor.name, grammar)?;
    let definitions = definitions(descriptor, &query, tree.root_node(), content);
    let (mut rows, parents) = symbols(descriptor, path, content, &definitions);
    symbol_id::disambiguate(&mut rows, &descriptor.id_prefix, path);
    // Parents by position, now that every ID is final.
    let ids: Vec<String> = rows.iter().map(|r| r.symbol_id.clone()).collect();
    for (row, parent) in rows.iter_mut().zip(parents) {
        row.parent = parent.map(|p| ids[p].clone());
    }
    Ok(collapse_duplicates(rows))
}

/// One symbol per ID: a C prototype and its definition in the same file are
/// the same symbol, kept as the larger of the two.
fn collapse_duplicates(rows: Vec<SymbolRow>) -> Vec<SymbolRow> {
    let size = |row: &SymbolRow| row.range.end.byte - row.range.start.byte;
    let mut kept: HashMap<String, usize> = HashMap::new();
    let mut out: Vec<SymbolRow> = Vec::with_capacity(rows.len());
    for row in rows {
        match kept.get(&row.symbol_id) {
            Some(&i) if size(&out[i]) >= size(&row) => {}
            Some(&i) => out[i] = row,
            None => {
                kept.insert(row.symbol_id.clone(), out.len());
                out.push(row);
            }
        }
    }
    out.sort_by_key(|row| {
        (
            row.range.start.byte,
            std::cmp::Reverse(row.range.end.byte),
            row.name_range.start.byte,
        )
    });
    out
}

fn definitions<'tree>(
    descriptor: &LanguageDescriptor,
    query: &Query,
    root: Node<'tree>,
    content: &str,
) -> Vec<Definition<'tree>> {
    let names = query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(query, root, content.as_bytes());
    let mut found = Vec::new();
    while let Some(m) = matches.next() {
        // One match may name several symbols, as `const a, b = 1, 2` does.
        let mut declared = Vec::new();
        let mut definition = None;
        let mut parents = Vec::new();
        for capture in m.captures() {
            let capture_name = names[capture.index as usize];
            if capture_name == "name" {
                declared.push(capture.node);
            } else if capture_name == "parent" {
                parents.push(capture.node);
            } else if let Some(kind) = capture_name.strip_prefix("definition.") {
                definition = Some((capture.node, kind));
            }
        }
        if let Some((node, kind)) = definition
            && let Some(kind) = descriptor.symbol_kind(kind)
        {
            for name in declared {
                found.push(Definition {
                    node,
                    name,
                    kind: kind.to_owned(),
                    parents: parents.clone(),
                    pattern: m.pattern_index,
                });
            }
        }
    }
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
    found
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

fn span(content: &str, node: Node<'_>) -> Span {
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
    versions: HashMap<String, String>,
}

impl PackIndexer {
    /// An indexer for a project's configuration.
    pub fn new(config: &Config) -> Self {
        let registry = registry();
        let versions = registry
            .languages
            .iter()
            .map(|d| {
                let version = match registry.grammars.get(&d.name) {
                    Some(grammar) => {
                        format!("{}-{}-{}", d.name, d.version, grammar_fingerprint(grammar))
                    }
                    None => format!("{}-{}", d.name, d.version),
                };
                (d.name.clone(), version)
            })
            .collect();
        Self {
            detector: Detector::new(config),
            versions,
        }
    }
}

/// Changes whenever the grammar or the query does: a hash of the query text
/// and of what the grammar exposes about itself.
fn grammar_fingerprint(grammar: &Grammar) -> String {
    let language = Language::new(grammar.language);
    let mut hasher = blake3::Hasher::new();
    hasher.update(grammar.tags.as_bytes());
    hasher.update(&language.abi_version().to_le_bytes());
    hasher.update(&language.node_kind_count().to_le_bytes());
    hasher.update(&language.field_count().to_le_bytes());
    if let Some(m) = language.metadata() {
        hasher.update(&[m.major_version, m.minor_version, m.patch_version]);
    }
    hasher.finalize().to_hex()[..8].to_owned()
}

impl Indexer for PackIndexer {
    fn parser_version(&self, language: &str) -> String {
        self.versions
            .get(language)
            .cloned()
            .unwrap_or_else(|| "text-1".to_owned())
    }

    fn index(&self, id: &FileId, content: &[u8]) -> Indexed {
        let detection = self.detector.detect(id.as_str(), content);
        let search_only = |status| Indexed {
            language: detection.language.clone(),
            status,
            symbols: Vec::new(),
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
        match extract(descriptor, grammar, id.as_str(), text) {
            Ok(symbols) => Indexed {
                language: detection.language,
                status: FileStatus::Indexed,
                symbols,
            },
            Err(_) => search_only(FileStatus::Error),
        }
    }
}
