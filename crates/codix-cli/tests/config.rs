//! `codix config`: every form, layering, validation and comment preservation.

use std::fs;
use std::path::{Path, PathBuf};

use codix_testkit::{Run, assert_golden, run_in};
use serde_json::Value;
use tempfile::TempDir;

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

const SHARED: &str = "\
# Facts about the code, shared by everyone.
[index]
# Generated parsers are large.
max_file_size = \"4MB\" # measured

[languages.overrides]
\"*.h\" = \"cpp\"
";

fn project(shared: Option<&str>) -> (TempDir, PathBuf) {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = codix_core::project::canonicalize(dir.path()).expect("canonical temp dir");
    fs::create_dir_all(root.join(".codix")).expect("create .codix");
    if let Some(shared) = shared {
        fs::write(root.join("codix.toml"), shared).expect("write codix.toml");
    }
    (dir, root)
}

fn config(root: &Path, args: &[&str]) -> Run {
    let mut all = vec!["config"];
    all.extend(args);
    run_in(root, CODIX, &all)
}

fn read(path: PathBuf) -> String {
    fs::read_to_string(path).expect("read config file")
}

#[test]
fn get_prints_the_effective_value() {
    let (_guard, root) = project(Some(SHARED));
    let out = config(&root, &["index.max_file_size"]);
    assert_eq!((out.code, out.stdout.as_str()), (Some(0), "4MB\n"));

    let out = config(&root, &["index.scan_interval"]);
    assert_eq!(out.stdout, "10m\n", "default");

    let out = config(&root, &["languages.overrides"]);
    assert_eq!(out.stdout, "languages.overrides.\"*.h\" = cpp\n");
}

#[test]
fn get_of_an_unset_key_exits_1() {
    let (_guard, root) = project(None);
    let out = config(&root, &["sources.roots"]);
    assert_eq!((out.code, out.stdout.as_str()), (Some(1), ""));
}

