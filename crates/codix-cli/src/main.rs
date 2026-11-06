//! The `codix` command-line interface: parses arguments into operation
//! requests and renders the responses. It holds no logic of its own.

// Providers register themselves at link time; naming each enabled one here is
// what links it in.
#[cfg(feature = "build-gradle")]
use codix_build_gradle as _;
#[cfg(feature = "build-maven")]
use codix_build_maven as _;
#[cfg(feature = "lang-c")]
use codix_lang_c as _;
#[cfg(feature = "lang-cpp")]
use codix_lang_cpp as _;
#[cfg(feature = "lang-csharp")]
use codix_lang_csharp as _;
#[cfg(feature = "lang-go")]
use codix_lang_go as _;
#[cfg(feature = "lang-java")]
use codix_lang_java as _;
#[cfg(feature = "lang-javascript")]
use codix_lang_javascript as _;
#[cfg(feature = "lang-json")]
use codix_lang_json as _;
#[cfg(feature = "lang-kotlin")]
use codix_lang_kotlin as _;
#[cfg(feature = "lang-markdown")]
use codix_lang_markdown as _;
#[cfg(feature = "lang-php")]
use codix_lang_php as _;
#[cfg(feature = "lang-python")]
use codix_lang_python as _;
#[cfg(feature = "lang-ruby")]
use codix_lang_ruby as _;
#[cfg(feature = "lang-rust")]
use codix_lang_rust as _;
#[cfg(feature = "lang-sql")]
use codix_lang_sql as _;
#[cfg(feature = "lang-toml")]
use codix_lang_toml as _;
#[cfg(feature = "lang-typescript")]
use codix_lang_typescript as _;
#[cfg(feature = "lang-xml")]
use codix_lang_xml as _;
#[cfg(feature = "lang-yaml")]
use codix_lang_yaml as _;
#[cfg(feature = "vcs-git")]
use codix_vcs_git as _;
#[cfg(feature = "vcs-hg")]
use codix_vcs_hg as _;
#[cfg(feature = "vcs-svn")]
use codix_vcs_svn as _;

use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::path::PathBuf;
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{Args, CommandFactory, FromArgMatches, Parser, Subcommand};
use codix_core::Error;
use codix_core::apply::{ApplyOp, ApplyRequest};
use codix_core::capabilities::{
    CapabilitiesOp, CapabilitiesRequest, LanguagesOp, LanguagesRequest,
};
use codix_core::config::{ConfigOp, ConfigRequest};
use codix_core::describe::describe;
use codix_core::error::exit;
use codix_core::indexing::{IndexOp, IndexRequest, InitOp, InitRequest, StatusOp, StatusRequest};
use codix_core::op::{Context, Operation, Outcome, Warning};
use codix_core::page::{DEFAULT_LIMIT, Page};
use codix_core::preflight::{Allow, Overrides};
use codix_core::previews::{DiscardOp, DiscardRequest, PreviewsOp, PreviewsRequest};
use codix_core::project::ScopeRequest;
use codix_core::query::{
    DefOp, DefRequest, FindOp, FindRequest, GrepOp, GrepRequest, OutlineOp, OutlineRequest, ShowOp,
    ShowRequest,
};
use codix_core::refs::{RefsOp, RefsRequest};
use codix_core::rename::{RenameOp, RenameRequest};
use codix_core::render::{self, Style, TextRender};
use codix_core::store::MatchMode;
use codix_daemon::client::{self, Route};
use codix_daemon::{control, server};

/// Headless code index for LLM agents.
#[derive(Parser)]
// `bin_name` keeps messages naming `codix` whatever argv[0] is (`codix.exe`,
// a renamed copy, a symlink).
#[command(
    name = "codix",
    bin_name = "codix",
    version,
    subcommand_required = true,
    arg_required_else_help = true
)]
struct Cli {
    /// Print JSON instead of text.
    #[arg(long, global = true)]
    json: bool,

