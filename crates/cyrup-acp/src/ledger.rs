//! The tool-call ledger and the terminal appender — the state the translator mutates.
//!
//! **Owner: agent B (`ACP-122`…`ACP-141`, `ACP-151`, `ACP-156`, `ACP-157`).**
//!
//! ADR-0028 finding F2, state half. Port of pi-acp v0.0.33 `src/acp/session.ts`'s **five parallel
//! collections** keyed by the same tool-call id — `currentToolCalls`, `fileSnapshots`,
//! `fileMutationToolCallIds`, `bashToolCallIds`, `bashOutputSnapshots` — whose *contents* imply
//! which of three kinds of tool call an id is. Those five are one sum type written as a product;
//! collapsing them makes the two invariants upstream enforces by hand ("the first emission for an
//! id is `tool_call`, never `tool_call_update`" and "status never regresses to `pending`") true by
//! construction rather than by the `existingStatus ? … : …` ternaries repeated at four sites.
//!
//! Nothing in this module performs I/O, touches `ConnectionTo`, or knows about tokio. The pre-
//! mutation file read is the shell's, supplied as a [`FileSnapshot`].
//!
//! # The emitters, and why they live here rather than in [`mod@crate::translate`]
//!
//! ADR-0028 F2's guarantee is that "a `tool_call_update` for an id that was never announced cannot
//! be constructed", and a guarantee that holds only inside `translate.rs` is a convention, not a
//! guarantee. So every `SessionUpdate` that names a tool-call id is built **here**, by exactly five
//! functions, each of which takes `&mut self` and consults the map first:
//!
//! * [`ToolCallLedger::announce`] — the only entry constructor, and the only producer of the
//!   `terminal_info` `_meta` and the `content[0].terminalId` (`ACP-139`).
//! * [`ToolCallLedger::update`] — `None` for an unannounced id, and its status argument is a
//!   [`ToolStatus`], which has no terminal value, so a "completed" update cannot be forged.
//! * [`ToolCallLedger::finish`] — the only producer of a terminal status, and it **closes** the
//!   entry, so nothing can follow it (`ACP-137`).
//! * [`ToolCallLedger::terminal_progress`] / [`ToolCallLedger::terminal_finish`] — `None` unless
//!   the entry's body is a terminal, so a `terminal_output` naming a terminal the client was never
//!   told about is unrepresentable (`ACP-139`).
//!
//! # [CYRUP-DELTA] — `ToolCallStatus` is not a field of [`ToolStatus`]
//!
//! **What differs.** Upstream's map is `Map<string, 'pending'|'in_progress'>` and the terminal
//! status is computed inline at the `tool_execution_end` site (`isError ? 'failed' : 'completed'`).
//! [`ToolStatus`] keeps upstream's two values, and the terminal pair reaches the wire only through
//! [`ToolCallLedger::finish`], which removes the entry in the same statement.
//!
//! **What it costs.** A caller that wants "mark completed but keep the row open" has no way to say
//! it. Nothing in the port wants that, and upstream's `cleanupToolCall` runs on exactly the same
//! two paths, so the restriction is free here and would have to be relaxed deliberately.

use std::collections::HashMap;
use std::path::{Path, PathBuf};

use agent_client_protocol::schema::v1::{
    Diff, Meta, SessionUpdate, Terminal, TerminalId, ToolCall, ToolCallContent, ToolCallId,
    ToolCallLocation, ToolCallStatus, ToolCallUpdate, ToolCallUpdateFields, ToolKind,
};
use serde_json::{Map, Value};

use crate::ids::AbsCwd;

/// How a tool name maps onto ACP rendering. **One classifier; three consumers** (the announce
/// shape, the ledger variant, and the ACP `ToolKind`).
///
/// Replaces pi-acp v0.0.33 `translate/bash.ts`'s `isBashTool`, `session.ts`'s `toToolKind`, and the
/// inline `toolName === "edit" || toolName === "write"` test in the `tool_execution_start` arm —
/// three independent string comparisons that can disagree, and upstream's do.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ToolClass {
    /// Renders as an ACP terminal: `terminal_info` / `terminal_output` / `terminal_exit`.
    Terminal,
    /// Mutates a file, so a pre-mutation snapshot is taken and a `Diff` may be emitted.
    Mutation,
    /// Reads a file.
    Read,
    /// Searches.
    Search,
    /// Everything else.
    Other,
}

impl ToolClass {
    /// Classify a tool name.
    ///
    /// Port of pi-acp v0.0.33 `translate/bash.ts`'s `isBashTool`, `session.ts`'s `toToolKind` and
    /// the inline `toolName === "edit" || toolName === "write"` test, unified (`ACP-151`,
    /// `ACP-157`, ADR-0028 F2).
    ///
    /// The names are cyrup's eight built-ins — `cyrup_tools::BUILTIN_NAMES` (`read`, `bash`,
    /// `powershell`, `edit`, `write`, `grep`, `find`, `ls`), declared in
    /// `crates/cyrup-tools/src/registry.rs`. They are **written out here rather than imported**
    /// because `cyrup-acp` does not depend on `cyrup-tools` and does not need to: this is a
    /// presentation mapping, not a registry. The report for this module files the one-line
    /// re-export that would let a test cross-check the two lists.
    ///
    /// # [CYRUP-DELTA] — three rows differ from a faithful port
    ///
    /// **What differs.**
    ///
    /// * **`powershell` is a terminal.** cyrup ships a second shell tool
    ///   (`crates/cyrup-tools/src/tools/powershell.rs`, `POWERSHELL_CONFIG`, `name: "powershell"`)
    ///   built by `ShellTool::powershell` from the **same engine** as `bash`, so its result shape,
    ///   its `BashDetails`, its `build_stream_update` truncation and its
    ///   `Command exited with code {n}` text are identical. Upstream's `isBashTool` is
    ///   `toolName.toLowerCase() === "bash"`, so a faithful port classifies it [`ToolClass::Other`]
    ///   — no terminal, no incremental output, no exit code, and the bug is invisible on a
    ///   developer's macOS or Linux machine. `ACP-157`.
    /// * **`grep` / `find` / `ls` are [`ToolClass::Search`]**, and `read` is [`ToolClass::Read`].
    ///   Upstream's `toToolKind` has three cases and collapses everything else to `other`, while
    ///   ACP has `ToolKind::{Search, Read}`. `ACP-151`.
    /// * **The match is ASCII-case-insensitive**, which is what removes upstream's *internal*
    ///   disagreement rather than a difference from it: `isBashTool` lowercases, so upstream calls
    ///   a tool named `Bash` a terminal, while `toToolKind` is case-**sensitive**, so the same call
    ///   is given `kind: "other"`. One classifier cannot disagree with itself. `ACP-138`'s verify
    ///   ("tool name `Bash` recognised") is the assertion.
    ///
    /// **What it costs.** A tool an MCP server names `Edit` now renders as `ToolKind::Edit` and
    /// takes the file-mutation snapshot path, where upstream would have left it `Other`. That is
    /// the right answer for cyrup's own tools and a guess for a foreign one; the guess is
    /// *cosmetic plus one wasted read*, never a wrong action, because the class only chooses how
    /// the call is rendered and whether a snapshot is taken.
    ///
    /// Returning [`ToolClass::Other`] for an unknown name is **correct**, not a stub: MCP servers
    /// and extensions choose tool names freely, so the classifier must have a total default. It is
    /// a runtime default, not a proof — which is ADR-0028 F2's "guarantee not gained".
    #[must_use]
    pub fn of(tool_name: &str) -> Self {
        const TERMINAL: [&str; 2] = ["bash", "powershell"];
        const MUTATION: [&str; 2] = ["edit", "write"];
        const READ: [&str; 1] = ["read"];
        const SEARCH: [&str; 3] = ["grep", "find", "ls"];

        let is = |set: &[&str]| set.iter().any(|n| n.eq_ignore_ascii_case(tool_name));

        if is(&TERMINAL) {
            ToolClass::Terminal
        } else if is(&MUTATION) {
            ToolClass::Mutation
        } else if is(&READ) {
            ToolClass::Read
        } else if is(&SEARCH) {
            ToolClass::Search
        } else {
            ToolClass::Other
        }
    }

    /// The ACP kind this class renders as.
    #[must_use]
    pub fn acp_kind(self) -> ToolKind {
        match self {
            ToolClass::Terminal => ToolKind::Execute,
            ToolClass::Mutation => ToolKind::Edit,
            ToolClass::Read => ToolKind::Read,
            ToolClass::Search => ToolKind::Search,
            ToolClass::Other => ToolKind::Other,
        }
    }

    /// What the shell must read before the core can decide. Pure.
    #[must_use]
    pub fn needs_snapshot(self) -> bool {
        matches!(self, ToolClass::Mutation)
    }

