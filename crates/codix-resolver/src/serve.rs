//! The resolver side of the protocol: the `main` of a resolver plugin.

use std::io::{BufRead, Write};
use std::process::ExitCode;

use crate::protocol::{Reply, Request};
use crate::{PROTOCOL, Resolver, VIEWS};

/// Serves `resolver` on stdin and stdout until `shutdown` or the end of
/// stdin. Exit code 1 when stdin or stdout fail, or a line is not a request.
pub fn serve(resolver: &mut dyn Resolver) -> ExitCode {
    let stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    for line in stdin.lines() {
        let Ok(line) = line else {
            return ExitCode::FAILURE;
        };
        let reply = match serde_json::from_str::<Request>(&line) {
            Ok(Request::Hello { .. }) => Reply::Hello {
                protocol: PROTOCOL,
                views: VIEWS,
            },
            Ok(Request::Resolve {
                index,
                scratch,
                files,
            }) => match crate::run(resolver, &index, &scratch, &files) {
                Ok(dependents) => Reply::Resolved { dependents },
                Err(message) => Reply::Error { message },
            },
            Ok(Request::Shutdown) => return ExitCode::SUCCESS,
            Err(_) => return ExitCode::FAILURE,
        };
        let written = serde_json::to_writer(&mut stdout, &reply)
            .map_err(std::io::Error::from)
            .and_then(|()| writeln!(stdout))
            .and_then(|()| stdout.flush());
        if written.is_err() {
            return ExitCode::FAILURE;
        }
    }
    ExitCode::SUCCESS
}
