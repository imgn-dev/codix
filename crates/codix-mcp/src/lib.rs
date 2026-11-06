//! `codix mcp`: the operations as MCP tools over stdio, with the same names,
//! descriptions and results as the CLI.
//!
//! The server is a router. Each call may name a `cwd`; the project is
//! discovered from it, and the call goes to that project's daemon, started
//! on the first call for it. Paths in results are relative to `cwd`.

use std::collections::HashSet;
use std::fmt::Write as _;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, PoisonError};

use codix_core::Error;
use codix_core::capabilities::{CapabilitiesOp, LanguagesOp};
use codix_core::config::ConfigOp;
use codix_core::daemon::running;
use codix_core::describe::describe;
use codix_core::indexing::{IndexOp, InitOp, StatusOp};
use codix_core::op::{Context, Operation, Warning};
use codix_core::previews::{DiscardOp, PreviewsOp};
use codix_core::query::{DefOp, FindOp, GrepOp, OutlineOp, ShowOp};
use codix_core::refs::RefsOp;
use codix_core::rename::RenameOp;
use codix_core::render::{Style, TextRender};
use codix_daemon::client::{self, Route};
use codix_daemon::control;
use rmcp::ErrorData as McpError;
use rmcp::ServiceExt;
use rmcp::handler::server::ServerHandler;
use rmcp::model::{
    CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation,
    JsonObject, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
};
use rmcp::service::{RequestContext, RoleServer};
use serde_json::Value;

/// The argument every tool takes besides its request.
const CWD: &str = "cwd";

/// Serves MCP on stdin and stdout until the client disconnects. `start` is
/// the default `cwd`; `exe` is the codix binary daemons are started with.
pub fn serve(start: PathBuf, exe: PathBuf) -> Result<(), Error> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .map_err(|source| Error::Io {
            path: "tokio runtime".into(),
            source,
        })?;
    runtime.block_on(async move {
        let router = Router {
            start: Arc::new(start),
            exe: Arc::new(exe),
            failed: Arc::default(),
        };
        let service =
            router
                .serve(rmcp::transport::stdio())
                .await
                .map_err(|e| Error::Internal {
                    message: format!("MCP initialization failed: {e}"),
                })?;
        service.waiting().await.map_err(|e| Error::Internal {
            message: format!("MCP server stopped: {e}"),
        })?;
        Ok(())
    })
}

#[derive(Debug, Clone)]
struct Router {
    start: Arc<PathBuf>,
    exe: Arc<PathBuf>,
    /// Roots whose daemon failed to start: their calls run in-process
    /// rather than try again each time.
    failed: Arc<Mutex<HashSet<PathBuf>>>,
}

impl ServerHandler for Router {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("codix", env!("CARGO_PKG_VERSION")))
            .with_instructions(
                "codix indexes source code. Prefer find, def, outline and show to grep and \
                 to reading whole files. Pass cwd, the directory you work in, to every call.",
            )
    }

    fn list_tools(
        &self,
        _: Option<PaginatedRequestParams>,
        _: RequestContext<RoleServer>,
    ) -> impl Future<Output = Result<ListToolsResult, McpError>> + Send + '_ {
        std::future::ready(Ok(ListToolsResult::with_all_items(tools())))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        _: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let router = self.clone();
        let result = tokio::task::spawn_blocking(move || router.call(&request))
            .await
            .map_err(|e| McpError::internal_error(format!("tool call failed: {e}"), None))?;
        Ok(result.into())
    }
}

macro_rules! operations {
    ($($op:ident),* $(,)?) => {
        /// One tool per operation.
        pub fn tools() -> Vec<Tool> {
            vec![$(tool::<$op>()),*]
        }

        impl Router {
            fn call(&self, request: &CallToolRequestParams) -> CallToolResult {
                let mut arguments = request.arguments.clone().unwrap_or_default();
                let cwd = match arguments.remove(CWD) {
                    None | Some(Value::Null) => self.start.as_ref().clone(),
                    Some(Value::String(cwd)) => self.start.join(cwd),
                    Some(other) => {
                        return failure(
                            &usage(format!("cwd must be a string, not {other}")),
                            &[],
                        );
                    }
                };
                $(
                    if request.name == <$op as Operation>::NAME {
                        return self.run(&$op, &cwd, arguments);
                    }
                )*
                failure(
                    &usage(format!("unknown tool `{}`", request.name)),
                    &[],
                )
            }
        }
    };
}

operations!(
    InitOp,
    IndexOp,
    StatusOp,
    CapabilitiesOp,
    LanguagesOp,
    ConfigOp,
    FindOp,
    DefOp,
    RefsOp,
    OutlineOp,
    ShowOp,
    GrepOp,
    RenameOp,
    PreviewsOp,
    DiscardOp,
);