    /// Use the project at this directory instead of searching upward.
    #[arg(long, global = true, value_name = "PATH")]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

/// The commands. Their descriptions come from `codix_core::describe`, applied
/// by [`described`], so `--help`, MCP and the docs say the same thing.
#[derive(Subcommand)]
enum Command {
    Init {
        /// Also write a workflow guide for agents to this file, relative to the root.
        #[arg(
            long,
            value_name = "PATH",
            num_args = 0..=1,
            default_missing_value = "CODIX.md"
        )]
        agent_docs: Option<PathBuf>,
    },
    Index {
        /// Rebuild the index from nothing.
        #[arg(long)]
        full: bool,
    },
    Status,
    Capabilities,
    Languages,
    Config(ConfigArgs),
    Find(FindArgs),
    Def {
        /// Symbol ID, its path part, `file:line:col`, or a name.
        symbol: String,
        #[command(flatten)]
        narrow: NarrowArgs,
    },
    Refs(RefsArgs),
    Rename(RenameArgs),
    Apply {
        /// The preview ID, as `rename` or `undo` returned it.
        id: String,
    },
    Previews,
    Discard {
        /// Preview IDs to drop.
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        ids: Vec<String>,
        /// Drop every pending preview.
        #[arg(long)]
        all: bool,
    },
    Outline {
        /// File or directory.
        path: String,
    },
    Show {
        /// Symbol ID, its path part, `file:line:col`, or a name.
        symbol: String,
        #[command(flatten)]
        narrow: NarrowArgs,
        /// Lines shown before and after the symbol.
        #[arg(long, default_value_t = 0, value_name = "N")]
        context: usize,
    },
    Grep(GrepArgs),
    Watch,
    #[command(subcommand)]
    Daemon(DaemonCommand),
    Mcp,
}

#[derive(Subcommand)]
enum DaemonCommand {
    Start,
    Stop {
        /// Stop every running daemon, whatever its project.
        #[arg(long)]
        all: bool,
    },
    List,
    /// Run the daemon in the foreground; `daemon start` runs this, detached.
    #[command(hide = true)]
    Run {
        /// Append log lines to this file instead of standard error.
        #[arg(long, value_name = "FILE")]
        log: Option<PathBuf>,
    },
}

#[derive(Args)]
struct ConfigArgs {
    /// Dotted key, such as `index.max_file_size` or `languages.overrides."*.h"`.
    #[arg(required_unless_present_any = ["list", "describe"])]
    key: Option<String>,
    /// New value; without it, the effective value is printed.
    #[arg(conflicts_with_all = ["unset", "list", "describe"])]
    value: Option<String>,
    #[command(flatten)]
    form: ConfigForm,
    /// With --list, the file each value comes from.
    #[arg(long, requires = "list")]
    show_origin: bool,
}

#[derive(Args)]
#[group(multiple = false)]
struct ConfigForm {
    /// Remove the key, back to its default.
    #[arg(long, requires = "key")]
    unset: bool,
    /// Every effective value.
    #[arg(long, conflicts_with = "key")]
    list: bool,
    /// Meaning, type, default, allowed values and file of a key, or of every key.
    #[arg(long)]
    describe: bool,
}

impl ConfigArgs {
    fn request(self) -> (ConfigRequest, bool) {
        let form = self.form;
        let request = match (self.key, self.value) {
            _ if form.list => ConfigRequest::List,
            (key, _) if form.describe => ConfigRequest::Describe { key },
            (Some(key), _) if form.unset => ConfigRequest::Unset { key },
            (Some(key), Some(value)) => ConfigRequest::Set { key, value },
            (Some(key), None) => ConfigRequest::Get { key },
            (None, _) => unreachable!("clap requires a key without --list or --describe"),
        };
        (request, self.show_origin)
    }
}

#[derive(Clone, Copy, clap::ValueEnum)]
enum ModeArg {
    Exact,
    Prefix,
    Fuzzy,
    Glob,
}

impl From<ModeArg> for MatchMode {
    fn from(mode: ModeArg) -> Self {
        match mode {
            ModeArg::Exact => Self::Exact,
            ModeArg::Prefix => Self::Prefix,
            ModeArg::Fuzzy => Self::Fuzzy,
            ModeArg::Glob => Self::Glob,
        }
    }
}