    /// Whether this class renders as an ACP terminal — upstream's `isBashTool`, now covering
    /// `powershell` (`ACP-157`).
    #[must_use]
    pub fn is_terminal(self) -> bool {
        matches!(self, ToolClass::Terminal)
    }
}

/// Pre-mutation file contents, captured by the shell.
///
/// `ACP-131` distinguishes three states and they are **not** the same: a file that existed and was
/// read ([`FileSnapshot::read`]), a file that did not exist ([`FileSnapshot::absent`], so the
/// diff's `old_text` is `null` — the write-to-new-file case), and a file that existed but could not
/// be read ([`FileSnapshot::unreadable`]). The third is why this is a struct with two fields rather
/// than an `Option<String>`: `ACP-135` says an edit whose pre-read **failed** must emit **no diff**,
/// which is a delta from upstream (it treats a failed read as "this is a new file" and emits a diff
/// whose `oldText` is `null`) and must be asserted explicitly.
///
/// # `ACP-156` — this type is also the **post**-mutation read
///
/// [`crate::translate::translate`] takes one `Option<FileSnapshot>` and the event says which read
/// it is: at `ToolExecutionStart` it is the pre-image, at `ToolExecutionEnd` the re-read. `before`
/// is therefore "the contents this read returned", named for its dominant use. The two are never
/// confused because the ledger stores only the first and the second is consumed in the same call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FileSnapshot {
    /// The path as the tool named it, before cwd resolution.
    pub path: PathBuf,
    /// The contents the read returned, or `None` when the file did not exist.
    pub before: Option<String>,
    /// Whether the read was attempted and failed, as distinct from the file being absent.
    pub unreadable: bool,
}

impl FileSnapshot {
    /// The file existed and was read.
    #[must_use]
    pub fn read(path: impl Into<PathBuf>, text: impl Into<String>) -> Self {
        Self {
            path: path.into(),
            before: Some(text.into()),
            unreadable: false,
        }
    }

    /// The file did not exist. The write-to-new-file case: a diff with `old_text: None`.
    #[must_use]
    pub fn absent(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            before: None,
            unreadable: false,
        }
    }

    /// The read was attempted and failed — `EACCES`, non-UTF-8, a confinement refusal from
    /// `TraversalFs::read` (`crates/cyrup-tools/src/isolation/traversal.rs`). **No diff may be
    /// built from this** (`ACP-135`).
    #[must_use]
    pub fn unreadable(path: impl Into<PathBuf>) -> Self {
        Self {
            path: path.into(),
            before: None,
            unreadable: true,
        }
    }

    /// Whether a diff may be built against this snapshot at all.
    #[must_use]
    pub fn is_diffable(&self) -> bool {
        !self.unreadable
    }
}

/// Tool-call status. **No transition back to `Pending` exists** — this is pi-acp's own "never
/// downgrade status" comment, made structural. Regressing to `pending` makes clients hide progress.
///
/// `Ord` is what [`ToolCallLedger::update`] takes the maximum over, so a downgrade is not
/// expressible at a call site rather than merely discouraged (`ACP-129`).
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum ToolStatus {
    /// Announced, not yet started.
    Pending,
    /// Running. The only transition, and it is one-way.
    InProgress,
}

impl ToolStatus {
    /// Advance. Idempotent, and there is no inverse.
    pub fn advance(&mut self) {
        *self = ToolStatus::InProgress;
    }

    /// The wire value. There is deliberately no inverse: `completed`/`failed` reach the wire only
    /// through [`ToolCallLedger::finish`].
    #[must_use]
    pub fn acp_status(self) -> ToolCallStatus {
        match self {
            ToolStatus::Pending => ToolCallStatus::Pending,
            ToolStatus::InProgress => ToolCallStatus::InProgress,
        }
    }
}

/// What a live tool call is carrying. Private variants by design: the map holds one concrete type,
/// so the kind is a runtime tag (ADR-0028 §5 rejects typestate here for exactly that reason), but
/// nothing outside this module can construct one.
enum StreamBody {
    /// Was `bashToolCallIds` + `bashOutputSnapshots`.
    Terminal(TerminalAppender),
    /// Was `fileMutationToolCallIds` + `fileSnapshots`. The `Option` is "the shell has not handed
    /// the pre-read over yet": a mutation announced from a streaming delta (`ACP-128`) has no
    /// snapshot until `ToolExecutionStart` arrives with one.
    Mutation(Option<FileSnapshot>),
    /// Was: nothing — the absence of an id from all four other collections.
    Plain,
}

/// One live tool call. All fields private.
pub struct ToolCallStream {
    status: ToolStatus,
    class: ToolClass,
    body: StreamBody,
}

impl ToolCallStream {
    /// The monotonic status.
    #[must_use]
    pub fn status(&self) -> ToolStatus {
        self.status
    }

    /// How this call renders.
    #[must_use]
    pub fn class(&self) -> ToolClass {
        self.class
    }

    /// The terminal appender, for a [`ToolClass::Terminal`] call.
    pub fn terminal_mut(&mut self) -> Option<&mut TerminalAppender> {
        match &mut self.body {
            StreamBody::Terminal(appender) => Some(appender),
            _ => None,
        }
    }

    /// The pre-mutation snapshot, for a [`ToolClass::Mutation`] call whose shell read has landed.
    #[must_use]
    pub fn snapshot(&self) -> Option<&FileSnapshot> {
        match &self.body {
            StreamBody::Mutation(snapshot) => snapshot.as_ref(),
            _ => None,
        }
    }
}

/// What [`TerminalAppender::push`] decided about a new output snapshot.
///
/// `ACP-140`. Upstream is `previous = snapshots[id] ?? ''; delta = next.startsWith(previous) ?
/// next.slice(previous.length) : next` — and that `: next` fallback is the whole problem.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Push {
    /// The snapshot did not grow. Emit no `terminal_output` at all — upstream also suppresses an
    /// empty delta.
    Nothing,
    /// The snapshot is a strict extension of what was already sent; this is the suffix.
    Append(String),
    /// The snapshot is a **slid window**: its head was dropped by the producer's tail truncation,
    /// but a suffix of what was already sent is still a prefix of it. This is the part past that
    /// overlap — the bytes that are genuinely new — and it is emitted exactly like
    /// [`Push::Append`].
    ///
    /// This is the case the real producer reaches on *every* update once a command's output passes
    /// `truncate_tail`'s limit (`build_stream_update` takes `acc.tail_string()` then
    /// `truncate_tail(…, TruncOpts::new(max_lines, max_bytes))`,
    /// `crates/cyrup-tools/src/tools/bash.rs`), so it is the common case for `cargo build`,
    /// `npm ci` or a wide `grep` — not an exotic one. Distinguishing it from [`Push::Append`]
    /// costs nothing on the wire and is what lets a test tell "the window slid and we recovered"
    /// apart from "the snapshot simply grew".
    Resync(String),
    /// The snapshot shares **no** usable overlap with what was already sent: the producer skipped
    /// more bytes between two flushes than one whole preview window holds, so there is no way to
    /// tell how much output is missing.
    ///
    /// Emitting nothing is the `ACP-Q26` decision. What is NOT true — and what
    /// [`ToolCallLedger::terminal_finish`] now repairs — is that "the same bytes reach the client
    /// as ordinary tool-call content": a [`ToolClass::Terminal`] call sends no `content` and no
    /// `rawOutput` at all, so a desync on the *final* push used to drop the tail of the command
    /// through every field at once.
    Desynced,
}

/// The shortest overlap [`TerminalAppender::push`] will accept as evidence that the producer's
/// window merely slid, rather than that output was skipped outright. `ACP-140`.
///
/// A real slide overlaps by `window_len - bytes_appended_since_the_last_flush`, and the real
/// producer's window is 2 000 lines / 50 KiB, so a genuine overlap is kilobytes. A *coincidental*
/// agreement between the tail of one chunk of output and the head of an unrelated one is a handful
/// of bytes — overwhelmingly the one-byte `"\n"` case, since previews end with a newline and a
/// window that starts on a blank line starts with one. Accepting that one byte would emit the
/// whole next window minus it, which is upstream's duplicate-the-tail bug with an off-by-one.
/// 32 bytes sits far below any real overlap and far above any plausible coincidence.
pub const MIN_RESYNC_OVERLAP: usize = 32;

