//! Helpers for the codix benchmarks: pinned repositories and timing.

use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};

/// A public repository at a fixed commit.
#[derive(Debug, Clone, Copy)]
pub struct Pinned {
    /// Directory name under `target/bench-repos`.
    pub name: &'static str,
    /// Clone URL.
    pub url: &'static str,
    /// Tag to clone.
    pub tag: &'static str,
    /// Commit the tag must point to.
    pub commit: &'static str,
}

/// Downloads `repo` at its tag into `dir` unless it is there, after checking
/// that the tag still points to the pinned commit. The tree comes from
/// GitHub's tarball of the tag, so the checkout has no `.git`.
///
/// # Panics
///
/// When the tag moved, or the download or the extraction fails.
pub fn checkout(dir: &Path, repo: Pinned) -> PathBuf {
    let path = dir.join(repo.name);
    if path.join(".complete").exists() {
        return path;
    }
    let refs = Command::new("git")
        .args([
            "ls-remote",
            repo.url,
            &format!("refs/tags/{}", repo.tag),
            &format!("refs/tags/{}^{{}}", repo.tag),
        ])
        .output()
        .expect("run git ls-remote");
    let refs = String::from_utf8_lossy(&refs.stdout);
    let commit = refs
        .lines()
        .find(|l| l.ends_with("^{}"))
        .or_else(|| refs.lines().next())
        .and_then(|l| l.split_whitespace().next())
        .unwrap_or_default();
    assert_eq!(
        commit, repo.commit,
        "{} {} is not at the pinned commit",
        repo.name, repo.tag
    );

    std::fs::create_dir_all(&path).expect("create the repository directory");
    let archive = dir.join(format!("{}.tar.gz", repo.name));
    let url = format!("{}/archive/refs/tags/{}.tar.gz", repo.url, repo.tag);
    let status = Command::new("curl")
        .args([
            "--silent",
            "--show-error",
            "--location",
            "--fail",
            "--output",
        ])
        .arg(&archive)
        .arg(&url)
        .status()
        .expect("run curl");
    assert!(status.success(), "downloading {url} failed");
    let status = Command::new("tar")
        .args(["-xzf"])
        .arg(&archive)
        .args(["--strip-components", "1", "-C"])
        .arg(&path)
        .status()
        .expect("run tar");
    assert!(status.success(), "extracting {} failed", archive.display());
    std::fs::remove_file(&archive).expect("remove the archive");
    std::fs::write(path.join(".complete"), repo.commit).expect("mark the download complete");
    path
}

/// Runs `codix` with `args` in `dir`, returning the wall-clock time.
///
/// # Panics
///
/// When codix cannot start or exits with a failure.
pub fn time(codix: &Path, dir: &Path, args: &[&str]) -> Duration {
    let start = Instant::now();
    let output = Command::new(codix)
        .args(args)
        .current_dir(dir)
        .output()
        .expect("run codix");
    let elapsed = start.elapsed();
    assert!(
        output.status.code().is_some_and(|c| c <= 1),
        "codix {args:?} failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    elapsed
}

/// The value below which `percent` percent of `samples` fall.
pub fn percentile(samples: &mut [Duration], percent: usize) -> Duration {
    samples.sort_unstable();
    let rank = (percent * samples.len()).div_ceil(100);
    samples[rank.clamp(1, samples.len()) - 1]
}

/// Files and lines in a checkout, `.codix/` excluded.
///
/// # Panics
///
/// When a directory cannot be read.
pub fn files_and_lines(repo: &Path) -> (usize, usize) {
    let mut files = 0;
    let mut lines = 0;
    let mut pending = vec![repo.to_path_buf()];
    while let Some(dir) = pending.pop() {
        for entry in std::fs::read_dir(&dir)
            .expect("read a directory")
            .filter_map(Result::ok)
        {
            let path = entry.path();
            let Ok(kind) = entry.file_type() else {
                continue;
            };
            if kind.is_dir() && entry.file_name() != ".codix" {
                pending.push(path);
            } else if kind.is_file() && entry.file_name() != ".complete" {
                files += 1;
                if let Ok(text) = std::fs::read(&path) {
                    lines += text.split(|b| *b == b'\n').count() - 1;
                }
            }
        }
    }
    (files, lines)
}
