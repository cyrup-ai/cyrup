//! The operations seam — `FsOps` / `ProcOps` (R-03-008, arch-03 §3.3).
//!
//! Every built-in tool is written against these trait objects, never against `std::fs`/
//! `tokio::process` directly. This is the single seam isolation (arch-12) and extensions (`ssh`,
//! `gondolin`) re-target without re-implementing tool logic (R-03-041). A [`Backend`] bundles both;
//! the default is the local backend over tokio fs/process. This is the CANONICAL definition — no
//! competing `Operations` trait exists.

pub mod local;
pub mod shell;

use cyrup_core::{CancelToken, EventStream, ToolError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use shell::{ShellConfig, Transport};

/// Access mode for [`FsOps::access`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    Read,
    ReadWrite,
}

/// File metadata returned by [`FsOps::metadata`].
#[derive(Clone, Debug)]
pub struct Meta {
    pub is_dir: bool,
    pub is_file: bool,
    pub len: u64,
    pub canonical: PathBuf,
}

/// A directory entry (name + path). `is_dir` is resolved by the tool via a follow-up `metadata`
/// call so unstattable entries can be skipped (R-03-035).
#[derive(Clone, Debug)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
}

/// Image type detected by extension (R-03-012).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ImageMime {
    Png,
    Jpeg,
    Gif,
    Webp,
    Bmp,
}

impl ImageMime {
    pub fn mime(self) -> &'static str {
        match self {
            ImageMime::Png => "image/png",
            ImageMime::Jpeg => "image/jpeg",
            ImageMime::Gif => "image/gif",
            ImageMime::Webp => "image/webp",
            ImageMime::Bmp => "image/bmp",
        }
    }

    pub fn from_extension(ext: &str) -> Option<Self> {
        match ext.to_ascii_lowercase().as_str() {
            "png" => Some(ImageMime::Png),
            "jpg" | "jpeg" => Some(ImageMime::Jpeg),
            "gif" => Some(ImageMime::Gif),
            "webp" => Some(ImageMime::Webp),
            "bmp" => Some(ImageMime::Bmp),
            _ => None,
        }
    }
}

/// Options for a tree walk (grep/find). Hidden files are skipped by default (ripgrep/fd parity);
/// `.gitignore` is always honored.
#[derive(Clone, Copy, Debug, Default)]
pub struct WalkOpts {
    pub include_hidden: bool,
}

/// A single walked path.
#[derive(Clone, Debug)]
pub struct WalkItem {
    pub path: PathBuf,
    pub is_dir: bool,
}

/// What to run, where, and how (transport) for [`ProcOps::exec`].
#[derive(Clone, Debug)]
pub struct ExecSpec {
    pub command: String,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub shell: ShellConfig,
}

/// Process outcome. `Killed` (cancel) and `TimedOut` are returned as `Ok` so `bash` can craft the
/// right error while preserving the accumulated output (R-03-023/024).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitStatus {
    Exited(i32),
    Killed,
    TimedOut,
}

/// Filesystem operations (R-03-008).
#[async_trait::async_trait]
pub trait FsOps: Send + Sync {
    async fn read(&self, path: &Path) -> Result<Vec<u8>, ToolError>;
    async fn write_atomic(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError>;
    async fn access(&self, path: &Path, mode: Access) -> Result<(), ToolError>;
    async fn metadata(&self, path: &Path) -> Result<Meta, ToolError>;
    async fn read_dir(&self, path: &Path) -> Result<Vec<DirEntry>, ToolError>;

    /// Detect an image type by extension (None for non-images). Sync; no I/O.
    fn detect_image_mime(&self, path: &Path) -> Option<ImageMime> {
        path.extension().and_then(|e| e.to_str()).and_then(ImageMime::from_extension)
    }

    /// Walk a tree for grep/find. Yields candidate paths (gitignore-aware for the local backend).
    fn walk(&self, root: &Path, opts: WalkOpts) -> EventStream<Result<WalkItem, ToolError>>;
}

/// Process operations (R-03-008). Streams combined stdout+stderr to `on_data`; honors cancel +
/// timeout; MUST kill the whole process tree on cancel/timeout (R-03-024/027).
#[async_trait::async_trait]
pub trait ProcOps: Send + Sync {
    async fn exec(
        &self,
        spec: ExecSpec,
        cancel: CancelToken,
        timeout: Option<Duration>,
        on_data: &mut (dyn for<'a> FnMut(&'a [u8]) + Send),
    ) -> Result<ExitStatus, ToolError>;
}

/// A bundle of both operation surfaces (arch-03 §3.3).
#[derive(Clone)]
pub struct Backend {
    pub fs: Arc<dyn FsOps>,
    pub proc: Arc<dyn ProcOps>,
}

impl Backend {
    /// The default local backend over tokio fs/process with the given shell.
    pub fn local(shell: ShellConfig) -> Self {
        Self {
            fs: Arc::new(local::LocalFs),
            proc: Arc::new(local::LocalProc::new(shell)),
        }
    }
}

impl Default for Backend {
    fn default() -> Self {
        Self::local(ShellConfig::detect())
    }
}
