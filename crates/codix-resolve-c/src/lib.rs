//! The C resolver: binds a use of a function or a global in one file to the
//! symbol of that name its `#include`s reach, directly or through other
//! headers.
//!
//! Include paths come from `compile_commands.json` at the project's root or
//! in `build/`, when there is one: every `-I`, `-iquote` and `-isystem`
//! directory of every entry. Otherwise they are the including file's
//! directory, then every `include` directory of the tree. A use binds when
//! exactly one symbol of the name is reached, or when those reached are all
//! in one file (a prototype and its inline definition); several in several
//! files, as configuration variants are, leave it unbound, and so do calls
//! through function pointers.

use std::collections::{BTreeSet, HashMap, HashSet, VecDeque};
use std::path::{Component, Path, PathBuf};

use codix_resolver::{Resolver, Scratch};
use rusqlite::Connection;

codix_resolver::register!("c", CResolver::new);

/// The C resolver.
#[derive(Debug, Default)]
pub struct CResolver;

impl CResolver {
    /// A resolver.
    pub fn new() -> Self {
        Self
    }
}

impl Resolver for CResolver {
    fn resolve(
        &mut self,
        index: &Connection,
        scratch: &mut Scratch,
        files: &[String],
    ) -> Result<Vec<String>, String> {
        let model = Model::load(index).map_err(|e| e.to_string())?;
        let changed: HashSet<i64> = files
            .iter()
            .filter_map(|path| model.ids.get(path).copied())
            .filter(|id| model.c_files.contains(id))
            .collect();
        let dependents = dependents(index, &model, &changed).map_err(|e| e.to_string())?;
        let targets: Vec<i64> = changed.iter().chain(&dependents).copied().collect();
        let refs = unbound_refs(index, &targets).map_err(|e| e.to_string())?;
        let mut reach = Reach::default();
        scratch.batch(|scratch| {
            for (id, file, name) in &refs {
                if let Some(target) = model.resolve(&mut reach, *file, name) {
                    scratch.bind(*id, target)?;
                }
            }
            Ok(())
        })?;
        Ok(dependents
            .iter()
            .filter_map(|id| model.paths.get(id).cloned())
            .collect())
    }
}

/// The kinds a plain name may mean across files.
const KINDS: &[&str] = &["function", "declaration", "variable"];

#[derive(Debug, Default)]
struct Model {
    /// Every indexed file's path, by ID, and back.
    paths: HashMap<i64, String>,
    ids: HashMap<String, i64>,
    /// The C files.
    c_files: HashSet<i64>,
    /// The includes of each file, as written: path and whether `<…>`.
    includes: HashMap<i64, Vec<(String, bool)>>,
    /// Directories searched for includes, root-relative.
    include_dirs: Vec<String>,
    /// The same, to look up.
    include_set: HashSet<String>,
    /// Files by their last path component, to find an include's candidates
    /// without trying every include directory.
    by_basename: HashMap<String, Vec<i64>>,
    /// Symbols that a name may mean across files, by name.
    symbols: HashMap<String, Vec<Sym>>,
}

/// A symbol a name may mean across files.
#[derive(Debug, Clone, Copy)]
struct Sym {
    id: i64,
    file: i64,
    /// A prototype, declared without its body.
    declaration: bool,
    /// Declared `static`: visible in its file only.
    is_static: bool,
}

/// The files each file reaches, computed once each.
#[derive(Debug, Default)]
struct Reach(HashMap<i64, HashSet<i64>>);

impl Reach {
    fn of(&mut self, model: &Model, file: i64) -> &HashSet<i64> {
        self.0.entry(file).or_insert_with(|| model.reachable(file))
    }
}