#[derive(Args)]
#[group(id = "mode", multiple = false)]
struct ModeArgs {
    /// The name equals the query.
    #[arg(long, num_args = 0, default_missing_value = "exact")]
    exact: Option<ModeArg>,
    /// The name starts with the query, ignoring case.
    #[arg(long, num_args = 0, default_missing_value = "prefix")]
    prefix: Option<ModeArg>,
    /// The name is within a few edits of the query, ignoring case.
    #[arg(long, num_args = 0, default_missing_value = "fuzzy")]
    fuzzy: Option<ModeArg>,
    /// The qualified name matches a glob, such as `Invoice.*`.
    #[arg(long, num_args = 0, default_missing_value = "glob")]
    glob: Option<ModeArg>,
}

impl ModeArgs {
    fn mode(&self) -> Option<MatchMode> {
        self.exact
            .or(self.prefix)
            .or(self.fuzzy)
            .or(self.glob)
            .map(MatchMode::from)
    }
}

#[derive(Args)]
struct FindArgs {
    /// Name to look for.
    query: String,
    #[command(flatten)]
    mode: ModeArgs,
    /// Keep symbols of this kind, such as method; repeatable.
    #[arg(long = "kind", value_name = "KIND")]
    kinds: Vec<String>,
    /// Keep symbols of this language; repeatable.
    #[arg(long = "lang", value_name = "LANGUAGE")]
    languages: Vec<String>,
    #[command(flatten)]
    scope: ScopeArgs,
    #[command(flatten)]
    page: PageArgs,
}

#[derive(Args)]
struct RefsArgs {
    /// Symbol ID, its path part, `file:line:col`, or a name.
    symbol: String,
    /// Consider symbols of this kind only; repeatable.
    #[arg(long = "kind", value_name = "KIND")]
    kinds: Vec<String>,
    #[command(flatten)]
    scope: ScopeArgs,
    #[command(flatten)]
    page: PageArgs,
}

#[derive(Args)]
struct RenameArgs {
    /// Symbol ID, its path part, `file:line:col`, or a name; a local by `file:line:col` only.
    symbol: String,
    /// The new name.
    new_name: String,
    #[command(flatten)]
    narrow: NarrowArgs,
    /// Rename every overload of the name together.
    #[arg(long)]
    all_overloads: bool,
    /// Allow a condition the checks refuse; repeatable.
    #[arg(long, value_enum, value_name = "CONDITION")]
    allow: Vec<AllowArg>,
    /// Edit the sites codix binds and leave unchanged the references it cannot bind, in other files or through other objects.
    #[arg(long)]
    exclude_unresolved: bool,
    /// At tier 1, where codix cannot bind names, rename every whole-word match of the name in its file.
    #[arg(long)]
    textual: bool,
}

impl RenameArgs {
    fn request(self) -> RenameRequest {
        RenameRequest {
            symbol: self.symbol,
            new_name: self.new_name,
            kinds: self.narrow.kinds,
            scope: self.narrow.scope.request(),
            all_overloads: self.all_overloads,
            overrides: Overrides {
                allow: self.allow.into_iter().map(Allow::from).collect(),
                exclude_unresolved: self.exclude_unresolved,
                textual: self.textual,
            },
        }
    }
}

impl RefsArgs {
    fn request(self) -> Result<RefsRequest, Error> {
        let page = self.page.page()?;
        Ok(RefsRequest {
            symbol: self.symbol,
            kinds: self.kinds,
            scope: self.scope.request(),
            page,
        })
    }
}

/// The conditions `--allow` takes.
#[derive(Clone, Copy, clap::ValueEnum)]
enum AllowArg {
    Shadowing,
    Overload,
    VisibilityChange,
    Generated,
}

impl From<AllowArg> for Allow {
    fn from(arg: AllowArg) -> Self {
        match arg {
            AllowArg::Shadowing => Self::Shadowing,
            AllowArg::Overload => Self::Overload,
            AllowArg::VisibilityChange => Self::VisibilityChange,
            AllowArg::Generated => Self::Generated,
        }
    }
}

/// Narrowing an ambiguous name.
#[derive(Args)]
struct NarrowArgs {
    /// Consider symbols of this kind only; repeatable.
    #[arg(long = "kind", value_name = "KIND")]
    kinds: Vec<String>,
    #[command(flatten)]
    scope: ScopeArgs,
}

