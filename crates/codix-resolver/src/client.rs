//! The core side of the protocol: a resolver plugin's process.

use std::io::{BufRead, BufReader, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

use crate::protocol::{Reply, Request};
use crate::{PROTOCOL, VIEWS};

/// How long a resolver may take to say hello.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);

/// A running resolver plugin.
pub struct Process {
    child: Child,
    stdin: ChildStdin,
    replies: Receiver<String>,
}

impl Process {
    /// Starts the resolver at `executable` and checks that it speaks this
    /// protocol and views version.
    pub fn start(executable: &Path) -> Result<Self, String> {
        let mut child = Command::new(executable)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("{}: {e}", executable.display()))?;
        let (Some(stdin), Some(stdout)) = (child.stdin.take(), child.stdout.take()) else {
            return Err("no pipes to the resolver".to_owned());
        };
        let (send, replies) = channel();
        std::thread::spawn(move || {
            for line in BufReader::new(stdout).lines() {
                let Ok(line) = line else { break };
                if send.send(line).is_err() {
                    break;
                }
            }
        });
        let mut process = Self {
            child,
            stdin,
            replies,
        };
        match process.ask(
            &Request::Hello {
                protocol: PROTOCOL,
                views: VIEWS,
            },
            HELLO_TIMEOUT,
        )? {
            Reply::Hello { protocol, views } if protocol == PROTOCOL && views == VIEWS => {
                Ok(process)
            }
            Reply::Hello { protocol, views } => Err(format!(
                "protocol {protocol} and views {views}, this codix speaks protocol {PROTOCOL} and views {VIEWS}"
            )),
            other => Err(format!("unexpected reply to hello: {other:?}")),
        }
    }

    /// Resolves `files`, writing into the scratch database at `scratch`;
    /// returns the dependents it resolved too.
    pub fn resolve(
        &mut self,
        index: &Path,
        scratch: &Path,
        files: &[String],
        timeout: Duration,
    ) -> Result<Vec<String>, String> {
        let request = Request::Resolve {
            index: PathBuf::from(index),
            scratch: PathBuf::from(scratch),
            files: files.to_vec(),
        };
        match self.ask(&request, timeout)? {
            Reply::Resolved { dependents } => Ok(dependents),
            Reply::Error { message } => Err(message),
            other @ Reply::Hello { .. } => Err(format!("unexpected reply to resolve: {other:?}")),
        }
    }

    fn ask(&mut self, request: &Request, timeout: Duration) -> Result<Reply, String> {
        let line = serde_json::to_string(request).map_err(|e| e.to_string())?;
        writeln!(self.stdin, "{line}")
            .and_then(|()| self.stdin.flush())
            .map_err(|e| format!("the resolver stopped reading: {e}"))?;
        let deadline = Instant::now() + timeout;
        let reply = self
            .replies
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| match e {
                RecvTimeoutError::Timeout => {
                    format!("no answer within {} s", timeout.as_secs())
                }
                RecvTimeoutError::Disconnected => match self.child.try_wait() {
                    Ok(Some(status)) => format!("the resolver exited ({status})"),
                    _ => "the resolver closed its output".to_owned(),
                },
            })?;
        serde_json::from_str(&reply).map_err(|e| format!("bad reply: {e}"))
    }
}

impl Drop for Process {
    fn drop(&mut self) {
        // Best effort: a resolver that does not stop when asked, or on the
        // end of its input, is killed.
        let _ = serde_json::to_string(&Request::Shutdown)
            .map_err(std::io::Error::other)
            .and_then(|line| writeln!(self.stdin, "{line}"));
        let deadline = Instant::now() + Duration::from_secs(1);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}
