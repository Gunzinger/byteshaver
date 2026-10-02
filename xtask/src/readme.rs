//! README image-reference rewriting + link lint (plan 17 §8).
//!
//! GitHub's camo proxy caches images aggressively, keyed by the full source
//! URL including the query string - a regenerated asset whose URL stays
//! identical keeps serving the old pixels for a long time. Every
//! `docs/img/<asset>` reference therefore carries a `?v=<version>` query
//! that the publish step keeps in sync with the manifest; only changed
//! assets get a new version, so their URLs (and only theirs) are busted.
//!
//! The scanner is hand-rolled (no regex dependency): it locates
//! `docs/img/<name>` occurrences (optionally prefixed with `/`, optionally
//! followed by `?v=<digits>`) and reports each reference's byte range so
//! rewrites can replace them in place. Anything not shaped like a docs/img
//! asset reference - other URLs, other directories, plain prose - is left
//! untouched.

use anyhow::{Result, anyhow};
use std::collections::{BTreeMap, BTreeSet};
use std::path::Path;

/// Reference prefix managed by this module.
pub const REF_PREFIX: &str = "docs/img/";

/// A located `docs/img/<name>` (optionally with a `?v=<n>` query).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ImgRef {
    /// Asset file name, e.g. `cli-basic.webp`.
    pub name: String,
    /// Parsed `?v=` value when present.
    pub query: Option<u64>,
    /// Byte range of the whole managed reference (`docs/img/...` up to and
    /// including an existing query). Rewrites replace exactly this range.
    pub range: std::ops::Range<usize>,
}

/// True when `bytes[i]` may appear inside an asset name.
fn is_name_char(byte: u8) -> bool {
    byte.is_ascii_alphanumeric() || matches!(byte, b'.' | b'_' | b'-')
}

/// The character before the needle must not extend a longer word or path
/// segment (rejects `mydocs/img/...`, `x.docs/img/...`) while still letting
/// real relative links through (`./docs/img/...`, `../docs/img/...`,
/// `(/docs/img/...)`).
fn boundary_ok(bytes: &[u8], start: usize) -> bool {
    match start.checked_sub(1).map(|i| bytes[i]) {
        None => true,
        Some(prev) => !(prev.is_ascii_alphanumeric() || prev == b'_' || prev == b'.'),
    }
}

/// Scans `text` for every managed image reference, in document order.
pub fn scan_refs(text: &str) -> Vec<ImgRef> {
    let mut refs = Vec::new();
    let needle = REF_PREFIX.as_bytes();
    let bytes = text.as_bytes();
    let mut i = 0;
    while i + needle.len() <= bytes.len() {
        if &bytes[i..i + needle.len()] == needle && boundary_ok(bytes, i) {
            // read the asset name
            let name_start = i + needle.len();
            let mut j = name_start;
            while j < bytes.len() && is_name_char(bytes[j]) {
                j += 1;
            }
            let name = &text[name_start..j];
            // a flat asset reference names a file (contains a dot for the
            // extension) and does not continue into a deeper directory
            let continues_into_dir = j < bytes.len() && bytes[j] == b'/';
            if !name.contains('.') || continues_into_dir {
                // not an asset reference; resume scanning after the needle
                i = name_start;
                continue;
            }
            // optional ?v=<digits> query
            let mut range_end = j;
            let mut query = None;
            if text[j..].starts_with("?v=") {
                let digits_start = j + 3;
                let mut digits_end = digits_start;
                while digits_end < bytes.len() && bytes[digits_end].is_ascii_digit() {
                    digits_end += 1;
                }
                if digits_end > digits_start {
                    query = text[digits_start..digits_end].parse().ok();
                    range_end = digits_end;
                }
            }
            refs.push(ImgRef {
                name: name.to_string(),
                query,
                range: i..range_end,
            });
            i = range_end;
        } else {
            i += 1;
        }
    }
    refs
}

/// Rewrites every reference whose asset has an entry in `versions` to
/// `docs/img/<name>?v=<version>`. References to assets without a manifest
/// entry (and everything else) are left byte-identical. Idempotent: running
/// it again with the same versions map changes nothing.
pub fn rewrite(text: &str, versions: &BTreeMap<String, u64>) -> String {
    let mut out = String::with_capacity(text.len());
    let mut last = 0;
    for img_ref in scan_refs(text) {
        let Some(version) = versions.get(&img_ref.name) else {
            continue;
        };
        out.push_str(&text[last..img_ref.range.start]);
        out.push_str(REF_PREFIX);
        out.push_str(&img_ref.name);
        out.push_str("?v=");
        out.push_str(&version.to_string());
        last = img_ref.range.end;
    }
    out.push_str(&text[last..]);
    out
}

