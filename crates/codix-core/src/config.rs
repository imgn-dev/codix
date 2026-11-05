//! Configuration in two layers: the shared `codix.toml` and the local
//! `.codix/config.toml`, the local one winning. Every key is defined in
//! [`schema`]; edits keep each file's comments and ordering.

pub mod schema;

use std::collections::BTreeMap;
use std::fs;
use std::io;
use std::path::Path;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use toml_edit::{Array, DocumentMut, Item, Table};

use crate::Error;
use crate::op::{Context, Operation, Outcome};
use crate::project::Project;
use schema::{ConfigFile, KEYS, KeyDef, Kind};

/// A parsed key: its definition and, for a map, the entry.
#[derive(Debug, Clone, Copy)]
pub struct Key<'a> {
    def: &'static KeyDef,
    entry: Option<&'a str>,
}

impl Key<'_> {
    /// The key's definition.
    pub fn def(&self) -> &'static KeyDef {
        self.def
    }

    /// The dotted key, with the map entry quoted when needed.
    pub fn display(&self) -> String {
        display_key(self.def, self.entry)
    }

    fn segments(&self) -> Vec<&str> {
        let mut segments: Vec<&str> = self.def.path.split('.').collect();
        segments.extend(self.entry);
        segments
    }
}

fn display_key(def: &KeyDef, entry: Option<&str>) -> String {
    match entry {
        Some(entry) if is_bare(entry) => format!("{}.{entry}", def.path),
        Some(entry) => format!(
            "{}.\"{}\"",
            def.path,
            entry.replace('\\', "\\\\").replace('"', "\\\"")
        ),
        None => def.path.to_owned(),
    }
}

fn is_bare(segment: &str) -> bool {
    !segment.is_empty()
        && segment
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

/// Splits a dotted key into segments; `"…"` quotes a segment containing dots.
fn split_key(text: &str) -> Option<Vec<String>> {
    let mut segments = Vec::new();
    let mut chars = text.chars().peekable();
    loop {
        let mut segment = String::new();
        if chars.peek() == Some(&'"') {
            chars.next();
            loop {
                match chars.next()? {
                    '"' => break,
                    '\\' => segment.push(chars.next()?),
                    c => segment.push(c),
                }
            }
            if segment.is_empty() {
                return None;
            }
        } else {
            while let Some(&c) = chars.peek() {
                if c == '.' {
                    break;
                }
                segment.push(c);
                chars.next();
            }
            if !is_bare(&segment) {
                return None;
            }
        }
        segments.push(segment);
        match chars.next() {
            None => return Some(segments),
            Some('.') => {}
            Some(_) => return None,
        }
    }
}

fn unknown_key(text: &str) -> Error {
    Error::InvalidConfig {
        cause: format!("unknown key `{text}`"),
        hint: schema::valid_keys(),
    }
}

/// Parses `text` against the schema. A map key without an entry is accepted
/// only when `allow_map` is set.
fn parse_key<'a>(text: &str, segments: &'a [String], allow_map: bool) -> Result<Key<'a>, Error> {
    for def in KEYS {
        let path: Vec<&str> = def.path.split('.').collect();
        if segments.len() < path.len() || segments[..path.len()] != path[..] {
            continue;
        }
        let rest = &segments[path.len()..];
        match (def.map, rest) {
            (false, []) => return Ok(Key { def, entry: None }),
            (true, []) if allow_map => return Ok(Key { def, entry: None }),
            (true, [entry]) => {
                globset::Glob::new(entry).map_err(|e| Error::InvalidConfig {
                    cause: format!("invalid glob `{entry}` in `{text}`: {}", e.kind()),
                    hint: "use gitignore-style globs, such as *.h or src/**/*.inc".to_owned(),
                })?;
                return Ok(Key {
                    def,
                    entry: Some(entry),
                });
            }
            _ => {}
        }
    }
    Err(unknown_key(text))
}

/// A value: one string, or a list for list keys.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(untagged)]
pub enum Value {
    /// A size, duration or language.
    Text(String),
    /// Directories.
    List(Vec<String>),
}

