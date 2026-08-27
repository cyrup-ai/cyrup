//! The operations seam — `FsOps` / `ProcOps` (R-03-008, arch-03 §3.3).
//!
//! Every built-in tool is written against these trait objects, never against `std::fs`/
//! `tokio::process` directly. This is the single seam isolation (arch-12) and extensions (`ssh`,
//! `gondolin`) re-target without re-implementing tool logic (R-03-041). A [`Backend`] bundles both;
//! the default is the local backend over tokio fs/process. This is the CANONICAL definition — no
//! competing `Operations` trait exists.

/// Cancellation pulled into a blocking `std::io::Read` consumer — `pub(crate)` because more
/// than one tool needs the same sentinel/adapter pair; not part of the public seam.
pub(crate) mod cancel_read;
pub mod local;
pub mod shell;
pub(crate) mod win;

use cyrup_core::{CancelToken, EventStream, ToolError};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

pub use local::{
    kill_pid, kill_process_tree, kill_tracked_detached_children, terminate_pid,
    track_detached_child_pid, untrack_detached_child_pid,
};
pub use shell::{ShellConfig, Transport, shell_env};

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
            return if buf.get(3) == Some(&0xf7) {
                None
            } else {
                Some(ImageMime::Jpeg)
            };
        }
        if starts_with(buf, &PNG_SIG) {
            return if is_png(buf) && !is_animated_png(buf) {
                Some(ImageMime::Png)
            } else {
                None
            };
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
    u32::from(buf.get(off).copied().unwrap_or(0))
        + (u32::from(buf.get(off + 1).copied().unwrap_or(0)) << 8)
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
        let next = offset
            .saturating_add(8)
            .saturating_add(chunk_len)
            .saturating_add(4);
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

/// Which upstream binary's ignore-file set a walk reproduces.
///
/// `.fdignore` and `.rgignore` are BOTH opt-in `WalkBuilder::add_custom_ignore_filename`
/// registrations in the `ignore` crate, and each is read by exactly ONE of the two tools pi
/// shells out to: fd reads `.fdignore` and a global `<config>/fd/ignore`; ripgrep reads
/// `.rgignore` and has no global ignore file of its own. Because `find` and `grep` share the one
/// `FsOps::walk` seam, that seam cannot register either name unconditionally without giving one
/// tool an exclusion source its upstream does not have. Naming the caller is the whole job of
/// this enum.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WalkFlavor {
    /// No tool-specific ignore sources: `.ignore` plus the gitignore family only. This is the
    /// `Default` so that a defaulted `WalkOpts` can never silently confer fd or ripgrep
    /// semantics on a walker that did not ask for them.
    #[default]
    Plain,
    /// fd (`find`, find.ts:225-269). Registers `.fdignore` and fd's global ignore file.
    Fd,
    /// ripgrep (`grep`, grep.ts:177 `ensureTool("rg")`, argv at `:220-224`). Registers
    /// `.rgignore`; ripgrep has no global ignore file, so nothing else attaches here.
    Rg,
}

impl WalkFlavor {
    /// The custom ignore FILENAME this flavor's upstream reads, if any. Custom ignore files
    /// outrank `.ignore` and every gitignore source (ignore 0.4.26 `dir.rs:580-585`).
    ///
    /// Exactly ONE name is ever returned, so `find` can never see `.rgignore` and `grep` can
    /// never see `.fdignore`: the cross-contamination a shared walk seam otherwise invites is
    /// structurally impossible rather than merely avoided.
    pub fn custom_ignore_filename(self) -> Option<&'static str> {
        match self {
            Self::Plain => None,
            Self::Fd => Some(".fdignore"),
            // ripgrep registers `.rgignore` gated on `!no_ignore_dot` — the SAME knob that
            // gates `.ignore` (ripgrep 14.1.0 `crates/core/flags/hiargs.rs:891, :897-899`).
            // Pi's argv passes neither `--no-ignore` nor `--no-ignore-dot` (grep.ts:220-224)
            // and cyrup never disables `WalkBuilder::ignore`, so this is unconditional,
            // exactly as it is upstream.
            Self::Rg => Some(".rgignore"),
        }
    }

    /// Whether this flavor's upstream reads a GLOBAL ignore file. Only fd does
    /// (fd 10.5.0 `src/walk.rs:371-386`); ripgrep has no equivalent.
    pub fn reads_fd_global_ignore(self) -> bool {
        matches!(self, Self::Fd)
    }
}

