//! OS integration for the file-table row actions (plan 11 §5): open a
//! file with the OS default viewer and reveal it in the system file
//! manager — hand-rolled on [`std::process::Command`], no extra
//! dependencies.
//!
//! The command construction is factored into pure `(program, args)`
//! functions so the argv shapes stay unit-testable; [`open_path`] and
//! [`reveal_in_folder`] only spawn detached and never block: failures
//! surface as strings (row tooltips), never dialogs and never panics.

use std::path::Path;
use std::process::Command;

/// Opens `path` with the OS default application.
pub fn open_path(path: &Path) -> Result<(), String> {
    let (program, args) = open_argv(path);
    spawn_detached(&program, &args)
}

/// Reveals `path` in the OS file manager (selecting it where supported).
///
/// Linux first tries the freedesktop `FileManager1.ShowItems` D-Bus call
/// (selects the file in the running manager) and falls back to opening
/// the containing folder with `xdg-open`.
pub fn reveal_in_folder(path: &Path) -> Result<(), String> {
    let (program, args) = reveal_argv(path);
    spawn_detached(&program, &args).or_else(|first_error| {
        if cfg!(target_os = "linux") {
            // no gdbus/FileManager1 available: open the parent folder
            let parent = path
                .parent()
                .unwrap_or(Path::new("/"))
                .display()
                .to_string();
            spawn_detached("xdg-open", &[parent])
        } else {
            Err(first_error)
        }
    })
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

/// Command line of [`reveal_in_folder`] (unit-tested argv shape). On
/// Linux this is the `gdbus` form of
/// `org.freedesktop.FileManager1.ShowItems`; [`reveal_in_folder`]
/// supplies the `xdg-open <parent>` fallback.
#[cfg(target_os = "windows")]
pub(crate) fn reveal_argv(path: &Path) -> (String, Vec<String>) {
    (
        "explorer".to_string(),
        vec![format!("/select,{}", path.display())],
    )
}

/// Command line of [`reveal_in_folder`] (unit-tested argv shape).
#[cfg(target_os = "macos")]
pub(crate) fn reveal_argv(path: &Path) -> (String, Vec<String>) {
    (
        "open".to_string(),
        vec!["-R".to_string(), path.display().to_string()],
    )
}

/// Command line of [`reveal_in_folder`] (unit-tested argv shape).
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
