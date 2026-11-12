//! The command line as invoked, rebuilt for a next page: the same program and
//! arguments, with the request fields the page changes, quoted for the
//! platform's shell when an argument needs it.

use std::borrow::Cow;
use std::collections::HashSet;
use std::ffi::OsString;

use clap::Command;

/// The flags that each select a `find` mode.
const MODES: [&str; 4] = ["--exact", "--prefix", "--fuzzy", "--glob"];

/// A command line, and which of its long options take a value.
pub(crate) struct Invocation {
    program: String,
    args: Vec<String>,
    valued: HashSet<String>,
}

impl Invocation {
    /// `argv` as the process got it, read against `command`'s options.
    pub(crate) fn new(argv: &[OsString], command: &Command) -> Self {
        let mut valued = HashSet::new();
        collect_valued(command, &mut valued);
        let mut argv = argv.iter().map(|a| a.to_string_lossy().into_owned());
        Self {
            program: argv.next().unwrap_or_else(|| "codix".to_owned()),
            args: argv.collect(),
            valued,
        }
    }

    /// The command for the request with `fields` changed: `symbol` and
    /// `query` replace the subcommand's argument, `mode` its mode flag, and
    /// `after` is set, or dropped when absent.
    pub(crate) fn next(&self, fields: &[(&str, String)]) -> String {
        let mut args = self.args.clone();
        // Another request starts at its first page unless `after` says where.
        self.remove_option(&mut args, "--after");
        for (field, _) in fields {
            match *field {
                "symbol" | "query" | "after" => {}
                "mode" => args.retain(|a| !MODES.contains(&a.as_str())),
                // A place replaces the directories the command is limited to.
                "path" => {
                    self.remove_option(&mut args, "--path");
                    args.retain(|a| a != "--here");
                }
                field => self.remove_option(&mut args, &format!("--{field}")),
            }
        }
        let mut added = Vec::new();
        for (field, value) in fields {
            match *field {
                "symbol" | "query" => {
                    if let Some(i) = self.positional(&args, 1) {
                        args[i].clone_from(value);
                    }
                }
                "mode" => added.push(format!("--{value}")),
                field => {
                    added.push(format!("--{field}"));
                    added.push(value.clone());
                }
            }
        }
        // Options go before a `--`, after which everything is an argument.
        let end = args.iter().position(|a| a == "--").unwrap_or(args.len());
        args.splice(end..end, added);

        let mut line = quote_program(&self.program);
        for arg in &args {
            line.push(' ');
            line.push_str(&quote(arg));
        }
        line
    }

    /// The index of the `n`th argument that is neither an option nor an
    /// option's value, the subcommand being the 0th.
    fn positional(&self, args: &[String], n: usize) -> Option<usize> {
        let mut seen = 0;
        let mut i = 0;
        let mut options = true;
        while i < args.len() {
            let arg = &args[i];
            if options && arg == "--" {
                options = false;
            } else if options && arg.starts_with('-') && arg.len() > 1 {
                if !arg.contains('=') && self.valued.contains(arg.as_str()) {
                    i += 1;
                }
            } else {
                if seen == n {
                    return Some(i);
                }
                seen += 1;
            }
            i += 1;
        }
        None
    }

    /// Removes every `name value` and `name=value`.
    fn remove_option(&self, args: &mut Vec<String>, name: &str) {
        let prefix = format!("{name}=");
        let mut i = 0;
        while i < args.len() {
            if args[i] == "--" {
                return;
            }
            if args[i] == name {
                let end = (i + 2).min(args.len());
                args.drain(i..end);
            } else if args[i].starts_with(&prefix) {
                args.remove(i);
            } else {
                if self.valued.contains(args[i].as_str()) {
                    i += 1;
                }
                i += 1;
            }
        }
    }
}

fn collect_valued(command: &Command, valued: &mut HashSet<String>) {
    for arg in command.get_arguments() {
        if arg.get_action().takes_values()
            && let Some(long) = arg.get_long()
        {
            valued.insert(format!("--{long}"));
        }
    }
    for sub in command.get_subcommands() {
        collect_valued(sub, valued);
    }
}

fn quote(arg: &str) -> Cow<'_, str> {
    if cfg!(windows) {
        powershell(arg)
    } else {
        posix(arg)
    }
}

/// PowerShell runs a quoted program only through its call operator.
fn quote_program(program: &str) -> String {
    match quote(program) {
        Cow::Owned(quoted) if cfg!(windows) => format!("& {quoted}"),
        quoted => quoted.into_owned(),
    }
}