/// Options for a tree walk (grep/find). Hidden files are skipped by default (ripgrep/fd parity).
///
/// `require_git` mirrors fd/ripgrep's `--require-git` behavior: when `false` (fd's
/// `--no-require-git`), `.gitignore` files are honored even outside a git repository; when `true`
/// (fd/ripgrep default), git-ignore semantics only apply inside a repo, so parent `.gitignore`
/// rules stop at nested repo boundaries. Pi's `find` sets this per search path (find.ts:226-240,
/// issue #5960); `grep` keeps the historical unconditional `false`.
///
/// `flavor` names the upstream binary being emulated so the shared walk seam can register the
/// tool-specific ignore sources — see [`WalkFlavor`].
#[derive(Clone, Copy, Debug, Default)]
pub struct WalkOpts {
    pub include_hidden: bool,
    pub require_git: bool,
    pub flavor: WalkFlavor,
}

/// A single walked path.
///
/// `is_dir` and `is_file` are BOTH carried because they are not complements. A symlink, a FIFO, a
/// socket and a device node are each neither, and separating those from a regular file is the
/// reason this type exists rather than a bare `PathBuf`. Both flags describe the ENTRY's own type
/// — `lstat` semantics, never followed — because that is the type the upstream binaries decide on:
/// `ignore` 0.4.26 builds a traversed entry's type from `std::fs::DirEntry::file_type`
/// (`walk.rs:322-333`, `:353-367`), which does not resolve the link.
///
/// `grep` filters on `is_file` ALONE, reproducing ripgrep's `SubjectBuilder`: a traversal-discovered
/// entry is searched only when `file_type().is_file()` holds, so an in-tree symlink is never opened
/// and its target is never searched under the link's name. `find` filters on `is_dir` ALONE and must
/// keep doing so — fd DOES list symlinks, and `find` uses the flag only to decide the trailing `/`
/// that marks a directory in its output.
#[derive(Clone, Debug)]
pub struct WalkItem {
    pub path: PathBuf,
    pub is_dir: bool,
    pub is_file: bool,
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
    async fn read_stream(&self, path: &Path) -> Result<Box<dyn std::io::Read + Send>, ToolError> {
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
        path.extension()
            .and_then(|e| e.to_str())
            .and_then(ImageMime::from_extension)
    }

    /// Walk a tree for grep/find. Yields candidate paths (gitignore-aware for the local backend).
    ///
    /// A yielded `Err` is a NON-FATAL per-entry event, not end-of-stream: the walk continues and
    /// further `Ok` items follow. Implementations MUST keep walking after emitting one, and
    /// consumers MUST keep polling. Whether such an error fails the *tool call* is the caller's
    /// decision and the two callers differ — `find` emulates fd, which swallows every traversal
    /// error and exits 0, while `grep` emulates ripgrep, which reports it and exits 2. The message
    /// carries no tool prefix for that reason.
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
        Err(ToolError::new(
            "argv exec is not supported by this process backend",
        ))
    }
}

/// Execution options for one [`BashOperations::exec`] call — the port of the options bag pi's
/// `BashOperations.exec` takes as its third argument (`{onData, signal, timeout, env}`,
/// `packages/coding-agent/src/core/tools/bash.ts:64-71` @v0.83.0).
///
/// `on_data` is pi's `onData: (data: Buffer) => void`: RAW bytes, combined stdout+stderr, sanitized
/// by the CALLER (`executeBashWithOperations` strips ANSI and normalizes CR inside its own `onData`
/// wrapper, `bash-executor.ts:78-102`), never by the backend.
///
/// `cancel` is pi's `signal?: AbortSignal` re-shaped as a poll+notify token, the same substitution
/// `ProcOps::exec` already makes; `timeout` is pi's `timeout?: number` in seconds, carried here as a
/// `Duration` because the seconds→ms conversion and the `MAX_TIMEOUT_MS` ceiling are the bash TOOL's
/// input validation (`bash.ts:20-38`), not the backend's.
///
/// `env` is ADDITIVE over the inherited parent environment and `env_remove` names keys to UNSET,
/// applied FIRST — identical to [`ExecSpec`], and for the same reason: pi materializes the whole
/// environment (`env: env ?? getShellEnv()`, `bash.ts:100`) so it can express a deletion by omission,
/// while cyrup inherits and therefore has to name it. An EMPTY `env` here is pi's `env: undefined`,
/// i.e. "use the inherited shell environment", NOT "run with an empty environment".
pub struct BashExecOptions<'a> {
    pub on_data: &'a mut (dyn for<'b> FnMut(&'b [u8]) + Send),
    pub cancel: CancelToken,
    pub timeout: Option<Duration>,
    pub env: Vec<(String, String)>,
    pub env_remove: Vec<String>,
}