/// The length of the longest suffix of `emitted` that is also a prefix of `next`. `ACP-140`.
///
/// KMP rather than the obvious `for k in (1..=m).rev() { emitted.ends_with(&next[..k]) }`: `next`
/// is a whole preview window (up to 50 KiB) and this runs once per output flush of every bash
/// command, so the quadratic form is ~10⁹ byte comparisons on the hot path. This is `O(|next|)`
/// for the failure table plus `O(|next|)` for the scan — `emitted` is truncated to its last
/// `|next|` bytes first, because a longer overlap than that cannot exist.
///
/// Byte-indexed, but the returned length is always a `char` boundary of **both** strings: the
/// matched region starts at `next[0]`, which is never a UTF-8 continuation byte, so the two
/// framings of the same bytes coincide.
fn prefix_overlap(emitted: &str, next: &str) -> usize {
    let pat = next.as_bytes();
    let m = pat.len();
    if m == 0 || emitted.is_empty() {
        return 0;
    }
    let all = emitted.as_bytes();
    let hay = all.get(all.len().saturating_sub(m)..).unwrap_or(all);

    // `fail[i]` = the longest proper prefix of `pat[..=i]` that is also its suffix.
    let mut fail = vec![0usize; m];
    let mut len = 0usize;
    for i in 1..m {
        let Some(&c) = pat.get(i) else { break };
        while len > 0 && pat.get(len) != Some(&c) {
            len = fail.get(len - 1).copied().unwrap_or(0);
        }
        if pat.get(len) == Some(&c) {
            len += 1;
        }
        if let Some(slot) = fail.get_mut(i) {
            *slot = len;
        }
    }

    // Scan the tail of `emitted`; `len` ends as the match length still live at its last byte,
    // which is exactly "longest prefix of `next` that is a suffix of `emitted`".
    let mut len = 0usize;
    for &c in hay {
        while len > 0 && pat.get(len) != Some(&c) {
            len = fail.get(len - 1).copied().unwrap_or(0);
        }
        if pat.get(len) == Some(&c) {
            len += 1;
        }
    }
    len
}

/// The append-only terminal output delta. `ACP-140`.
///
/// Port of pi-acp v0.0.33 `session.ts`'s `bashOutputSnapshots` map plus `translate/bash.ts`'s
/// `bashOutputDelta`, with the `: next` fallback replaced by the overlap recovery documented on
/// [`TerminalAppender::push`].
#[derive(Debug, Default)]
pub struct TerminalAppender {
    /// The last snapshot offered, whose END is the end of everything sent — which is the only
    /// property [`TerminalAppender::push`] reads it for. It is NOT the whole transcript: the
    /// producer's window slides, so this holds one window, not the command's entire output.
    emitted: String,
}

impl TerminalAppender {
    /// The last snapshot offered — the tail of what has been sent, not the whole of it. See the
    /// field's own note.
    #[must_use]
    pub fn emitted(&self) -> &str {
        &self.emitted
    }

    /// Offer a new output snapshot and learn what to emit.
    ///
    /// # The producer is a sliding window, so "prefix extension" is the exception
    ///
    /// The snapshot this is fed is **not** a growing string. `build_stream_update` takes
    /// `acc.tail_string()` and then `truncate_tail(…, TruncOpts::new(2000, 50 KiB))`
    /// (`crates/cyrup-tools/src/tools/bash.rs`, `crates/cyrup-tools/src/truncate.rs`), which
    /// selects whole lines working *backwards from the end*. So for any command exceeding 2 000
    /// lines or 50 KiB — `cargo build`, `npm ci`, a wide `grep`, on a first use — every subsequent
    /// preview begins LATER than the last and `strip_prefix` fails on all of them. Treating that
    /// as an unrecoverable desync froze the terminal pane at the first ~50 KiB for the rest of the
    /// command, permanently, with only a `tracing::debug!` anywhere: not "exactly one gap", which
    /// is what an earlier version of this doc and its test claimed on the strength of a synthetic
    /// sequence whose window stops sliding.
    ///
    /// So the slide is recovered instead: [`prefix_overlap`] finds the longest suffix of what has
    /// already been sent that is still a prefix of the new window, and the bytes past it are a
    /// [`Push::Resync`] — the same emission as [`Push::Append`], with no duplication and no gap.
    /// The overlap must be at least [`MIN_RESYNC_OVERLAP`] to count; below that it is indexing
    /// noise rather than evidence of continuity.
    ///
    /// # `ACP-Q26` / the desync policy, still decided: emit nothing and record the gap
    ///
    /// A [`Push::Desynced`] now means the producer skipped more than a whole window between two
    /// flushes, so the amount of missing output is unknown. The three candidates were re-appending
    /// the whole preview (upstream's `: next`, and the one that is actively wrong — it duplicates
    /// output in a terminal that appends), emitting a visible marker (a cyrup-invented string in a
    /// pane the user reads as their own shell's output), and emitting nothing. Nothing still wins,
    /// but its old justification — "the same bytes still reach the client as ordinary tool-call
    /// content" — was false for a terminal, which sends no `content` and no `rawOutput`. That half
    /// is repaired at [`ToolCallLedger::terminal_finish`], which falls back to a text content block
    /// when the FINAL push desyncs, so the tail of a command is never lost through every field at
    /// once. [`Push::Desynced`] stays a named outcome the caller must match on rather than an
    /// `Option::None` it can ignore, and the caller still `tracing::debug!`s it.
    ///
    /// # A desync **re-bases**, it does not latch
    ///
    /// `emitted` is replaced on every path, exactly as upstream stores `next` unconditionally.
    /// Not re-basing would compare every later snapshot against a prefix the tool has permanently
    /// dropped. Since `emitted` is only ever read as *the tail of what was sent* — both
    /// `strip_prefix` and [`prefix_overlap`] anchor at its end — holding the window rather than
    /// the whole transcript is also what keeps this `O(|window|)` per flush instead of growing
    /// with the command's total output.
    ///
    /// The clean fix is `ACP-Q26`'s other half — stream from the tool's `OutputAccumulator`
    /// (`crates/cyrup-tools/src/output.rs`), which holds the untruncated tail, making the prefix
    /// invariant true by construction and deleting [`Push::Resync`] and [`Push::Desynced`] both.
    /// That is a new seam between `cyrup-tools` and `cyrup-acp` and is out of scope for this port;
    /// it is recorded here so it is a known option, not a discovery.
    ///
    /// Asserted by `a_slid_window_resyncs_on_the_overlap`,
    /// `a_genuine_discontinuity_is_still_a_silent_gap` and
    /// `a_sliding_window_never_repeats_or_drops_a_line`.
    pub fn push(&mut self, next: &str) -> Push {
        if next == self.emitted {
            return Push::Nothing;
        }
        if let Some(suffix) = next.strip_prefix(self.emitted.as_str()) {
            let suffix = suffix.to_string();
            self.emitted = next.to_string();
            return Push::Append(suffix);
        }
        let overlap = prefix_overlap(&self.emitted, next);
        let resync = (overlap >= MIN_RESYNC_OVERLAP)
            .then(|| next.get(overlap..))
            .flatten()
            .map(str::to_string);
        self.emitted = next.to_string();
        match resync {
            // The whole new window was already sent: a slide of zero new bytes.
            Some(data) if data.is_empty() => Push::Nothing,
            Some(data) => Push::Resync(data),
            None => Push::Desynced,
        }
    }
}

/// Everything [`ToolCallLedger::announce`] needs, as one value.
///
/// A struct rather than seven positional arguments: the announce is built at three call sites in
/// [`mod@crate::translate`] (the streaming-delta surfacing of `ACP-128`, the bash arm of `ACP-139`, and
/// the generic `tool_execution_start` arm of `ACP-131`) and a positional list of two ids, a string,
/// a status and two collections is exactly the shape that gets transposed.
pub struct Announce {
    /// The ACP tool-call id. It is also the terminal id for a [`ToolClass::Terminal`] call — see
    /// [`ToolCallLedger::announce`].
    pub id: ToolCallId,
    /// The classification, which decides the ledger body, the ACP kind and the terminal `_meta`.
    pub class: ToolClass,
    /// `title` on the wire. For a terminal this is the command; otherwise the tool name.
    pub title: String,
    /// The status to announce at. `ACP-128` announces [`ToolStatus::Pending`] from a streaming
    /// delta; `ACP-131` announces [`ToolStatus::InProgress`] when no delta preceded it.
    pub status: ToolStatus,
    /// Resolved absolute locations (`ACP-130`). Empty means "send no `locations` key".
    pub locations: Vec<ToolCallLocation>,
    /// `rawInput`. `None` means "send no `rawInput` key".
    pub raw_input: Option<Value>,
    /// The shell's pre-mutation read, for a [`ToolClass::Mutation`] call (`ACP-131`).
    pub snapshot: Option<FileSnapshot>,
}

/// The changed fields of a `tool_call_update`. **No status field**: the status is a separate,
/// typed argument to [`ToolCallLedger::update`] and [`ToolCallLedger::finish`], which is what makes
/// a downgrade unexpressible (`ACP-129`).
#[derive(Default)]
pub struct UpdatePatch {
    /// A new title — used by `ACP-138`'s bash arm, where the command only becomes known once the
    /// model has finished streaming the arguments.
    pub title: Option<String>,
    /// Replace the content collection. `None` sends no `content` key.
    pub content: Option<Vec<ToolCallContent>>,
    /// Replace the locations collection. `None` sends no `locations` key.
    pub locations: Option<Vec<ToolCallLocation>>,
    /// `rawInput`.
    pub raw_input: Option<Value>,
    /// `rawOutput`. `ACP-135`: omitted whenever a structured diff is present.
    pub raw_output: Option<Value>,
}

