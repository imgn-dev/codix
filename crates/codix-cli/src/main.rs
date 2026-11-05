//! The `codix` command-line interface: parses arguments into operation
//! requests and renders the responses. It holds no logic of its own.

mod render;

use std::env;
use std::ffi::OsString;
use std::io::{self, IsTerminal, Write};
use std::process::ExitCode;

use clap::error::ErrorKind;
use clap::{Args, Parser, Subcommand};
use codix_core::Error;
use codix_core::debug::{DebugEcho, DebugEchoRequest};
use codix_core::error::exit;
use codix_core::op::{Context, Operation, Outcome};
use codix_core::page::{DEFAULT_LIMIT, Page};
use render::{Style, TextRender};

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

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Echo synthetic results, to exercise output rendering.
    #[command(name = "debug-echo", hide = true)]
    DebugEcho(DebugEchoArgs),
}

#[derive(Args)]
struct DebugEchoArgs {
    /// Number of results.
    count: usize,
    /// File to sort results around.
    #[arg(long)]
    anchor: Option<String>,
    /// Fail with this error code.
    #[arg(long)]
    fail: Option<String>,
    #[command(flatten)]
    page: PageArgs,
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
struct Output {
    json: bool,
    stdout: Style,
    stderr: Style,
}

impl Output {
    fn detect(json: bool) -> Self {
        let color_allowed = env::var_os("NO_COLOR").is_none_or(|value| value.is_empty());
        Self {
            json,
            stdout: Style {
                color: color_allowed && io::stdout().is_terminal(),
            },
            stderr: Style {
                color: color_allowed && io::stderr().is_terminal(),
            },
        }
    }

    fn response<R: serde::Serialize + TextRender + Outcome>(
        &self,
        command: &str,
        response: &R,
    ) -> u8 {
        let mut stdout = io::stdout().lock();
        let written = if self.json {
            render::json_response(&mut stdout, command, response, &[])
        } else {
            response.render_text(&mut stdout, self.stdout)
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
            render::json_error(&mut stdout, error).and_then(|()| stdout.flush())
        } else {
            render::text_error(&mut io::stderr().lock(), error, self.stderr)
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

fn run<O>(output: &Output, op: &O, ctx: &Context, request: Result<O::Request, Error>) -> u8
where
    O: Operation,
    O::Response: TextRender,
{
    match request.and_then(|request| op.run(ctx, request)) {
        Ok(response) => output.response(O::NAME, &response),
        Err(error) => output.error(&error),
    }
}

/// Turns a clap parse failure into a `usage` error, except for help and
/// version requests, which clap prints itself.
fn usage_error(error: &clap::Error) -> Error {
    let rendered = error.render().to_string();
    let cause = rendered
        .lines()
        .next()
        .unwrap_or_default()
        .trim_start_matches("error: ")
        .to_owned();
    let hint = rendered
        .lines()
        .find_map(|line| line.strip_prefix("Usage: "))
        .map_or_else(
            || "see `codix --help`".to_owned(),
            |usage| format!("usage: {usage}; see --help"),
        );
    Error::Usage { cause, hint }
}

fn main() -> ExitCode {
    let args: Vec<OsString> = env::args_os().collect();
    let cli = match Cli::try_parse_from(&args) {
        Ok(cli) => cli,
        Err(e) => {
            if matches!(
                e.kind(),
                ErrorKind::DisplayHelp
                    | ErrorKind::DisplayVersion
                    | ErrorKind::DisplayHelpOnMissingArgumentOrSubcommand
            ) {
                e.exit();
            }
            let json = args.iter().skip(1).any(|arg| arg == "--json");
            return ExitCode::from(Output::detect(json).error(&usage_error(&e)));
        }
    };
    let output = Output::detect(cli.json);
    let ctx = match env::current_dir() {
        Ok(cwd) => Context::new(cwd),
        Err(source) => {
            return ExitCode::from(output.error(&Error::Io {
                path: ".".into(),
                source,
            }));
        }
    };
    let code = match cli.command {
        Command::DebugEcho(args) => {
            let request = args.page.page().map(|page| DebugEchoRequest {
                count: args.count,
                anchor: args.anchor,
                fail: args.fail,
                page,
            });
            run(&output, &DebugEcho, &ctx, request)
        }
    };
    ExitCode::from(code)
}
