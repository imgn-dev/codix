//! `codix mcp`: JSON-RPC over the server's stdin and stdout, as an MCP client
//! sends it.
//!
//! Calls go to each project's daemon, which the server starts. Inside the
//! Claude Code sandbox no daemon can start, and calls run in-process; the
//! results are the same. `live_*` tests check the daemon itself, so they run
//! on CI; locally inside the sandbox, the suite runs with `--skip live_`.

use std::collections::HashMap;
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, ChildStdout, Command, Stdio};

use codix_testkit::fixture::{files_only, indexed};
use serde_json::{Value, json};

const CODIX: &str = env!("CARGO_BIN_EXE_codix");

/// A running `codix mcp`, initialized; on drop, its input closes and every
/// daemon it started is stopped.
struct Client {
    child: Child,
    input: Option<ChildStdin>,
    output: BufReader<ChildStdout>,
    /// Responses read while waiting for another, by ID.
    early: HashMap<u64, Value>,
    runtime: tempfile::TempDir,
}

impl Client {
    fn start(cwd: &Path) -> Self {
        let runtime = tempfile::tempdir().expect("runtime dir");
        let mut child = Command::new(CODIX)
            .arg("mcp")
            .current_dir(cwd)
            .env("CODIX_RUNTIME_DIR", runtime.path())
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .expect("start codix mcp");
        let input = child.stdin.take();
        let output = BufReader::new(child.stdout.take().expect("stdout"));
        let mut client = Self {
            child,
            input,
            output,
            early: HashMap::new(),
            runtime,
        };
        let init = client.request(
            1,
            "initialize",
            &json!({
                "protocolVersion": "2025-11-25",
                "capabilities": {},
                "clientInfo": { "name": "codix-tests", "version": "0" }
            }),
        );
        assert_eq!(init["result"]["serverInfo"]["name"], "codix", "{init}");
        client.send(&json!({ "jsonrpc": "2.0", "method": "notifications/initialized" }));
        client
    }

    fn send(&mut self, message: &Value) {
        let input = self.input.as_mut().expect("stdin open");
        writeln!(input, "{message}").expect("write");
        input.flush().expect("flush");
    }

    fn send_request(&mut self, id: u64, method: &str, params: &Value) {
        self.send(&json!({ "jsonrpc": "2.0", "id": id, "method": method, "params": params }));
    }

    fn response(&mut self, id: u64) -> Value {
        if let Some(response) = self.early.remove(&id) {
            return response;
        }
        loop {
            let mut line = String::new();
            assert!(
                self.output.read_line(&mut line).expect("read") > 0,
                "server exited"
            );
            let message: Value = serde_json::from_str(&line).expect("JSON-RPC");
            match message["id"].as_u64() {
                Some(got) if got == id => return message,
                Some(other) => {
                    self.early.insert(other, message);
                }
                None => {}
            }
        }
    }

    fn request(&mut self, id: u64, method: &str, params: &Value) -> Value {
        self.send_request(id, method, params);
        self.response(id)
    }

    /// Calls `tool` and returns its result.
    fn call(&mut self, id: u64, tool: &str, arguments: &Value) -> Value {
        let response = self.request(
            id,
            "tools/call",
            &json!({ "name": tool, "arguments": arguments }),
        );
        response["result"].clone()
    }
}

impl Drop for Client {
    fn drop(&mut self) {
        // Closing stdin ends the session; the server exits.
        drop(self.input.take());
        self.child.wait().ok();
        Command::new(CODIX)
            .args(["daemon", "stop", "--all"])
            .env("CODIX_RUNTIME_DIR", self.runtime.path())
            .output()
            .ok();
    }
}

fn text(result: &Value) -> &str {
    result["content"][0]["text"].as_str().unwrap_or_default()
}

const BILLING: &[(&str, &str)] = &[
    (
        "src/Invoice.java",
        "class Invoice {\n    long total() { return 0; }\n}\n",
    ),
    (
        "src/Cart.java",
        "class Cart {\n    long total() { return 1; }\n}\n",
    ),
];

#[test]
fn tools_are_the_operations_and_each_takes_a_cwd() {
    let (_guard, root) = files_only(&[]);
    let mut client = Client::start(&root);
    let listed = client.request(2, "tools/list", &json!({}));
    let tools = listed["result"]["tools"].as_array().expect("tools");
    let names: Vec<&str> = tools.iter().filter_map(|t| t["name"].as_str()).collect();
    assert_eq!(names, codix_core::dispatch::OPERATIONS);
    for tool in tools {
        assert_eq!(tool["inputSchema"]["type"], "object", "{tool}");
        assert!(
            tool["inputSchema"]["properties"]["cwd"].is_object(),
            "{tool}"
        );
        assert_eq!(
            tool["outputSchema"]["properties"]["schema_version"]["const"], 1,
            "{tool}"
        );
    }
}

