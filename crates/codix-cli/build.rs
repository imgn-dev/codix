//! For the tests: `with_<feature>` (such as `with_lang_java`) is set when a
//! provider is there at run time, compiled in by its feature or, with
//! `plugins`, installed in a directory of `CODIX_PLUGIN_PATH` at build time.
//! So the tests of a provider run against either flavour.

use std::io::Write as _;
use std::path::Path;

/// Each provider feature, and its plugin directories.
const PROVIDERS: &[(&str, &[&str])] = &[
    ("lang-c", &["language/c"]),
    ("lang-cpp", &["language/cpp"]),
    ("lang-csharp", &["language/csharp"]),
    ("lang-go", &["language/go"]),
    ("lang-java", &["language/java"]),
    ("lang-javascript", &["language/javascript"]),
    ("lang-json", &["language/json"]),
    ("lang-kotlin", &["language/kotlin"]),
    ("lang-markdown", &["language/markdown"]),
    ("lang-php", &["language/php"]),
    ("lang-python", &["language/python"]),
    ("lang-ruby", &["language/ruby"]),
    ("lang-rust", &["language/rust"]),
    ("lang-sql", &["language/sql"]),
    ("lang-toml", &["language/toml"]),
    ("lang-typescript", &["language/typescript", "language/tsx"]),
    ("lang-xml", &["language/xml"]),
    ("lang-yaml", &["language/yaml"]),
    ("build-gradle", &["build/gradle"]),
    ("build-maven", &["build/maven"]),
    ("vcs-git", &["vcs/git"]),
    ("vcs-hg", &["vcs/mercurial"]),
    ("vcs-svn", &["vcs/subversion"]),
];

fn main() {
    let mut out = std::io::stdout().lock();
    writeln!(out, "cargo::rerun-if-env-changed=CODIX_PLUGIN_PATH").expect("write to cargo");
    let plugins = std::env::var_os("CARGO_FEATURE_PLUGINS")
        .and(std::env::var_os("CODIX_PLUGIN_PATH"))
        .map(|path| std::env::split_paths(&path).collect::<Vec<_>>())
        .unwrap_or_default();
    for (feature, dirs) in PROVIDERS {
        let cfg = format!("with_{}", feature.replace('-', "_"));
        writeln!(out, "cargo::rustc-check-cfg=cfg({cfg})").expect("write to cargo");
        let variable = format!("CARGO_FEATURE_{}", feature.to_uppercase().replace('-', "_"));
        let installed = || {
            dirs.iter().all(|dir| {
                plugins
                    .iter()
                    .any(|base| Path::new(base).join(dir).join("plugin.toml").is_file())
            })
        };
        if std::env::var_os(variable).is_some() || installed() {
            writeln!(out, "cargo::rustc-cfg={cfg}").expect("write to cargo");
        }
    }
}
