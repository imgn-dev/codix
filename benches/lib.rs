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

/// spring-framework v6.2.0: about 1.6M lines in 10k files, mostly Java.
pub const SPRING: Pinned = Pinned {
    name: "spring-framework",
    url: "https://github.com/spring-projects/spring-framework",
    tag: "v6.2.0",
    commit: "5024bb72279188f1d0dcfe19e8abdd4bdb9887c8",
};

/// The workspace root.
pub fn workspace() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR")).join("..")
}

/// Builds the release binary with every provider and returns its path.
///
/// # Panics
///
/// When cargo cannot run or the build fails.
pub fn build_codix() -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let status = Command::new(cargo)
        .args([
            "build",
            "--release",
            "--quiet",
            "-p",
            "codix-cli",
            "--features",
            "full",
        ])
        .current_dir(workspace())
        .status()
        .expect("run cargo build");
    assert!(status.success(), "building codix failed");
    workspace()
        .join("target/release")
        .join(format!("codix{}", std::env::consts::EXE_SUFFIX))
}

/// Builds the modular flavour with every plugin, as it installs, into
/// `target/bench-dist`, and returns its binary: `bin/codix`, which finds the
/// plugins in `lib/codix/plugins` beside it.
///
/// # Panics
///
/// When cargo cannot run or the build fails.
pub fn build_modular() -> PathBuf {
    let cargo = std::env::var("CARGO").unwrap_or_else(|_| "cargo".to_owned());
    let dist = workspace().join("target/bench-dist");
    if dist.exists() {
        std::fs::remove_dir_all(&dist).expect("remove the previous build");
    }
    let output = Command::new(cargo)
        .args(["xtask", "dist"])
        .arg(&dist)
        .current_dir(workspace())
        .output()
        .expect("run cargo xtask dist");
    assert!(
        output.status.success(),
        "building the modular flavour failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    dist.join("bin")
        .join(format!("codix{}", std::env::consts::EXE_SUFFIX))
}

/// Today's date in UTC, as `YYYY-MM-DD`: the name of the results files.
///
/// # Panics
///
/// When the clock is set before 1970.
pub fn today() -> String {
    let seconds = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .expect("a clock after 1970")
        .as_secs();
    let (year, month, day) = civil(seconds / 86_400);
    format!("{year:04}-{month:02}-{day:02}")
}

/// The proleptic Gregorian date `days` days after 1970-01-01, after Howard
/// Hinnant's `civil_from_days`.
fn civil(days: u64) -> (u64, u64, u64) {
    let z = days + 719_468;
    let era = z / 146_097;
    let doe = z % 146_097;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = yoe + era * 400 + u64::from(month <= 2);
    (year, month, day)
}

#[cfg(test)]
mod tests {
    use super::civil;

    #[test]
    fn days_since_the_epoch_are_dates() {
        assert_eq!(civil(0), (1970, 1, 1));
        assert_eq!(civil(59), (1970, 3, 1));
        assert_eq!(civil(11_016), (2000, 2, 29));
        assert_eq!(civil(11_017), (2000, 3, 1));
        assert_eq!(civil(20_769), (2026, 11, 12));
        assert_eq!(civil(20_818), (2026, 12, 31));
        assert_eq!(civil(20_819), (2027, 1, 1));
    }
}