/// A PER-CALL, pluggable command-execution backend for the bash seams — the port of pi's
/// `BashOperations` (`packages/coding-agent/src/core/tools/bash.ts:52-73` @v0.83.0: *"Pluggable
/// operations for the bash tool. Override these to delegate command execution to remote systems (for
/// example SSH)."*).
///
/// **Why this is a SEPARATE trait from [`ProcOps`] rather than a re-use of it.** The two seams have
/// different lifetimes and different suppliers, which is exactly pi's split:
///
/// * [`ProcOps`] is the SESSION-lifetime backend chosen at construction — pi has no interface for it
///   at all, because in pi it is just "whatever `spawn` does". It carries the argv surface
///   ([`ProcOps::exec_argv`], pi `execCommand`, `exec.ts:34-46`) that the capability-scoped `exec`
///   grant needs and that a bash backend has no business providing.
/// * `BashOperations` is the PER-CALL override an *extension* supplies: `BashToolOptions.operations`
///   (`bash.ts:186-188`) for the agent-loop `bash` tool, and `UserBashEventResult.operations`
///   (`extensions/types.ts:1078-1081`) for one single `user_bash` command — pi's `executeBash`
///   resolves it fresh on every invocation, `options?.operations ?? createLocalBashOperations({
///   shellPath })` (`agent-session.ts:2782`). It is a ONE-METHOD interface upstream and stays one
///   here; widening it to `ProcOps` would demand an argv backend from every `ssh`/sandbox extension
///   that only ever wanted to redirect a shell command.
///
/// The default implementation is [`LocalBashOperations`] — pi's `createLocalBashOperations`
/// (`bash.ts:82`), which upstream exports to extensions for exactly the wrap-then-delegate case
/// (`packages/coding-agent/src/index.ts:281`).
///
/// [CYRUP-DELTA, mechanism] A WASM guest cannot RETURN an implementation of this trait: ADR-0002
/// (`docs/adr/ADR-0002-extension-io-is-serde.md`, rule 4) makes extension I/O values rather than
/// references, so the guest half of `UserBashEventResult.operations` needs a registration import plus
/// a keyed dispatch export in `crates/cyrup-ext/wit/world.wit` before an extension can supply one.
/// That round-trip is NOT built yet; the register entry naming its cost lives in
/// `crates/cyrup-ext/src/lib.rs` (DRIFT-004 / SEAM-015). This trait is the host-side half and is
/// complete: any in-host caller — the isolation decorators (arch-12), a future keyed guest proxy —
/// can already supply one.
///
/// The CONSUMER side is built as well: `cyrup_session_svc::BashOptions::operations` carries an
/// `Arc<dyn BashOperations>` through `execute_bash_with_user_event` into `execute_bash`, which
/// resolves pi's `options?.operations ?? createLocalBashOperations({ shellPath })`
/// (`agent-session.ts:2782`) and routes the whole sanitize/buffer/spill pipeline over whichever
/// backend won. The one remaining half of DRIFT-004 / SEAM-015 is the guest round-trip above.
#[async_trait::async_trait]
pub trait BashOperations: Send + Sync {
    /// Execute `command` in `cwd`, streaming combined stdout+stderr to `opts.on_data`.
    ///
    /// Pi returns `{ exitCode: number | null }` (`bash.ts:72`), where `null` is "killed"; cyrup
    /// returns the richer [`ExitStatus`] its callers already interpret, so a cancel
    /// ([`ExitStatus::Killed`]) and a timeout ([`ExitStatus::TimedOut`]) stay distinguishable
    /// instead of collapsing into pi's single `null` and being re-derived from the signal afterwards.
    /// A genuine backend failure (spawn error, missing cwd, …) is an `Err` — pi `throw`s those and
    /// `executeBashWithOperations` re-throws everything that is not an abort (`bash-executor.ts:154`).
    async fn exec(
        &self,
        command: &str,
        cwd: &Path,
        opts: BashExecOptions<'_>,
    ) -> Result<ExitStatus, ToolError>;
}

/// Pi's built-in local-shell [`BashOperations`] — `createLocalBashOperations(options?: {shellPath?})`
/// (`packages/coding-agent/src/core/tools/bash.ts:82-148` @v0.83.0).
///
/// The shell is resolved **per `exec` call**, not baked in at construction: upstream's returned
/// closure calls `getShellConfig(options?.shellPath)` on every invocation (`bash.ts:89`), so a
/// `shellPath` setting changed mid-session takes effect on the next command. Storing a resolved
/// [`ShellConfig`] here instead would silently pin the shell at session-build time, which is the
/// exact divergence `AgentSession::execute_bash` already documents on the immediate-bash path.
///
/// `shell_path` = `None` is upstream's absent `shellPath`, i.e. auto-detection.
pub struct LocalBashOperations {
    proc: Arc<dyn ProcOps>,
    shell_path: Option<String>,
}

