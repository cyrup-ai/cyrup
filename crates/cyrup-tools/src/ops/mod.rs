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

pub use local::{kill_pid, terminate_pid};
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

/// Pi `mime.ts:3` `const IMAGE_TYPE_SNIFF_BYTES = 4100;` — the size of the header buffer
/// `detectSupportedImageMimeTypeFromFile` reads (mime.ts:28-29) before handing it to
/// `detectSupportedImageMimeType`. Every structural check downstream therefore sees AT MOST this
/// many bytes, and `isAnimatedPng` in particular bails `return false` the moment its chunk walk
/// steps past the buffer (mime.ts:51) — so an `acTL` beyond the window is INVISIBLE upstream and
/// the file is reported as `image/png`. `mime.ts` is byte-identical at v0.83.0 and v0.84.1.
pub const IMAGE_TYPE_SNIFF_BYTES: usize = 4100;

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
    ///
    /// The caller MUST pass at most [`IMAGE_TYPE_SNIFF_BYTES`] bytes — the window Pi's
    /// `detectSupportedImageMimeTypeFromFile` reads (mime.ts:25-34) before calling this same
    /// function. The bound is not applied inside here because it is Pi's read size, not a property
    /// of the predicate: mime.ts:30 passes `buffer.subarray(0, bytesRead)` of a
    /// `Buffer.alloc(IMAGE_TYPE_SNIFF_BYTES)`. See [`ImageMime::from_file_head`], which applies it.
    pub fn from_magic(buf: &[u8]) -> Option<Self> {
        Self::from_magic_unbounded(buf)
    }

    /// [`ImageMime::from_magic`] over the same window Pi reads — the port of
    /// `detectSupportedImageMimeTypeFromFile` (mime.ts:25-34) for a caller that already holds the
    /// whole file, which is cyrup's shape because the bytes arrive through the remote-aware
    /// [`FsOps::read`] seam rather than from a local `open`+`read`.
    ///
    /// [CYRUP-DELTA, mechanism only] Pi reads exactly [`IMAGE_TYPE_SNIFF_BYTES`] from the file;
    /// cyrup slices that prefix off the buffer `FsOps::read` returned. The window handed to the
    /// sniffer — and therefore every verdict — is identical. The read itself is deliberately NOT
    /// bounded: the image branch needs the full bytes to encode.
    pub fn from_file_head(buf: &[u8]) -> Option<Self> {
        Self::from_magic_unbounded(buf.get(..IMAGE_TYPE_SNIFF_BYTES).unwrap_or(buf))
    }

    fn from_magic_unbounded(buf: &[u8]) -> Option<Self> {
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

/// Options for a tree walk (grep/find). Hidden files are skipped by default (ripgrep/fd parity).
///
/// `require_git` mirrors fd/ripgrep's `--require-git` behavior: when `false` (fd's
/// `--no-require-git`), `.gitignore` files are honored even outside a git repository; when `true`
/// (fd/ripgrep default), git-ignore semantics only apply inside a repo, so parent `.gitignore`
/// rules stop at nested repo boundaries. Pi's `find` sets this per search path (find.ts:226-240,
/// issue #5960); `grep` keeps the historical unconditional `false`.
#[derive(Clone, Copy, Debug, Default)]
pub struct WalkOpts {
    pub include_hidden: bool,
    pub require_git: bool,
}

/// A single walked path.
#[derive(Clone, Debug)]
pub struct WalkItem {
    pub path: PathBuf,
    pub is_dir: bool,
}

/// What to run, where, and how (transport) for [`ProcOps::exec`].
///
/// `env` is ADDITIVE over the inherited parent environment; `env_remove` names keys to UNSET in the
/// child and is applied FIRST, so a key present in both is deleted and then set — the order Pi's
/// `resolveSpawnContext` uses when it deletes the five session keys and repopulates them
/// (bash.ts:165-181). Pi can express deletion implicitly because it materializes the whole
/// environment; cyrup inherits it, so the removal has to be named.
#[derive(Clone, Debug)]
pub struct ExecSpec {
    pub command: String,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
    pub shell: ShellConfig,
}

/// A DIRECT argv (shell:false) exec request (Pi `execCommand`, exec.ts:34-46): `program` is run with
/// `args` as a real argv vector — NO shell, NO word-splitting — in `cwd`. This is the capability-scoped
/// `exec` grant path (arch-08 exec), distinct from the shell-based [`ExecSpec`] the `bash` tool uses.
///
/// `env` is ADDITIVE on top of the spawned process's normal (fully inherited) environment, never a
/// full replacement — but the `exec` grant's own host boundary (`cyrup-session-svc::host_services
/// ::exec`) never actually populates it from guest input and always passes an empty `Vec`: Pi's real
/// `execCommand` (exec.ts:41-45) never accepts an env override at all, so honoring a guest-supplied
/// one would be new ambient authority with no Pi equivalent. The field itself stays generic backend
/// plumbing for any FUTURE trusted (non-guest-controlled) caller that legitimately needs it.
#[derive(Clone, Debug)]
pub struct ArgvSpec {
    pub program: String,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    pub env: Vec<(String, String)>,
}

/// Captured output of an argv exec (Pi `ExecResult`, exec.ts:23-28): stdout and stderr collected
/// SEPARATELY (unlike the `bash` streaming seam which merges them), plus the exit status.
///
/// `killed` mirrors Pi's `killed` flag (exec.ts:49,97): it is set the instant a SIGTERM/SIGKILL
/// escalation is INITIATED (cancel or timeout) and is otherwise completely orthogonal to `status` —
/// exactly like Pi's `killProcess()` sets its own `killed` local independent of the `code` that
/// `waitForChildProcess` resolves with (`child-process.ts:73-80`: `finalize(exitCode)` always
/// carries the REAL observed exit code, even for a process that catches SIGTERM and exits itself
/// mid-grace). Callers must NOT infer "was killed" from `status` alone.
#[derive(Clone, Debug)]
pub struct ArgvOutput {
    pub status: ExitStatus,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
    pub killed: bool,
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

    /// Open `path` for a **streaming, bounded-memory** read — the seam `grep` searches through.
    ///
    /// Pi never reads a candidate file into the agent process at all on the search path: `grep`
    /// runs the search in a separate ripgrep process (`spawn(rgPath, …)`, grep.ts:226) whose
    /// bounded read buffer decouples file size from Node's heap entirely. cyrup's search is
    /// in-process (the declared `ignore`/`grep-searcher` delta, grep.rs:1-3), so the equivalent
    /// property has to come from the seam: this returns a `Read` that `grep_searcher`'s
    /// `search_reader` pulls through its own rolling buffer instead of the whole-file `Vec<u8>`
    /// that `FsOps::read` materializes before binary detection can even reject the file.
    ///
    /// The returned handle is a blocking [`std::io::Read`] because `grep_searcher`'s API is
    /// synchronous; callers MUST drive it from `tokio::task::spawn_blocking`, never inline.
    ///
    /// Default implementation: read the whole file and hand back a cursor over it. That is the
    /// correct fallback for a backend that genuinely cannot stream (a remote/RPC filesystem), and
    /// it keeps every decorator in `isolation/` forwarding unchanged.
    async fn read_stream(
        &self,
        path: &Path,
    ) -> Result<Box<dyn std::io::Read + Send>, ToolError> {
        Ok(Box::new(std::io::Cursor::new(self.read(path).await?)))
    }

    /// Write `bytes` to `path` **through the existing inode**, creating the file if it is absent —
    /// the single mutation seam both `write` and `edit` use.
    ///
    /// This is Pi's one injected write op: `defaultWriteOperations.writeFile` (write.ts:32-35) and
    /// `defaultEditOperations.writeFile` (edit.ts:83-87) are both
    /// `(path, content) => fsWriteFile(path, content, "utf-8")`, i.e. `fs/promises`' `writeFile`
    /// with the default `{ mode: 0o666, flag: "w" }` — `open(2)` with `O_WRONLY|O_CREAT|O_TRUNC`,
    /// then `write(2)`, then `close(2)`. Implementations MUST preserve that contract's observable
    /// consequences: an existing file's mode/owner is untouched (the creation mode applies only
    /// when `O_CREAT` actually creates), symlinks are FOLLOWED (no `O_NOFOLLOW`), hard links keep
    /// sharing the inode, and a target the caller cannot write fails rather than being replaced.
    ///
    /// **Not atomic, by construction.** Truncation happens at `open`, before any new byte is
    /// written, so a crash / `ENOSPC` / `EIO` mid-write leaves a truncated or partial file with no
    /// rollback, and there is no `fsync`. Pi accepts that trade for metadata fidelity: preserving
    /// inode identity and writing atomically are mutually exclusive at the syscall level. Callers
    /// that need durable, all-or-nothing config writes must use a different primitive
    /// (`cyrup_config::lock::write_atomic`), not this seam. Concurrency is provided one level up by
    /// [`crate::FileMutationLocks`] (Pi `withFileMutationQueue`, write.ts:9 / edit.ts:22), never by
    /// this method.
    async fn write_in_place(&self, path: &Path, bytes: &[u8]) -> Result<(), ToolError>;

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

    /// Run a command as a DIRECT argv vector (shell:false; Pi `execCommand`, exec.ts:34-46), buffering
    /// stdout and stderr SEPARATELY. Honors cancel + timeout, killing the whole process tree on
    /// cancel/timeout (R-03-024/027). Backs the capability-scoped `exec` grant an extension calls
    /// (arch-08 exec); the default is unsupported so only the local backend runs argv execs.
    async fn exec_argv(
        &self,
        _spec: ArgvSpec,
        _cancel: CancelToken,
        _timeout: Option<Duration>,
    ) -> Result<ArgvOutput, ToolError> {
        Err(ToolError::new("argv exec is not supported by this process backend"))
    }
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