#[derive(Args)]
struct GrepArgs {
    /// Regular expression, matched line by line.
    pattern: String,
    /// Search files of this language; repeatable.
    #[arg(long = "lang", value_name = "LANGUAGE")]
    languages: Vec<String>,
    /// Keep matches inside a symbol of this kind; repeatable.
    #[arg(long = "kind", value_name = "KIND")]
    kinds: Vec<String>,
    #[command(flatten)]
    scope: ScopeArgs,
    #[command(flatten)]
    page: PageArgs,
}

#[derive(Args)]
struct ScopeArgs {
    /// Limit results to the current directory's subtree.
    #[arg(long)]
    here: bool,
    /// Limit results to this subtree; repeatable.
    #[arg(long, value_name = "DIR")]
    path: Vec<String>,
}

impl ScopeArgs {
    fn request(self) -> ScopeRequest {
        ScopeRequest {
            here: self.here,
            paths: self.path,
        }
    }
}

#[derive(Args)]
struct PageArgs {
    /// Maximum number of results.
    #[arg(long, default_value_t = DEFAULT_LIMIT)]
    limit: usize,
    /// Number of results to skip.
    #[arg(long, default_value_t = 0)]
    offset: usize,
}

impl PageArgs {
    fn page(&self) -> Result<Page, Error> {
        Page::new(self.limit, self.offset)
    }
}

/// Where and how to write.
#[derive(Clone)]
struct Output {
    json: bool,
    /// `config --list --show-origin`.
    show_origin: bool,
    stdout: Style,
    stderr: Style,
    /// Warnings about the project, added to every response and error.
    warnings: Vec<Warning>,
}

impl Output {
    fn detect(json: bool) -> Self {
        let color_allowed = env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
        Self {
            json,
            show_origin: false,
            stdout: Style {
                color: color_allowed && io::stdout().is_terminal(),
                show_origin: false,
                tools: false,
            },
            stderr: Style {
                color: color_allowed && io::stderr().is_terminal(),
                show_origin: false,
                tools: false,
            },
            warnings: Vec::new(),
        }
    }

    /// A one-line note on stderr while a long command runs, on a terminal only.
    fn progress(&self, what: &str) {
        if !self.json && io::stderr().is_terminal() {
            let mut stderr = io::stderr().lock();
            // Overwritten by the next line; a failed write only loses the note.
            write!(stderr, "{what}…\r")
                .and_then(|()| stderr.flush())
                .ok();
        }
    }

    /// In text mode, warnings go to stderr before the output.
    fn text_warnings(&self) -> io::Result<()> {
        if self.json {
            return Ok(());
        }
        render::text_warnings(&mut io::stderr().lock(), &self.warnings, self.stderr)
    }

    fn response<R: serde::Serialize + TextRender + Outcome>(
        &self,
        command: &str,
        response: &R,
    ) -> u8 {
        let mut stdout = io::stdout().lock();
        let written = if self.json {
            render::json_response(&mut stdout, command, response, &self.warnings)
        } else if let Err(e) = self.text_warnings() {
            Err(e)
        } else {
            let style = Style {
                show_origin: self.show_origin,
                ..self.stdout
            };
            response.render_text(&mut stdout, style)
        };
        let code = if response.has_results() {
            exit::SUCCESS
        } else {
            exit::NO_RESULTS
        };
        finish(written.and_then(|()| stdout.flush()), code)
    }

    fn error(&self, error: &Error) -> u8 {
        let written = if self.json {
            let mut stdout = io::stdout().lock();
            render::json_error(&mut stdout, error, &self.warnings).and_then(|()| stdout.flush())
        } else {
            self.text_warnings()
                .and_then(|()| render::text_error(&mut io::stderr().lock(), error, self.stderr))
        };
        finish(written, error.exit_code())
    }
}

/// A closed pipe (`codix … | head`) is not a failure; other write errors are.
fn finish(written: io::Result<()>, code: u8) -> u8 {
    match written {
        Err(e) if e.kind() != io::ErrorKind::BrokenPipe => exit::INTERNAL,
        _ => code,
    }
}