impl LocalBashOperations {
    /// Over the default local process backend (pi `createLocalBashOperations({ shellPath })`).
    pub fn new(shell_path: Option<String>) -> Self {
        Self {
            proc: Arc::new(local::LocalProc::new()),
            shell_path,
        }
    }

    /// Over an explicit [`ProcOps`] — the form the isolation decorators (arch-12) need, so a
    /// sandboxed or traversal-guarded backend keeps its wrapping when it is reached through the
    /// per-call bash seam rather than through [`Backend::proc`].
    pub fn with_proc(proc: Arc<dyn ProcOps>, shell_path: Option<String>) -> Self {
        Self { proc, shell_path }
    }
}

#[async_trait::async_trait]
impl BashOperations for LocalBashOperations {
    async fn exec(
        &self,
        command: &str,
        cwd: &Path,
        opts: BashExecOptions<'_>,
    ) -> Result<ExitStatus, ToolError> {
        // Pi resolves the shell INSIDE `exec` (bash.ts:89) — see the struct doc. A missing custom
        // `shellPath` is an `Err` here, matching `ShellConfig::resolve`'s contract and the
        // `Custom shell path not found` error both existing bash front-ends already surface.
        let shell = ShellConfig::resolve(self.shell_path.as_deref())?;
        let BashExecOptions {
            on_data,
            cancel,
            timeout,
            env,
            env_remove,
        } = opts;
        self.proc
            .exec(
                ExecSpec {
                    command: command.to_string(),
                    cwd: cwd.to_path_buf(),
                    env,
                    env_remove,
                    shell,
                },
                cancel,
                timeout,
                on_data,
            )
            .await
    }
}

/// A bundle of both operation surfaces (arch-03 §3.3).
#[derive(Clone)]
pub struct Backend {
    pub fs: Arc<dyn FsOps>,
    pub proc: Arc<dyn ProcOps>,
}

impl Backend {
    /// The default local backend over tokio fs/process. No shell is baked in — every `bash` seam
    /// resolves its own per call (Pi bash.ts:91), and [`local::LocalProc`] resolves the platform
    /// default itself for a spec that carries none.
    pub fn local() -> Self {
        Self {
            fs: Arc::new(local::LocalFs),
            proc: Arc::new(local::LocalProc::new()),
        }
    }
}

impl Default for Backend {
    fn default() -> Self {
        Self::local()
    }
}

#[cfg(test)]
mod bash_operations_tests {
    // `indexing_slicing` joins the two the crate's other test modules already allow: the
    // assertions below index a `Vec` whose length the preceding `assert_eq!` has just pinned, and
    // a panic there IS the test failure. Without it `cargo clippy -p cyrup-tools --all-targets`
    // is RED on this file — the crate lints are `deny`, and clippy lints do not fire under
    // `cargo check`, so this stayed invisible to a check-only gate.
    #![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing)]
    use super::*;
    use std::sync::Mutex;

    /// A [`ProcOps`] that records the [`ExecSpec`] it was handed and replays a canned chunk, so the
    /// adapter's forwarding can be asserted without spawning anything.
    #[derive(Default)]
    struct RecordingProc {
        seen: Mutex<Vec<ExecSpec>>,
        emit: Option<&'static [u8]>,
    }