impl Value {
    /// The value as typed on the command line.
    pub fn display(&self) -> String {
        match self {
            Self::Text(text) => text.clone(),
            Self::List(items) => items.join(","),
        }
    }

    fn to_item(&self) -> Item {
        match self {
            Self::Text(text) => toml_edit::value(text.as_str()),
            Self::List(items) => {
                toml_edit::value(items.iter().map(String::as_str).collect::<Array>())
            }
        }
    }
}

/// Validates a value typed on the command line.
fn parse_value(kind: Kind, text: &str) -> Option<Value> {
    let text = text.trim();
    match kind {
        Kind::Size => schema::parse_size(text).map(|_| Value::Text(text.to_owned())),
        Kind::Duration => schema::parse_duration(text).map(|_| Value::Text(text.to_owned())),
        Kind::Language => codix_provider::known::LANGUAGES
            .contains(&text)
            .then(|| Value::Text(text.to_owned())),
        Kind::PathList => {
            let paths: Vec<String> = text
                .split(',')
                .map(str::trim)
                .filter(|p| !p.is_empty())
                .map(|p| p.trim_end_matches('/').to_owned())
                .collect();
            (!paths.is_empty() && paths.iter().all(|p| is_root_relative(p)))
                .then_some(Value::List(paths))
        }
    }
}

fn is_root_relative(path: &str) -> bool {
    !path.is_empty()
        && !path.starts_with('/')
        && !path.contains('\\')
        && !path.contains(':')
        && path.split('/').all(|part| !part.is_empty() && part != "..")
}

/// Reads a value from a config file.
fn value_from_item(kind: Kind, item: &Item) -> Option<Value> {
    match (kind, item) {
        (Kind::Size, item) if item.as_integer().is_some() => {
            let bytes = item.as_integer()?;
            (bytes >= 0).then(|| Value::Text(bytes.to_string()))
        }
        (Kind::PathList, item) => {
            let paths: Option<Vec<&str>> = item.as_array()?.iter().map(|v| v.as_str()).collect();
            parse_value(kind, &paths?.join(","))
        }
        (_, item) => parse_value(kind, item.as_str()?),
    }
}

fn invalid_value(key: &str, text: &str, kind: Kind) -> Error {
    Error::InvalidConfig {
        cause: format!("invalid value `{text}` for {key}"),
        hint: format!("expected a {}", kind.describe()),
    }
}

/// Where an effective value comes from.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum Origin {
    /// The schema's default.
    Default,
    /// `codix.toml`.
    Shared,
    /// `.codix/config.toml`.
    Local,
}

impl Origin {
    /// The origin as shown by `--show-origin`.
    pub fn display(self) -> &'static str {
        match self {
            Self::Default => "default",
            Self::Shared => ConfigFile::Shared.path(),
            Self::Local => ConfigFile::Local.path(),
        }
    }
}

/// One effective setting.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct Setting {
    /// Dotted key.
    pub key: String,
    /// Effective value.
    pub value: Value,
    /// Where the value comes from.
    pub origin: Origin,
}

/// A value read from a file or the schema, with the key it belongs to.
#[derive(Debug, Clone)]
struct Stored {
    def: &'static KeyDef,
    entry: Option<String>,
    value: Value,
    origin: Origin,
}

/// The effective configuration of a project.
#[derive(Debug, Clone)]
pub struct Config {
    settings: BTreeMap<String, Stored>,
}

impl Config {
    /// Loads both files, validating every key and value.
    pub fn load(project: &Project) -> Result<Self, Error> {
        let mut settings = BTreeMap::new();
        for def in KEYS {
            if let (false, Some(default)) = (def.map, def.default) {
                settings.insert(
                    def.path.to_owned(),
                    Stored {
                        def,
                        entry: None,
                        value: Value::Text(default.to_owned()),
                        origin: Origin::Default,
                    },
                );
            }
        }
        for (file, origin) in [
            (ConfigFile::Shared, Origin::Shared),
            (ConfigFile::Local, Origin::Local),
        ] {
            let Some(doc) = read_document(project, file)?.1 else {
                continue;
            };
            for stored in settings_in(&doc, file, origin)? {
                settings.insert(display_key(stored.def, stored.entry.as_deref()), stored);
            }
        }
        Ok(Self { settings })
    }