/// Runs an operation through the project's daemon when one runs, otherwise
/// in-process; both render the same way.
fn run<O>(output: &Output, op: &O, ctx: &Context, request: Result<O::Request, Error>) -> u8
where
    O: Operation,
    O::Response: TextRender,
{
    let request = match request {
        Ok(request) => request,
        Err(error) => return output.error(&error),
    };
    let (outcome, warnings) = match client::call::<O>(ctx, &request) {
        Route::Daemon { outcome, warnings } => (outcome, warnings),
        Route::InProcess { warnings } => {
            let ctx = ctx.clone().with_project_warnings();
            let mut all = ctx.warnings().to_vec();
            all.extend(warnings);
            (op.run(&ctx, request), all)
        }
    };
    let output = Output {
        warnings,
        ..*output
    };
    match outcome {
        Ok(response) => output.response(O::NAME, &response),
        Err(error) => output.error(&error),
    }
}

fn daemon(output: &Output, ctx: &Context, command: DaemonCommand) -> u8 {
    match command {
        DaemonCommand::Start => {
            let result = ctx.project().and_then(|project| {
                let exe = env::current_exe().map_err(|source| Error::Io {
                    path: "codix".into(),
                    source,
                })?;
                control::start(project, &exe)
            });
            output_of(output, "daemon start", result)
        }
        DaemonCommand::Stop { all: true } => output_of(output, "daemon stop", control::stop_all()),
        DaemonCommand::Stop { all: false } => {
            output_of(output, "daemon stop", ctx.project().and_then(control::stop))
        }
        DaemonCommand::List => output_of(output, "daemon list", control::list()),
        DaemonCommand::Run { log } => match ctx
            .project()
            .and_then(|p| server::run(p.root(), log.as_deref()))
        {
            Ok(()) => exit::SUCCESS,
            Err(error) => output.error(&error),
        },
    }
}

fn output_of<R>(output: &Output, command: &str, result: Result<R, Error>) -> u8
where
    R: serde::Serialize + TextRender + Outcome,
{
    match result {
        Ok(response) => output.response(command, &response),
        Err(error) => output.error(&error),
    }
}

/// The command line with each command's canonical description: its summary
/// for `--help` lists, the whole description for `codix help <command>` and
/// `--help`, and its example after the flags.
fn described(cli: clap::Command) -> clap::Command {
    fn apply(command: clap::Command, path: &str) -> clap::Command {
        let names: Vec<String> = command
            .get_subcommands()
            .map(|c| c.get_name().to_owned())
            .collect();
        let mut command = command;
        for name in names {
            let full = if path.is_empty() {
                name.clone()
            } else {
                format!("{path} {name}")
            };
            command = command.mut_subcommand(&name, |sub| {
                let sub = match describe(&full) {
                    Some(d) => sub
                        .about(d.summary)
                        .long_about(d.details())
                        .after_long_help(format!("Example:\n  {}", d.example)),
                    None => sub,
                };
                apply(sub, &full)
            });
        }
        command
    }
    apply(cli, "")
}

/// Turns a clap parse failure into a `usage` error, except for help and
/// version requests, which clap prints itself.
fn usage_error(error: &clap::Error) -> Error {
    let rendered = error.render().to_string();
    // The first line, plus the indented lines clap lists under it (such as
    // the missing arguments), on one line.
    let mut lines = rendered.lines();
    let mut cause = lines
        .next()
        .unwrap_or_default()
        .trim_start_matches("error: ")
        .to_owned();
    for detail in lines.take_while(|line| line.starts_with(' ')) {
        cause.push(' ');
        cause.push_str(detail.trim());
    }
    let hint = rendered
        .lines()
        .find_map(|line| line.strip_prefix("Usage: "))
        .map_or_else(
            || "see `codix --help`".to_owned(),
            |usage| format!("usage: {usage}; see --help"),
        );
    Error::Usage { cause, hint }
}

