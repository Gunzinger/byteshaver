//! OS integration for the file-table row actions (plan 11 §5): open a
//! file with the OS default viewer and reveal it in the system file
//! manager — hand-rolled on [`std::process::Command`], no extra
//! dependencies.
//!
//! The command construction is factored into pure `(program, args)`
//! functions so the argv shapes stay unit-testable; [`open_path`] and
//! [`reveal_in_folder`] only spawn detached and never block: failures
//! surface as strings (row tooltips), never dialogs and never panics.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Opens `path` with the OS default application.
pub fn open_path(path: &Path) -> Result<(), String> {
    let (program, args) = open_argv(path);
    spawn_detached(&program, &args)
}

/// Reveals `path` in the OS file manager (selecting it where supported).
///
/// The path is resolved to its canonical absolute form first (plan 15
/// F14: relative or non-canonical paths defeat both `explorer /select,`
/// and the D-Bus `ShowItems` URI).
///
/// Linux first tries the freedesktop `FileManager1.ShowItems` D-Bus call
/// (selects the file in the running manager); the `gdbus` child is
/// spawned detached but **watched** — a non-zero exit inside the watch
/// window (no `FileManager1` service registered) or a missing binary
/// falls back to opening the containing folder with `xdg-open`. Windows
/// normalizes the path for `explorer /select,` and opens the parent when
/// the spawn itself fails. macOS uses `open -R`.
pub fn reveal_in_folder(path: &Path) -> Result<(), String> {
    let resolved = resolve_absolute(path);
    #[cfg(target_os = "linux")]
    return reveal_linux(&resolved);
    #[cfg(not(target_os = "linux"))]
    {
        let (program, args) = reveal_argv(&resolved);
        spawn_detached(&program, &args).or_else(|first_error| {
            if cfg!(target_os = "windows") {
                // explorer itself failed to launch: open the folder instead
                spawn_detached("explorer", &[parent_folder(&resolved)])
            } else {
                Err(first_error)
            }
        })
    }
}

/// Linux reveal: the `gdbus` form of `org.freedesktop.FileManager1.
/// ShowItems`, with its exit status observed (plan 15 F14 — it used to be
/// spawned detached and forgotten): a non-zero exit within the watch
/// window (no `FileManager1` service answered) or a missing `gdbus`
/// binary falls back to `xdg-open <parent>`.
#[cfg(target_os = "linux")]
fn reveal_linux(path: &Path) -> Result<(), String> {
    const WATCH_WINDOW: std::time::Duration = std::time::Duration::from_millis(1_000);
    const WATCH_POLL: std::time::Duration = std::time::Duration::from_millis(50);

    let parent = parent_folder(path);
    let (program, args) = reveal_argv(path);
    let mut child = match Command::new(&program).args(&args).spawn() {
        Ok(child) => child,
        // no gdbus binary: straight to the containing folder
        Err(_) => return spawn_detached("xdg-open", &[parent]),
    };
    std::thread::Builder::new()
        .name("byteshaver-reveal-watch".to_string())
        .spawn(move || {
            // a real ShowItems call answers within the watch window;
            // exiting non-zero inside it means no FileManager1 service is
            // registered (or the call failed) → open the folder instead.
            // Still running past the window counts as success (never
            // double-open).
            let deadline = std::time::Instant::now() + WATCH_WINDOW;
            loop {
                match child.try_wait() {
                    Ok(Some(status)) => {
                        if !status.success()
                            && let Err(error) = Command::new("xdg-open").arg(&parent).spawn()
                        {
                            eprintln!("xdg-open reveal fallback failed: {error}");
                        }
                        return;
                    }
                    Ok(None) if std::time::Instant::now() < deadline => {
                        std::thread::sleep(WATCH_POLL);
                    }
                    Ok(None) | Err(_) => return,
                }
            }
        })
        .map_err(|error| format!("reveal watcher could not be started: {error}"))?;
    Ok(())
}