/// Live tool calls. **`announce` is the only entry constructor**, so an update for an unannounced
/// id cannot be produced inside this module.
///
/// `clear` is bounded teardown: the shell calls it on `AgentSettled`. Upstream never does, so a
/// tool call whose `tool_execution_end` never arrives leaks an entry for the life of the session
/// (`ACP-137`).
///
/// # Why the cwd is a field and not a parameter
///
/// `ACP-130` requires every emitted `ToolCallLocation.path` and every `Diff.path` to be absolute,
/// resolved against the session cwd. Holding an [`AbsCwd`] here — the only constructor takes one —
/// means a ledger that could emit a relative path does not exist, rather than every emit site
/// having to remember to resolve. It is also the right home: a ledger is per session, and so is the
/// cwd.
pub struct ToolCallLedger {
    cwd: AbsCwd,
    open: HashMap<ToolCallId, ToolCallStream>,
}

impl ToolCallLedger {
    /// An empty ledger for one session.
    #[must_use]
    pub fn new(cwd: AbsCwd) -> Self {
        Self {
            cwd,
            open: HashMap::new(),
        }
    }

    /// The session cwd every path is resolved against.
    #[must_use]
    pub fn cwd(&self) -> &AbsCwd {
        &self.cwd
    }

    /// Resolve a tool-supplied path against the session cwd — pi-acp `session.ts`'s
    /// `isAbsolute(path) ? path : resolvePath(this.cwd, path)` (`ACP-130`).
    #[must_use]
    pub fn resolve(&self, path: &Path) -> PathBuf {
        self.cwd.resolve(path)
    }

    /// The only way an id enters the ledger. Returns the `tool_call` announce.
    ///
    /// Port of pi-acp v0.0.33 `session.ts`'s `emitBashToolCall` (the `includeTerminal: true` case)
    /// and the two `sessionUpdate: 'tool_call'` emits in `handlePiEvent`'s `message_update` and
    /// `tool_execution_start` arms (`ACP-128`, `ACP-131`, `ACP-139`).
    ///
    /// The `terminal_info` `_meta` and the `content[0].terminalId` are emitted **by this function
    /// and nothing else**, which is upstream's `includeTerminal: !existingStatus` made structural:
    /// a second terminal for one tool call cannot occur.
    ///
    /// Note the deliberate id reuse: **the terminal id IS the tool call id** — one string in two
    /// protocol namespaces (`translate/bash.ts`'s `bashTerminalContent(toolCallId)`). ADR-0028 §5
    /// says to carry that as a comment rather than a conversion barrier, and this is the comment.
    ///
    /// # [CYRUP-DELTA] — announcing the same id twice replaces rather than duplicates
    ///
    /// **What differs.** Nothing at the wire, because [`mod@crate::translate`] asks
    /// [`ToolCallLedger::contains`] first and never announces a known id. If a future caller did,
    /// the map entry is replaced and a second `tool_call` goes out — the "second `tool_call` for a
    /// known id" that `ACP-129`'s verify forbids. **What it costs.** The guarantee is the caller's,
    /// not the type's; `a_known_id_is_never_re_announced_by_the_translator` is where it is asserted.
    pub fn announce(&mut self, req: Announce) -> SessionUpdate {
        let Announce {
            id,
            class,
            title,
            status,
            locations,
            raw_input,
            snapshot,
        } = req;

        let body = match class {
            ToolClass::Terminal => StreamBody::Terminal(TerminalAppender::default()),
            ToolClass::Mutation => StreamBody::Mutation(snapshot),
            _ => StreamBody::Plain,
        };
        self.open.insert(
            id.clone(),
            ToolCallStream {
                status,
                class,
                body,
            },
        );

        let mut call = ToolCall::new(id.clone(), title)
            .kind(class.acp_kind())
            .status(status.acp_status())
            .locations(locations);
        if let Some(raw_input) = raw_input {
            call = call.raw_input(raw_input);
        }
        if class.is_terminal() {
            call = call
                .content(vec![ToolCallContent::Terminal(Terminal::new(
                    TerminalId::new(id.0.clone()),
                ))])
                .meta(terminal_info_meta(&id, self.cwd.as_path()));
        }
        SessionUpdate::ToolCall(call)
    }

