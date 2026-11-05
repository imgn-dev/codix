//! Reading a `claude -p --output-format stream-json` transcript: the tool
//! calls with their results, the final answer, turns, tokens and cost.

use std::collections::HashMap;

use serde_json::Value;

/// The prefix Claude Code gives the tools of the `codix` MCP server.
const TOOL_PREFIX: &str = "mcp__codix__";

/// One tool call and what it returned.
#[derive(Debug, Clone, PartialEq)]
pub(crate) struct ToolCall {
    /// The tool, without the MCP prefix.
    pub(crate) tool: String,
    /// Its arguments.
    pub(crate) input: Value,
    /// The text it returned.
    pub(crate) result: String,
    /// Whether it returned an error.
    pub(crate) is_error: bool,
}

/// What Claude Code started the run with, from its `init` event.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Setup {
    /// Whether the event was there.
    pub(crate) seen: bool,
    /// Every tool the model was given.
    pub(crate) tools: Vec<String>,
    /// MCP servers, by name.
    pub(crate) mcp_servers: Vec<String>,
    /// Plugins loaded.
    pub(crate) plugins: Vec<String>,
    /// The output style.
    pub(crate) output_style: String,
}

impl Setup {
    /// What would make the run measure more than codix's own names and
    /// descriptions: other tools, servers, plugins, or an output style.
    pub(crate) fn leaks(&self) -> Vec<String> {
        if !self.seen {
            return vec!["no init event".to_owned()];
        }
        let mut leaks = Vec::new();
        for tool in self.tools.iter().filter(|t| !t.starts_with(TOOL_PREFIX)) {
            leaks.push(format!("tool {tool}"));
        }
        for server in self.mcp_servers.iter().filter(|s| *s != "codix") {
            leaks.push(format!("MCP server {server}"));
        }
        if !self.mcp_servers.iter().any(|s| s == "codix") {
            leaks.push("no codix MCP server".to_owned());
        }
        for plugin in &self.plugins {
            leaks.push(format!("plugin {plugin}"));
        }
        if !self.output_style.is_empty() && !self.output_style.eq_ignore_ascii_case("default") {
            leaks.push(format!("output style {}", self.output_style));
        }
        leaks
    }
}

/// What a run did.
#[derive(Debug, Clone, Default, PartialEq)]
pub(crate) struct Transcript {
    /// What the run started with.
    pub(crate) setup: Setup,
    /// Tool calls in order.
    pub(crate) calls: Vec<ToolCall>,
    /// The final answer.
    pub(crate) answer: String,
    /// Turns, as Claude Code counts them.
    pub(crate) turns: u64,
    /// Input tokens, cache reads and writes included.
    pub(crate) input_tokens: u64,
    /// Output tokens.
    pub(crate) output_tokens: u64,
    /// Cost in US dollars, as Claude Code reports it.
    pub(crate) cost_usd: f64,
    /// Why the run failed, when Claude Code says it did.
    pub(crate) error: Option<String>,
}

/// Reads a transcript, one JSON event per line; lines that are not JSON
/// are ignored.
pub(crate) fn parse(stream: &str) -> Transcript {
    let mut transcript = Transcript::default();
    let mut pending: HashMap<String, usize> = HashMap::new();
    for event in stream
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
    {
        let content = event["message"]["content"].as_array();
        match event["type"].as_str() {
            Some("system") if event["subtype"] == "init" => {
                let names = |key: &str, field: Option<&str>| -> Vec<String> {
                    event[key]
                        .as_array()
                        .into_iter()
                        .flatten()
                        .filter_map(|v| match field {
                            Some(field) => v[field].as_str(),
                            None => v.as_str(),
                        })
                        .map(str::to_owned)
                        .collect()
                };
                transcript.setup = Setup {
                    seen: true,
                    tools: names("tools", None),
                    mcp_servers: names("mcp_servers", Some("name")),
                    plugins: names("plugins", Some("name")),
                    output_style: event["output_style"]
                        .as_str()
                        .unwrap_or_default()
                        .to_owned(),
                };
            }
            Some("assistant") => {
                for block in content.into_iter().flatten() {
                    if block["type"] == "tool_use" {
                        let name = block["name"].as_str().unwrap_or_default();
                        pending.insert(
                            block["id"].as_str().unwrap_or_default().to_owned(),
                            transcript.calls.len(),
                        );
                        transcript.calls.push(ToolCall {
                            tool: name.strip_prefix(TOOL_PREFIX).unwrap_or(name).to_owned(),
                            input: block["input"].clone(),
                            result: String::new(),
                            is_error: false,
                        });
                    }
                }
            }
            Some("user") => {
                for block in content.into_iter().flatten() {
                    if block["type"] != "tool_result" {
                        continue;
                    }
                    let id = block["tool_use_id"].as_str().unwrap_or_default();
                    if let Some(call) = pending.remove(id).and_then(|i| transcript.calls.get_mut(i))
                    {
                        call.result = text_of(&block["content"]);
                        call.is_error = block["is_error"].as_bool().unwrap_or(false);
                    }
                }
            }
            Some("result") => {
                event["result"]
                    .as_str()
                    .unwrap_or_default()
                    .clone_into(&mut transcript.answer);
                transcript.turns = event["num_turns"].as_u64().unwrap_or(0);
                transcript.cost_usd = event["total_cost_usd"].as_f64().unwrap_or(0.0);
                let usage = &event["usage"];
                transcript.input_tokens = [
                    "input_tokens",
                    "cache_read_input_tokens",
                    "cache_creation_input_tokens",
                ]
                .iter()
                .filter_map(|k| usage[*k].as_u64())
                .sum();
                transcript.output_tokens = usage["output_tokens"].as_u64().unwrap_or(0);
                if event["is_error"] == true {
                    transcript.error = Some(transcript.answer.clone());
                }
            }
            _ => {}
        }
    }
    transcript
}