/// Canonical absolute form of `path` for OS integration: `canonicalize()`
/// when the entry exists, lexical absolutization otherwise (reveal
/// targets are freshly written files, so the canonicalize fast path
/// dominates; the fallback keeps freshly-picked-but-just-renamed targets
/// working). Unit-tested at the argv level via the pure transforms.
pub(crate) fn resolve_absolute(path: &Path) -> PathBuf {
    path.canonicalize()
        .unwrap_or_else(|_| absolutize_lexical(path))
}

/// Lexical absolutization: makes `path` absolute against the current
/// directory and collapses `.`/`..` components textually (no symlink
/// resolution, no existence check). Pure given an absolute input.
fn absolutize_lexical(path: &Path) -> PathBuf {
    let joined = if path.is_absolute() {
        path.to_path_buf()
    } else {
        std::env::current_dir()
            .unwrap_or_else(|_| PathBuf::from("/"))
            .join(path)
    };
    let mut resolved = PathBuf::new();
    for component in joined.components() {
        match component {
            std::path::Component::CurDir => {}
            std::path::Component::ParentDir => {
                resolved.pop();
            }
            component => resolved.push(component.as_os_str()),
        }
    }
    resolved
}

/// Display form of `path`'s parent folder (root when there is none).
fn parent_folder(path: &Path) -> String {
    path.parent()
        .unwrap_or(Path::new("/"))
        .display()
        .to_string()
}

/// Command line of [`open_path`] (unit-tested argv shape).
#[cfg(target_os = "windows")]
pub(crate) fn open_argv(path: &Path) -> (String, Vec<String>) {
    // the empty string is `start`'s window-title argument (quoted paths
    // would otherwise be parsed as a title)
    (
        "cmd".to_string(),
        vec![
            "/C".to_string(),
            "start".to_string(),
            String::new(),
            path.display().to_string(),
        ],
    )
}

/// Command line of [`open_path`] (unit-tested argv shape).
#[cfg(not(target_os = "windows"))]
pub(crate) fn open_argv(path: &Path) -> (String, Vec<String>) {
    if cfg!(target_os = "macos") {
        ("open".to_string(), vec![path.display().to_string()])
    } else {
        ("xdg-open".to_string(), vec![path.display().to_string()])
    }
}

/// Command line of [`reveal_in_folder`] (unit-tested argv shape). The
/// path goes through [`normalize_windows_select`] first: `explorer
/// /select,` silently opens its default view (Win11 "Home") unless it
/// gets a plain backslash path without the extended-length prefix (plan
/// 15 F14).
#[cfg(target_os = "windows")]
pub(crate) fn reveal_argv(path: &Path) -> (String, Vec<String>) {
    (
        "explorer".to_string(),
        vec![format!("/select,{}", normalize_windows_select(path))],
    )
}

/// Explorer `/select,` argument form of `path`: the `\\?\UNC\` and `\\?\`
/// extended-length prefixes (`canonicalize()` adds them) are stripped and
/// every separator becomes a backslash. Pure string transforms;
/// unit-tested on every host.
#[cfg(any(target_os = "windows", test))]
fn normalize_windows_select(path: &Path) -> String {
    let text = path.as_os_str().to_string_lossy();
    let text = if let Some(unc) = text.strip_prefix(r"\\?\UNC\") {
        format!(r"\\{unc}")
    } else if let Some(plain) = text.strip_prefix(r"\\?\") {
        plain.to_string()
    } else {
        text.to_string()
    };
    text.replace('/', "\\")
}

/// Command line of [`reveal_in_folder`] (unit-tested argv shape); macOS
/// reveals through `open -R` and receives the canonicalized absolute path
/// [`resolve_absolute`] produced.
#[cfg(target_os = "macos")]
pub(crate) fn reveal_argv(path: &Path) -> (String, Vec<String>) {
    (
        "open".to_string(),
        vec!["-R".to_string(), path.display().to_string()],
    )
}