impl Model {
    fn load(index: &Connection) -> rusqlite::Result<Self> {
        let mut model = Self::default();
        let mut stmt = index.prepare("SELECT id, path, language FROM v1_files")?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, String>(2)?,
            ))
        })? {
            let (id, path, language) = row?;
            if language == "c" {
                model.c_files.insert(id);
            }
            model.ids.insert(path.clone(), id);
            model.paths.insert(id, path);
        }
        let mut stmt =
            index.prepare("SELECT file_id, name, system FROM v1_imports WHERE kind = 'include'")?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, String>(1)?,
                r.get::<_, bool>(2)?,
            ))
        })? {
            let (file, name, system) = row?;
            model.includes.entry(file).or_default().push((name, system));
        }
        let mut stmt = index.prepare(
            "SELECT s.id, s.file_id, s.name, s.kind, s.parent, s.signature FROM v1_symbols s
             JOIN v1_files f ON f.id = s.file_id WHERE f.language IN ('c', 'cpp')",
        )?;
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, i64>(0)?,
                r.get::<_, i64>(1)?,
                r.get::<_, String>(2)?,
                r.get::<_, String>(3)?,
                r.get::<_, Option<String>>(4)?,
                r.get::<_, Option<String>>(5)?,
            ))
        })? {
            let (id, file, name, kind, parent, signature) = row?;
            // Members and locals of a type are reached through it.
            if parent.is_none() && KINDS.contains(&kind.as_str()) {
                let is_static = signature
                    .as_deref()
                    .is_some_and(|s| s.split(['(', ' ', '\t', '\n']).any(|w| w == "static"));
                model.symbols.entry(name).or_default().push(Sym {
                    id,
                    file,
                    declaration: kind == "declaration",
                    is_static,
                });
            }
        }
        model.include_dirs = match project_root(index).and_then(|root| compile_commands(&root)) {
            Some(dirs) => dirs,
            None => include_directories(model.paths.values()),
        };
        model.include_set = model.include_dirs.iter().cloned().collect();
        for (&id, path) in &model.paths {
            let base = path.rsplit('/').next().unwrap_or(path);
            model
                .by_basename
                .entry(base.to_owned())
                .or_default()
                .push(id);
        }
        Ok(model)
    }

    /// The files `file` includes, directly or through other headers.
    fn reachable(&self, file: i64) -> HashSet<i64> {
        let mut seen = HashSet::from([file]);
        let mut queue = VecDeque::from([file]);
        while let Some(current) = queue.pop_front() {
            for (written, system) in self.includes.get(&current).into_iter().flatten() {
                for header in self.header(current, written, *system) {
                    if seen.insert(header) {
                        queue.push_back(header);
                    }
                }
            }
        }
        seen
    }

    /// The files an include may name: beside the including file for `"…"`,
    /// then in each include directory.
    fn header(&self, from: i64, written: &str, system: bool) -> Vec<i64> {
        let mut found = Vec::new();
        let beside = (!system)
            .then(|| self.paths.get(&from))
            .flatten()
            .and_then(|path| Path::new(path).parent().map(Path::to_path_buf));
        if let Some(dir) = beside
            && let Some(path) = normalized(&dir.join(written))
            && let Some(&id) = self.ids.get(&path)
        {
            found.push(id);
        }
        let Some(written) = normalized(Path::new(written)) else {
            return found;
        };
        let base = written.rsplit('/').next().unwrap_or(&written);
        for &id in self.by_basename.get(base).into_iter().flatten() {
            let path = &self.paths[&id];
            let dir = if *path == written {
                Some("")
            } else {
                path.strip_suffix(written.as_str())
                    .and_then(|p| p.strip_suffix('/'))
            };
            if dir.is_some_and(|d| self.include_set.contains(d)) && !found.contains(&id) {
                found.push(id);
            }
        }
        found
    }

    /// What `name` used in `file` binds to: the one symbol of the name the
    /// file reaches, or several in one file, its own first; a prototype
    /// stands for the one definition whose file reaches it, when there is one.
    fn resolve(&self, reach: &mut Reach, file: i64, name: &str) -> Option<i64> {
        let all = self.symbols.get(name)?;
        let reached = reach.of(self, file);
        let candidates: Vec<Sym> = all
            .iter()
            .copied()
            .filter(|s| reached.contains(&s.file))
            .collect();
        let files: BTreeSet<i64> = candidates.iter().map(|s| s.file).collect();
        if files.len() != 1 {
            return None;
        }
        let chosen = candidates
            .iter()
            .find(|s| s.file == file && !s.declaration)
            .or_else(|| candidates.iter().find(|s| !s.declaration))
            .or_else(|| candidates.first())
            .copied()?;
        if !chosen.declaration {
            return Some(chosen.id);
        }
        let header = chosen.file;
        let definitions: Vec<i64> = all
            .iter()
            .filter(|s| !s.declaration && !s.is_static)
            .filter(|s| reach.of(self, s.file).contains(&header))
            .map(|s| s.id)
            .collect();
        Some(match definitions.as_slice() {
            [one] => *one,
            _ => chosen.id,
        })
    }
}

/// The project's root: the directory holding `.codix/`, where the index is.
fn project_root(index: &Connection) -> Option<PathBuf> {
    let path: String = index
        .query_row(
            "SELECT file FROM pragma_database_list WHERE name = 'main'",
            [],
            |r| r.get(0),
        )
        .ok()?;
    Path::new(&path).parent()?.parent().map(Path::to_path_buf)
}

