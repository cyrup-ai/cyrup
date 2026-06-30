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

pub use shell::{shell_env, ShellConfig, Transport};

/// Access mode for [`FsOps::access`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Access {
    /// Existence-only probe (`F_OK`). Pi `pathExists` (path-utils.ts:31-38) uses `F_OK` to SELECT
    /// the read-path variant; the readability (`R_OK`) check is then a SEPARATE step on the chosen
    /// path that does NOT fall through to other variants (read.ts:238-241).
    Exists,
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

    /// Detect a supported image type from its **magic bytes** (Pi `detectSupportedImageMimeType`,
    /// mime.ts:6-23). This is content sniffing, not the extension — Pi opens the file and reads the
    /// header; cyrup sniffs the bytes returned by the (remote-aware) [`FsOps::read`] seam. Animated
    /// PNG (`acTL` before `IDAT`) and the JPEG `0xFF 0xD8 0xFF 0xF7` (lossless JPEG) variant are
    /// rejected; BMP headers are structurally validated.
    pub fn from_magic(buf: &[u8]) -> Option<Self> {
        const PNG_SIG: [u8; 8] = [0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a];
        if starts_with(buf, &[0xff, 0xd8, 0xff]) {
            return if buf.get(3) == Some(&0xf7) { None } else { Some(ImageMime::Jpeg) };
        }
        if starts_with(buf, &PNG_SIG) {
            return if is_png(buf) && !is_animated_png(buf) { Some(ImageMime::Png) } else { None };
        }
        if starts_with_ascii(buf, 0, b"GIF") {
            return Some(ImageMime::Gif);
        }
        if starts_with_ascii(buf, 0, b"RIFF") && starts_with_ascii(buf, 8, b"WEBP") {
            return Some(ImageMime::Webp);
        }
        if starts_with_ascii(buf, 0, b"BM") && is_bmp(buf) {
            return Some(ImageMime::Bmp);
        }
        None
    }
}

fn starts_with(buf: &[u8], bytes: &[u8]) -> bool {
    buf.len() >= bytes.len() && buf.iter().zip(bytes).all(|(a, b)| a == b)
}

fn starts_with_ascii(buf: &[u8], offset: usize, text: &[u8]) -> bool {
    match buf.get(offset..offset + text.len()) {
        Some(slice) => slice == text,
        None => false,
    }
}

fn read_u16_le(buf: &[u8], off: usize) -> u32 {
    u32::from(buf.get(off).copied().unwrap_or(0)) + (u32::from(buf.get(off + 1).copied().unwrap_or(0)) << 8)
}

fn read_u32_be(buf: &[u8], off: usize) -> u32 {
    (u32::from(buf.get(off).copied().unwrap_or(0)) << 24)
        | (u32::from(buf.get(off + 1).copied().unwrap_or(0)) << 16)
        | (u32::from(buf.get(off + 2).copied().unwrap_or(0)) << 8)
        | u32::from(buf.get(off + 3).copied().unwrap_or(0))
}

fn read_u32_le(buf: &[u8], off: usize) -> u32 {
    u32::from(buf.get(off).copied().unwrap_or(0))
        | (u32::from(buf.get(off + 1).copied().unwrap_or(0)) << 8)
        | (u32::from(buf.get(off + 2).copied().unwrap_or(0)) << 16)
        | (u32::from(buf.get(off + 3).copied().unwrap_or(0)) << 24)
}

/// `isPng` (mime.ts:36-40): IHDR chunk with declared length 13 immediately after the signature.
fn is_png(buf: &[u8]) -> bool {
    buf.len() >= 16 && read_u32_be(buf, 8) == 13 && starts_with_ascii(buf, 12, b"IHDR")
}

/// `isAnimatedPng` (mime.ts:42-55): an `acTL` chunk appearing before the first `IDAT`.
fn is_animated_png(buf: &[u8]) -> bool {
    let mut offset = 8usize; // PNG_SIGNATURE.length
    while offset + 8 <= buf.len() {
        let chunk_len = read_u32_be(buf, offset) as usize;
        let chunk_type_offset = offset + 4;
        if starts_with_ascii(buf, chunk_type_offset, b"acTL") {
            return true;
        }
        if starts_with_ascii(buf, chunk_type_offset, b"IDAT") {
            return false;
        }
        let next = offset.saturating_add(8).saturating_add(chunk_len).saturating_add(4);
        if next <= offset || next > buf.len() {
            return false;
        }
        offset = next;
    }
    false
}

/// `isBmp` (mime.ts:57-81): structural validation of the BMP/DIB header.
fn is_bmp(buf: &[u8]) -> bool {
    if buf.len() < 26 {
        return false;
    }
    let declared_file_size = read_u32_le(buf, 2);
    let pixel_data_offset = read_u32_le(buf, 10);
    let dib_header_size = read_u32_le(buf, 14);
    if declared_file_size != 0 && declared_file_size < 26 {
        return false;
    }
    if pixel_data_offset < 14 + dib_header_size {
        return false;
    }
    if declared_file_size != 0 && pixel_data_offset >= declared_file_size {
        return false;
    }
    let (color_planes, bits_per_pixel) = if dib_header_size == 12 {
        (read_u16_le(buf, 22), read_u16_le(buf, 24))
    } else if (40..=124).contains(&dib_header_size) {
        if buf.len() < 30 {
            return false;
        }
        (read_u16_le(buf, 26), read_u16_le(buf, 28))
    } else {
        return false;
    };
    color_planes == 1 && [1, 4, 8, 16, 24, 32].contains(&bits_per_pixel)
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
/// right error while preserving the accumulated output (R-03-023/024). `Signaled` is a process that
/// died to an external signal (no exit code) without our cancel — Pi returns `exitCode: null` and
/// `bash` treats it as **success** with the output preserved (bash.ts:405), distinct from a cancel.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExitStatus {
    Exited(i32),
    Signaled,
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