/// Command line of [`reveal_in_folder`] (unit-tested argv shape). On
/// Linux this is the `gdbus` form of
/// `org.freedesktop.FileManager1.ShowItems`; [`reveal_in_folder`]
/// supplies the watched `xdg-open <parent>` fallback and hands this
/// function a canonicalized absolute path (so the [`file_uri`] argument
/// is always absolute).
#[cfg(not(any(target_os = "windows", target_os = "macos")))]
pub(crate) fn reveal_argv(path: &Path) -> (String, Vec<String>) {
    (
        "gdbus".to_string(),
        vec![
            "call".to_string(),
            "--session".to_string(),
            "--dest".to_string(),
            "org.freedesktop.FileManager1".to_string(),
            "--object-path".to_string(),
            "/org/freedesktop/FileManager1".to_string(),
            "--method".to_string(),
            "org.freedesktop.FileManager1.ShowItems".to_string(),
            format!("[{}]", file_uri(path)),
            String::new(),
        ],
    )
}

/// Spawns `program args` detached from the GUI process; only the spawn
/// itself is waited for (the child outlives the row click). Errors
/// (missing helper binary, …) become the caller's tooltip string.
fn spawn_detached(program: &str, args: &[String]) -> Result<(), String> {
    Command::new(program)
        .args(args)
        .spawn()
        .map(|_child| ())
        .map_err(|error| format!("{} could not be launched: {error}", program))
}