/// Parses the command line; help and version requests exit here, as clap
/// prints them, and other failures are rendered as `usage` errors.
fn parse() -> Result<Cli, ExitCode> {
    let args: Vec<OsString> = env::args_os().collect();
    described(Cli::command())
        .try_get_matches_from(&args)
        .and_then(|matches| Cli::from_arg_matches(&matches))
        .map_err(|e| {
            if matches!(
                e.kind(),
                ErrorKind::DisplayHelp
                    | ErrorKind::DisplayVersion
                    | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            ) {
                e.exit();
            }
            let json = args.iter().skip(1).any(|arg| arg == "--json");
            ExitCode::from(Output::detect(json).error(&usage_error(&e)))
        })
}

fn main() -> ExitCode {
    let cli = match parse() {
        Ok(cli) => cli,
        Err(code) => return code,
    };
    let output = Output::detect(cli.json);
    let ctx = match env::current_dir()
        .map_err(|source| Error::Io {
            path: ".".into(),
            source,
        })
        .and_then(|cwd| Context::locate(&cwd, cli.root.as_deref()))
    {
        Ok(ctx) => ctx,
        Err(error) => return ExitCode::from(output.error(&error)),
    };
    // An operation may go to the daemon, which sends the project's warnings;
    // the other commands run here.
    let ctx = match cli.command {
        Command::Daemon(_) | Command::Watch | Command::Mcp => ctx.with_project_warnings(),
        _ => ctx,
    };
    let output = Output {
        warnings: ctx.warnings().to_vec(),
        ..output
    };
    ExitCode::from(command(&output, &ctx, cli.command))
}

/// Runs a command; its exit code.
fn command(output: &Output, ctx: &Context, command: Command) -> u8 {
    match command {
        Command::Init { agent_docs } => {
            output.progress("indexing");
            run(output, &InitOp, ctx, Ok(InitRequest { agent_docs }))
        }
        Command::Index { full } => {
            output.progress("indexing");
            run(output, &IndexOp, ctx, Ok(IndexRequest { full }))
        }
        Command::Status => run(output, &StatusOp, ctx, Ok(StatusRequest {})),
        Command::Capabilities => run(output, &CapabilitiesOp, ctx, Ok(CapabilitiesRequest {})),
        Command::Languages => run(output, &LanguagesOp, ctx, Ok(LanguagesRequest {})),
        Command::Config(args) => {
            let (request, show_origin) = args.request();
            let output = Output {
                show_origin,
                ..output.clone()
            };
            run(&output, &ConfigOp, ctx, Ok(request))
        }
        Command::Find(args) => {
            let request = args.page.page().map(|page| FindRequest {
                query: args.query,
                mode: args.mode.mode(),
                kinds: args.kinds,
                languages: args.languages,
                scope: args.scope.request(),
                page,
            });
            run(output, &FindOp, ctx, request)
        }
        Command::Def { symbol, narrow } => {
            let request = DefRequest {
                symbol,
                kinds: narrow.kinds,
                scope: narrow.scope.request(),
            };
            run(output, &DefOp, ctx, Ok(request))
        }
        Command::Refs(args) => run(output, &RefsOp, ctx, args.request()),
        Command::Outline { path } => run(output, &OutlineOp, ctx, Ok(OutlineRequest { path })),
        Command::Rename(args) => run(output, &RenameOp, ctx, Ok(args.request())),
        Command::Apply { id } => run(output, &ApplyOp, ctx, Ok(ApplyRequest { id })),
        Command::Previews => run(output, &PreviewsOp, ctx, Ok(PreviewsRequest {})),
        Command::Discard { ids, all } => {
            run(output, &DiscardOp, ctx, Ok(DiscardRequest { ids, all }))
        }
        Command::Show {
            symbol,
            narrow,
            context,
        } => {
            let request = ShowRequest {
                symbol,
                kinds: narrow.kinds,
                scope: narrow.scope.request(),
                context,
            };
            run(output, &ShowOp, ctx, Ok(request))
        }
        Command::Daemon(command) => daemon(output, ctx, command),
        Command::Watch => daemon(output, ctx, DaemonCommand::Run { log: None }),
        Command::Mcp => match env::current_exe()
            .map_err(|source| Error::Io {
                path: "codix".into(),
                source,
            })
            .and_then(|exe| codix_mcp::serve(ctx.cwd().to_path_buf(), exe))
        {
            Ok(()) => exit::SUCCESS,
            Err(error) => output.error(&error),
        },
        Command::Grep(args) => {
            let request = args.page.page().map(|page| GrepRequest {
                pattern: args.pattern,
                languages: args.languages,
                kinds: args.kinds,
                scope: args.scope.request(),
                page,
            });
            run(output, &GrepOp, ctx, request)
        }
    }
}

