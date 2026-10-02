//! External tool discovery and child-process plumbing.
//!
//! Tool lookup order: the directories named in `MEDIA_TOOLS_PATH` (a
//! colon-separated extra bin-dir list, e.g. `target/media-tools/bin` as
//! populated by the bootstrap script), then the regular `PATH`. This lets a
//! pinned/known-good tool set win over whatever happens to be installed
//! system-wide, while system tools still work when nothing is pinned.

use anyhow::{anyhow, Context, Result};
use std::path::{Path, PathBuf};
use std::process::Command;

/// Environment variable holding extra tool directories (colon-separated).
pub const MEDIA_TOOLS_PATH_VAR: &str = "MEDIA_TOOLS_PATH";

/// The extra tool directories requested via `MEDIA_TOOLS_PATH`.
pub fn media_tools_dirs() -> Vec<PathBuf> {
    match std::env::var_os(MEDIA_TOOLS_PATH_VAR) {
        Some(value) => std::env::split_paths(&value)
            .filter(|p| !p.as_os_str().is_empty())
            .collect(),
        None => Vec::new(),
    }
}

/// Finds an executable by name: `MEDIA_TOOLS_PATH` dirs first, then `PATH`.
pub fn lookup(name: &str) -> Option<PathBuf> {
    let mut dirs = media_tools_dirs();
    if let Some(path_var) = std::env::var_os("PATH") {
        dirs.extend(std::env::split_paths(&path_var));
    }
    for dir in dirs {
        let candidate = dir.join(name);
        if is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// "found" check: a regular file with at least one executable bit (unix).
fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        path.is_file()
            && std::fs::metadata(path)
                .map(|meta| meta.permissions().mode() & 0o111 != 0)
                .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// First line of `<tool> --version`, or "unknown" when the tool exists but
/// does not answer. Tries stderr as a fallback (some tools version there).
pub fn version_of(path: &Path) -> String {
    let Ok(output) = Command::new(path).arg("--version").output() else {
        return "unknown".to_string();
    };
    let stdout = String::from_utf8_lossy(&output.stdout);
    let line = stdout
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(str::to_string);
    line.or_else(|| {
        let stderr = String::from_utf8_lossy(&output.stderr);
        stderr
            .lines()
            .map(str::trim)
            .find(|l| !l.is_empty())
            .map(str::to_string)
    })
    .unwrap_or_else(|| "unknown".to_string())
}

/// Runs a child process with captured output. On success returns the captured
/// stdout; on failure returns an error carrying the command line and the tail
/// of stderr (the interesting part for ffmpeg/vhs diagnostics).
pub fn run(cmd: &mut Command) -> Result<String> {
    let output = cmd
        .output()
        .with_context(|| format!("spawning {}", render(cmd)))?;
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        let tail: Vec<&str> = stderr.lines().rev().take(15).collect();
        let mut tail: Vec<String> = tail.into_iter().rev().map(str::to_string).collect();
        if tail.is_empty() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            tail.extend(stdout.lines().rev().take(15).map(str::to_string));
        }
        return Err(anyhow!(
            "command failed with {}: {}\n{}",
            output.status,
            render(cmd),
            tail.join("\n")
        ));
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

/// Program + args rendering for error messages.
fn render(cmd: &Command) -> String {
    let mut line = String::new();
    if let Some(dir) = cmd.get_current_dir() {
        line.push_str(&format!("(cwd: {}) ", dir.display()));
    }
    line.push_str(&cmd.get_program().to_string_lossy());
    for arg in cmd.get_args() {
        line.push(' ');
        line.push_str(&arg.to_string_lossy());
    }
    line
}
