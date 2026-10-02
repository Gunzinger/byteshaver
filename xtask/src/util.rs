//! Small filesystem + hashing helpers shared by the xtask modules.
//!
//! Everything is intentionally plain `std::fs` - no xshell or similar - so
//! the helper set stays auditable and the dependency tree lean.

use anyhow::{Context, Result};
use sha2::{Digest, Sha256};
use std::fs;
use std::path::{Path, PathBuf};

/// Workspace root. xtask sits at `<root>/xtask` and `CARGO_MANIFEST_DIR` is
/// baked in at compile time, so every derived path is correct regardless of
/// the caller's cwd (children are always given an explicit cwd instead).
pub fn repo_root() -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("xtask/Cargo.toml must have a parent (the workspace root)")
        .to_path_buf()
}

/// Hex-encoded sha256 of a file's bytes.
pub fn sha256_file(path: &Path) -> Result<String> {
    let bytes = fs::read(path).with_context(|| format!("reading {}", path.display()))?;
    Ok(sha256_hex(&bytes))
}

/// Hex-encoded sha256 of an in-memory byte slice.
pub fn sha256_hex(bytes: &[u8]) -> String {
    let mut hasher = Sha256::new();
    hasher.update(bytes);
    format!("{:x}", hasher.finalize())
}

/// Recursively copies `src` to `dst` (creating `dst`), preserving the
/// directory structure relative to `src`. Regular files only; anything else
/// (symlinks etc.) is a hard error - the fixture/tape trees contain none.
pub fn copy_tree(src: &Path, dst: &Path) -> Result<()> {
    fs::create_dir_all(dst).with_context(|| format!("creating directory {}", dst.display()))?;
    for entry in fs::read_dir(src).with_context(|| format!("reading {}", src.display()))? {
        let entry = entry.with_context(|| format!("reading {}", src.display()))?;
        let from = entry.path();
        let to = dst.join(entry.file_name());
        let file_type = entry
            .file_type()
            .with_context(|| format!("reading type of {}", from.display()))?;
        if file_type.is_dir() {
            copy_tree(&from, &to)?;
        } else if file_type.is_file() {
            fs::copy(&from, &to)
                .with_context(|| format!("copying {} -> {}", from.display(), to.display()))?;
        } else {
            anyhow::bail!("unsupported file type (symlink?): {}", from.display());
        }
    }
    Ok(())
}

pub fn remove_dir_if_exists(path: &Path) -> Result<()> {
    if path.exists() {
        fs::remove_dir_all(path).with_context(|| format!("removing {}", path.display()))?;
    }
    Ok(())
}

pub fn ensure_dir(path: &Path) -> Result<()> {
    fs::create_dir_all(path).with_context(|| format!("creating {}", path.display()))
}

/// File size in bytes (0 when the file cannot be stat'ed).
pub fn file_size(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

/// Sorted list of file names directly inside `dir` (empty when missing).
pub fn list_files(dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    if !dir.is_dir() {
        return Ok(names);
    }
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        if entry
            .file_type()
            .with_context(|| format!("reading {}", dir.display()))?
            .is_file()
        {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}

/// Sorted list of subdirectory names directly inside `dir` (empty when missing).
pub fn list_dirs(dir: &Path) -> Result<Vec<String>> {
    let mut names = Vec::new();
    if !dir.is_dir() {
        return Ok(names);
    }
    for entry in fs::read_dir(dir).with_context(|| format!("reading {}", dir.display()))? {
        let entry = entry.with_context(|| format!("reading {}", dir.display()))?;
        if entry
            .file_type()
            .with_context(|| format!("reading {}", dir.display()))?
            .is_dir()
        {
            names.push(entry.file_name().to_string_lossy().into_owned());
        }
    }
    names.sort();
    Ok(names)
}
