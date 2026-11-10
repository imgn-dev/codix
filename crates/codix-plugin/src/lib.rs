//! Plugins: where they are found, their manifests, their checks, and the
//! native loader of their grammars.
//!
//! Linking this crate adds the plugins installed on the machine to the
//! registry of `codix-provider`. A plugin is a directory
//! `<search dir>/<kind>/<name>/` holding `plugin.toml` and `descriptor.toml`,
//! and for a language its queries and its grammar as a shared library; one
//! that fails its checks is listed with the reason and otherwise ignored.

use std::ffi::OsString;
use std::fs;
use std::path::{Path, PathBuf};

use codix_provider::{
    Descriptor, Plugin, PluginBuild, PluginGrammar, PluginSource, ProviderKind, Rejected,
    Rejection, toml_error,
};
use serde::Deserialize;

pub mod build;
mod native;

/// The manifest schema this codix reads.
pub const SCHEMA: u32 = 1;

/// The manifest's file name.
pub const MANIFEST: &str = "plugin.toml";

/// The descriptor's file name.
pub const DESCRIPTOR: &str = "descriptor.toml";

/// A language plugin's `tags` query.
pub const TAGS: &str = "queries/tags.scm";

/// A language plugin's `locals` query, if it has scopes.
pub const LOCALS: &str = "queries/locals.scm";

/// Directories listed in this variable are searched first.
pub const PATH_VAR: &str = "CODIX_PLUGIN_PATH";

codix_provider::inventory::submit! {
    PluginSource { discover: installed }
}

fn installed() -> Vec<Result<Plugin, Rejected>> {
    discover(&search_path())
}

/// What `plugin.toml` holds.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
struct Manifest {
    schema: u32,
    kind: ProviderKind,
    name: String,
    version: String,
    /// A language plugin's grammar library: a file name in its directory.
    library: Option<String>,
    /// The function the library exports that returns the grammar, such as
    /// `tree_sitter_java`.
    symbol: Option<String>,
    /// A build plugin's executable, relative to its directory, such as
    /// `bin/codix-build-maven`.
    executable: Option<String>,
}

/// The directories searched for plugins, in order: each directory of
/// `CODIX_PLUGIN_PATH`; the user's data directory
/// (`$XDG_DATA_HOME/codix/plugins`, by default
/// `~/.local/share/codix/plugins`; `%LOCALAPPDATA%\codix\plugins` on
/// Windows); and `lib/codix/plugins` next to the binary's directory, as
/// invoked and with symlinks resolved. Never the project's: a repository must
/// not be able to bring native code.
pub fn search_path() -> Vec<PathBuf> {
    search_path_from(
        &|key| std::env::var_os(key),
        std::env::args_os().next(),
        std::env::current_exe().ok(),
    )
}

fn search_path_from(
    env: &dyn Fn(&str) -> Option<OsString>,
    argv0: Option<OsString>,
    exe: Option<PathBuf>,
) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(path) = env(PATH_VAR) {
        dirs.extend(std::env::split_paths(&path).filter(|p| p.is_absolute()));
    }
    if let Some(data) = user_data_dir(env) {
        dirs.push(data.join("codix").join("plugins"));
    }
    let resolved = exe.as_ref().and_then(|exe| exe.canonicalize().ok());
    for binary in [argv0.and_then(|a| invoked(env, &a)), exe, resolved]
        .into_iter()
        .flatten()
    {
        if let Some(prefix) = binary.parent().and_then(Path::parent) {
            dirs.push(prefix.join("lib").join("codix").join("plugins"));
        }
    }
    let mut seen: Vec<PathBuf> = Vec::new();
    dirs.retain(|dir| {
        let key = dir.canonicalize().unwrap_or_else(|_| dir.clone());
        let new = !seen.contains(&key);
        seen.push(key);
        new
    });
    dirs
}

fn user_data_dir(env: &dyn Fn(&str) -> Option<OsString>) -> Option<PathBuf> {
    let absolute = |key| env(key).map(PathBuf::from).filter(|p| p.is_absolute());
    if cfg!(windows) {
        return absolute("LOCALAPPDATA");
    }
    absolute("XDG_DATA_HOME").or_else(|| absolute("HOME").map(|home| home.join(".local/share")))
}

