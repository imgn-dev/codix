//! The `codix` command-line interface: parses arguments into operation
//! requests and renders the responses. It holds no logic of its own.

mod render;

// Providers register themselves at link time; naming each enabled one here is
// what links it in.
#[cfg(feature = "lang-java")]
use codix_lang_java as _;
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
use clap::{Args, Parser, Subcommand};
use codix_core::Error;
use codix_core::config::{ConfigOp, ConfigRequest};
use codix_core::debug::{DebugEcho, DebugEchoRequest};
use codix_core::error::exit;
use codix_core::op::{Context, Operation, Outcome, Warning};
use codix_core::page::{DEFAULT_LIMIT, Page};
use codix_core::project::ScopeRequest;
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

    /// Use the project at this directory instead of searching upward.
    #[arg(long, global = true, value_name = "PATH")]
    root: Option<PathBuf>,

    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Read or edit codix.toml and .codix/config.toml, like `git config`.
    Config(ConfigArgs),
    /// Echo synthetic results, to exercise output rendering.
    #[command(name = "debug-echo", hide = true)]
    DebugEcho(DebugEchoArgs),
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
            },
            stderr: Style {
                color: color_allowed && io::stderr().is_terminal(),
                show_origin: false,
            },
            warnings: Vec::new(),
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
    let ctx = match env::current_dir()
        .map_err(|source| Error::Io {
            path: ".".into(),
            source,
        })
        .and_then(|cwd| Context::discover(&cwd, cli.root.as_deref()))
    {
        Ok(ctx) => ctx,
        Err(error) => return ExitCode::from(output.error(&error)),
    };
    let output = Output {
        warnings: ctx.warnings().to_vec(),
        ..output
    };
    let code = match cli.command {
        Command::Config(args) => {
            let (request, show_origin) = args.request();
            let output = Output {
                show_origin,
                ..output
            };
            run(&output, &ConfigOp, &ctx, Ok(request))
        }
        Command::DebugEcho(args) => {
            let request = args.page.page().map(|page| DebugEchoRequest {
                count: args.count,
                anchor: args.anchor,
                fail: args.fail,
                scope: args.scope.request(),
                page,
            });
            run(&output, &DebugEcho, &ctx, request)
        }
    };
    ExitCode::from(code)
}

#[cfg(test)]
mod tests {
    use codix_provider::{ProviderKind, known, registry};

    #[test]
    fn compiled_in_providers_match_enabled_features() {
        let expected: Vec<(ProviderKind, &str)> = [
            (
                cfg!(feature = "lang-java"),
                (ProviderKind::Language, "java"),
            ),
            (cfg!(feature = "vcs-git"), (ProviderKind::Vcs, "git")),
            (cfg!(feature = "vcs-hg"), (ProviderKind::Vcs, "mercurial")),
            (cfg!(feature = "vcs-svn"), (ProviderKind::Vcs, "subversion")),
        ]
        .into_iter()
        .filter_map(|(enabled, provider)| enabled.then_some(provider))
        .collect();
        assert_eq!(registry().names(), expected);
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
    fn full_build_has_every_known_vcs() {
        let compiled: Vec<&str> = registry().vcs.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(compiled, known::VCS);
    }
}