#[cfg(test)]
mod tests {
    /// Each tool takes its command's flags, under the same names and with the
    /// same help, so an agent can move between the CLI and MCP.
    #[test]
    fn every_tool_takes_its_commands_flags_with_the_same_help() {
        use std::collections::BTreeMap;

        let trimmed = |text: &str| text.trim().trim_end_matches('.').to_owned();
        let cli = super::Cli::command();
        let tools = codix_mcp::tools();
        let names: Vec<&str> = tools.iter().map(|t| t.name.as_ref()).collect();
        assert_eq!(names, codix_core::dispatch::OPERATIONS);
        for tool in &tools {
            let name = tool.name.as_ref();
            let details = codix_core::describe::describe(name)
                .map(codix_core::describe::Description::details);
            assert_eq!(tool.description.as_deref(), details.as_deref(), "{name}");
            // `config` takes one form per request, unlike its flags.
            if name == "config" {
                continue;
            }
            let command = cli
                .find_subcommand(name)
                .unwrap_or_else(|| panic!("no `{name}` command"));
            let mut flags: BTreeMap<String, String> = command
                .get_arguments()
                .map(|arg| {
                    let key = arg
                        .get_long()
                        .map_or_else(|| arg.get_id().to_string(), |l| l.replace('-', "_"));
                    (
                        key,
                        trimmed(&arg.get_help().map(ToString::to_string).unwrap_or_default()),
                    )
                })
                .collect();
            let properties = tool.input_schema["properties"]
                .as_object()
                .unwrap_or_else(|| panic!("{name}: no properties"));
            let mut inputs: BTreeMap<String, String> = properties
                .iter()
                .filter(|(key, _)| key.as_str() != "cwd")
                .map(|(key, schema)| {
                    let help = schema["description"].as_str().unwrap_or_default();
                    (key.clone(), trimmed(help))
                })
                .collect();
            // One `mode` property stands for the four mode flags.
            if name == "find" {
                for mode in ["exact", "prefix", "fuzzy", "glob"] {
                    assert!(flags.remove(mode).is_some(), "find --{mode}");
                }
                assert!(inputs.remove("mode").is_some(), "find mode");
            }
            assert_eq!(inputs, flags, "{name}");
        }
    }

    #[test]
    fn every_command_has_a_description_and_every_description_a_command() {
        fn visit(command: &clap::Command, path: &str, seen: &mut Vec<String>) {
            for sub in command.get_subcommands() {
                if sub.is_hide_set() || sub.get_name() == "help" {
                    continue;
                }
                let full = if path.is_empty() {
                    sub.get_name().to_owned()
                } else {
                    format!("{path} {}", sub.get_name())
                };
                assert!(
                    codix_core::describe::describe(&full).is_some(),
                    "`{full}` has no description"
                );
                visit(sub, &full, seen);
                seen.push(full);
            }
        }
        let mut seen = Vec::new();
        visit(&super::Cli::command(), "", &mut seen);
        for d in codix_core::describe::COMMANDS {
            assert!(
                seen.iter().any(|s| s == d.name),
                "`{}` is not a command",
                d.name
            );
        }
    }

    use clap::CommandFactory;
    use codix_provider::{ProviderKind, known, registry};

