//! The JSON output contract: every request, response, envelope and error
//! schema is checked in under `schemas/v<N>/`, N being `SCHEMA_VERSION`.
//!
//! An additive change is blessed with `CODIX_BLESS=1`. A breaking one needs
//! `SCHEMA_VERSION` incremented first, which blesses into a new directory;
//! blessing it in place fails.

use std::collections::BTreeSet;
use std::fs;
use std::path::PathBuf;

use codix_core::dispatch::schemas;
use codix_core::render::{SCHEMA_VERSION, envelope_schema, error_schema};
use codix_testkit::BLESS_VAR;
use codix_testkit::schema::{Side, breaking_changes};
use serde_json::Value;

fn generated() -> Vec<(String, Value, Side)> {
    let mut all = Vec::new();
    for (name, request, response) in schemas() {
        all.push((
            format!("{name}.request.json"),
            request.to_value(),
            Side::Request,
        ));
        all.push((
            format!("{name}.response.json"),
            response.to_value(),
            Side::Response,
        ));
    }
    all.push((
        "envelope.json".to_owned(),
        envelope_schema().to_value(),
        Side::Response,
    ));
    all.push((
        "error.json".to_owned(),
        error_schema().to_value(),
        Side::Response,
    ));
    all
}

#[test]
fn json_schemas_match_their_snapshots() {
    let dir = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("../../schemas")
        .join(format!("v{SCHEMA_VERSION}"));
    let bless = std::env::var_os(BLESS_VAR).is_some_and(|v| v == "1");
    let bump = format!(
        "a breaking change: increment SCHEMA_VERSION in crates/codix-core/src/render.rs, then bless into schemas/v{}",
        SCHEMA_VERSION + 1
    );
    let mut problems = Vec::new();
    let generated = generated();
    for (file, schema, side) in &generated {
        let path = dir.join(file);
        let text = serde_json::to_string_pretty(schema).expect("schema JSON") + "\n";
        match fs::read_to_string(&path) {
            Ok(snapshot) if snapshot == text => {}
            Ok(snapshot) => {
                let old: Value = serde_json::from_str(&snapshot).expect("snapshot JSON");
                let breaking = breaking_changes(&old, schema, *side);
                if !breaking.is_empty() {
                    problems.push(format!("{file}: {bump}:\n  {}", breaking.join("\n  ")));
                } else if bless {
                    fs::write(&path, text).expect("write snapshot");
                } else {
                    problems.push(format!(
                        "{file}: an additive change; run with {BLESS_VAR}=1 to update the snapshot"
                    ));
                }
            }
            Err(_) if bless => {
                fs::create_dir_all(&dir).expect("create the snapshot directory");
                fs::write(&path, text).expect("write snapshot");
            }
            Err(_) => problems.push(format!(
                "{file}: no snapshot; run with {BLESS_VAR}=1 to create it"
            )),
        }
    }
    let names: BTreeSet<&str> = generated.iter().map(|(file, _, _)| file.as_str()).collect();
    for entry in fs::read_dir(&dir).into_iter().flatten().flatten() {
        let file = entry.file_name().to_string_lossy().into_owned();
        if !names.contains(file.as_str()) {
            problems.push(format!("{file}: its command is gone, {bump}"));
        }
    }
    assert!(problems.is_empty(), "{}", problems.join("\n"));
}
