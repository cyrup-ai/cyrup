//! The `ext-fs` WIT import: capability-scoped file reads and writes, resolved by the host against
//! the `capabilities.fs` roots the extension's `extension.json` declared.

use super::Ctx;

impl Ctx {
    /// Read a file through the capability-scoped `ext-fs` grant (EXT-055). `path` is relative to the
    /// project root; the host resolves it against the `capabilities.fs` roots the extension's
    /// `extension.json` declared (`["read:.", "write:.cyrup/todo"]`) and refuses anything outside
    /// them — including a declaration-free manifest, which grants no root at all.
    ///
    /// Before EXT-054/EXT-055 the `ext-fs` interface had no SDK wrapper and no host-side root, so it
    /// was unreachable from a guest in both directions at once.
    pub fn read_file(&self, path: &str) -> Result<Vec<u8>, String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ext_fs::read_file(path);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = path;
            Err("ext-fs unavailable on host target".into())
        }
    }

    /// Write a file through the capability-scoped `ext-fs` grant (EXT-055). Requires a `write:`
    /// root in `capabilities.fs` covering `path` — a `read:` grant is refused, which is the whole
    /// point of the manifest having two modes.
    pub fn write_file(&self, path: &str, data: &[u8]) -> Result<(), String> {
        #[cfg(target_arch = "wasm32")]
        {
            return crate::guest::bindings::cyrup::ext::ext_fs::write_file(path, data);
        }
        #[cfg(not(target_arch = "wasm32"))]
        {
            let _ = (path, data);
            Err("ext-fs unavailable on host target".into())
        }
    }
}