    /// Every effective setting, sorted by key.
    pub fn settings(&self) -> Vec<Setting> {
        self.settings
            .iter()
            .map(|(key, stored)| Setting {
                key: key.clone(),
                value: stored.value.clone(),
                origin: stored.origin,
            })
            .collect()
    }

    fn text(&self, path: &str) -> Option<&str> {
        match self.settings.get(path).map(|stored| &stored.value) {
            Some(Value::Text(text)) => Some(text),
            _ => None,
        }
    }

    /// `index.max_file_size`, in bytes.
    pub fn max_file_size(&self) -> u64 {
        self.text("index.max_file_size")
            .and_then(schema::parse_size)
            .unwrap_or(2 << 20)
    }

    /// `daemon.idle_timeout`.
    pub fn idle_timeout(&self) -> Duration {
        self.text("daemon.idle_timeout")
            .and_then(schema::parse_duration)
            .unwrap_or(Duration::from_mins(30))
    }

    /// `index.scan_interval`.
    pub fn scan_interval(&self) -> Duration {
        self.text("index.scan_interval")
            .and_then(schema::parse_duration)
            .unwrap_or(Duration::from_secs(600))
    }

    /// `languages.overrides`: (glob, language) pairs, sorted by glob.
    pub fn language_overrides(&self) -> Vec<(String, String)> {
        self.settings
            .values()
            .filter(|stored| stored.def.path == "languages.overrides")
            .filter_map(|stored| match &stored.value {
                Value::Text(language) => Some((stored.entry.clone()?, language.clone())),
                Value::List(_) => None,
            })
            .collect()
    }

    /// `sources.roots`, empty when not set.
    pub fn source_roots(&self) -> Vec<String> {
        match self
            .settings
            .get("sources.roots")
            .map(|stored| &stored.value)
        {
            Some(Value::List(roots)) => roots.clone(),
            _ => Vec::new(),
        }
    }
}

/// Reads and parses one config file; `None` when it does not exist.
fn read_document(
    project: &Project,
    file: ConfigFile,
) -> Result<(String, Option<DocumentMut>), Error> {
    let path = project.root().join(file.path());
    let text = match fs::read_to_string(&path) {
        Ok(text) => text,
        Err(e) if e.kind() == io::ErrorKind::NotFound => return Ok((String::new(), None)),
        Err(source) => return Err(Error::Io { path, source }),
    };
    let doc = text
        .parse::<DocumentMut>()
        .map_err(|e| Error::InvalidConfig {
            cause: format!(
                "{}: invalid TOML: {}",
                file.path(),
                e.message().lines().next().unwrap_or_default()
            ),
            hint: format!("fix the syntax of {}", file.path()),
        })?;
    Ok((text, Some(doc)))
}

/// Every setting in a document, refusing unknown keys and invalid values.
fn settings_in(doc: &DocumentMut, file: ConfigFile, origin: Origin) -> Result<Vec<Stored>, Error> {
    let mut leaves = Vec::new();
    collect_leaves(doc.as_table(), &mut Vec::new(), &mut leaves);
    let mut settings = Vec::new();
    for (segments, item) in &leaves {
        let text = segments
            .iter()
            .map(|s| {
                if is_bare(s) {
                    s.clone()
                } else {
                    format!("\"{s}\"")
                }
            })
            .collect::<Vec<_>>()
            .join(".");
        let key = parse_key(&text, segments, false).map_err(|e| match e {
            Error::InvalidConfig { cause, hint } => Error::InvalidConfig {
                cause: format!("{}: {cause}", file.path()),
                hint,
            },
            other => other,
        })?;
        let value = value_from_item(key.def.kind, item).ok_or_else(|| Error::InvalidConfig {
            cause: format!(
                "{}: invalid value `{}` for {}",
                file.path(),
                item.to_string().trim(),
                key.display()
            ),
            hint: format!("expected a {}", key.def.kind.describe()),
        })?;
        settings.push(Stored {
            def: key.def,
            entry: key.entry.map(str::to_owned),
            value,
            origin,
        });
    }
    Ok(settings)
}