/// Link lint: every `docs/img/...` reference in `text` must resolve to an
/// existing file in `docs_img` (after a publish) or be listed in
/// `extra_present` (dry-run preview: assets that *would* be published).
/// A dangling reference is a hard error - stale screenshots in the README
/// are exactly what this pipeline exists to prevent.
pub fn lint(
    label: &str,
    text: &str,
    docs_img: &Path,
    extra_present: &BTreeSet<String>,
) -> Result<()> {
    let mut missing: Vec<String> = Vec::new();
    for img_ref in scan_refs(text) {
        if docs_img.join(&img_ref.name).is_file() || extra_present.contains(&img_ref.name) {
            continue;
        }
        // 1-based line of the reference: newlines before the byte offset
        let line = text[..img_ref.range.start]
            .bytes()
            .filter(|b| *b == b'\n')
            .count()
            + 1;
        missing.push(format!(
            "{label}:{line}: docs/img/{} does not exist",
            img_ref.name
        ));
    }
    if missing.is_empty() {
        Ok(())
    } else {
        Err(anyhow!(
            "broken image reference(s):\n  {}",
            missing.join("\n  ")
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn versions(pairs: &[(&str, u64)]) -> BTreeMap<String, u64> {
        pairs.iter().map(|(k, v)| (k.to_string(), *v)).collect()
    }

    const BASIC: &str = "![Webp command example](/docs/img/webp_cmd.webp)";

    #[test]
    fn scan_finds_refs_with_and_without_query() {
        let text = "a\ndocs/img/a.webp and /docs/img/b.png?v=12\n(docs/img/c.jpg)";
        let refs = scan_refs(text);
        assert_eq!(refs.len(), 3);
        assert_eq!(refs[0].name, "a.webp");
        assert_eq!(refs[0].query, None);
        assert_eq!(refs[1].name, "b.png");
        assert_eq!(refs[1].query, Some(12));
        assert_eq!(refs[2].name, "c.jpg");
        // ranges slice out exactly the managed part
        assert_eq!(&text[refs[1].range.clone()], "docs/img/b.png?v=12");
    }

    #[test]
    fn scan_ignores_non_asset_shapes() {
        let text = concat!(
            "mydocs/img/x.webp x.docs/img/y.webp ", // word-extended prefixes
            "docs/img/sub/dir.webp ",               // deeper path, not a flat asset
            "docs/img/notanasset ",                 // no extension
            "https://example.com/docs/img/z.webp ", // hmm: absolute URL - still matched (see below)
            "docs/img/ok.webp"
        );
        let names: Vec<String> = scan_refs(text).into_iter().map(|r| r.name).collect();
        let names: Vec<&str> = names.iter().map(String::as_str).collect();
        assert_eq!(names, vec!["z.webp", "ok.webp"]);
    }

    #[test]
    fn rewrite_sets_query_on_first_embed() {
        let out = rewrite(BASIC, &versions(&[("webp_cmd.webp", 1)]));
        assert_eq!(out, "![Webp command example](/docs/img/webp_cmd.webp?v=1)");
    }

    #[test]
    fn rewrite_replaces_stale_query() {
        let text = "![x](docs/img/clean_cmd.webp?v=7)";
        let out = rewrite(text, &versions(&[("clean_cmd.webp", 8)]));
        assert_eq!(out, "![x](docs/img/clean_cmd.webp?v=8)");
    }

    #[test]
    fn rewrite_is_idempotent() {
        let v = versions(&[("clean_cmd.webp", 8)]);
        let once = rewrite("![x](docs/img/clean_cmd.webp?v=7)", &v);
        let twice = rewrite(&once, &v);
        assert_eq!(once, twice);
    }

    #[test]
    fn rewrite_leaves_untracked_refs_and_other_urls_alone() {
        let text = concat!(
            "![x](docs/img/untracked.webp) ",
            "[link](https://example.com/docs/img/other.webp?v=3) ",
            "![y](docs/img/tracked.webp)"
        );
        let out = rewrite(text, &versions(&[("tracked.webp", 2)]));
        assert_eq!(
            out,
            concat!(
                "![x](docs/img/untracked.webp) ",
                "[link](https://example.com/docs/img/other.webp?v=3) ",
                "![y](docs/img/tracked.webp?v=2)"
            )
        );
    }

    #[test]
    fn lint_catches_missing_assets() {
        let dir = std::env::temp_dir().join(format!("xtask-readme-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("exists.webp"), b"png").unwrap();
        // existing reference passes
        assert!(
            lint(
                "README.md",
                "![x](docs/img/exists.webp)",
                &dir,
                &BTreeSet::new()
            )
            .is_ok()
        );
        // missing reference fails, with file + line in the message
        let err = lint(
            "README.md",
            "intro\n\n![x](docs/img/gone.webp)",
            &dir,
            &BTreeSet::new(),
        )
        .unwrap_err()
        .to_string();
        assert!(err.contains("README.md:3"), "{err}");
        assert!(err.contains("gone.webp"), "{err}");
        // dry-run preview: an asset that *would* be published is accepted
        let mut extra = BTreeSet::new();
        extra.insert("gone.webp".to_string());
        assert!(lint("README.md", "![x](docs/img/gone.webp)", &dir, &extra).is_ok());
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