    /// A `tool_call_update` for an announced id, at **at least** `at_least`.
    ///
    /// Port of pi-acp v0.0.33 `session.ts`'s three `sessionUpdate: 'tool_call_update'` emits in the
    /// `message_update`, `tool_execution_start` and `tool_execution_update` arms, plus the
    /// `const status = existingStatus ?? 'pending'` line and its "IMPORTANT: never downgrade
    /// status" comment (`ACP-129`, `ACP-134`).
    ///
    /// The emitted status is `max(stored, at_least)`, so a late `ToolCallDelta` arriving after a
    /// `ToolExecutionStart` re-sends `in_progress` rather than dragging the client's row back to
    /// `pending` and hiding its progress UI. **No call site can express a downgrade**: the argument
    /// is a [`ToolStatus`], the maximum is taken here, and there is no setter.
    ///
    /// Returns `None` when `id` was never announced — the "client never saw this tool call" case,
    /// which upstream emits anyway and Zed silently drops.
    pub fn update(
        &mut self,
        id: &ToolCallId,
        at_least: ToolStatus,
        patch: UpdatePatch,
    ) -> Option<SessionUpdate> {
        let stream = self.open.get_mut(id)?;
        stream.status = stream.status.max(at_least);
        Some(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            id.clone(),
            patch.into_fields(stream.status.acp_status()),
        )))
    }

    /// The terminal `tool_call_update` for an announced id, and the **only** producer of
    /// `completed`/`failed`. Closes the entry (`ACP-137`'s `cleanupToolCall`).
    ///
    /// Port of pi-acp v0.0.33 `session.ts`'s `tool_execution_end` arm's emit plus its
    /// `cleanupToolCall(toolCallId)`, which deletes from all five collections (`ACP-135`).
    ///
    /// Returns `None` for an unannounced id, and because the entry is gone afterwards a second
    /// `finish` — or any `update` — for the same id also returns `None`. That is what makes "a tool
    /// call cannot be resurrected after it completed" a property of the type.
    pub fn finish(
        &mut self,
        id: &ToolCallId,
        is_error: bool,
        patch: UpdatePatch,
    ) -> Option<SessionUpdate> {
        self.open.remove(id)?;
        let status = if is_error {
            ToolCallStatus::Failed
        } else {
            ToolCallStatus::Completed
        };
        Some(SessionUpdate::ToolCallUpdate(ToolCallUpdate::new(
            id.clone(),
            patch.into_fields(status),
        )))
    }

    /// A running terminal's output delta. `None` unless `id` is an announced [`ToolClass::Terminal`]
    /// call, so a `terminal_output` naming a terminal the client was never told about is
    /// unrepresentable (`ACP-139`).
    ///
    /// Port of pi-acp v0.0.33 `session.ts`'s `emitBashOutputUpdate` with
    /// `status: 'in_progress'` (`ACP-140`). The update carries **no** `content` and **no**
    /// `rawOutput`: for a terminal, everything rides `_meta`.
    ///
    /// # [CYRUP-DELTA] — an update with nothing in it is not sent
    ///
    /// **What differs.** Upstream emits on every `bash_execution_update` regardless. A snapshot
    /// that repeats what was already appended produces `Push::Nothing`, and a call that is already
    /// `in_progress` produces no status transition either, so the frame that reaches the wire is
    /// `{sessionUpdate, toolCallId, status: "in_progress", _meta: {}}` — no information at all.
    /// A third of the `tool_call_update`s in a driven transcript were this shape, two of the six a
    /// bare `echo` produced. Each one makes a client re-render a tool row for no change, and the
    /// ratio holds for a command emitting hundreds of snapshots, so it is per-chunk overhead on
    /// the hot path rather than a fixed cost.
    ///
    /// **What it costs.** A client counting `tool_call_update`s per command sees fewer. Nothing
    /// else: the two things this frame could carry — the `in_progress` transition and the output
    /// delta — are each still emitted the moment they exist, which is why the suppression is
    /// `no transition AND no data` rather than a debounce.
    pub fn terminal_progress(&mut self, id: &ToolCallId, output: &str) -> Option<SessionUpdate> {
        let stream = self.open.get_mut(id)?;
        let StreamBody::Terminal(appender) = &mut stream.body else {
            return None;
        };
        let meta = terminal_delta_meta(id, appender.push(output));
        let before = stream.status;
        stream.status.advance();
        if meta.is_empty() && stream.status == before {
            return None;
        }
        Some(SessionUpdate::ToolCallUpdate(
            ToolCallUpdate::new(
                id.clone(),
                ToolCallUpdateFields::new().status(ToolCallStatus::InProgress),
            )
            .meta(meta),
        ))
    }

    /// A terminal's final delta plus its `terminal_exit`, and the close.
    ///
    /// Port of pi-acp v0.0.33 `session.ts`'s `emitBashOutputUpdate` with
    /// `status: isError ? 'failed' : 'completed'` followed by `cleanupToolCall`, and
    /// `translate/bash.ts`'s `bashTerminalExitMeta` (`ACP-140`, `ACP-141`).
    ///
    /// `signal` is always the literal `null`, never omitted — upstream's shape, kept because a
    /// client distinguishing "no signal" from "unknown" would read an absent key as the latter.
    /// `ACP-Q27`, decided: cyrup **could** tell `ExitStatus::{Killed, TimedOut, Signaled}` apart,
    /// but that distinction is not present in the tool result this layer receives (see
    /// [`crate::translate::bash_exit_code`]), so inventing a signal name here would be a fiction.
    ///
    /// # [CYRUP-DELTA] — a desynced FINAL push falls back to a text content block (`ACP-140`)
    ///
    /// **What differs.** Upstream's terminal updates carry no `content` ever, and neither do
    /// cyrup's — the whole point of `terminal_info`/`terminal_output`/`terminal_exit` is that the
    /// client renders a terminal, not a text blob. But the "emit nothing on a desync" policy is
    /// only defensible while the bytes reach the client *somewhere*, and for a terminal there is
    /// nowhere else: no `content`, no `rawOutput`. A desync on the last push therefore used to
    /// drop the tail of the command — the part the user actually wants — through every field at
    /// once. So when, and only when, the final push is [`Push::Desynced`], the final update also
    /// carries `content: [{type: "text", …}]` holding the whole last preview.
    ///
    /// **What it costs.** On that one frame a client that renders both a terminal and the content
    /// array shows the tail twice: once in the pane (as far as it got) and once as text. That is
    /// strictly better than losing it, it happens only on a real discontinuity, and it never
    /// happens on the [`Push::Resync`] path, which is what a long command actually takes.
    pub fn terminal_finish(
        &mut self,
        id: &ToolCallId,
        output: &str,
        is_error: bool,
        exit_code: i32,
    ) -> Option<SessionUpdate> {
        let stream = self.open.get_mut(id)?;
        let StreamBody::Terminal(appender) = &mut stream.body else {
            return None;
        };
        let push = appender.push(output);
        let lost = push == Push::Desynced;
        let mut meta = terminal_delta_meta(id, push);
        let mut exit = Map::new();
        exit.insert("terminal_id".to_string(), Value::String(id.0.to_string()));
        exit.insert("exit_code".to_string(), Value::from(exit_code));
        exit.insert("signal".to_string(), Value::Null);
        meta.insert("terminal_exit".to_string(), Value::Object(exit));

        self.open.remove(id);
        let status = if is_error {
            ToolCallStatus::Failed
        } else {
            ToolCallStatus::Completed
        };
        let mut fields = ToolCallUpdateFields::new().status(status);
        if lost && !output.is_empty() {
            fields = fields.content(vec![ToolCallContent::from(output.to_string())]);
        }
        Some(SessionUpdate::ToolCallUpdate(
            ToolCallUpdate::new(id.clone(), fields).meta(meta),
        ))
    }

    /// Hand the shell's pre-mutation read to a [`ToolClass::Mutation`] call announced earlier from
    /// a streaming delta (`ACP-128` announces, `ACP-131` reads).
    ///
    /// Returns `false` when `id` is unknown or is not a mutation, so a snapshot cannot be attached
    /// to a terminal or to a plain call and later mistaken for a diff base.
    pub fn attach_snapshot(&mut self, id: &ToolCallId, snapshot: FileSnapshot) -> bool {
        match self.open.get_mut(id).map(|s| &mut s.body) {
            Some(StreamBody::Mutation(slot)) => {
                *slot = Some(snapshot);
                true
            }
            _ => false,
        }
    }

    /// The live entry for `id`, or `None` — the "client never saw this tool call" case, which
    /// upstream can emit and Zed silently drops (`ACP-129`).
    pub fn get_mut(&mut self, id: &ToolCallId) -> Option<&mut ToolCallStream> {
        self.open.get_mut(id)
    }

    /// The live entry for `id`, read-only.
    #[must_use]
    pub fn get(&self, id: &ToolCallId) -> Option<&ToolCallStream> {
        self.open.get(id)
    }

    /// How `id` renders, if it is live. Upstream asks `bashToolCallIds.has(id)` and
    /// `fileMutationToolCallIds.has(id)` — two Sets answering one question (`ACP-134`).
    #[must_use]
    pub fn class_of(&self, id: &ToolCallId) -> Option<ToolClass> {
        self.open.get(id).map(|s| s.class)
    }

    /// Whether `id` has been announced.
    #[must_use]
    pub fn contains(&self, id: &ToolCallId) -> bool {
        self.open.contains_key(id)
    }

    /// Drop one entry — upstream's `cleanupToolCall`, which deletes from five collections
    /// (`ACP-137`).
    pub fn close(&mut self, id: &ToolCallId) -> bool {
        self.open.remove(id).is_some()
    }

    /// How many calls are open. For `ACP-137`'s "all per-call maps are empty" assertion.
    #[must_use]
    pub fn len(&self) -> usize {
        self.open.len()
    }

    /// Whether the ledger is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.open.is_empty()
    }

    /// Bounded teardown, called by the shell on `AgentSettled`. See the type docs for why upstream
    /// has no counterpart and leaks.
    pub fn clear(&mut self) {
        self.open.clear();
    }
}

impl UpdatePatch {
    fn into_fields(self, status: ToolCallStatus) -> ToolCallUpdateFields {
        let mut fields = ToolCallUpdateFields::new().status(status);
        if let Some(title) = self.title {
            fields = fields.title(title);
        }
        if let Some(content) = self.content {
            fields = fields.content(content);
        }
        if let Some(locations) = self.locations {
            fields = fields.locations(locations);
        }
        if let Some(raw_input) = self.raw_input {
            fields = fields.raw_input(raw_input);
        }
        if let Some(raw_output) = self.raw_output {
            fields = fields.raw_output(raw_output);
        }
        fields
    }
}

/// A structured ACP diff for a completed file mutation (`ACP-135`).
///
/// Port of pi-acp v0.0.33 `session.ts`'s `content = [{ type: 'diff', path: snapshot.path,
/// oldText: snapshot.oldText, newText }]`.
///
/// # [CYRUP-DELTA] — two divergences, both deliberate
///
/// **What differs.**
///
/// 1. **`path` is the resolved absolute path**, where upstream passes `snapshot.path` — the
///    original, possibly relative string the tool was called with. ACP's own field doc reads "The
///    absolute file path being modified", and a client that resolves a relative diff path against
///    something other than the session cwd shows the diff against the wrong file.
/// 2. **An unreadable pre-read produces no diff at all.** Upstream's condition is
///    `snapshot.oldText === null || newText !== snapshot.oldText`, and its `catch` stores
///    `oldText: null`, so an `EACCES` on the pre-read is indistinguishable from "this file did not
///    exist" and the client is shown a diff claiming the whole file is new. [`FileSnapshot`]'s
///    third state removes the conflation.
///
/// **What it costs.** (1) nothing — the absolute path is strictly more information. (2) an edit
/// whose pre-read failed now renders as plain text rather than as a (wrong) whole-file diff, so a
/// user on a permission-denied file sees less. That is the correct direction: a fabricated diff is
/// worse than no diff.
pub(crate) fn diff_content(path: &Path, before: Option<&str>, after: &str) -> ToolCallContent {
    let diff = Diff::new(path.to_path_buf(), after.to_string());
    ToolCallContent::Diff(match before {
        Some(before) => diff.old_text(before.to_string()),
        None => diff,
    })
}