fn collect_leaves(
    table: &dyn toml_edit::TableLike,
    prefix: &mut Vec<String>,
    out: &mut Vec<(Vec<String>, Item)>,
) {
    for (name, item) in table.iter() {
        prefix.push(name.to_owned());
        match item.as_table_like() {
            Some(inner) => collect_leaves(inner, prefix, out),
            None => out.push((prefix.clone(), item.clone())),
        }
        prefix.pop();
    }
}

/// Writes `text` to `path` through a temporary file and a rename.
fn write_atomically(path: &Path, text: &str) -> Result<(), Error> {
    let io_error = |source| Error::Io {
        path: path.to_path_buf(),
        source,
    };
    let temp = path.with_extension("toml.codix-tmp");
    fs::write(&temp, text).map_err(io_error)?;
    fs::rename(&temp, path).map_err(io_error)
}

/// Removes the item at `segments`, then any table left empty by the removal,
/// unless comments sit above its header.
fn remove_path(table: &mut Table, segments: &[&str]) -> bool {
    match segments {
        [] => false,
        [last] => table.remove(last).is_some(),
        [first, rest @ ..] => {
            let Some(inner) = table.get_mut(first).and_then(Item::as_table_mut) else {
                return false;
            };
            let removed = remove_path(inner, rest);
            if removed && inner.is_empty() && !has_comment(inner) {
                table.remove(first);
            }
            removed
        }
    }
}

fn has_comment(table: &Table) -> bool {
    table
        .decor()
        .prefix()
        .and_then(|prefix| prefix.as_str())
        .is_some_and(|prefix| prefix.contains('#'))
}

/// The `config` operation: read and edit both configuration layers.
pub struct ConfigOp;

/// What `config` does.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum ConfigRequest {
    /// Print the effective value of a key.
    Get {
        /// Dotted key.
        key: String,
    },
    /// Set a key in its own file.
    Set {
        /// Dotted key.
        key: String,
        /// Value as typed.
        value: String,
    },
    /// Remove a key from both files, back to its default.
    Unset {
        /// Dotted key.
        key: String,
    },
    /// Every effective value.
    List,
    /// Meaning, type, default and file of one key or of every key.
    Describe {
        /// Dotted key; every key when absent.
        key: Option<String>,
    },
}

/// A key's documentation.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, schemars::JsonSchema)]
pub struct KeyDescription {
    /// Dotted key; maps show a `"<glob>"` entry.
    pub key: String,
    /// What the key means.
    pub description: String,
    /// Type and accepted values.
    #[serde(rename = "type")]
    pub kind: String,
    /// Default value, if any.
    pub default: Option<String>,
    /// Owning file.
    pub file: String,
}

/// What `config` answers.
#[derive(Debug, Clone, Serialize, Deserialize, schemars::JsonSchema)]
#[serde(tag = "form", rename_all = "snake_case")]
pub enum ConfigResponse {
    /// Effective values of the key: one, or every entry of a map.
    Get {
        /// Whether the key is a whole map, answered with all its entries.
        map: bool,
        /// The settings found; empty when the key has no value.
        settings: Vec<Setting>,
    },
    /// The key was written.
    Set {
        /// Dotted key.
        key: String,
        /// Stored value.
        value: Value,
        /// File written.
        file: String,
        /// What the change implies.
        effect: String,
    },
    /// The key was removed, or was not set.
    Unset {
        /// Dotted key.
        key: String,
        /// Files it was removed from; empty when it was not set.
        files: Vec<String>,
        /// What the change implies; absent when nothing changed.
        effect: Option<String>,
    },
    /// Every effective value.
    List {
        /// Sorted by key.
        settings: Vec<Setting>,
    },
    /// Key documentation.
    Describe {
        /// Sorted by key.
        keys: Vec<KeyDescription>,
    },
}

impl Outcome for ConfigResponse {
    fn has_results(&self) -> bool {
        match self {
            Self::Get { settings, .. } => !settings.is_empty(),
            Self::Unset { files, .. } => !files.is_empty(),
            Self::Set { .. } | Self::List { .. } | Self::Describe { .. } => true,
        }
    }
}