/// The binary as invoked: `argv[0]` made absolute, or when it is a bare name,
/// the first file of that name in `PATH`.
fn invoked(env: &dyn Fn(&str) -> Option<OsString>, argv0: &OsString) -> Option<PathBuf> {
    let path = Path::new(argv0);
    if path.components().count() > 1 {
        return std::path::absolute(path).ok();
    }
    let names: Vec<OsString> = if cfg!(windows) && path.extension().is_none() {
        let mut exe = argv0.clone();
        exe.push(".exe");
        vec![exe, argv0.clone()]
    } else {
        vec![argv0.clone()]
    };
    std::env::split_paths(&env("PATH")?)
        .filter(|dir| dir.is_absolute())
        .flat_map(|dir| names.iter().map(move |name| dir.join(name)))
        .find(|candidate| candidate.is_file())
}

/// Every plugin in `dirs`, in search order, then by kind and name: each
/// checked, or rejected with the reason.
pub fn discover(dirs: &[PathBuf]) -> Vec<Result<Plugin, Rejected>> {
    let mut found = Vec::new();
    for dir in dirs {
        for kind in [
            ProviderKind::Language,
            ProviderKind::Build,
            ProviderKind::Vcs,
        ] {
            let Ok(entries) = fs::read_dir(dir.join(kind.to_string())) else {
                continue;
            };
            let mut plugins: Vec<(String, PathBuf)> = entries
                .filter_map(Result::ok)
                .filter(|entry| entry.path().is_dir())
                .map(|entry| {
                    (
                        entry.file_name().to_string_lossy().into_owned(),
                        entry.path(),
                    )
                })
                .collect();
            plugins.sort();
            for (name, path) in plugins {
                found.push(check(kind, &name, &path).map_err(|reason| Rejected {
                    kind,
                    name,
                    dir: path,
                    rejection: Rejection::Invalid(reason),
                }));
            }
        }
    }
    found
}

/// Checks the plugin in `dir`, found as the `kind` provider `name`.
fn check(kind: ProviderKind, name: &str, dir: &Path) -> Result<Plugin, String> {
    let manifest: Manifest = {
        let text = read(dir, MANIFEST)?;
        toml::from_str(&text).map_err(|e| format!("{MANIFEST}: {}", toml_error(&text, &e)))?
    };
    if manifest.schema != SCHEMA {
        return Err(format!(
            "{MANIFEST}: schema {}, this codix reads schema {SCHEMA}",
            manifest.schema
        ));
    }
    if manifest.kind != kind {
        return Err(format!(
            "{MANIFEST}: kind {}, in the {kind} directory",
            manifest.kind
        ));
    }
    if manifest.name != name {
        return Err(format!(
            "{MANIFEST}: name {}, in a directory named {name}",
            manifest.name
        ));
    }
    let descriptor = Descriptor::parse(kind, &read(dir, DESCRIPTOR)?)
        .map_err(|e| format!("{DESCRIPTOR}: {e}"))?;
    if descriptor.name() != name {
        return Err(format!(
            "{DESCRIPTOR}: name {}, in a directory named {name}",
            descriptor.name()
        ));
    }
    let grammar = match kind {
        ProviderKind::Language => Some(grammar(dir, manifest.library, manifest.symbol)?),
        _ if manifest.library.is_some() || manifest.symbol.is_some() => {
            return Err(format!(
                "{MANIFEST}: library and symbol are for language plugins"
            ));
        }
        _ => None,
    };
    let build = match (kind, manifest.executable) {
        (ProviderKind::Build, Some(executable)) => Some(build(dir, &executable)?),
        (ProviderKind::Build, None) => {
            return Err(format!("{MANIFEST}: a build plugin names its executable"));
        }
        (_, Some(_)) => {
            return Err(format!("{MANIFEST}: executable is for build plugins"));
        }
        (_, None) => None,
    };
    Ok(Plugin {
        version: manifest.version,
        dir: dir.to_path_buf(),
        descriptor,
        grammar,
        build,
    })
}

/// A build plugin's executable, which must be in its directory.
fn build(dir: &Path, executable: &str) -> Result<PluginBuild, String> {
    let relative = Path::new(executable);
    if !relative
        .components()
        .all(|c| matches!(c, std::path::Component::Normal(_)))
    {
        return Err(format!(
            "{MANIFEST}: executable {executable} is not a path in the plugin's directory"
        ));
    }
    let executable = dir.join(relative);
    if !executable.is_file() {
        return Err(format!("{}: not found", executable.display()));
    }
    Ok(PluginBuild {
        executable,
        run: build::run,
    })
}