/// The tool for `O`: its name and canonical description, and its request's
/// schema plus `cwd`. It declares no output schema and its results carry no
/// structured content: clients then give the model the compact text, as the
/// CLI prints it, rather than the much larger JSON.
fn tool<O>() -> Tool
where
    O: Operation,
    O::Request: schemars::JsonSchema,
{
    let description =
        describe(O::NAME).map_or_else(String::new, codix_core::describe::Description::details);
    let mut input = object(schemars::schema_for!(O::Request));
    input.insert("type".to_owned(), Value::from("object"));
    let properties = input
        .entry("properties")
        .or_insert_with(|| Value::Object(JsonObject::new()));
    if let Value::Object(properties) = properties {
        properties.insert(
            CWD.to_owned(),
            serde_json::json!({
                "type": "string",
                "description": "The directory to run from, as a shell's current directory: the \
                    project is found from there, and paths in results are relative to it. \
                    Defaults to where the server started."
            }),
        );
    }
    Tool::new(O::NAME, description, Arc::new(input))
}

fn object(schema: schemars::Schema) -> JsonObject {
    match schema.to_value() {
        Value::Object(object) => object,
        _ => JsonObject::new(),
    }
}

impl Router {
    fn run<O>(&self, op: &O, cwd: &Path, arguments: JsonObject) -> CallToolResult
    where
        O: Operation,
        O::Response: TextRender,
    {
        let request: O::Request = match serde_json::from_value(Value::Object(arguments)) {
            Ok(request) => request,
            Err(e) => return failure(&usage(format!("invalid arguments: {e}")), &[]),
        };
        let ctx = match Context::locate(cwd, None) {
            Ok(ctx) => ctx,
            Err(error) => return failure(&error, &[]),
        };
        if O::NAME != InitOp::NAME {
            self.ensure_daemon(&ctx);
        }
        let (outcome, warnings) = match client::call::<O>(&ctx, &request) {
            Route::Daemon { outcome, warnings } => (outcome, warnings),
            Route::InProcess { warnings } => {
                let ctx = ctx.clone().with_project_warnings();
                let mut all = ctx.warnings().to_vec();
                all.extend(warnings);
                (op.run(&ctx, request), all)
            }
        };
        let result = match outcome {
            Ok(response) => success::<O>(&response, &warnings),
            Err(error) => failure(&error, &warnings),
        };
        // A project `init` just created has no daemon yet.
        if O::NAME == InitOp::NAME
            && let Ok(ctx) = Context::locate(cwd, None)
        {
            self.ensure_daemon(&ctx);
        }
        result
    }

    /// Starts the daemon of the context's project if it has none; without one,
    /// the call runs in-process.
    fn ensure_daemon(&self, ctx: &Context) {
        let Ok(project) = ctx.project() else {
            return;
        };
        let mut failed = self.failed.lock().unwrap_or_else(PoisonError::into_inner);
        if project.require_indexed().is_err()
            || failed.contains(project.root())
            || !matches!(running(project), Ok(None))
        {
            return;
        }
        match control::start(project, &self.exe) {
            // Another server may have started it meanwhile.
            Ok(_) | Err(Error::DaemonRunning { .. }) => {}
            Err(_) => {
                failed.insert(project.root().to_path_buf());
            }
        }
    }
}

fn success<O>(response: &O::Response, warnings: &[Warning]) -> CallToolResult
where
    O: Operation,
    O::Response: TextRender,
{
    let mut text = warning_lines(warnings);
    let mut rendered = Vec::new();
    let style = Style {
        color: false,
        show_origin: false,
        tools: true,
    };
    if let Err(e) = response.render_text(&mut rendered, style) {
        return failure(
            &Error::Internal {
                message: format!("cannot render the result: {e}"),
            },
            warnings,
        );
    }
    text.push_str(&String::from_utf8_lossy(&rendered));
    CallToolResult::success(vec![ContentBlock::text(text)])
}

/// An error result: text only, since the tool's output schema describes
/// successful results.
fn failure(error: &Error, warnings: &[Warning]) -> CallToolResult {
    let hint = match error.code() {
        // The CLI's hint names a command; here, the tool.
        "no_project" | "not_indexed" => {
            "call the init tool with cwd at the project root".to_owned()
        }
        _ => error.hint().replace("`codix find <name>`", "the find tool"),
    };
    let mut text = warning_lines(warnings);
    // Writing to a String cannot fail.
    let _ = writeln!(text, "error[{}]: {}", error.code(), error.cause());
    for candidate in error.candidates() {
        let _ = writeln!(text, "  {candidate}");
    }
    for site in error.sites() {
        let l = &site.location;
        let _ = writeln!(text, "  {}:{}:{}  {}", l.path, l.line, l.column, site.note);
    }
    let _ = writeln!(text, "hint: {hint}");
    CallToolResult::error(vec![ContentBlock::text(text)])
}

fn warning_lines(warnings: &[Warning]) -> String {
    let mut text = String::new();
    for w in warnings {
        // Writing to a String cannot fail.
        let _ = writeln!(text, "warning[{}]: {}", w.code, w.message);
    }
    text
}

fn usage(cause: String) -> Error {
    Error::Usage {
        cause,
        hint: "check the tool's input schema".to_owned(),
    }
}