impl Operation for ConfigOp {
    const NAME: &'static str = "config";
    type Request = ConfigRequest;
    type Response = ConfigResponse;

    fn run(&self, ctx: &Context, request: ConfigRequest) -> Result<ConfigResponse, Error> {
        if let ConfigRequest::Describe { key } = &request {
            return describe(key.as_deref());
        }
        let project = ctx.project()?;
        match request {
            ConfigRequest::Get { key } => get(project, &key),
            ConfigRequest::Set { key, value } => set(project, &key, &value),
            ConfigRequest::Unset { key } => unset(project, &key),
            ConfigRequest::List => Ok(ConfigResponse::List {
                settings: Config::load(project)?.settings(),
            }),
            ConfigRequest::Describe { .. } => unreachable!("handled above"),
        }
    }
}

fn key_segments(text: &str) -> Result<Vec<String>, Error> {
    split_key(text).ok_or_else(|| unknown_key(text))
}

fn get(project: &Project, text: &str) -> Result<ConfigResponse, Error> {
    let segments = key_segments(text)?;
    let key = parse_key(text, &segments, true)?;
    let config = Config::load(project)?;
    let settings = config
        .settings()
        .into_iter()
        .filter(|s| {
            if key.entry.is_none() && key.def.map {
                config
                    .settings
                    .get(&s.key)
                    .is_some_and(|stored| std::ptr::eq(stored.def, key.def))
            } else {
                s.key == key.display()
            }
        })
        .collect();
    Ok(ConfigResponse::Get {
        map: key.def.map && key.entry.is_none(),
        settings,
    })
}

fn set(project: &Project, text: &str, raw: &str) -> Result<ConfigResponse, Error> {
    let segments = key_segments(text)?;
    let key = parse_key(text, &segments, false)?;
    let value = parse_value(key.def.kind, raw)
        .ok_or_else(|| invalid_value(&key.display(), raw, key.def.kind))?;
    let before = Config::load(project)?;
    let file = key.def.file;
    if file == ConfigFile::Local && !project.data_dir().is_dir() {
        return Err(Error::NotIndexed {
            root: project.root().to_path_buf(),
        });
    }
    let (_, doc) = read_document(project, file)?;
    let mut doc = doc.unwrap_or_default();
    let key_segments = key.segments();
    let (last, parents) = key_segments.split_last().expect("keys have a segment");
    let mut table = doc.as_table_mut();
    for parent in parents {
        let entry = table.entry(parent).or_insert_with(|| {
            let mut t = Table::new();
            t.set_implicit(true);
            Item::Table(t)
        });
        table = entry.as_table_mut().ok_or_else(|| Error::InvalidConfig {
            cause: format!("{}: `{parent}` is not a table", file.path()),
            hint: format!("fix {} by hand, then retry", file.path()),
        })?;
    }
    match table.get_mut(last) {
        // Assigning through the existing entry keeps the key's comments; the
        // value's own decor (an end-of-line comment) is copied over.
        Some(existing) => {
            let mut item = value.to_item();
            if let (Some(old), Some(new)) = (existing.as_value(), item.as_value_mut()) {
                *new.decor_mut() = old.decor().clone();
            }
            *existing = item;
        }
        None => {
            table.insert(last, value.to_item());
        }
    }
    write_atomically(&project.root().join(file.path()), &doc.to_string())?;
    Ok(ConfigResponse::Set {
        key: key.display(),
        value,
        file: file.path().to_owned(),
        effect: effect(project, &key, &before)?,
    })
}

fn unset(project: &Project, text: &str) -> Result<ConfigResponse, Error> {
    let segments = key_segments(text)?;
    let key = parse_key(text, &segments, true)?;
    let before = Config::load(project)?;
    let mut files = Vec::new();
    for file in [ConfigFile::Shared, ConfigFile::Local] {
        let (_, Some(mut doc)) = read_document(project, file)? else {
            continue;
        };
        if remove_path(doc.as_table_mut(), &key.segments()) {
            write_atomically(&project.root().join(file.path()), &doc.to_string())?;
            files.push(file.path().to_owned());
        }
    }
    let effect = if files.is_empty() {
        None
    } else {
        Some(effect(project, &key, &before)?)
    };
    Ok(ConfigResponse::Unset {
        key: key.display(),
        files,
        effect,
    })
}

