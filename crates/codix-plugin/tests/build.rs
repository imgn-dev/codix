//! A build plugin that fails, hangs, or answers badly gives an error naming
//! the cause, never a model.

use std::time::Duration;

use codix_plugin::build::run_within;
use codix_testkit::plugin::script as fake;

// One test, so no other test forks while a fake is open for writing.
#[test]
fn failures_are_errors_with_their_cause() {
    let dir = tempfile::tempdir().expect("temp dir");
    let root = dir.path();
    let cases = [
        (
            fake(
                root,
                "exits",
                "echo 'no pom here' >&2; exit 1",
                "echo no pom here 1>&2\r\nexit /b 1",
            ),
            "): no pom here",
        ),
        (
            fake(
                root,
                "hangs",
                "exec sleep 60",
                "ping -n 60 127.0.0.1 >nul 2>nul",
            ),
            "no answer within 1 s",
        ),
        (
            fake(root, "garbage", "echo not json", "echo not json"),
            "bad answer: ",
        ),
        (
            fake(
                root,
                "future",
                r#"echo '{"protocol": 2, "model": {"modules": [], "unsupported": []}}'"#,
                r#"echo {"protocol": 2, "model": {"modules": [], "unsupported": []}}"#,
            ),
            "protocol 2, this codix speaks protocol 1",
        ),
        (root.join("absent"), "absent: "),
    ];
    for (plugin, cause) in cases {
        let error = run_within(&plugin, root, Duration::from_secs(1))
            .expect_err("no model")
            .to_string();
        assert!(
            error.starts_with(cause) || error.contains(cause),
            "{error} / {cause}"
        );
    }
}

/// The answer's schema, for plugin authors. A change that breaks it needs a
/// new protocol version.
#[test]
fn the_protocol_schema_matches_its_snapshot() {
    let schema = schemars::schema_for!(codix_provider::Answer);
    let text = serde_json::to_string_pretty(&schema).expect("schema JSON") + "\n";
    codix_testkit::assert_golden(
        std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../schemas/plugins/build-1.json"),
        &text,
    );
}