/// Percent-encoded `file://` URI of an absolute path (RFC 3986 unreserved
/// set kept verbatim, everything else `%XX`-escaped; backslashes map to
/// forward slashes for Windows paths). Pure; unit-tested.
#[cfg(any(target_os = "linux", test))]
#[must_use]
pub(crate) fn file_uri(path: &Path) -> String {
    let text = path.as_os_str().to_string_lossy();
    let mut uri = String::from("file://");
    for byte in text.as_bytes() {
        match byte {
            b'\\' => uri.push('/'),
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                uri.push(*byte as char)
            }
            _ => uri.push_str(&format!("%{byte:02X}")),
        }
    }
    uri
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    // ---- file URI encoding (all platforms) ---------------------------------

    #[test]
    fn file_uri_escapes_unreserved_only() {
        assert_eq!(file_uri(Path::new("/x/a.png")), "file:///x/a.png");
        assert_eq!(
            file_uri(Path::new("/photos/my cat & dog (2024).png")),
            "file:///photos/my%20cat%20%26%20dog%20%282024%29.png"
        );
        assert_eq!(
            file_uri(Path::new("/umlaute/örb.jpg")),
            "file:///umlaute/%C3%B6rb.jpg"
        );
        // spaces, hashes and question marks must never stay raw
        for raw in ["a b", "a#b", "a?b", "a%b"] {
            let uri = file_uri(Path::new(&format!("/x/{raw}")));
            let encoded = &uri["file:///x/".len()..];
            assert_ne!(encoded, raw, "raw {raw} leaked unescaped");
        }
    }

    // ---- reveal path normalization (plan 15 F14) -----------------------------

    #[test]
    fn normalize_windows_select_strips_prefixes_and_forces_backslashes() {
        // forward-slash input (the defect behind the Documents fallback)
        let select = |text: &str| normalize_windows_select(Path::new(text));
        assert_eq!(select("C:/x/my folder/a.png"), r"C:\x\my folder\a.png");
        // already-normalized input passes through
        assert_eq!(select(r"C:\x\a.png"), r"C:\x\a.png");
        // the extended-length prefix canonicalize() adds defeats /select,
        assert_eq!(select(r"\\?\C:\x\a.png"), r"C:\x\a.png");
        // …and its UNC form maps back onto a share path
        assert_eq!(select(r"\\?\UNC\server\share\a.png"), r"\\server\share\a.png");
    }

    #[cfg(unix)]
    #[test]
    fn absolutize_lexical_collapses_dot_components_without_the_fs() {
        assert_eq!(
            absolutize_lexical(Path::new("/x/./y/../z.png")),
            PathBuf::from("/x/z.png")
        );
        // .. at the root stays at the root (pop on "/" is a no-op)
        assert_eq!(
            absolutize_lexical(Path::new("/../../a.png")),
            PathBuf::from("/a.png")
        );
    }

    #[test]
    fn resolve_absolute_canonicalizes_or_absolutizes_lexically() {
        // an existing file resolves through canonicalize()
        let file = std::env::temp_dir().join(format!("byteshaver-reveal-{}", std::process::id()));
        std::fs::write(&file, b"x").expect("write probe file");
        assert_eq!(
            resolve_absolute(&file),
            file.canonicalize().expect("probe file exists")
        );
        let _ = std::fs::remove_file(&file);

        // a missing file falls back to lexical absolutization
        let missing = std::env::temp_dir().join("byteshaver-reveal-missing.png");
        assert_eq!(resolve_absolute(&missing), missing);

        // relative inputs resolve against the current directory
        let cwd = std::env::current_dir().expect("cwd");
        assert_eq!(absolutize_lexical(Path::new("a.png")), cwd.join("a.png"));
    }

    #[cfg(target_os = "windows")]
    #[test]
    fn argv_shapes_windows() {
        let (program, args) = open_argv(&PathBuf::from(r"C:\x\a.png"));
        assert_eq!(
            (program.as_str(), args.as_slice()),
            ("cmd", &["/C", "start", "", "C:\\x\\a.png"])
        );
        let (program, args) = reveal_argv(&PathBuf::from(r"C:\x\a.png"));
        assert_eq!(
            (program.as_str(), args.as_slice()),
            ("explorer", &["/select,C:\\x\\a.png"])
        );
        // forward-slash paths must not leak into /select, (plan 15 F14)
        let (program, args) = reveal_argv(&PathBuf::from("C:/x/out/a.webp"));
        assert_eq!(
            (program.as_str(), args.as_slice()),
            ("explorer", &["/select,C:\\x\\out\\a.webp"])
        );
        assert_eq!(
            file_uri(&PathBuf::from(r"C:\x\a.png")),
            "file:///C:/x/a.png"
        );
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn argv_shapes_macos() {
        let (program, args) = open_argv(&PathBuf::from("/x/a.png"));
        assert_eq!((program.as_str(), args.as_slice()), ("open", &["/x/a.png"]));
        let (program, args) = reveal_argv(&PathBuf::from("/x/a.png"));
        assert_eq!(
            (program.as_str(), args.as_slice()),
            ("open", &["-R", "/x/a.png"])
        );
    }

    #[cfg(not(any(target_os = "windows", target_os = "macos")))]
    #[test]
    fn argv_shapes_linux() {
        let (program, args) = open_argv(&PathBuf::from("/x/a.png"));
        assert_eq!(program, "xdg-open");
        assert_eq!(args, vec!["/x/a.png"]);
        let (program, args) = reveal_argv(&PathBuf::from("/x/a.png"));
        assert_eq!(program, "gdbus");
        assert_eq!(
            args[..8],
            [
                "call",
                "--session",
                "--dest",
                "org.freedesktop.FileManager1",
                "--object-path",
                "/org/freedesktop/FileManager1",
                "--method",
                "org.freedesktop.FileManager1.ShowItems"
            ]
        );
        assert_eq!(args[8], "[file:///x/a.png]");
        assert_eq!(args[9], "");
    }

    #[test]
    fn spawn_failures_surface_as_error_strings() {
        // a program that cannot exist: the error is a message, never a panic
        let result = spawn_detached("byteshaver-gui-definitely-not-a-binary", &[]);
        assert!(result.is_err());
        assert!(
            result
                .expect_err("message")
                .contains("could not be launched")
        );
    }
}