/// `_meta: { terminal_info: { terminal_id, cwd } }`.
///
/// Port of pi-acp v0.0.33 `translate/bash.ts`'s `bashTerminalInfoMeta`. The keys are
/// **snake_case**, unlike every other field this crate emits, because that is Zed's display-only
/// terminal convention and the wire is what it is.
///
/// # `ACP-Q25`, answered: the typed `terminal/*` family is **not** a cut here
///
/// The typed client family *is* ungated in schema 1.7.0 — `CreateTerminalRequest`,
/// `TerminalOutputRequest`, `WaitForTerminalExitRequest`, `KillTerminalCommandRequest` and
/// `ReleaseTerminalRequest` all sit in `v1/client.rs` with no `#[cfg(feature = "unstable_*")]`, and
/// `ClientCapabilities.terminal` advertises support. It is still the wrong mechanism, and the
/// reason is not stylistic: `terminal/create` asks **the client to execute a command**. cyrup's
/// shell tool has already run it, in-process, through its own `Backend { fs, proc }` with the
/// session's confinement, permission policy and `OutputAccumulator`. Routing it through
/// `terminal/create` would either run the command twice or move execution out of cyrup's sandbox
/// and into the editor's. There is no agent→client message for *reporting* the output of a terminal
/// the agent owns, which is precisely the hole the `_meta` convention fills.
///
/// So the `_meta` protocol stays, and `ACP-139`/`ACP-140`/`ACP-141` are ports rather than cuts.
/// What the answer does buy is a bound on the ceremony: it is three flat objects built by three
/// private functions in this file and nothing else, and the day a client honours a typed
/// agent-owned-terminal message they are the three sites that change.
fn terminal_info_meta(id: &ToolCallId, cwd: &Path) -> Meta {
    let mut info = Map::new();
    info.insert("terminal_id".to_string(), Value::String(id.0.to_string()));
    info.insert("cwd".to_string(), Value::String(cwd.display().to_string()));
    let mut meta = Map::new();
    meta.insert("terminal_info".to_string(), Value::Object(info));
    meta
}

/// `_meta: { terminal_output: { terminal_id, data } }`, or an empty `_meta` when there is nothing
/// to append.
///
/// Port of pi-acp v0.0.33 `translate/bash.ts`'s `bashTerminalOutputMeta`, gated on upstream's
/// `...(delta ? … : {})`. [`Push::Resync`] emits exactly like [`Push::Append`] — a slid window is
/// still append-only output once the overlap is subtracted (`ACP-140`). Only [`Push::Desynced`]
/// takes the "emit nothing" branch — the `ACP-Q26` decision recorded on [`TerminalAppender::push`]
/// — and it is logged so a silent gap is at least a loud one in the agent's own logs.
fn terminal_delta_meta(id: &ToolCallId, push: Push) -> Meta {
    let mut meta = Map::new();
    match push {
        Push::Append(data) | Push::Resync(data) => {
            let mut out = Map::new();
            out.insert("terminal_id".to_string(), Value::String(id.0.to_string()));
            out.insert("data".to_string(), Value::String(data));
            meta.insert("terminal_output".to_string(), Value::Object(out));
        }
        Push::Nothing => {}
        Push::Desynced => {
            tracing::debug!(
                terminal_id = %id.0,
                "ACP-140: bash preview shares no overlap with what was sent (the producer skipped \
                 more than one whole preview window); emitting no terminal_output for this update"
            );
        }
    }
    meta
}

#[cfg(test)]
#[allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing
)]
mod tests {
    use super::*;

    fn ledger() -> ToolCallLedger {
        ToolCallLedger::new(AbsCwd::parse("/work").expect("absolute"))
    }

    fn announce_of(id: &str, class: ToolClass) -> Announce {
        Announce {
            id: ToolCallId::new(id),
            class,
            title: "t".into(),
            status: ToolStatus::Pending,
            locations: Vec::new(),
            raw_input: None,
            snapshot: None,
        }
    }

    fn as_json(update: &SessionUpdate) -> Value {
        serde_json::to_value(update).expect("SessionUpdate serializes")
    }

    /// ADR-0028 §7 — keep ONE canary on the module's privacy boundary rather than a matrix.
    /// `announce` is the only entry constructor, so an update for an unannounced id has no path
    /// into the ledger.
    #[test]
    fn announce_is_the_only_entry_constructor() {
        let mut ledger = ledger();
        let id = ToolCallId::new("t1");
        assert!(ledger.get_mut(&id).is_none(), "no entry before announce");
        assert!(
            ledger
                .update(&id, ToolStatus::InProgress, UpdatePatch::default())
                .is_none(),
            "an update for an unannounced id yields nothing to send"
        );
        assert!(
            ledger.finish(&id, false, UpdatePatch::default()).is_none(),
            "and neither does a finish"
        );
        ledger.announce(announce_of("t1", ToolClass::Terminal));
        assert!(ledger.contains(&id));
        assert_eq!(ledger.len(), 1);
        assert!(ledger.close(&id));
        assert!(!ledger.close(&id), "close is idempotent");
    }

    /// The second canary: [`ToolStatus`] has no backward transition, so the "never downgrade
    /// status" comment cannot be violated. `ACP-129`.
    #[test]
    fn status_never_regresses() {
        let mut status = ToolStatus::Pending;
        status.advance();
        assert_eq!(status, ToolStatus::InProgress);
        status.advance();
        assert_eq!(status, ToolStatus::InProgress, "advance is idempotent");
    }

    /// ACP-129's verify, at the ledger: advance to `InProgress`, then replay a `Pending` advance —
    /// the emitted update must carry `in_progress`, and it must be a `tool_call_update`, never a
    /// second `tool_call`.
    #[test]
    fn a_replayed_pending_advance_still_emits_in_progress() {
        let mut ledger = ledger();
        let id = ToolCallId::new("t1");
        ledger.announce(announce_of("t1", ToolClass::Other));

        let started = ledger
            .update(&id, ToolStatus::InProgress, UpdatePatch::default())
            .expect("announced");
        assert_eq!(as_json(&started)["status"], "in_progress");

        // The late delta. Upstream's `existingStatus ?? 'pending'` is what this replaces.
        let late = ledger
            .update(&id, ToolStatus::Pending, UpdatePatch::default())
            .expect("announced");
        let late = as_json(&late);
        assert_eq!(late["sessionUpdate"], "tool_call_update");
        assert_eq!(
            late["status"], "in_progress",
            "a client that sees in_progress fall back to pending hides its progress UI"
        );
        assert_eq!(
            ledger.get(&id).map(ToolCallStream::status),
            Some(ToolStatus::InProgress)
        );
    }

    /// ACP-135/ACP-137 — `finish` is the ONLY producer of a terminal status, and it closes the
    /// entry, so nothing can follow it.
    #[test]
    fn a_finished_call_cannot_be_resurrected() {
        let mut ledger = ledger();
        let id = ToolCallId::new("t1");
        ledger.announce(announce_of("t1", ToolClass::Other));

        let done = ledger
            .finish(&id, true, UpdatePatch::default())
            .expect("announced");
        assert_eq!(as_json(&done)["status"], "failed");
        assert!(!ledger.contains(&id));
        assert!(
            ledger
                .update(&id, ToolStatus::InProgress, UpdatePatch::default())
                .is_none(),
            "no update can follow a finish"
        );
        assert!(ledger.finish(&id, false, UpdatePatch::default()).is_none());
    }

    /// ACP-137 — the ledger is bounded: `clear` on `AgentSettled` drops a call whose
    /// `tool_execution_end` never arrived. Upstream leaks it for the session's life.
    #[test]
    fn the_ledger_is_bounded_at_settle() {
        let mut ledger = ledger();
        ledger.announce(announce_of("t1", ToolClass::Other));
        ledger.announce(announce_of("t2", ToolClass::Other));
        assert_eq!(ledger.len(), 2);
        ledger.clear();
        assert!(ledger.is_empty());
    }

    /// `acp_kind` is the ONE mapping, so a `Terminal` call is `Execute` at every consumer.
    #[test]
    fn the_acp_kind_mapping_is_total() {
        assert_eq!(ToolClass::Terminal.acp_kind(), ToolKind::Execute);
        assert_eq!(ToolClass::Mutation.acp_kind(), ToolKind::Edit);
        assert_eq!(ToolClass::Read.acp_kind(), ToolKind::Read);
        assert_eq!(ToolClass::Search.acp_kind(), ToolKind::Search);
        assert_eq!(ToolClass::Other.acp_kind(), ToolKind::Other);
        assert!(ToolClass::Mutation.needs_snapshot());
        assert!(!ToolClass::Terminal.needs_snapshot());
        assert!(ToolClass::Terminal.is_terminal());
    }