    #[async_trait::async_trait]
    impl ProcOps for RecordingProc {
        async fn exec(
            &self,
            spec: ExecSpec,
            _cancel: CancelToken,
            _timeout: Option<Duration>,
            on_data: &mut (dyn for<'a> FnMut(&'a [u8]) + Send),
        ) -> Result<ExitStatus, ToolError> {
            #[allow(clippy::unwrap_used)]
            self.seen.lock().unwrap().push(spec);
            if let Some(bytes) = self.emit {
                on_data(bytes);
            }
            Ok(ExitStatus::Exited(0))
        }
    }

    fn opts<'a>(on_data: &'a mut (dyn for<'b> FnMut(&'b [u8]) + Send)) -> BashExecOptions<'a> {
        BashExecOptions {
            on_data,
            cancel: CancelToken::new(),
            timeout: None,
            env: Vec::new(),
            env_remove: Vec::new(),
        }
    }

    /// `LocalBashOperations` forwards pi's `(command, cwd, {onData, timeout, env})` onto the
    /// [`ProcOps`] seam verbatim, streaming the backend's raw bytes straight through — pi's `onData`
    /// receives the RAW `Buffer` and the CALLER sanitizes (`bash-executor.ts:78-102` @v0.83.0), so a
    /// backend that filtered here would double-sanitize.
    #[tokio::test]
    async fn local_bash_operations_forwards_command_cwd_and_env_onto_the_proc_seam() {
        let proc = Arc::new(RecordingProc {
            seen: Mutex::new(Vec::new()),
            emit: Some(b"hi\x1b[0m"),
        });
        let ops = LocalBashOperations::with_proc(proc.clone(), None);

        let mut streamed: Vec<u8> = Vec::new();
        let mut sink = |b: &[u8]| streamed.extend_from_slice(b);
        let status = ops
            .exec(
                "echo hi",
                Path::new("/tmp"),
                BashExecOptions {
                    on_data: &mut sink,
                    cancel: CancelToken::new(),
                    timeout: Some(Duration::from_secs(5)),
                    env: vec![("A".into(), "1".into())],
                    env_remove: vec!["PI_SESSION_ID".into()],
                },
            )
            .await
            .expect("recording backend cannot fail");

        assert_eq!(status, ExitStatus::Exited(0));
        // Raw bytes, ANSI intact — the sanitization is the caller's (pi `bash-executor.ts:84`).
        assert_eq!(streamed, b"hi\x1b[0m");

        let seen = proc.seen.lock().unwrap();
        assert_eq!(
            seen.len(),
            1,
            "exactly one exec per `BashOperations::exec` call"
        );
        assert_eq!(seen[0].command, "echo hi");
        assert_eq!(seen[0].cwd, Path::new("/tmp"));
        assert_eq!(seen[0].env, vec![("A".to_string(), "1".to_string())]);
        assert_eq!(seen[0].env_remove, vec!["PI_SESSION_ID".to_string()]);
        assert!(
            !seen[0].shell.program.as_os_str().is_empty(),
            "the adapter must resolve a shell itself, not leave it to `LocalProc`'s baked default"
        );
    }

    /// Pi resolves the shell INSIDE the `exec` closure (`bash.ts:89`: `getShellConfig(options
    /// ?.shellPath)`), not when `createLocalBashOperations` returns. The observable consequence is
    /// that a bad `shellPath` fails on the CALL and never reaches the process backend — a
    /// construction-time resolve would either have to make the constructor fallible or would pin a
    /// stale shell for the session's life.
    ///
    /// Presence before absence: the same constructor with a REAL shell path runs and reaches the
    /// backend, so the `Err` below is the path check firing and not the adapter being inert.
    #[tokio::test]
    async fn local_bash_operations_resolves_the_shell_per_call_so_a_bad_path_fails_before_spawn() {
        let proc = Arc::new(RecordingProc::default());

        // Presence: a resolvable shell path reaches the backend.
        let good = ShellConfig::try_detect()
            .expect("unix detection cannot fail (shell.ts:119)")
            .program;
        let ok_ops =
            LocalBashOperations::with_proc(proc.clone(), Some(good.to_string_lossy().into_owned()));
        let mut noop = |_: &[u8]| {};
        ok_ops
            .exec("true", Path::new("/tmp"), opts(&mut noop))
            .await
            .expect("a resolvable shellPath must execute");
        assert_eq!(proc.seen.lock().unwrap().len(), 1);
        assert_eq!(proc.seen.lock().unwrap()[0].shell.program, good);

        // Absence: constructing with a missing path is fine — only `exec` fails.
        let bad_ops = LocalBashOperations::with_proc(
            proc.clone(),
            Some("/definitely/not/a/shell/on/this/box".to_string()),
        );
        let mut noop2 = |_: &[u8]| {};
        let err = bad_ops
            .exec("true", Path::new("/tmp"), opts(&mut noop2))
            .await
            .expect_err("a missing custom shellPath must fail the call");
        assert!(
            err.to_string().contains("Custom shell path not found"),
            "pi's message (`shell.ts:67-120` via `ShellConfig::resolve`), got: {err}"
        );
        assert_eq!(
            proc.seen.lock().unwrap().len(),
            1,
            "the failing call must NOT have reached the process backend"
        );
    }

    /// `LocalBashOperations` is usable as `Arc<dyn BashOperations>` — the object-safety the per-call
    /// override seam exists for (pi's `operations?: BashOperations` is an interface value, both on
    /// `BashToolOptions` (`bash.ts:186-188`) and on `UserBashEventResult` (`types.ts:1078-1081`)).
    #[test]
    fn bash_operations_is_object_safe() {
        let _erased: Arc<dyn BashOperations> = Arc::new(LocalBashOperations::new(None));
    }
}
