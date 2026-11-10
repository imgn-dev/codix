//! Routing an operation: to the project's daemon when one runs and this codix
//! can talk to it, otherwise in-process.

use std::io::BufReader;

use codix_core::Error;
use codix_core::daemon::{DaemonRecord, PROTOCOL, running};
use codix_core::op::{Context, Operation, Warning};
use codix_core::project::Project;
use interprocess::local_socket::{GenericFilePath, Stream, prelude::*};

use crate::control::{compatible, stop};
use crate::protocol::{OpRequest, Request, Response, read_frame, write_frame};

/// Where an operation runs.
#[derive(Debug)]
pub enum Route<T> {
    /// The daemon ran it.
    Daemon {
        /// Its response or error.
        outcome: Result<T, Error>,
        /// Warnings about the project, from the daemon.
        warnings: Vec<Warning>,
    },
    /// Run it in-process.
    InProcess {
        /// Warnings to add to the context's, such as a daemon stopped on the
        /// way.
        warnings: Vec<Warning>,
    },
}

/// Sends `request` to the daemon of the context's project, if one runs.
///
/// A daemon of another codix version or protocol is stopped, with a warning,
/// since it would hold the write lock against this process.
pub fn call<O: Operation>(ctx: &Context, request: &O::Request) -> Route<O::Response> {
    let in_process = Route::InProcess {
        warnings: Vec::new(),
    };
    let Ok(project) = ctx.project() else {
        return in_process;
    };
    let Ok(Some(record)) = running(project) else {
        return in_process;
    };
    if !compatible(&record) {
        return replace(project, &record, ctx);
    }
    let op = OpRequest {
        name: O::NAME.to_owned(),
        cwd: ctx.cwd().to_path_buf(),
        root: ctx.root_override().then(|| project.root().to_path_buf()),
        request: match serde_json::to_value(request) {
            Ok(value) => value,
            Err(e) => return internal(ctx, format!("cannot serialize the request: {e}")),
        },
    };
    match send(&record.socket, &Request::Op(op)) {
        Ok(Response::Done { result, warnings }) => Route::Daemon {
            outcome: serde_json::from_value(result).map_err(|e| Error::Internal {
                message: format!("the daemon sent a response this codix cannot read: {e}"),
            }),
            warnings,
        },
        Ok(Response::Failed { error, warnings }) => Route::Daemon {
            outcome: Err(Error::Remote(Box::new(error))),
            warnings,
        },
        Ok(other) => internal(ctx, format!("the daemon answered {other:?}")),
        // It may have stopped since: idle, or asked to.
        Err(_) if matches!(running(project), Ok(None)) => in_process,
        Err(e) => internal(
            ctx,
            format!(
                "the daemon (pid {}) does not answer on {}: {e}; stop it with `codix daemon stop`",
                record.pid, record.socket
            ),
        ),
    }
}

fn replace<T>(project: &Project, record: &DaemonRecord, ctx: &Context) -> Route<T> {
    match stop(project) {
        Ok(_) => Route::InProcess {
            warnings: vec![Warning {
                code: "daemon_stopped".to_owned(),
                message: if record.protocol == PROTOCOL
                    && record.version == env!("CARGO_PKG_VERSION")
                {
                    format!(
                        "stopped the codix {} daemon (pid {}), which runs with other plugins; `codix daemon start` starts one with the plugins installed now",
                        record.version, record.pid
                    )
                } else {
                    format!(
                        "stopped the codix {} daemon (pid {}), which this codix cannot talk to; `codix daemon start` starts one of this version",
                        record.version, record.pid
                    )
                },
            }],
        },
        Err(error) => Route::Daemon {
            outcome: Err(error),
            warnings: ctx.warnings().to_vec(),
        },
    }
}

fn internal<T>(ctx: &Context, message: String) -> Route<T> {
    Route::Daemon {
        outcome: Err(Error::Internal { message }),
        warnings: ctx.warnings().to_vec(),
    }
}

/// Sends one request and waits for its response, as long as it takes: an
/// operation such as `index` may run for minutes.
fn send(socket: &str, request: &Request) -> std::io::Result<Response> {
    let stream = Stream::connect(socket.to_fs_name::<GenericFilePath>()?)?;
    let mut writer = &stream;
    write_frame(&mut writer, request)?;
    read_frame(&mut BufReader::new(&stream))?.ok_or_else(|| {
        std::io::Error::new(
            std::io::ErrorKind::UnexpectedEof,
            "the daemon closed the connection",
        )
    })
}
