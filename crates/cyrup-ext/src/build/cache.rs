//! Content-addressed artifact cache (arch-08 §4.2 / §6.4, R-ARCH-EXT-016). Cache key = BLAKE3 of
//! (normalized source-tree hash ⊕ toolchain id ⊕ WIT world version). A hit skips `cargo` entirely
//! and instantiates directly; a miss triggers a Tier-1 build.

use crate::error::ExtError;
use std::path::{Path, PathBuf};

/// A content-addressed cache key (hex BLAKE3).
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct CacheKey(pub String);

impl CacheKey {
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// Compute the cache key from a normalized source-tree hash, the toolchain id, and the WIT world
/// version (arch-08 §4.2). The same inputs always yield the same key; any change busts the cache.
pub fn cache_key(source_tree_hash: &[u8], toolchain_id: &str, world_version: &str) -> CacheKey {
    let mut hasher = blake3::Hasher::new();
    hasher.update(source_tree_hash);
    hasher.update(b"\x00");
    hasher.update(toolchain_id.as_bytes());
    hasher.update(b"\x00");
    hasher.update(world_version.as_bytes());
    CacheKey(hasher.finalize().to_hex().to_string())
}

/// Hash a source tree deterministically: walk files in sorted order, folding each relative path and
/// its contents into one BLAKE3 digest. Hidden dirs (`target`, `.git`) are skipped.
pub fn hash_source_tree(root: &Path) -> Result<[u8; 32], ExtError> {
    let mut entries: Vec<PathBuf> = Vec::new();
    collect_files(root, &mut entries)?;
    entries.sort();
    let mut hasher = blake3::Hasher::new();
    for path in &entries {
        let rel = path.strip_prefix(root).unwrap_or(path);
        hasher.update(rel.to_string_lossy().as_bytes());
        hasher.update(b"\x00");
        let bytes = std::fs::read(path)?;
        hasher.update(&(bytes.len() as u64).to_le_bytes());
        hasher.update(&bytes);
    }
    Ok(*hasher.finalize().as_bytes())
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>) -> Result<(), ExtError> {
    for entry in std::fs::read_dir(dir)? {
        let entry = entry?;
        let path = entry.path();
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name == "target" || name == ".git" {
            continue;
        }
        let ft = entry.file_type()?;
        if ft.is_dir() {
            collect_files(&path, out)?;
        } else if ft.is_file() {
            out.push(path);
        }
    }
    Ok(())
}

/// The artifact cache directory (default `~/.cache/cyrup/ext-artifacts/`).
pub struct ArtifactCache {
    root: PathBuf,
}

impl ArtifactCache {
    pub fn new(root: PathBuf) -> Self {
        Self { root }
    }

    /// The default cache location (`$XDG_CACHE_HOME` or `~/.cache`).
    pub fn default_location() -> Self {
        let base = std::env::var_os("XDG_CACHE_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache")))
            .unwrap_or_else(|| PathBuf::from(".cache"));
        Self::new(base.join("cyrup").join("ext-artifacts"))
    }

    /// The directory for a given key.
    pub fn dir_for(&self, key: &CacheKey) -> PathBuf {
        self.root.join(key.as_str())
    }

    /// The component artifact path for a key.
    pub fn artifact_for(&self, key: &CacheKey) -> PathBuf {
        self.dir_for(key).join("extension.component.wasm")
    }

    /// An isolated cargo `--target-dir` for this key's Tier-1 build, so the nested `cargo build`
    /// never contends with the workspace target dir (e.g. when invoked under `cargo test`).
    pub fn build_dir(&self, key: &CacheKey) -> PathBuf {
        self.dir_for(key).join("build")
    }

    /// True iff a built artifact exists for this key (a hit skips `cargo`, R-ARCH-EXT-016).
    pub fn is_hit(&self, key: &CacheKey) -> bool {
        self.artifact_for(key).is_file()
    }

    /// Store a built component under its key.
    pub fn store(&self, key: &CacheKey, component_bytes: &[u8]) -> Result<PathBuf, ExtError> {
        let dir = self.dir_for(key);
        std::fs::create_dir_all(&dir)?;
        let path = self.artifact_for(key);
        std::fs::write(&path, component_bytes)?;
        Ok(path)
    }
}