/// A language plugin's grammar and queries. The library is only checked to
/// be there: it is loaded on the first file of the language.
fn grammar(
    dir: &Path,
    library: Option<String>,
    symbol: Option<String>,
) -> Result<PluginGrammar, String> {
    let (Some(library), Some(symbol)) = (library, symbol) else {
        return Err(format!(
            "{MANIFEST}: a language plugin names its library and symbol"
        ));
    };
    if Path::new(&library).file_name() != Some(library.as_ref()) {
        return Err(format!(
            "{MANIFEST}: library {library} is not a file name in the plugin's directory"
        ));
    }
    let library = dir.join(library);
    if !library.is_file() {
        return Err(format!("{}: not found", library.display()));
    }
    let locals = match fs::read_to_string(dir.join(LOCALS)) {
        Ok(text) => Some(text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(format!("{LOCALS}: {e}")),
    };
    Ok(PluginGrammar {
        library,
        symbol,
        tags: read(dir, TAGS)?,
        locals,
        load: native::load,
    })
}

fn read(dir: &Path, file: &str) -> Result<String, String> {
    fs::read_to_string(dir.join(file)).map_err(|e| format!("{file}: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    const HG: &str = "name = \"mercurial\"\ndetect = [\".hg\"]\n\n[ignore]\nstrategy = \"none\"\n";

    fn manifest(kind: &str, name: &str) -> String {
        format!("schema = 1\nkind = \"{kind}\"\nname = \"{name}\"\nversion = \"1.0.0\"\n")
    }

    fn plugin(root: &Path, kind: &str, name: &str, files: &[(&str, &str)]) -> PathBuf {
        let dir = root.join(kind).join(name);
        fs::create_dir_all(&dir).expect("plugin dir");
        for (file, text) in files {
            fs::write(dir.join(file), text).expect("write");
        }
        dir
    }

    fn reason(found: &Result<Plugin, Rejected>) -> &str {
        match found {
            Err(Rejected {
                rejection: Rejection::Invalid(reason),
                ..
            }) => reason,
            other => panic!("not invalid: {other:?}"),
        }
    }

    #[test]
    fn a_vcs_plugin_is_found_with_its_descriptor() {
        let root = tempfile::tempdir().expect("temp dir");
        let dir = plugin(
            root.path(),
            "vcs",
            "mercurial",
            &[(MANIFEST, &manifest("vcs", "mercurial")), (DESCRIPTOR, HG)],
        );
        let found = discover(&[root.path().to_path_buf()]);
        let [Ok(plugin)] = found.as_slice() else {
            panic!("{found:?}");
        };
        assert_eq!(plugin.version, "1.0.0");
        assert_eq!(plugin.dir, dir);
        assert_eq!(plugin.descriptor.kind(), ProviderKind::Vcs);
        assert_eq!(plugin.descriptor.name(), "mercurial");
    }

    #[test]
    fn plugins_are_found_in_search_order_then_by_kind_and_name() {
        let first = tempfile::tempdir().expect("temp dir");
        let second = tempfile::tempdir().expect("temp dir");
        for name in ["b", "a"] {
            plugin(first.path(), "vcs", name, &[]);
        }
        plugin(first.path(), "language", "z", &[]);
        plugin(second.path(), "vcs", "a", &[]);
        let found = discover(&[second.path().to_path_buf(), first.path().to_path_buf()]);
        let order: Vec<(PathBuf, String)> = found
            .iter()
            .map(|f| {
                let rejected = f.as_ref().expect_err("no manifest");
                (rejected.dir.clone(), rejected.name.clone())
            })
            .collect();
        assert_eq!(
            order,
            [
                (second.path().join("vcs/a"), "a".to_owned()),
                (first.path().join("language/z"), "z".to_owned()),
                (first.path().join("vcs/a"), "a".to_owned()),
                (first.path().join("vcs/b"), "b".to_owned()),
            ]
        );
    }

    #[test]
    fn invalid_plugins_are_rejected_with_the_reason() {
        let cases: &[(&str, &str, String, &str, &str)] = &[
            ("vcs", "hg", String::new(), HG, "plugin.toml: "),
            (
                "vcs",
                "hg",
                "schema = 1\nkind = \"vcs\"\nname = \"hg\"\nversion = \"1\"\nx = 1\n".to_owned(),
                HG,
                "plugin.toml: line 5: unknown field `x`",
            ),
            (
                "vcs",
                "hg",
                manifest("vcs", "hg").replace("schema = 1", "schema = 2"),
                HG,
                "plugin.toml: schema 2, this codix reads schema 1",
            ),
            (
                "vcs",
                "hg",
                manifest("build", "hg"),
                HG,
                "plugin.toml: kind build, in the vcs directory",
            ),
            (
                "vcs",
                "hg",
                manifest("vcs", "git"),
                HG,
                "plugin.toml: name git, in a directory named hg",
            ),
            (
                "vcs",
                "hg",
                manifest("vcs", "hg"),
                "name = 3\n",
                "descriptor.toml: line 1: invalid type",
            ),
            (
                "vcs",
                "hg",
                manifest("vcs", "hg"),
                HG,
                "descriptor.toml: name mercurial, in a directory named hg",
            ),
            (
                "vcs",
                "mercurial",
                manifest("vcs", "mercurial") + "symbol = \"x\"\n",
                HG,
                "plugin.toml: library and symbol are for language plugins",
            ),
            (
                "build",
                "maven",
                manifest("build", "maven"),
                MAVEN,
                "plugin.toml: a build plugin names its executable",
            ),
            (
                "build",
                "maven",
                manifest("build", "maven") + "executable = \"../codix-build-maven\"\n",
                MAVEN,
                "plugin.toml: executable ../codix-build-maven is not a path in the plugin's directory",
            ),
            (
                "build",
                "maven",
                manifest("build", "maven") + "executable = \"bin/codix-build-maven\"\n",
                MAVEN,
                "codix-build-maven: not found",
            ),
            (
                "vcs",
                "mercurial",
                manifest("vcs", "mercurial") + "executable = \"x\"\n",
                HG,
                "plugin.toml: executable is for build plugins",
            ),
        ];
        for (kind, name, manifest, descriptor, expected) in cases {
            let root = tempfile::tempdir().expect("temp dir");
            let mut files = vec![(DESCRIPTOR, *descriptor)];
            if !manifest.is_empty() {
                files.push((MANIFEST, manifest.as_str()));
            }
            plugin(root.path(), kind, name, &files);
            let found = discover(&[root.path().to_path_buf()]);
            assert_eq!(found.len(), 1, "{expected}");
            let reason = reason(&found[0]);
            assert!(
                reason.starts_with(expected) || reason.ends_with(expected),
                "{reason} / {expected}"
            );
        }
    }

    const MAVEN: &str = "name = \"maven\"\ndetect = [\"pom.xml\"]\n";

    #[test]
    fn a_build_plugin_is_found_with_its_executable() {
        let root = tempfile::tempdir().expect("temp dir");
        let manifest = manifest("build", "maven") + "executable = \"bin/codix-build-maven\"\n";
        let dir = plugin(
            root.path(),
            "build",
            "maven",
            &[(MANIFEST, &manifest), (DESCRIPTOR, MAVEN)],
        );
        fs::create_dir(dir.join("bin")).expect("bin");
        fs::write(dir.join("bin/codix-build-maven"), "").expect("executable");
        let found = discover(&[root.path().to_path_buf()]);
        let [Ok(plugin)] = found.as_slice() else {
            panic!("{found:?}");
        };
        let build = plugin.build.as_ref().expect("build");
        assert_eq!(build.executable, dir.join("bin/codix-build-maven"));
    }

    const KOTLIN: &str = "name = \"kotlin\"\nversion = 1\nid_prefix = \"kotlin\"\n";

    fn language_manifest(library: &str) -> String {
        manifest("language", "kotlin")
            + &format!("library = \"{library}\"\nsymbol = \"tree_sitter_kotlin\"\n")
    }

    #[test]
    fn a_language_plugin_is_found_with_its_grammar_unloaded() {
        let root = tempfile::tempdir().expect("temp dir");
        let manifest = language_manifest("kotlin.so");
        let dir = plugin(
            root.path(),
            "language",
            "kotlin",
            &[
                (MANIFEST, &manifest),
                (DESCRIPTOR, KOTLIN),
                ("kotlin.so", "not a library: discovery does not open it"),
            ],
        );
        fs::create_dir(dir.join("queries")).expect("queries");
        fs::write(dir.join(TAGS), "(identifier) @name").expect("tags");
        let found = discover(&[root.path().to_path_buf()]);
        let [Ok(plugin)] = found.as_slice() else {
            panic!("{found:?}");
        };
        let grammar = plugin.grammar.as_ref().expect("grammar");
        assert_eq!(grammar.library, dir.join("kotlin.so"));
        assert_eq!(grammar.symbol, "tree_sitter_kotlin");
        assert_eq!(grammar.tags, "(identifier) @name");
        assert_eq!(grammar.locals, None);
    }

    #[test]
    fn invalid_language_plugins_are_rejected_with_the_reason() {
        let cases: &[(String, &[&str], &str)] = &[
            (
                manifest("language", "kotlin"),
                &["kotlin.so", TAGS],
                "plugin.toml: a language plugin names its library and symbol",
            ),
            (
                language_manifest("../kotlin.so"),
                &["kotlin.so", TAGS],
                "plugin.toml: library ../kotlin.so is not a file name in the plugin's directory",
            ),
            (
                language_manifest("kotlin.so"),
                &[TAGS],
                "kotlin.so: not found",
            ),
            (
                language_manifest("kotlin.so"),
                &["kotlin.so"],
                "queries/tags.scm: ",
            ),
        ];
        for (manifest, files, expected) in cases {
            let root = tempfile::tempdir().expect("temp dir");
            let dir = plugin(
                root.path(),
                "language",
                "kotlin",
                &[(MANIFEST, manifest), (DESCRIPTOR, KOTLIN)],
            );
            fs::create_dir(dir.join("queries")).expect("queries");
            for file in *files {
                fs::write(dir.join(file), "").expect("write");
            }
            let found = discover(&[root.path().to_path_buf()]);
            let reason = reason(&found[0]);
            assert!(reason.contains(expected), "{reason} / {expected}");
        }
    }

    #[test]
    fn search_path_order() {
        let root = tempfile::tempdir().expect("temp dir");
        let root = root.path().canonicalize().expect("canonical");
        let (one, two) = (root.join("one"), root.join("two"));
        let bin = root.join("prefix/bin");
        fs::create_dir_all(&bin).expect("bin");
        fs::write(bin.join("codix"), "").expect("binary");
        let path = std::env::join_paths([&one, &two]).expect("join");
        let vars = [
            (PATH_VAR, path.clone()),
            ("XDG_DATA_HOME", root.join("data").into_os_string()),
            ("HOME", root.join("home").into_os_string()),
            ("LOCALAPPDATA", root.join("local").into_os_string()),
            ("PATH", bin.clone().into_os_string()),
        ];
        let env = |key: &str| vars.iter().find(|(k, _)| *k == key).map(|(_, v)| v.clone());
        let dirs = search_path_from(&env, Some("codix".into()), Some(bin.join("codix")));
        let data = if cfg!(windows) { "local" } else { "data" };
        assert_eq!(
            dirs,
            [
                one,
                two,
                root.join(data).join("codix").join("plugins"),
                root.join("prefix")
                    .join("lib")
                    .join("codix")
                    .join("plugins"),
            ]
        );
    }

    #[cfg(not(windows))]
    #[test]
    fn the_data_directory_defaults_to_the_home_folder() {
        let env = |key: &str| (key == "HOME").then(|| OsString::from("/home/u"));
        assert_eq!(
            search_path_from(&env, None, None),
            [PathBuf::from("/home/u/.local/share/codix/plugins")]
        );
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_binary_is_searched_both_as_invoked_and_resolved() {
        let root = tempfile::tempdir().expect("temp dir");
        let root = root.path().canonicalize().expect("canonical");
        let cellar = root.join("cellar/codix/bin");
        fs::create_dir_all(&cellar).expect("cellar");
        fs::write(cellar.join("codix"), "").expect("binary");
        let bin = root.join("prefix/bin");
        fs::create_dir_all(&bin).expect("bin");
        std::os::unix::fs::symlink(cellar.join("codix"), bin.join("codix")).expect("link");
        let env = |_: &str| None;
        let dirs = search_path_from(
            &env,
            Some(bin.join("codix").into_os_string()),
            Some(bin.join("codix")),
        );
        assert_eq!(
            dirs,
            [
                root.join("prefix/lib/codix/plugins"),
                root.join("cellar/codix/lib/codix/plugins"),
            ]
        );
    }
}