/// A tool result's text: a string, or the text blocks of a list.
fn text_of(content: &Value) -> String {
    match content {
        Value::String(text) => text.clone(),
        Value::Array(blocks) => blocks
            .iter()
            .filter_map(|b| b["text"].as_str())
            .collect::<Vec<_>>()
            .join("\n"),
        _ => String::new(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const STREAM: &str = r#"{"type":"system","subtype":"init","tools":["mcp__codix__find"],"mcp_servers":[{"name":"codix","status":"connected"}],"plugins":[],"output_style":"default"}
{"type":"assistant","message":{"content":[{"type":"text","text":"Looking."},{"type":"tool_use","id":"t1","name":"mcp__codix__def","input":{"symbol":"Assert.notNull"}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t1","is_error":true,"content":[{"type":"text","text":"error[ambiguous_symbol]: \"Assert.notNull\" matches 2 symbols"}]}]}}
{"type":"assistant","message":{"content":[{"type":"tool_use","id":"t2","name":"mcp__codix__show","input":{"symbol":"Assert.notNull","path":["spring-core/src/main"]}}]}}
{"type":"user","message":{"content":[{"type":"tool_result","tool_use_id":"t2","content":"throw new IllegalArgumentException(message);"}]}}
not json
{"type":"result","subtype":"success","is_error":false,"num_turns":3,"result":"It throws IllegalArgumentException.","total_cost_usd":0.0123,"usage":{"input_tokens":10,"cache_read_input_tokens":1000,"cache_creation_input_tokens":200,"output_tokens":50}}
"#;

    #[test]
    fn tool_calls_answer_and_usage_are_read() {
        let t = parse(STREAM);
        assert_eq!(t.calls.len(), 2);
        assert_eq!(t.calls[0].tool, "def");
        assert!(t.calls[0].is_error);
        assert!(t.calls[0].result.starts_with("error[ambiguous_symbol]"));
        assert_eq!(t.calls[1].tool, "show");
        assert_eq!(t.calls[1].input["path"][0], "spring-core/src/main");
        assert!(!t.calls[1].is_error);
        assert_eq!(t.answer, "It throws IllegalArgumentException.");
        assert_eq!((t.turns, t.input_tokens, t.output_tokens), (3, 1210, 50));
        assert!((t.cost_usd - 0.0123).abs() < 1e-9);
        assert_eq!(t.error, None);
        assert_eq!(t.setup.leaks(), Vec::<String>::new());
    }

    #[test]
    fn anything_but_codix_in_the_setup_is_a_leak() {
        let t = parse(
            r#"{"type":"system","subtype":"init","tools":["Bash","mcp__codix__find"],"mcp_servers":[{"name":"codix"},{"name":"other"}],"plugins":[{"name":"frontend-design"}],"output_style":"Concise"}"#,
        );
        assert_eq!(
            t.setup.leaks(),
            [
                "tool Bash",
                "MCP server other",
                "plugin frontend-design",
                "output style Concise"
            ]
        );
        assert_eq!(parse("").setup.leaks(), ["no init event"]);
    }

    #[test]
    fn a_failed_run_says_why() {
        let t =
            parse(r#"{"type":"result","is_error":true,"result":"Not logged in","num_turns":1}"#);
        assert_eq!(t.error.as_deref(), Some("Not logged in"));
    }
}