    #[test]
    fn compiled_in_providers_match_enabled_features() {
        let mut expected: Vec<(ProviderKind, &str)> = [
            (
                cfg!(feature = "lang-java"),
                (ProviderKind::Language, "java"),
            ),
            (cfg!(feature = "lang-go"), (ProviderKind::Language, "go")),
            (
                cfg!(feature = "lang-typescript"),
                (ProviderKind::Language, "typescript"),
            ),
            (
                cfg!(feature = "lang-typescript"),
                (ProviderKind::Language, "tsx"),
            ),
            (
                cfg!(feature = "lang-python"),
                (ProviderKind::Language, "python"),
            ),
            (
                cfg!(feature = "lang-kotlin"),
                (ProviderKind::Language, "kotlin"),
            ),
            (
                cfg!(feature = "lang-javascript"),
                (ProviderKind::Language, "javascript"),
            ),
            (
                cfg!(feature = "lang-rust"),
                (ProviderKind::Language, "rust"),
            ),
            (cfg!(feature = "lang-c"), (ProviderKind::Language, "c")),
            (cfg!(feature = "lang-cpp"), (ProviderKind::Language, "cpp")),
            (
                cfg!(feature = "lang-csharp"),
                (ProviderKind::Language, "csharp"),
            ),
            (cfg!(feature = "lang-php"), (ProviderKind::Language, "php")),
            (
                cfg!(feature = "lang-ruby"),
                (ProviderKind::Language, "ruby"),
            ),
            (
                cfg!(feature = "lang-json"),
                (ProviderKind::Language, "json"),
            ),
            (
                cfg!(feature = "lang-yaml"),
                (ProviderKind::Language, "yaml"),
            ),
            (
                cfg!(feature = "lang-toml"),
                (ProviderKind::Language, "toml"),
            ),
            (cfg!(feature = "lang-xml"), (ProviderKind::Language, "xml")),
            (
                cfg!(feature = "lang-markdown"),
                (ProviderKind::Language, "markdown"),
            ),
            (cfg!(feature = "lang-sql"), (ProviderKind::Language, "sql")),
            (
                cfg!(feature = "build-gradle"),
                (ProviderKind::Build, "gradle"),
            ),
            (
                cfg!(feature = "build-maven"),
                (ProviderKind::Build, "maven"),
            ),
            (cfg!(feature = "vcs-git"), (ProviderKind::Vcs, "git")),
            (cfg!(feature = "vcs-hg"), (ProviderKind::Vcs, "mercurial")),
            (cfg!(feature = "vcs-svn"), (ProviderKind::Vcs, "subversion")),
        ]
        .into_iter()
        .filter_map(|(enabled, provider)| enabled.then_some(provider))
        .collect();
        expected.sort_unstable();
        assert_eq!(registry().names(), expected);
    }

    #[test]
    fn compiled_in_languages_with_a_grammar_are_tier_1_or_2() {
        use codix_core::capabilities::{Tier, capabilities};
        for language in capabilities().languages {
            let compiled = registry()
                .languages
                .iter()
                .find(|d| d.name == language.name);
            let expected = match compiled {
                Some(d) if !d.search_only && registry().grammars.contains_key(&d.name) => {
                    if registry().grammars[&d.name].locals.is_some() {
                        Tier::Scopes
                    } else {
                        Tier::Symbols
                    }
                }
                Some(_) => Tier::Search,
                None if language.name == "text" => Tier::Search,
                None => Tier::NotInBuild,
            };
            assert_eq!(language.tier, expected, "{}", language.name);
        }
    }

    #[test]
    fn every_compiled_in_provider_is_known() {
        for (kind, name) in registry().names() {
            assert!(
                known::is_known(kind, name),
                "{kind} provider {name} is missing from known"
            );
        }
    }

    #[cfg(feature = "full")]
    #[test]
    fn full_build_has_every_known_language() {
        let compiled: Vec<&str> = registry()
            .languages
            .iter()
            .map(|d| d.name.as_str())
            .collect();
        let known: Vec<&str> = known::LANGUAGES
            .iter()
            .copied()
            .filter(|l| *l != "text")
            .collect();
        assert_eq!(compiled, known);
    }

    #[cfg(feature = "full")]
    #[test]
    fn full_build_has_every_known_build_system() {
        let compiled: Vec<&str> = registry().builds.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(compiled, known::BUILDS);
    }

    #[cfg(feature = "full")]
    #[test]
    fn full_build_has_every_known_vcs() {
        let compiled: Vec<&str> = registry().vcs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(compiled, known::VCS);
    }
}
