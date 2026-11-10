//! The client of the build plugin protocol: runs a build plugin's executable
//! as `<executable> discover --protocol 1 <root>`, in `<root>`, with no
//! stdin, and reads its model from stdout.

use std::io::Read;
use std::path::Path;
use std::process::{Child, Command, Stdio};
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use codix_provider::{Answer, BuildModel, PROTOCOL, PluginError};

/// How long a build plugin may take.
pub const TIMEOUT: Duration = Duration::from_secs(30);

/// The model of the build at `root`, from the plugin's `executable`.
pub(crate) fn run(executable: &Path, root: &Path) -> Result<BuildModel, PluginError> {
    run_within(executable, root, TIMEOUT)
}

/// Like [`run`], waiting at most `timeout`.
pub fn run_within(
    executable: &Path,
    root: &Path,
    timeout: Duration,
) -> Result<BuildModel, PluginError> {
    let mut child = Command::new(executable)
        .args(["discover", "--protocol", &PROTOCOL.to_string()])
        .arg(root)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| PluginError::new(format!("{}: {e}", executable.display())))?;
    // Read both pipes while waiting, so a plugin that writes a lot never
    // blocks on a full pipe.
    let stdout = drain(child.stdout.take());
    let stderr = drain(child.stderr.take());
    let Some(status) = wait(&mut child, timeout)? else {
        // Left behind: a reader may wait on a pipe the plugin passed on.
        drop((stdout, stderr));
        return Err(PluginError::new(format!(
            "no answer within {} s",
            timeout.as_secs()
        )));
    };
    let stdout = joined(stdout);
    if !status.success() {
        let stderr = String::from_utf8_lossy(&joined(stderr)).into_owned();
        let first = stderr.lines().find(|l| !l.trim().is_empty()).unwrap_or("");
        return Err(PluginError::new(match first {
            "" => format!("failed ({status})"),
            line => format!("failed ({status}): {}", line.trim()),
        }));
    }
    let answer: Answer = serde_json::from_slice(&stdout)
        .map_err(|e| PluginError::new(format!("bad answer: {e}")))?;
    if answer.protocol != PROTOCOL {
        return Err(PluginError::new(format!(
            "protocol {}, this codix speaks protocol {PROTOCOL}",
            answer.protocol
        )));
    }
    Ok(answer.model)
}

fn drain(pipe: Option<impl Read + Send + 'static>) -> Option<JoinHandle<Vec<u8>>> {
    pipe.map(|mut pipe| {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            // A read error ends the output: what was read is what is judged.
            let _ = pipe.read_to_end(&mut bytes);
            bytes
        })
    })
}

fn joined(reader: Option<JoinHandle<Vec<u8>>>) -> Vec<u8> {
    reader.and_then(|r| r.join().ok()).unwrap_or_default()
}

/// The exit status, or `None` when the plugin is still running after
/// `timeout`: it is then killed.
fn wait(
    child: &mut Child,
    timeout: Duration,
) -> Result<Option<std::process::ExitStatus>, PluginError> {
    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => return Ok(Some(status)),
            Ok(None) if Instant::now() < deadline => {
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(None) => {
                // Already gone if this fails; either way it is reaped below.
                let _ = child.kill();
                let _ = child.wait();
                return Ok(None);
            }
            Err(e) => return Err(PluginError::new(format!("waiting for the plugin: {e}"))),
        }
    }
}
