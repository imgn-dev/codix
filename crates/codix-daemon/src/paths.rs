//! Where a daemon's socket and registry entry live.

use std::env;
use std::fs;
#[cfg(unix)]
use std::io;
use std::path::{Path, PathBuf};

use codix_core::Error;
#[cfg(unix)]
use codix_core::daemon::SOCKET_FILE;
use codix_core::project::Project;

/// Overrides the user runtime directory, for tests only.
pub const RUNTIME_DIR_VAR: &str = "CODIX_RUNTIME_DIR";

/// The longest socket path the platform takes, its terminating NUL excluded:
/// `sun_path` is 108 bytes on Linux and 104 on macOS and the BSDs.
#[cfg(target_os = "linux")]
const SOCKET_PATH_MAX: usize = 107;
#[cfg(all(unix, not(target_os = "linux")))]
const SOCKET_PATH_MAX: usize = 103;

/// The first 16 hex digits of BLAKE3 over the canonical root: names the
/// project's socket and registry entry outside the project.
pub fn root_hash(root: &Path) -> String {
    let hash = blake3::hash(root.to_string_lossy().as_bytes());
    hash.to_hex()[..16].to_owned()
}

/// The user runtime directory: `$XDG_RUNTIME_DIR/codix` on Linux,
/// `$TMPDIR/codix` on macOS (a per-user directory there),
/// `%LOCALAPPDATA%\codix\run` on Windows; `codix-<user>` in the temp
/// directory otherwise.
pub fn runtime_dir() -> PathBuf {
    match env::var_os(RUNTIME_DIR_VAR) {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir),
        _ => platform_runtime_dir(),
    }
}

#[cfg(target_os = "macos")]
fn platform_runtime_dir() -> PathBuf {
    env::temp_dir().join("codix")
}

#[cfg(all(unix, not(target_os = "macos")))]
fn platform_runtime_dir() -> PathBuf {
    match env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join("codix"),
        _ => {
            let user = env::var("USER").unwrap_or_else(|_| "user".to_owned());
            env::temp_dir().join(format!("codix-{user}"))
        }
    }
}

#[cfg(windows)]
fn platform_runtime_dir() -> PathBuf {
    env::var_os("LOCALAPPDATA")
        .map_or_else(env::temp_dir, PathBuf::from)
        .join("codix")
        .join("run")
}

/// Creates `dir` and its parents, readable by the user only on Unix.
pub fn create_private_dir(dir: &Path) -> Result<(), Error> {
    let mut builder = fs::DirBuilder::new();
    builder.recursive(true);
    #[cfg(unix)]
    std::os::unix::fs::DirBuilderExt::mode(&mut builder, 0o700);
    builder.create(dir).map_err(|source| Error::Io {
        path: dir.to_path_buf(),
        source,
    })
}

/// The daemon's socket: `.codix/daemon.sock` when its path fits the
/// platform's limit, else `<runtime dir>/<root hash>.sock`; on Windows, the
/// named pipe `\\.\pipe\codix-<root hash>`.
pub fn socket_name(project: &Project) -> Result<String, Error> {
    #[cfg(unix)]
    {
        let local = project.data_dir().join(SOCKET_FILE);
        let path = if local.as_os_str().len() <= SOCKET_PATH_MAX {
            local
        } else {
            let dir = runtime_dir();
            create_private_dir(&dir)?;
            dir.join(format!("{}.sock", root_hash(project.root())))
        };
        if path.as_os_str().len() > SOCKET_PATH_MAX {
            return Err(Error::Io {
                source: io::Error::new(
                    io::ErrorKind::InvalidFilename,
                    format!("the socket path is longer than {SOCKET_PATH_MAX} bytes"),
                ),
                path,
            });
        }
        utf8(path)
    }
    #[cfg(windows)]
    {
        Ok(format!(r"\\.\pipe\codix-{}", root_hash(project.root())))
    }
}

#[cfg(unix)]
fn utf8(path: PathBuf) -> Result<String, Error> {
    path.into_os_string()
        .into_string()
        .map_err(|path| Error::Io {
            source: io::Error::new(io::ErrorKind::InvalidFilename, "the path is not UTF-8"),
            path: path.into(),
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_hashes_are_short_and_stable() {
        let a = root_hash(Path::new("/work/a"));
        assert_eq!(a.len(), 16);
        assert_eq!(a, root_hash(Path::new("/work/a")));
        assert_ne!(a, root_hash(Path::new("/work/b")));
    }
}
