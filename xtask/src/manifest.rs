//! `docs/media/manifest.json` - the xtask's source of truth for "what is
//! current" (plan 17 §8).
//!
//! Shape:
//!
//! ```json
//! {
//!   "version": 1,
//!   "generated_by": "xtask v0.6.0-6-g4a38f3d; vhs 0.10.0; ffmpeg 7.1",
//!   "assets": { "cli-basic.webp": { "sha256": "<hex>", "version": 3 } }
//! }
//! ```
//!
//! The publish step hashes every final asset it writes into `docs/img/`,
//! bumps `version` only when the bytes changed, and the README rewrite pass
//! copies the version into every `docs/img/<asset>?v=<version>` reference.
//! The version bump defeats GitHub's camo proxy cache, which keys on the
//! full source URL including the query string.

use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::Path;

/// Per-asset record: hash of the committed bytes + cache-busting version.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AssetRecord {
    pub sha256: String,
    pub version: u64,
}

/// The manifest document. `BTreeMap` keeps asset order stable so re-saving
/// unchanged state produces byte-identical JSON (no churn in review PRs).
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Manifest {
    /// Manifest schema version (currently always 1).
    pub version: u32,
    /// Free-form provenance line ("xtask <describe>; vhs <ver>; ffmpeg <ver>").
    #[serde(default)]
    pub generated_by: String,
    /// Asset name (file name in docs/img/) -> record.
    #[serde(default)]
    pub assets: BTreeMap<String, AssetRecord>,
}

impl Manifest {
    /// Loads the manifest; a missing or malformed file is a hard error (the
    /// callers that tolerate absence use [`Manifest::load_or_default`]).
    pub fn load(path: &Path) -> Result<Manifest> {
        let text = std::fs::read_to_string(path)
            .with_context(|| format!("reading manifest {}", path.display()))?;
        serde_json::from_str(&text)
            .with_context(|| format!("parsing manifest {}", path.display()))
    }

    /// Loads the manifest, treating a missing file as an empty manifest
    /// (first publish on a fresh checkout). Parse errors still fail: a
    /// corrupt manifest must not be silently reset.
    pub fn load_or_default(path: &Path) -> Result<Manifest> {
        if path.exists() {
            Manifest::load(path)
        } else {
            Ok(Manifest {
                version: 1,
                ..Default::default()
            })
        }
    }

    /// Pretty JSON + trailing newline.
    pub fn to_json(&self) -> Result<String> {
        let mut json = serde_json::to_string_pretty(self).context("serializing manifest")?;
        json.push('\n');
        Ok(json)
    }

    pub fn save(&self, path: &Path) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).with_context(|| {
                format!("creating manifest directory {}", parent.display())
            })?;
        }
        std::fs::write(path, self.to_json()?)
            .with_context(|| format!("writing manifest {}", path.display()))?;
        Ok(())
    }

    /// Inserts or refreshes an asset record. Returns `true` when the version
    /// was bumped: either the bytes changed vs the record (version + 1) or
    /// the asset is new (version 1). Unchanged assets keep their version so
    /// the README `?v=` queries (and thereby the camo cache) stay stable.
    pub fn upsert(&mut self, name: &str, sha256: String) -> bool {
        match self.assets.get(name) {
            Some(record) if record.sha256 == sha256 => false,
            Some(record) => {
                let version = record.version + 1;
                self.assets
                    .insert(name.to_string(), AssetRecord { sha256, version });
                true
            }
            None => {
                self.assets
                    .insert(name.to_string(), AssetRecord { sha256, version: 1 });
                true
            }
        }
    }

    /// name -> version map for the README rewrite pass.
    pub fn versions(&self) -> BTreeMap<String, u64> {
        self.assets
            .iter()
            .map(|(name, record)| (name.clone(), record.version))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn manifest_with(name: &str, sha: &str, version: u64) -> Manifest {
        Manifest {
            version: 1,
            generated_by: "test".into(),
            assets: BTreeMap::from([(
                name.to_string(),
                AssetRecord {
                    sha256: sha.to_string(),
                    version,
                },
            )]),
        }
    }

    #[test]
    fn round_trips_through_json() {
        let manifest = manifest_with("cli-basic.webp", "abc123", 4);
        let json = manifest.to_json().unwrap();
        let parsed: Manifest = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, manifest);
        // field order + shape are part of the contract (reviewable diffs)
        assert!(json.contains("\"version\": 1"));
        assert!(json.contains("\"generated_by\": \"test\""));
        assert!(json.contains("\"sha256\": \"abc123\""));
        assert!(json.contains("\"version\": 4"));
    }

    #[test]
    fn upsert_bumps_version_only_on_hash_change() {
        let mut manifest = manifest_with("a.webp", "aaa", 2);
        // same hash: no bump
        assert!(!manifest.upsert("a.webp", "aaa".into()));
        assert_eq!(manifest.assets["a.webp"].version, 2);
        // new hash: version + 1
        assert!(manifest.upsert("a.webp", "bbb".into()));
        assert_eq!(manifest.assets["a.webp"].version, 3);
        // new asset: version 1
        assert!(manifest.upsert("b.webp", "ccc".into()));
        assert_eq!(manifest.assets["b.webp"].version, 1);
    }

    #[test]
    fn load_or_default_tolerates_missing_but_not_corrupt() {
        let dir = std::env::temp_dir().join(format!("xtask-manifest-test-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("manifest.json");
        // missing -> empty manifest
        let manifest = Manifest::load_or_default(&path).unwrap();
        assert!(manifest.assets.is_empty());
        // corrupt -> hard error (must not be silently reset)
        std::fs::write(&path, "{ not json").unwrap();
        assert!(Manifest::load_or_default(&path).is_err());
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[test]
    fn save_is_byte_stable_for_unchanged_state() {
        let mut manifest = manifest_with("a.webp", "aaa", 1);
        manifest.upsert("b.webp", "bbb".into());
        let first = manifest.to_json().unwrap();
        // a no-change upsert pass must not alter the serialization
        manifest.upsert("a.webp", "aaa".into());
        manifest.upsert("b.webp", "bbb".into());
        assert_eq!(manifest.to_json().unwrap(), first);
    }
}