/// An argument for a POSIX shell, single-quoted unless every character is one
/// no shell treats specially.
fn posix(arg: &str) -> Cow<'_, str> {
    let safe = |c: char| c.is_ascii_alphanumeric() || "_-./:=,@%+".contains(c);
    if !arg.is_empty() && arg.chars().all(safe) {
        Cow::Borrowed(arg)
    } else {
        Cow::Owned(format!("'{}'", arg.replace('\'', r"'\''")))
    }
}

/// An argument for PowerShell, single-quoted unless every character is one it
/// does not treat specially.
fn powershell(arg: &str) -> Cow<'_, str> {
    let safe = |c: char| c.is_ascii_alphanumeric() || r"_-./:=+%\".contains(c);
    if !arg.is_empty() && arg.chars().all(safe) {
        Cow::Borrowed(arg)
    } else {
        Cow::Owned(format!("'{}'", arg.replace('\'', "''")))
    }
}

#[cfg(test)]
mod tests {
    use clap::{Arg, ArgAction};

    use super::*;

    fn command() -> Command {
        let after = || Arg::new("after").long("after");
        Command::new("codix")
            .arg(Arg::new("root").long("root").global(true))
            .arg(
                Arg::new("json")
                    .long("json")
                    .action(ArgAction::SetTrue)
                    .global(true),
            )
            .subcommand(
                Command::new("refs")
                    .arg(Arg::new("symbol"))
                    .arg(Arg::new("path").long("path"))
                    .arg(after()),
            )
            .subcommand(
                Command::new("find")
                    .arg(Arg::new("query"))
                    .arg(Arg::new("prefix").long("prefix").action(ArgAction::SetTrue))
                    .arg(after()),
            )
    }

    fn next(argv: &[&str], fields: &[(&str, &str)]) -> String {
        let argv: Vec<OsString> = argv.iter().map(OsString::from).collect();
        let fields: Vec<(&str, String)> =
            fields.iter().map(|(f, v)| (*f, (*v).to_owned())).collect();
        Invocation::new(&argv, &command()).next(&fields)
    }

    #[test]
    fn the_symbol_is_replaced_and_after_appended() {
        assert_eq!(
            next(
                &["codix", "refs", "getBean", "--path", "src"],
                &[
                    ("symbol", "java:A.java::A.getBean#12ab"),
                    ("after", "src/B.java")
                ]
            ),
            "codix refs 'java:A.java::A.getBean#12ab' --path src --after src/B.java"
        );
    }

    #[test]
    fn option_values_are_not_taken_for_the_symbol() {
        assert_eq!(
            next(
                &[
                    "./codix", "--root", "x", "refs", "--path", "getBean", "getBean"
                ],
                &[("symbol", "ID"), ("after", "a.rs")]
            ),
            "./codix --root x refs --path getBean ID --after a.rs"
        );
    }

    #[test]
    fn a_previous_after_is_replaced_in_both_forms() {
        assert_eq!(
            next(
                &[
                    "codix",
                    "find",
                    "m",
                    "--after",
                    "a.rs",
                    "--after=b.rs",
                    "--prefix"
                ],
                &[("mode", "prefix"), ("after", "c.rs:3:1")]
            ),
            "codix find m --prefix --after c.rs:3:1"
        );
    }

    #[test]
    fn a_path_replaces_the_scope() {
        assert_eq!(
            next(
                &[
                    "codix", "refs", "x", "--path", "a", "--path=b", "--after", "a/f.rs"
                ],
                &[("path", "a/c")]
            ),
            "codix refs x --path a/c"
        );
    }

    #[test]
    fn options_go_before_a_double_dash() {
        assert_eq!(
            next(
                &["codix", "find", "--", "-m"],
                &[("mode", "exact"), ("after", "a.rs")]
            ),
            "codix find --exact --after a.rs -- -m"
        );
    }

    #[test]
    fn posix_quotes_only_what_a_shell_would_read() {
        assert_eq!(posix("src/a.rs:12:3"), "src/a.rs:12:3");
        assert_eq!(posix("a b"), "'a b'");
        assert_eq!(posix("Invoice.*"), "'Invoice.*'");
        assert_eq!(posix("it's"), r"'it'\''s'");
        assert_eq!(posix("~/x"), "'~/x'");
        assert_eq!(posix(""), "''");
    }

    #[test]
    fn powershell_quotes_only_what_it_would_read() {
        assert_eq!(powershell(r"C:\codix\codix.exe"), r"C:\codix\codix.exe");
        assert_eq!(powershell("a#b"), "'a#b'");
        assert_eq!(powershell("it's"), "'it''s'");
        assert_eq!(powershell("@x"), "'@x'");
    }
}