    /// ACP-151 / ACP-157 — the table over **every** name cyrup registers by default
    /// (`cyrup_tools::BUILTIN_NAMES`), asserting none falls through to `Other` unintentionally, and
    /// that `powershell` is a terminal where a faithful port leaves it generic.
    #[test]
    fn every_cyrup_builtin_is_classified_and_powershell_is_a_terminal() {
        // `ACP-157` — the cross-check is against the REGISTRY, not against a hand-kept copy of it.
        // A ninth built-in would otherwise ship silently classified `Other`, which is exactly the
        // failure this unit exists for: `powershell` was invisible to upstream's `isBashTool` for
        // precisely that reason, one shell tool later.
        for name in cyrup_tools::BUILTIN_NAMES {
            assert_ne!(
                ToolClass::of(name),
                ToolClass::Other,
                "`{name}` is a cyrup built-in with no `ToolClass::of` row — add one"
            );
        }
        let builtins = [
            ("read", ToolClass::Read),
            ("bash", ToolClass::Terminal),
            ("powershell", ToolClass::Terminal),
            ("edit", ToolClass::Mutation),
            ("write", ToolClass::Mutation),
            ("grep", ToolClass::Search),
            ("find", ToolClass::Search),
            ("ls", ToolClass::Search),
        ];
        assert_eq!(
            builtins.len(),
            cyrup_tools::BUILTIN_NAMES.len(),
            "this table names every built-in, and the registry is the authority on how many"
        );
        for (name, class) in builtins {
            assert!(
                cyrup_tools::BUILTIN_NAMES.contains(&name),
                "`{name}` is not in the registry"
            );
            assert_eq!(ToolClass::of(name), class, "{name}");
        }
        // ACP-157: the row upstream's exact `isBashTool` cannot reach.
        assert_eq!(ToolClass::of("powershell").acp_kind(), ToolKind::Execute);
        // ACP-138: `Bash` is recognised — and, unlike upstream, by the SAME classifier that
        // decides the kind, so `isBashTool` and `toToolKind` cannot disagree about it.
        assert_eq!(ToolClass::of("Bash"), ToolClass::Terminal);
        assert_eq!(ToolClass::of("Bash").acp_kind(), ToolKind::Execute);
        assert_eq!(ToolClass::of("PowerShell"), ToolClass::Terminal);
        // An MCP or extension tool name has a total default, and it is `Other`.
        assert_eq!(ToolClass::of("mcp__github__list_issues"), ToolClass::Other);
        assert_eq!(ToolClass::of(""), ToolClass::Other);
    }

    /// ACP-139 — the terminal `_meta` and the terminal content ride the announce and **nothing
    /// else**, snake_case, with the terminal id equal to the tool-call id.
    #[test]
    fn the_terminal_meta_rides_the_announce_and_nothing_else() {
        let mut ledger = ledger();
        let id = ToolCallId::new("call-1");
        let first = ledger.announce(Announce {
            title: "ls -la".into(),
            ..announce_of("call-1", ToolClass::Terminal)
        });
        let first = as_json(&first);
        assert_eq!(first["sessionUpdate"], "tool_call");
        assert_eq!(first["title"], "ls -la");
        assert_eq!(first["kind"], "execute");
        assert_eq!(first["content"][0]["type"], "terminal");
        assert_eq!(first["content"][0]["terminalId"], "call-1");
        assert_eq!(first["_meta"]["terminal_info"]["terminal_id"], "call-1");
        assert_eq!(first["_meta"]["terminal_info"]["cwd"], "/work");

        // The second emission for the same id carries neither.
        let second = ledger
            .update(&id, ToolStatus::InProgress, UpdatePatch::default())
            .expect("announced");
        let second = as_json(&second);
        assert_eq!(second["sessionUpdate"], "tool_call_update");
        assert!(second.get("content").is_none(), "{second}");
        assert!(
            second
                .get("_meta")
                .and_then(|m| m.get("terminal_info"))
                .is_none(),
            "a second terminal_info would open a second terminal: {second}"
        );

        // A non-terminal announce carries no terminal machinery at all.
        let plain = as_json(&ledger.announce(announce_of("call-2", ToolClass::Read)));
        assert!(plain.get("content").is_none(), "{plain}");
        assert!(plain.get("_meta").is_none(), "{plain}");
    }

    /// ACP-139 — a terminal delta for an id the client was never told about is unrepresentable,
    /// and so is one for a call that is not a terminal.
    #[test]
    fn only_an_announced_terminal_can_produce_terminal_meta() {
        let mut ledger = ledger();
        let unknown = ToolCallId::new("nope");
        assert!(ledger.terminal_progress(&unknown, "out").is_none());
        assert!(ledger.terminal_finish(&unknown, "out", false, 0).is_none());

        ledger.announce(announce_of("edit-1", ToolClass::Mutation));
        let edit = ToolCallId::new("edit-1");
        assert!(
            ledger.terminal_progress(&edit, "out").is_none(),
            "a mutation has no terminal to append to"
        );
        assert!(
            ledger.contains(&edit),
            "and the refusal did not consume the entry"
        );
    }

    /// ACP-140 — the appender emits only the suffix while the snapshot grows.
    #[test]
    fn a_growing_snapshot_yields_only_the_suffix() {
        let mut appender = TerminalAppender::default();
        assert_eq!(appender.push(""), Push::Nothing);
        assert_eq!(appender.push("a"), Push::Append("a".into()));
        assert_eq!(appender.push("ab"), Push::Append("b".into()));
        assert_eq!(appender.push("ab"), Push::Nothing);
        assert_eq!(appender.push("abcd"), Push::Append("cd".into()));
        assert_eq!(appender.emitted(), "abcd");
    }

    /// One line of the corpus these tests slide a window over. Long enough that a single line of
    /// overlap clears [`MIN_RESYNC_OVERLAP`], which is what the real 50 KiB window does by four
    /// orders of magnitude.
    fn corpus_line(n: usize) -> String {
        format!("line {n:04}: Compiling cyrup-acp v0.0.0 (/home/user/cyrup)\n")
    }

    /// What the real producer hands this module: `build_stream_update` takes `acc.tail_string()`
    /// and then `truncate_tail`, which selects whole lines working BACKWARDS from the end. So the
    /// preview after `upto` lines have been written is the last `window` of them — a window that
    /// slides, not a string that grows.
    fn tail_window(upto: usize, window: usize) -> String {
        (upto.saturating_sub(window)..upto).map(corpus_line).collect()
    }

    /// ACP-140 — the case the real producer reaches on every flush of any command past
    /// `truncate_tail`'s limit: the window slid, so the preview is not a prefix extension, but the
    /// bytes past the overlap are still ordinary append-only output.
    #[test]
    fn a_slid_window_resyncs_on_the_overlap() {
        let mut appender = TerminalAppender::default();
        // Four lines fit in the window, so the first two pushes still grow.
        assert_eq!(appender.push(&tail_window(1, 4)), Push::Append(corpus_line(0)));
        assert_eq!(appender.push(&tail_window(2, 4)), Push::Append(corpus_line(1)));
        // From here the window is full and every later preview has dropped its head. Before this
        // fix each of these was `Push::Desynced` — forever, for the rest of the command.
        assert_eq!(
            appender.push(&tail_window(5, 4)),
            Push::Resync([corpus_line(2), corpus_line(3), corpus_line(4)].concat()),
            "three lines were written between flushes; the fourth is the overlap"
        );
        assert_eq!(appender.push(&tail_window(6, 4)), Push::Resync(corpus_line(5)));
        assert_eq!(
            appender.push(&tail_window(9, 4)),
            Push::Resync([corpus_line(6), corpus_line(7), corpus_line(8)].concat())
        );
        // A window that slid past everything already sent is a repeat of nothing new.
        assert_eq!(appender.push(&tail_window(9, 4)), Push::Nothing);
    }

    /// ACP-140 / ACP-Q26 — the residue after the slide is recovered: a preview sharing no usable
    /// overlap with what was sent is still a named, silent gap rather than upstream's duplicated
    /// tail, and it re-bases so the very next preview resumes.
    #[test]
    fn a_genuine_discontinuity_is_still_a_silent_gap() {
        let mut appender = TerminalAppender::default();
        assert_eq!(
            appender.push("line1\nline2\n"),
            Push::Append("line1\nline2\n".into())
        );
        // Five bytes of overlap ("ine2\n") is indexing noise, not evidence of continuity: below
        // MIN_RESYNC_OVERLAP the honest answer about how much was skipped is "unknown".
        assert_eq!(appender.push("ine2\nline3\n"), Push::Desynced);
        assert_eq!(
            appender.emitted(),
            "ine2\nline3\n",
            "re-based, so the NEXT update is a clean suffix rather than a second desync"
        );
        assert_eq!(
            appender.push("ine2\nline3\nline4\n"),
            Push::Append("line4\n".into())
        );
    }

    /// ACP-140's whole-command property, at the ledger: over a command whose output outgrows the
    /// preview window many times, the concatenated `terminal_output.data` is the command's output
    /// **exactly** — every line once, in order, none dropped and none repeated. Upstream's `: next`
    /// fallback repeats the window on every flush; the pre-fix cyrup behaviour dropped everything
    /// after the first full window.
    #[test]
    fn a_sliding_window_never_repeats_or_drops_a_line() {
        let mut ledger = ledger();
        let id = ToolCallId::new("sh-1");
        ledger.announce(announce_of("sh-1", ToolClass::Terminal));

        let window = 4;
        let mut seen = String::new();
        // 20 lines through a 4-line window: 16 flushes past the point of no return.
        for upto in 1..=20 {
            let Some(update) = ledger.terminal_progress(&id, &tail_window(upto, window)) else {
                continue;
            };
            if let Some(data) = as_json(&update)["_meta"]["terminal_output"]["data"].as_str() {
                seen.push_str(data);
            }
        }
        let whole: String = (0..20).map(corpus_line).collect();
        assert_eq!(
            seen, whole,
            "the terminal pane must hold the command's entire output, once"
        );
        for n in 0..20 {
            assert_eq!(
                seen.matches(&corpus_line(n)).count(),
                1,
                "line {n} appears more than once — that is upstream's duplicate-the-tail bug"
            );
        }
    }