/// The include directories of `compile_commands.json`, root-relative; `None`
/// without one.
fn compile_commands(root: &Path) -> Option<Vec<String>> {
    let path = [
        root.join("compile_commands.json"),
        root.join("build/compile_commands.json"),
    ]
    .into_iter()
    .find(|p| p.is_file())?;
    let entries: serde_json::Value =
        serde_json::from_str(&std::fs::read_to_string(path).ok()?).ok()?;
    let mut dirs: Vec<String> = Vec::new();
    for entry in entries.as_array()? {
        // A relative directory is the root's.
        let directory = root.join(entry["directory"].as_str().unwrap_or("."));
        let words: Vec<String> = match (&entry["arguments"], &entry["command"]) {
            (serde_json::Value::Array(args), _) => args
                .iter()
                .filter_map(|a| a.as_str().map(str::to_owned))
                .collect(),
            (_, serde_json::Value::String(command)) => {
                command.split_whitespace().map(str::to_owned).collect()
            }
            _ => continue,
        };
        let mut words = words.iter();
        while let Some(word) = words.next() {
            let dir = ["-I", "-iquote", "-isystem"].iter().find_map(|flag| {
                let rest = word.strip_prefix(flag)?;
                if rest.is_empty() {
                    words.clone().next().cloned()
                } else {
                    Some(rest.to_owned())
                }
            });
            let Some(dir) = dir else {
                continue;
            };
            let absolute = directory.join(dir);
            if let Ok(relative) = absolute.strip_prefix(root)
                && let Some(relative) = normalized(relative)
                && !dirs.contains(&relative)
            {
                dirs.push(relative);
            }
        }
    }
    Some(dirs)
}

/// Every directory named `include` among the files' ancestors, sorted.
fn include_directories<'a>(paths: impl Iterator<Item = &'a String>) -> Vec<String> {
    let mut dirs = BTreeSet::new();
    for path in paths {
        let mut prefix = PathBuf::new();
        for component in Path::new(path)
            .parent()
            .into_iter()
            .flat_map(Path::components)
        {
            prefix.push(component);
            if component.as_os_str() == "include"
                && let Some(dir) = normalized(&prefix)
            {
                dirs.insert(dir);
            }
        }
    }
    dirs.into_iter().collect()
}

/// A relative path with `.` and `..` removed and `/` between parts; `None`
/// when it leaves the root.
fn normalized(path: &Path) -> Option<String> {
    let mut parts: Vec<String> = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(part) => parts.push(part.to_str()?.to_owned()),
            Component::ParentDir => {
                parts.pop()?;
            }
            Component::CurDir => {}
            Component::RootDir | Component::Prefix(_) => return None,
        }
    }
    Some(parts.join("/"))
}

/// The C files to resolve again because they use, unresolved, a name the
/// changed files define, but the changed files: reindexing a file drops what
/// was bound to its symbols, and what nothing bound may bind now.
fn dependents(
    index: &Connection,
    model: &Model,
    changed: &HashSet<i64>,
) -> rusqlite::Result<Vec<i64>> {
    let names: Vec<&str> = model
        .symbols
        .iter()
        .filter(|(_, found)| found.iter().any(|s| changed.contains(&s.file)))
        .map(|(name, _)| name.as_str())
        .collect();
    let mut found = HashSet::new();
    for chunk in names.chunks(500) {
        let marks = vec!["?"; chunk.len()].join(", ");
        let mut stmt = index.prepare(&format!(
            "SELECT DISTINCT r.file_id FROM v1_refs r JOIN v1_files f ON f.id = r.file_id
             LEFT JOIN v1_resolved x ON x.ref = r.id
             WHERE f.language = 'c' AND r.state != 'bound' AND x.ref IS NULL
               AND r.name IN ({marks})"
        ))?;
        for row in stmt.query_map(rusqlite::params_from_iter(chunk), |r| r.get::<_, i64>(0))? {
            let file = row?;
            if !changed.contains(&file) {
                found.insert(file);
            }
        }
    }
    let mut found: Vec<i64> = found.into_iter().collect();
    found.sort_unstable();
    Ok(found)
}

/// The plain names of `files` their scopes leave unbound, called or read:
/// ID, file and name. Members and calls through pointers have a receiver.
fn unbound_refs(index: &Connection, files: &[i64]) -> rusqlite::Result<Vec<(i64, i64, String)>> {
    let mut refs = Vec::new();
    let mut stmt = index.prepare(
        "SELECT id, file_id, name FROM v1_refs
         WHERE file_id = ?1 AND state != 'bound' AND receiver IS NULL
           AND context IN ('call', 'read')",
    )?;
    for &file in files {
        for row in stmt.query_map([file], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?)))? {
            refs.push(row?);
        }
    }
    Ok(refs)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn paths_are_normalized_within_the_root() {
        assert_eq!(
            normalized(Path::new("kernel/sched/../../include/linux/sched.h")),
            Some("include/linux/sched.h".to_owned())
        );
        assert_eq!(normalized(Path::new("../x.h")), None);
    }

    #[test]
    fn include_directories_are_found_in_the_tree() {
        let paths = [
            "include/linux/sched.h".to_owned(),
            "arch/x86/include/asm/a.h".to_owned(),
            "kernel/sched/core.c".to_owned(),
        ];
        assert_eq!(
            include_directories(paths.iter()),
            ["arch/x86/include", "include"]
        );
    }
}