#[cfg(feature = "lang-java")]
#[test]
fn results_carry_the_cli_text_and_json() {
    let (_guard, root) = indexed(CODIX, BILLING);
    let mut client = Client::start(&root);
    let result = client.call(
        2,
        "find",
        &json!({ "query": "Invoice", "cwd": root.join("src") }),
    );
    assert_eq!(result["isError"], false, "{result}");

    let cli = Command::new(CODIX)
        .args(["find", "Invoice"])
        .current_dir(root.join("src"))
        .output()
        .expect("codix find");
    assert_eq!(text(&result), String::from_utf8_lossy(&cli.stdout));
    let cli_json = Command::new(CODIX)
        .args(["find", "Invoice", "--json"])
        .current_dir(root.join("src"))
        .output()
        .expect("codix find --json");
    let expected: Value = serde_json::from_slice(&cli_json.stdout).expect("JSON");
    assert_eq!(result["structuredContent"], expected);
}

#[cfg(feature = "lang-java")]
#[test]
fn one_server_answers_for_several_projects() {
    let (_a_guard, a) = indexed(CODIX, &[("A.java", "class Alpha {}\n")]);
    let (_b_guard, b) = indexed(CODIX, &[("B.java", "class Beta {}\n")]);
    let mut client = Client::start(&a);
    let alpha = client.call(2, "find", &json!({ "query": "Alpha" }));
    assert!(text(&alpha).starts_with("A.java:1:7"), "{alpha}");
    let beta = client.call(3, "find", &json!({ "query": "Beta", "cwd": b }));
    assert!(text(&beta).starts_with("B.java:1:7"), "{beta}");
    let none = client.call(4, "find", &json!({ "query": "Beta", "cwd": a }));
    assert_eq!(
        none["structuredContent"]["result"]["results"]["total"], 0,
        "{none}"
    );
}

#[test]
fn calls_in_flight_are_answered_independently() {
    let (_guard, root) = indexed(CODIX, BILLING);
    let mut client = Client::start(&root);
    client.send_request(
        2,
        "tools/call",
        &json!({ "name": "grep", "arguments": { "pattern": "total" } }),
    );
    client.send_request(
        3,
        "tools/call",
        &json!({ "name": "status", "arguments": {} }),
    );
    let status = client.response(3);
    let grep = client.response(2);
    assert_eq!(status["result"]["isError"], false, "{status}");
    assert_eq!(
        grep["result"]["structuredContent"]["result"]["matches"]["total"], 2,
        "{grep}"
    );
}

#[test]
fn errors_are_tool_errors_that_say_what_to_do() {
    let (_guard, root) = indexed(CODIX, BILLING);
    let mut client = Client::start(&root);
    let unknown = client.call(2, "def", &json!({ "symbol": "Nowhere" }));
    assert_eq!(unknown["isError"], true, "{unknown}");
    assert!(
        text(&unknown).starts_with("error[unknown_symbol]: "),
        "{unknown}"
    );

    let bad = client.call(3, "find", &json!({ "query": "x", "limit": 0 }));
    assert_eq!(bad["isError"], true, "{bad}");
    assert!(text(&bad).starts_with("error[usage]: "), "{bad}");

    let (_elsewhere_guard, elsewhere) = files_only(&[]);
    let outside = client.call(4, "status", &json!({ "cwd": elsewhere }));
    assert!(
        text(&outside).contains("hint: call the init tool with cwd at the project root"),
        "{outside}"
    );
}

#[test]
fn init_through_mcp_creates_the_project() {
    let (_guard, root) = files_only(&[("notes.txt", "notes\n")]);
    let mut client = Client::start(&root);
    let init = client.call(2, "init", &json!({}));
    assert_eq!(init["isError"], false, "{init}");
    assert!(root.join(".codix/index.db").is_file());
    let status = client.call(3, "status", &json!({}));
    assert_eq!(status["isError"], false, "{status}");
}

#[test]
fn live_the_first_call_starts_the_projects_daemon() {
    let (_guard, root) = indexed(CODIX, BILLING);
    let mut client = Client::start(&root);
    let status = client.call(2, "status", &json!({}));
    assert!(text(&status).contains("daemon: pid "), "{status}");
    let listed = Command::new(CODIX)
        .args(["daemon", "list", "--json"])
        .env("CODIX_RUNTIME_DIR", client.runtime.path())
        .output()
        .expect("daemon list");
    let listed: Value = serde_json::from_slice(&listed.stdout).expect("JSON");
    assert_eq!(
        listed["result"]["daemons"][0]["daemon"]["root"],
        root.display().to_string()
    );
}
