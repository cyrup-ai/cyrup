//! The bundled prompt/skill tree, embedded in the binary (FLUX-001).
//!
//! `build.rs` walks `resources/` at compile time and generates the [`BUNDLED_FILES`] table this
//! module `include!`s, so the fifteen `/flux/*` templates, their `_docs/`, and the `flux` skill
//! travel INSIDE the binary — the "package data" half of upstream's
//! `flux_bootstrap/installer.py` @v0.0.40 (`BUNDLED_DIR = Path(__file__).parent / "bundled"`,
//! `:47`), which cyrup cannot spell as a path because a Rust binary has no `__file__`. The other
//! half, copying the payload into the config dir, is [`crate::install`].

use std::sync::OnceLock;

use sha2::{Digest, Sha256};

/// One embedded file: its path relative to `resources/` (forward slashes, e.g.
/// `prompts/flux/exec.md`) and its bytes.
#[derive(Clone, Copy, Debug)]
pub struct BundledFile {
    /// Path relative to the bundle root, `/`-separated, never starting with `/` or `.`.
    pub rel: &'static str,
    /// The file's content, byte-exact.
    pub bytes: &'static [u8],
}

include!(concat!(env!("OUT_DIR"), "/bundled.rs"));

/// Every embedded file, sorted by [`BundledFile::rel`] (`installer.py:134` sorts the walk too).
#[must_use]
pub fn bundled_files() -> &'static [BundledFile] {
    BUNDLED_FILES
}

/// The bytes of one embedded file by bundle-relative path, or `None` when the bundle has no such
/// file.
#[must_use]
pub fn bundled_file(rel: &str) -> Option<&'static [u8]> {
    BUNDLED_FILES.iter().find(|f| f.rel == rel).map(|f| f.bytes)
}

/// SHA-256 hex of one payload — `installer.py:79-80` `_sha256_bytes`.
#[must_use]
pub fn sha256_hex(bytes: &[u8]) -> String {
    format!("{:x}", Sha256::digest(bytes))
}

/// A hex digest over every embedded path and payload, in table order: the value that changes
/// exactly when the bundle's content changes. Computed once per process.
///
/// Upstream's install is gated on the code-puppy VERSION string alone (`installer.py:165-167`
/// `needs_install`, `register_callbacks.py:38-44` `_current_version`). cyrup gates on
/// [`crate::install::bundle_marker`], which carries this fingerprint as well: a same-version
/// rebuild with an edited template (every from-source development build) would otherwise keep
/// serving the previously installed copy until the next version bump. Inference, not an upstream
/// rule — labelled as such in the FLUX-001 closure.
#[must_use]
pub fn bundle_fingerprint() -> &'static str {
    static FINGERPRINT: OnceLock<String> = OnceLock::new();
    FINGERPRINT.get_or_init(|| {
        let mut hasher = Sha256::new();
        for file in BUNDLED_FILES {
            hasher.update(file.rel.as_bytes());
            hasher.update([0u8]);
            hasher.update(file.bytes);
            hasher.update([0u8]);
        }
        format!("{:x}", hasher.finalize())
    })
}