    /// ACP-140 — the second half: a terminal update carries no `content` and no `rawOutput`, so a
    /// desync on the FINAL push had no field left to deliver the tail of the command through. It
    /// now falls back to a text content block.
    #[test]
    fn the_tail_of_a_desynced_final_push_survives_as_content() {
        let mut ledger = ledger();
        let id = ToolCallId::new("sh-1");
        ledger.announce(announce_of("sh-1", ToolClass::Terminal));
        let _ = ledger.terminal_progress(&id, "aaa\n");

        let update = ledger
            .terminal_finish(&id, "zzz\n", false, 0)
            .expect("the terminal closes");
        let json = as_json(&update);
        assert!(
            json["_meta"]["terminal_output"].is_null(),
            "the desynced delta is still suppressed in `_meta`: {json}"
        );
        assert_eq!(
            json["content"][0]["content"]["text"], "zzz\n",
            "…but the bytes are not lost: {json}"
        );
        assert_eq!(json["_meta"]["terminal_exit"]["exit_code"], 0);
    }

    /// ACP-140 — and the fallback is *only* for the lost case: an ordinary terminal close carries
    /// no `content` at all, which is upstream's shape and what makes a client render the pane
    /// rather than a text blob.
    #[test]
    fn an_ordinary_terminal_close_still_carries_no_content() {
        let mut ledger = ledger();
        let id = ToolCallId::new("sh-1");
        ledger.announce(announce_of("sh-1", ToolClass::Terminal));
        let _ = ledger.terminal_progress(&id, &tail_window(6, 4));

        let update = ledger
            .terminal_finish(&id, &tail_window(8, 4), false, 0)
            .expect("the terminal closes");
        let json = as_json(&update);
        assert!(
            json["content"].is_null(),
            "a resynced close needs no fallback: {json}"
        );
        assert_eq!(
            json["_meta"]["terminal_output"]["data"],
            [corpus_line(6), corpus_line(7)].concat(),
            "the final delta rides `_meta`, as upstream: {json}"
        );
    }

    /// An update with nothing in it is not sent.
    ///
    /// The observed defect: a third of all `tool_call_update` frames in a driven transcript were
    /// `{sessionUpdate, toolCallId, status: "in_progress", _meta: {}}` — no delta, no transition,
    /// nothing for a client to do but re-render the row.
    #[test]
    fn a_snapshot_that_says_nothing_sends_nothing() {
        let mut ledger = ledger();
        let id = ToolCallId::new("sh-1");
        ledger.announce(announce_of("sh-1", ToolClass::Terminal));

        // The first snapshot carries the `pending -> in_progress` transition even when the
        // command has produced no output yet, so it IS sent — that transition is what starts the
        // client's spinner.
        let first = ledger.terminal_progress(&id, "").expect("the transition");
        let first = as_json(&first);
        assert_eq!(first["status"], "in_progress");
        assert!(first["_meta"].get("terminal_output").is_none());

        // Every later repeat of the same empty preview has neither, so nothing is sent.
        assert!(ledger.terminal_progress(&id, "").is_none());
        assert!(ledger.terminal_progress(&id, "").is_none());

        // Real output resumes immediately — this is a suppression of empty frames, not a debounce.
        let real = ledger.terminal_progress(&id, "out\n").expect("data");
        assert_eq!(as_json(&real)["_meta"]["terminal_output"]["data"], "out\n");
        // …and a snapshot that merely repeats it is silent again.
        assert!(ledger.terminal_progress(&id, "out\n").is_none());

        // The close is never suppressed: `terminal_exit` is always something to say.
        let done = ledger
            .terminal_finish(&id, "out\n", false, 0)
            .expect("terminal");
        assert_eq!(as_json(&done)["_meta"]["terminal_exit"]["exit_code"], 0);
    }

    /// ACP-141 — `terminal_exit` carries the code and an explicit `signal: null`, and the finish
    /// closes the entry.
    #[test]
    fn the_terminal_exit_meta_is_shaped_exactly_as_upstream() {
        let mut ledger = ledger();
        let id = ToolCallId::new("sh-1");
        ledger.announce(announce_of("sh-1", ToolClass::Terminal));
        let update = ledger
            .terminal_finish(&id, "boom\n", true, 42)
            .expect("terminal");
        let update = as_json(&update);
        assert_eq!(update["status"], "failed");
        assert_eq!(update["_meta"]["terminal_output"]["data"], "boom\n");
        assert_eq!(update["_meta"]["terminal_exit"]["terminal_id"], "sh-1");
        assert_eq!(update["_meta"]["terminal_exit"]["exit_code"], 42);
        assert!(
            update["_meta"]["terminal_exit"]["signal"].is_null(),
            "signal is the literal null, never omitted: {update}"
        );
        assert!(
            update.get("content").is_none() && update.get("rawOutput").is_none(),
            "for a terminal everything rides _meta: {update}"
        );
        assert!(!ledger.contains(&id), "terminal_finish closes the entry");
    }

    /// ACP-131 — the three snapshot states are distinct, and only two of them can produce a diff.
    #[test]
    fn the_three_snapshot_states_are_distinct() {
        let content = FileSnapshot::read("/work/a.rs", "old");
        let absent = FileSnapshot::absent("/work/a.rs");
        let unreadable = FileSnapshot::unreadable("/work/a.rs");
        assert_eq!(content.before.as_deref(), Some("old"));
        assert_eq!(absent.before, None);
        assert_eq!(unreadable.before, None);
        assert_ne!(absent, unreadable, "upstream conflates exactly these two");
        assert!(content.is_diffable() && absent.is_diffable());
        assert!(
            !unreadable.is_diffable(),
            "ACP-135: an edit whose pre-read failed emits NO diff"
        );
    }

    /// ACP-135 — the diff's `path` is the resolved absolute path, and `old_text` is omitted only
    /// for the write-to-new-file case.
    #[test]
    fn the_diff_carries_an_absolute_path_and_a_nullable_old_text() {
        let new_file = serde_json::to_value(diff_content(
            &AbsCwd::parse("/work")
                .expect("absolute")
                .resolve(Path::new("a.rs")),
            None,
            "hello",
        ))
        .expect("serializes");
        assert_eq!(new_file["type"], "diff");
        assert_eq!(new_file["path"], "/work/a.rs");
        assert_eq!(new_file["newText"], "hello");
        assert!(
            new_file.get("oldText").is_none(),
            "None omits the key: {new_file}"
        );

        let edited =
            serde_json::to_value(diff_content(Path::new("/work/a.rs"), Some("old"), "new"))
                .expect("serializes");
        assert_eq!(edited["oldText"], "old");
        assert_eq!(edited["newText"], "new");
    }

    /// A snapshot can only be attached to a mutation, and only to a live id.
    #[test]
    fn a_snapshot_attaches_only_to_a_live_mutation() {
        let mut ledger = ledger();
        let snap = FileSnapshot::read("a.rs", "old");
        assert!(!ledger.attach_snapshot(&ToolCallId::new("gone"), snap.clone()));

        ledger.announce(announce_of("sh-1", ToolClass::Terminal));
        assert!(!ledger.attach_snapshot(&ToolCallId::new("sh-1"), snap.clone()));

        ledger.announce(announce_of("e-1", ToolClass::Mutation));
        let edit = ToolCallId::new("e-1");
        assert!(
            ledger
                .get(&edit)
                .and_then(ToolCallStream::snapshot)
                .is_none()
        );
        assert!(ledger.attach_snapshot(&edit, snap.clone()));
        assert_eq!(
            ledger.get(&edit).and_then(ToolCallStream::snapshot),
            Some(&snap)
        );
    }

    /// `class_of` answers the question upstream asks of two `Set`s (`bashToolCallIds`,
    /// `fileMutationToolCallIds`), which is how they can disagree.
    #[test]
    fn class_of_replaces_two_disagreeing_sets() {
        let mut ledger = ledger();
        ledger.announce(announce_of("a", ToolClass::Terminal));
        ledger.announce(announce_of("b", ToolClass::Mutation));
        assert_eq!(
            ledger.class_of(&ToolCallId::new("a")),
            Some(ToolClass::Terminal)
        );
        assert_eq!(
            ledger.class_of(&ToolCallId::new("b")),
            Some(ToolClass::Mutation)
        );
        assert_eq!(ledger.class_of(&ToolCallId::new("c")), None);
    }
}