#[test]
fn set_preserves_comments_and_order() {
    let (_guard, root) = project(Some(SHARED));
    let out = config(&root, &["index.max_file_size", "8MB"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(
        out.stdout,
        "set index.max_file_size = 8MB in codix.toml\n\
         files crossing the new limit will be reindexed on the next scan\n"
    );
    let out = config(&root, &["sources.roots", "src/main/java,gen"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);

    assert_eq!(
        read(root.join("codix.toml")),
        SHARED.replace("\"4MB\"", "\"8MB\"")
            + "\n[sources]\nroots = [\"src/main/java\", \"gen\"]\n"
    );
}

#[test]
fn unset_then_set_round_trips_the_file() {
    let (_guard, root) = project(Some(SHARED));
    config(&root, &["--unset", "languages.overrides.\"*.h\""]);
    let out = config(&root, &["languages.overrides.\"*.h\"", "cpp"]);
    assert_eq!(out.code, Some(0), "{}", out.stderr);
    assert_eq!(read(root.join("codix.toml")), SHARED);
}

#[test]
fn unset_of_a_missing_key_exits_1() {
    let (_guard, root) = project(None);
    let out = config(&root, &["--unset", "index.max_file_size"]);
    assert_eq!(out.code, Some(1));
    assert_eq!(out.stdout, "index.max_file_size was not set\n");
}

#[test]
fn local_keys_go_to_the_local_file() {
    let (_guard, root) = project(None);
    let out = config(&root, &["index.scan_interval", "5m"]);
    assert_eq!(
        out.stdout.lines().next(),
        Some("set index.scan_interval = 5m in .codix/config.toml")
    );
    assert_eq!(
        read(root.join(".codix/config.toml")),
        "[index]\nscan_interval = \"5m\"\n"
    );
    assert!(!root.join("codix.toml").exists());
}

#[test]
fn local_keys_need_the_data_directory() {
    let (_guard, root) = project(Some(""));
    fs::remove_dir(root.join(".codix")).expect("remove .codix");
    let out = config(&root, &["index.scan_interval", "5m"]);
    assert_eq!(out.code, Some(6));
    assert!(
        out.stderr.starts_with("error[not_indexed]"),
        "{}",
        out.stderr
    );
}

#[test]
fn local_overrides_shared_and_show_origin_says_so() {
    let (_guard, root) = project(Some(SHARED));
    fs::write(
        root.join(".codix/config.toml"),
        "[index]\nmax_file_size = \"16MB\"\n",
    )
    .expect("write local config");

    let out = config(&root, &["index.max_file_size"]);
    assert_eq!(out.stdout, "16MB\n");

    let out = config(&root, &["--list", "--show-origin"]);
    assert_eq!(
        out.stdout,
        "default             daemon.idle_timeout = 30m\n\
         .codix/config.toml  index.max_file_size = 16MB\n\
         default             index.scan_interval = 10m\n\
         codix.toml          languages.overrides.\"*.h\" = cpp\n"
    );

    let out = config(&root, &["--list"]);
    assert_eq!(
        out.stdout.lines().nth(1),
        Some("index.max_file_size = 16MB")
    );
}

#[test]
fn unknown_keys_and_invalid_values_write_nothing() {
    let (_guard, root) = project(Some(SHARED));
    let cases: [(&[&str], &str); 5] = [
        (
            &["index.max_file_sise", "1MB"],
            "unknown key `index.max_file_sise`",
        ),
        (
            &["index.max_file_size", "huge"],
            "invalid value `huge` for index.max_file_size",
        ),
        (
            &["languages.overrides.\"*.h\"", "cobol"],
            "invalid value `cobol`",
        ),
        (
            &["sources.roots", "../elsewhere"],
            "invalid value `../elsewhere`",
        ),
        (
            &["languages.overrides", "cpp"],
            "unknown key `languages.overrides`",
        ),
    ];
    for (args, cause) in cases {
        let out = config(&root, args);
        assert_eq!(out.code, Some(2), "{args:?}: {}", out.stderr);
        assert!(
            out.stderr
                .starts_with(&format!("error[invalid_config]: {cause}")),
            "{args:?}: {}",
            out.stderr
        );
        assert_eq!(read(root.join("codix.toml")), SHARED, "{args:?}");
        assert!(!root.join(".codix/config.toml").exists(), "{args:?}");
    }

    let out = config(&root, &["index.max_file_sise", "1MB"]);
    assert!(
        out.stderr
            .contains("hint: valid keys: daemon.idle_timeout, index.max_file_size, "),
        "{}",
        out.stderr
    );
}

#[test]
fn invalid_files_are_reported_and_left_alone() {
    let (_guard, root) = project(Some("[index]\nmax_file_size = \"lots\"\n"));
    let out = config(&root, &["index.scan_interval", "5m"]);
    assert_eq!(out.code, Some(2));
    assert!(
        out.stderr.starts_with(
            "error[invalid_config]: codix.toml: invalid value `\"lots\"` for index.max_file_size"
        ),
        "{}",
        out.stderr
    );
    assert!(!root.join(".codix/config.toml").exists());
}

#[test]
fn describe_comes_from_the_schema() {
    let (_guard, root) = project(None);
    let out = config(&root, &["--describe"]);
    assert_eq!(out.code, Some(0));
    assert_golden(
        format!(
            "{}/tests/golden/config-describe.txt",
            env!("CARGO_MANIFEST_DIR")
        ),
        &out.stdout,
    );

    let out = config(&root, &["--describe", "--json"]);
    assert_golden(
        format!(
            "{}/tests/golden/config-describe.json",
            env!("CARGO_MANIFEST_DIR")
        ),
        &out.stdout,
    );
}

#[test]
fn every_form_has_json() {
    let (_guard, root) = project(Some(SHARED));
    let json = |args: &[&str]| -> Value {
        let mut all = args.to_vec();
        all.push("--json");
        serde_json::from_str(&config(&root, &all).stdout).expect("JSON output")
    };
    let get = json(&["index.max_file_size"]);
    assert_eq!(get["result"]["settings"][0]["origin"], "shared");
    let set = json(&["sources.roots", "src,lib"]);
    assert_eq!(set["result"]["value"], serde_json::json!(["src", "lib"]));
    let unset = json(&["--unset", "sources.roots"]);
    assert_eq!(unset["result"]["files"], serde_json::json!(["codix.toml"]));
    let list = json(&["--list"]);
    assert_eq!(list["result"]["form"], "list");
}