/// What a change to `key` implies. For keys that change how files are
/// indexed, the affected files are marked for reindex, and counted, when the
/// project has an index; `before` is the configuration before the change.
fn effect(project: &Project, key: &Key<'_>, before: &Config) -> Result<String, Error> {
    let affects = |path: &str| -> bool {
        match (key.def.path, key.entry) {
            ("languages.overrides", Some(glob)) => {
                override_matcher(glob).is_some_and(|m| m.is_match(path))
            }
            ("languages.overrides", None) => before
                .language_overrides()
                .iter()
                .any(|(glob, _)| override_matcher(glob).is_some_and(|m| m.is_match(path))),
            ("sources.roots", _) => {
                let after = Config::load(project)
                    .map(|c| c.source_roots())
                    .unwrap_or_default();
                let id = crate::project::FileId::from_canonical(path);
                before.source_roots().iter().chain(&after).any(|root| {
                    id.is_within(&crate::project::FileId::from_canonical(root.as_str()))
                })
            }
            _ => false,
        }
    };
    if !matches!(key.def.path, "languages.overrides" | "sources.roots")
        || !project.index_path().is_file()
    {
        return Ok(key.def.effect.to_owned());
    }
    let mut store = crate::store::Store::open(&project.index_path())?;
    let affected: Vec<String> = store
        .snapshot()?
        .files()?
        .into_iter()
        .map(|f| f.path)
        .filter(|path| affects(path))
        .collect();
    let marked = store.mark_pending(&affected)?;
    Ok(format!(
        "{marked} {} will be reindexed on the next scan",
        if marked == 1 { "file" } else { "files" }
    ))
}

/// The matcher of a `languages.overrides` glob, against root-relative paths.
/// `*` crosses directories, so `*.h` matches every `.h` file.
pub fn override_matcher(glob: &str) -> Option<globset::GlobMatcher> {
    globset::Glob::new(glob).ok().map(|g| g.compile_matcher())
}

fn describe(key: Option<&str>) -> Result<ConfigResponse, Error> {
    let defs: Vec<&KeyDef> = match key {
        Some(text) => {
            let segments = key_segments(text)?;
            vec![parse_key(text, &segments, true)?.def]
        }
        None => KEYS.iter().collect(),
    };
    let keys = defs
        .into_iter()
        .map(|def| KeyDescription {
            key: def.display(),
            description: def.description.to_owned(),
            kind: def.kind.describe(),
            default: def.default.map(str::to_owned),
            file: def.file.path().to_owned(),
        })
        .collect();
    Ok(ConfigResponse::Describe { keys })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keys_split_on_dots_outside_quotes() {
        assert_eq!(
            split_key(r#"languages.overrides."*.h""#).unwrap(),
            ["languages", "overrides", "*.h"]
        );
        assert_eq!(
            split_key("index.max_file_size").unwrap(),
            ["index", "max_file_size"]
        );
        assert!(split_key("index..x").is_none());
        assert!(split_key(r#"a."unterminated"#).is_none());
    }

    #[test]
    fn map_entries_are_quoted_when_needed() {
        let def = KEYS
            .iter()
            .find(|k| k.path == "languages.overrides")
            .expect("languages.overrides is a key");
        assert_eq!(
            display_key(def, Some("*.h")),
            r#"languages.overrides."*.h""#
        );
        assert_eq!(
            display_key(def, Some("Makefile")),
            "languages.overrides.Makefile"
        );
    }

    #[test]
    fn path_lists_must_stay_inside_the_root() {
        assert_eq!(
            parse_value(Kind::PathList, "src/main/java, gen/"),
            Some(Value::List(vec![
                "src/main/java".to_owned(),
                "gen".to_owned()
            ]))
        );
        assert_eq!(parse_value(Kind::PathList, "../x"), None);
        assert_eq!(parse_value(Kind::PathList, "/abs"), None);
    }
}
