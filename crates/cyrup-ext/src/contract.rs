//! The block / mutate / notify reducer types (arch-08 §3.3). A single handler returns a
//! [`HookOutcome`]; the dispatcher folds outcomes left-to-right in load order. For `[mutate]`,
//! later handlers observe the folded value (chaining, R-08-011).

use crate::event::HostEvent;
use cyrup_agent::AgentMessage;
use cyrup_core::{Content, Message, TerminateHint};
use serde_json::Value;
use std::sync::Arc;

/// What a single handler contributes (arch-08 §3.3).
#[derive(Clone, Debug)]
pub enum HookOutcome {
    /// notify-only events; return ignored (R-08-009).
    Noop,
    /// `[block]` — short-circuits the action with an optional reason. First block wins.
    Block {
        reason: Option<String>,
        /// `tool_call` only (EXT-049) — pi `ToolCallEventResult.terminate`
        /// (`pi/packages/coding-agent/src/core/extensions/types.ts:1072-1079` @v0.84.1, ABSENT at
        /// the ported v0.83.0 baseline): "Hint that the agent should stop after the current tool
        /// batch when this call is blocked. Early termination only happens when every finalized
        /// tool result in the batch sets this to true." Consumed at
        /// `packages/agent/src/agent-loop.ts:636-646`, folded by `shouldTerminateToolBatch` at
        /// `:583` into `hasMoreToolCalls = !executedToolBatch.terminate` at `:216` — the every()
        /// rule lives in the agent, not here, so a single blocking handler setting this does NOT
        /// end the run on its own.
        ///
        /// [`TerminateHint::Unspecified`] is pi's `undefined`. Ignored on every non-`tool_call`
        /// seam, exactly as upstream ignores it (no other `*EventResult` declares the field). A
        /// guest's WIT `bool` arrives through [`TerminateHint::from_guest_bool`]: `false` is
        /// "nothing said", never an explicit `Continue`.
        terminate: TerminateHint,
    },
    /// `[mutate]` — a typed patch applied to the in-flight value.
    Mutate(EventPatch),
    /// `input`/`user_bash` "handled"/"provide": the extension fully serviced it.
    Handled(HandledValue),
}

/// A fully-serviced result (arch-08 §3.3). Open-shaped; carried as JSON.
#[derive(Clone, Debug)]
pub struct HandledValue(pub Value);

/// Typed, event-specific patch payloads (arch-08 §3.3). `serde_json::Value` only for genuinely
/// open fields (tool args, custom payloads); fixed shapes stay typed.
#[derive(Clone, Debug)]
pub enum EventPatch {
    /// `tool_call`: rewrite the tool input (R-08-010).
    ToolInput(Value),
    /// `tool_result`: replace-not-merge override of result fields (R-08-011). `usage` mirrors Pi
    /// `ToolResultEventResult.usage` (types.ts:1085-1090): `Some` REPLACES the tool's usage in
    /// full — there is no deep merge (types.ts:70-78).
    ToolResult {
        content: Option<Vec<Content>>,
        details: Option<Value>,
        is_error: Option<bool>,
        usage: Option<cyrup_core::Usage>,
        /// pi `ToolResultEventResult.terminate` folded by `afterResult.terminate ?? result.terminate`
        /// (agent-loop.ts:739): `None` = the key was absent from the patch and the tool's own hint
        /// stands; `Some(hint)` replaces it. Decoded from the guest's JSON patch, so a present
        /// `false` IS [`TerminateHint::Continue`]. Additive: a guest that never sends the key
        /// changes nothing.
        terminate: Option<TerminateHint>,
    },
    /// `context`: filter/replace the message list.
    Context { messages: Vec<Arc<AgentMessage>> },
    /// `message_end`: replace the message.
    Message(Box<Message>),
    /// `before_agent_start`: system-prompt replacement + optional injection.
    SystemPromptAndInject {
        system: Option<String>,
        inject: Option<Box<Message>>,
    },
    /// `input` (Pi `action:"transform"`, runner.ts:1116-1119): rewrite the submission text and
    /// (optionally) its images. `images: None` keeps the current images (Pi `result.images ??
    /// currentImages`); `Some(_)` replaces them. Folds across handlers — a later handler observes
    /// the rewritten text/images (R-08-011).
    Input {
        text: String,
        images: Option<Vec<Content>>,
    },
    /// `before_provider_request` (Pi runner.ts:946-978): a handler's return value REPLACES the
    /// outbound payload wholesale (`currentPayload = handlerResult`); later handlers observe the
    /// replacement. Open-shaped: the provider request body crosses as `serde_json::Value`.
    ProviderRequest(Value),
    /// `before_provider_headers` (EXT-009; pi `BeforeProviderHeadersEvent`,
    /// extensions/types.ts:686-689 @v0.83.0). Upstream handlers "mutate `headers` in place … the
    /// return value is ignored. A `null` value deletes that header" (:681-685), so this is a
    /// PATCH object rather than a replacement: each key is set to its value, and a key whose value
    /// is `null` is REMOVED. That asymmetry is the whole point of the event — a proxy or auth-shim
    /// extension deletes a header it must not send, and setting it to `""` would still send it.
    ProviderHeaders(Value),
    /// `session_before_compact` (Pi `SessionBeforeCompactResult.compaction`, types.ts:1079): an
    /// extension-supplied compaction override (a `CompactionResult`: `{summary, firstKeptEntryId?,
    /// tokensBefore?, details?}`). The LAST override wins across the chain; the producer threads its
    /// `summary`/`details` into the appended compaction entry (marked `fromExtension`).
    CompactionOverride(Value),
    /// `session_before_tree` (Pi `SessionBeforeTreeResult`, types.ts:1082-1094): an extension-supplied
    /// summary/customInstructions/label override for the branch summarization. Open-shaped.
    TreeOverride(Value),
}

impl HostEvent {
    /// Fold a `[mutate]` patch into this event so the NEXT handler observes it (R-08-011).
    /// A patch whose shape does not match the event is ignored (degrade, never panic — §8).
    pub fn apply_patch(&mut self, patch: EventPatch) {
        match (self, patch) {
            (HostEvent::ToolCall { input, .. }, EventPatch::ToolInput(v)) => *input = v,
            (
                HostEvent::ToolResult {
                    content,
                    details,
                    is_error,
                    usage,
                    terminate,
                    ..
                },
                EventPatch::ToolResult {
                    content: c,
                    details: d,
                    is_error: e,
                    usage: u,
                    terminate: t,
                },
            ) => {
                if let Some(c) = c {
                    *content = c;
                }
                if d.is_some() {
                    *details = d;
                }
                if let Some(e) = e {
                    *is_error = e;
                }
                // Pi `ToolResultEventResult.usage` (types.ts:1088): an omitted key keeps the
                // current value, a present one REPLACES it in full (no deep merge, types.ts:70-78).
                if u.is_some() {
                    *usage = u;
                }
                // Replace-not-merge, same shape as `is_error`: an omitted key keeps the tool's hint.
                if let Some(t) = t {
                    *terminate = t;
                }
            }
            (HostEvent::Context { messages }, EventPatch::Context { messages: m }) => *messages = m,
            // `message_end` (Pi runner.ts:785): a replacement message MUST keep the same role; a
            // mismatched role is rejected (the replacement is dropped, the original kept) — no panic.
            (HostEvent::MessageEnd { message }, EventPatch::Message(m)) => {
                if message_role(message) == message_role(&m) {
                    *message = *m;
                }
            }
            // `before_agent_start` (Pi runner.ts:980): replace the system prompt AND/OR accumulate
            // an injected message across the handler chain.
            (
                HostEvent::BeforeAgentStart {
                    system_prompt,
                    injected,
                    ..
                },
                EventPatch::SystemPromptAndInject { system, inject },
            ) => {
                if let Some(s) = system {
                    *system_prompt = s;
                }
                if let Some(m) = inject {
                    injected.push(*m);
                }
            }
            // `input` (Pi runner.ts:1116-1119): always rewrite the text; replace images only when
            // the handler supplied them (`Some`), else keep the folded-so-far images.
            (HostEvent::Input { text, images, .. }, EventPatch::Input { text: t, images: i }) => {
                *text = t;
                if let Some(i) = i {
                    *images = i;
                }
            }
            // `before_provider_request` (Pi runner.ts:962): the handler's return value REPLACES the
            // payload wholesale; the next handler sees the replacement.
            (HostEvent::BeforeProviderRequest { payload }, EventPatch::ProviderRequest(v)) => {
                *payload = v;
            }
            // `before_provider_headers` (pi types.ts:681-685): in-place mutation semantics — set
            // each supplied key, DELETE the ones whose value is `null`. A non-object patch is
            // ignored (degrade, never panic).
            (HostEvent::BeforeProviderHeaders { headers }, EventPatch::ProviderHeaders(v)) => {
                if let (Some(dst), Some(src)) = (headers.as_object_mut(), v.as_object()) {
                    for (k, val) in src {
                        if val.is_null() {
                            dst.remove(k);
                        } else {
                            dst.insert(k.clone(), val.clone());
                        }
                    }
                }
            }
            // `session_before_compact` (Pi `SessionBeforeCompactResult.compaction`): capture the
            // extension-supplied compaction override on the event so the producer folds it back.
            (
                HostEvent::SessionBeforeCompact {
                    override_result, ..
                },
                EventPatch::CompactionOverride(v),
            ) => *override_result = Some(v),
            // `session_before_tree` (Pi `SessionBeforeTreeResult`): capture the summary/label override.
            (
                HostEvent::SessionBeforeTree {
                    override_result, ..
                },
                EventPatch::TreeOverride(v),
            ) => *override_result = Some(v),
            // Shape mismatch: ignore (degrade gracefully).
            _ => {}
        }
    }
}

/// The role discriminant of an LLM message (for the `message_end` same-role rule, R-08-011).
fn message_role(m: &Message) -> &'static str {
    match m {
        Message::User { .. } => "user",
        Message::Assistant(_) => "assistant",
        Message::ToolResult { .. } => "toolResult",
    }
}

/// The reduced result of dispatching a `[block]`/`[mutate]` event (arch-08 §6.1).
#[derive(Debug)]
pub enum Reduced {
    /// No block; the (possibly folded) event proceeds. Boxed: `HostEvent` is much larger than the
    /// other variants, so boxing keeps `Reduced` small (clippy::large_enum_variant).
    Pass(Box<HostEvent>),
    /// First `Block` wins; carries the reason, the blocking extension id, and (on `tool_call`
    /// only) pi's `ToolCallEventResult.terminate` hint — see [`HookOutcome::Block::terminate`].
    Blocked {
        reason: Option<String>,
        terminate: TerminateHint,
        by: cyrup_core::ExtensionId,
    },
    /// An extension fully serviced the action. Carries the id of the extension that produced the
    /// winning value, exactly as [`Self::Blocked`] does.
    ///
    /// **Why attribution.** Pi's `emitUserBash` returns the FIRST truthy handler's whole
    /// `UserBashEventResult` (`extensions/runner.ts:1005-1032` @v0.84.4), and the RPC host then
    /// reads `eventResult.operations` off it (`modes/rpc/rpc-mode.ts:581`) — a live
    /// `BashOperations` OBJECT. ADR-0002 makes cyrup's extension I/O values, so the callable half
    /// cannot ride inside [`HandledValue`]; the host has to ask the winning extension for it. That
    /// is only possible if the reduction says WHICH extension won, which is what `by` is for
    /// (SEAM-015). The same field on `Blocked` exists for the same reason and predates this.
    Handled {
        value: HandledValue,
        by: cyrup_core::ExtensionId,
    },
}

/// One terminal-input handler's answer (EXT-021; pi `TerminalInputHandler`'s return,
/// `packages/coding-agent/src/core/extensions/types.ts:113` @v0.83.0:
/// `{ consume?: boolean; data?: string } | undefined`).
///
/// Both members stay `Option` because upstream's fold (`packages/tui/src/tui.ts:773-788`) tests
/// `result?.consume` (truthy) and `result?.data !== undefined` — so `{data: ""}` REWRITES the
/// buffer to empty (and the keystroke is then dropped by the end-of-fold length check at `:784`),
/// while `{}` leaves it alone. Collapsing either to a bare `bool`/`String` would erase that
/// distinction.
///
/// A `None` return from a handler is upstream's `undefined`: "I looked at it and did nothing".
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct TerminalInputResult {
    pub consume: Option<bool>,
    pub data: Option<String>,
}

/// What the host tells its caller to do with one raw terminal-input chunk, after folding every
/// subscriber (EXT-021). The Rust shape of pi's `TUI.handleInput` outcome
/// (`packages/tui/src/tui.ts:773-788`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalInputDecision {
    /// Deliver `data` to the editor. Equal to the input when no handler rewrote it.
    Deliver(String),
    /// Drop the keystroke entirely — either a handler returned `consume: true` (`:777-779`) or the
    /// fold ended with an empty string (`:784-786`).
    Consume,
}

// =================================================================================================
// The raw-terminal-key table (EXT-021 / UW-7)
// =================================================================================================

/// One keyboard modifier set, in pi's own bit layout (`packages/tui/src/keys.ts:292-297`
/// @v0.83.0: `shift: 1, alt: 2, ctrl: 4, super: 8`).
///
/// The layout is pi's rather than crossterm's because it is what goes ON THE WIRE: the kitty
/// CSI-u and xterm `modifyOtherKeys` encodings this module emits and parses carry `modifier + 1`
/// as a decimal field, and that field is defined by the terminal protocols pi is decoding, not by
/// any Rust crate. Keeping the same integer here means [`encode_terminal_key`] and
/// [`decode_terminal_key`] never have to translate twice.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Hash)]
pub struct TerminalKeyModifiers(u8);

impl TerminalKeyModifiers {
    /// No modifier held — pi's `modifier === 0` arms.
    pub const NONE: Self = Self(0);
    /// pi `MODIFIERS.shift` (`keys.ts:293`).
    pub const SHIFT: Self = Self(1);
    /// pi `MODIFIERS.alt` (`keys.ts:294`).
    pub const ALT: Self = Self(2);
    /// pi `MODIFIERS.ctrl` (`keys.ts:295`).
    pub const CTRL: Self = Self(4);
    /// pi `MODIFIERS.super` (`keys.ts:296`).
    pub const SUPER: Self = Self(8);

    /// The raw pi bitmask.
    #[must_use]
    pub const fn bits(self) -> u8 {
        self.0
    }

    /// Build from the raw pi bitmask, dropping the lock bits pi itself masks off before every
    /// comparison (`LOCK_MASK = 64 + 128`, Caps Lock + Num Lock, `keys.ts:299`; applied at
    /// `matchesKittySequence` `:656-657`). A terminal that reports Caps Lock must not turn a plain
    /// `j` into an unmatched key.
    #[must_use]
    pub const fn from_bits(bits: u8) -> Self {
        Self(bits & !(64 | 128))
    }

    /// Whether nothing (after the lock mask) is held.
    #[must_use]
    pub const fn is_empty(self) -> bool {
        self.0 == 0
    }

    /// Set union, for building a modifier set from a `KeyEvent`'s flags.
    #[must_use]
    pub const fn union(self, other: Self) -> Self {
        Self(self.0 | other.0)
    }

    /// Whether every bit of `other` is held.
    #[must_use]
    pub const fn contains(self, other: Self) -> bool {
        (self.0 & other.0) == other.0
    }
}

/// The closed vocabulary of keys the raw-terminal-input seam encodes and decodes (UW-7).
///
/// **Deliberately small.** It is exactly what pi-subagents' fleet-status roster matches on —
/// `matchesKey(data, "down"|"up"|"left")`, `matchesKey(data, "escape")`, `matchesKey(data,
/// Key.enter)` and the printable `"j"`/`"k"` arms (`pi-subagents/src/tui/fleet-status.ts:706-753`
/// @v0.68.0) — plus [`TerminalKey::Right`], which costs nothing (it is the fourth member of the
/// same arrow family and shares every code path with [`TerminalKey::Left`]) and which the roster
/// genuinely needs: pi's catch-all `this.deactivate(); return undefined;` (`:752-753`) fires on
/// `→` exactly as it does on `←`'s siblings, and a key the encoder cannot express never reaches
/// the widget at all.
///
/// pi's `matchesKey` (`packages/tui/src/keys.ts:820` @v0.83.0) covers ~40 more key ids. They are
/// NOT ported: a `KeyEvent` this table cannot express is delivered to the editor WITHOUT
/// consulting the seam (see [`encode_terminal_key`]'s contract), which is the conservative
/// direction — a keystroke is never dropped, only never offered.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum TerminalKey {
    /// pi `matchesKey(data, "up")` (`keys.ts:1043-1054`).
    Up,
    /// pi `matchesKey(data, "down")` (`keys.ts:1056-1072`).
    Down,
    /// pi `matchesKey(data, "left")` (`keys.ts:1074-1098`).
    Left,
    /// pi `matchesKey(data, "right")` (`keys.ts:1100-1124`).
    Right,
    /// pi `matchesKey(data, "escape")` (`keys.ts:838-846`).
    Escape,
    /// pi `matchesKey(data, Key.enter)` (`keys.ts:884-931`).
    Enter,
    /// A printable character — pi's single-key arm (`keys.ts:1145-1194`).
    Char(char),
}

/// One keystroke on the raw-terminal-input wire: which key, which modifiers, and whether it is a
/// RELEASE rather than a press.
///
/// `release` is the kitty-keyboard-protocol event type 3 (`keys.ts:501-504`), which pi surfaces
/// through `isKeyRelease(data)` (`:527-549`) and which
/// `pi-subagents/src/tui/fleet-status.ts:699` checks before anything else.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub struct TerminalKeyEvent {
    /// Which key.
    pub key: TerminalKey,
    /// Which modifiers were held.
    pub modifiers: TerminalKeyModifiers,
    /// `true` for a kitty event-type-3 release.
    pub release: bool,
}

impl TerminalKeyEvent {
    /// An unmodified press of `key`.
    #[must_use]
    pub const fn press(key: TerminalKey) -> Self {
        Self {
            key,
            modifiers: TerminalKeyModifiers::NONE,
            release: false,
        }
    }
}

/// The kitty CSI-u codepoint pi assigns each non-printable key (`keys.ts:301-307`); arrows have no
/// CSI-u codepoint in the legacy protocol and are carried by their `A`/`B`/`C`/`D` terminator
/// instead, so they return `None` here.
const fn csi_u_codepoint(key: TerminalKey) -> Option<u32> {
    match key {
        // pi `CODEPOINTS.escape` (`keys.ts:302`).
        TerminalKey::Escape => Some(27),
        // pi `CODEPOINTS.enter` (`keys.ts:304`).
        TerminalKey::Enter => Some(13),
        TerminalKey::Char(c) => Some(c as u32),
        TerminalKey::Up | TerminalKey::Down | TerminalKey::Left | TerminalKey::Right => None,
    }
}

/// The CSI final byte each arrow is carried by — pi's `arrowCodes` map (`keys.ts:614`) read
/// backwards, and the last character of `LEGACY_KEY_SEQUENCES.{up,down,right,left}` (`:369-372`).
const fn arrow_final_byte(key: TerminalKey) -> Option<u8> {
    match key {
        TerminalKey::Up => Some(b'A'),
        TerminalKey::Down => Some(b'B'),
        TerminalKey::Right => Some(b'C'),
        TerminalKey::Left => Some(b'D'),
        _ => None,
    }
}

/// Encode one keystroke as the bytes a tty would have sent — the ENCODE half of the one table the
/// raw-terminal-input seam is defined by (UW-7).
///
/// # Why this exists at all
///
/// The seam carries `data: string` because pi's does: pi's `TUI.handleInput` hands its listeners
/// the chunk it read off the tty, unparsed (`packages/tui/src/tui.ts:773-788` @v0.83.0), and
/// `pi-subagents` matches it with `matchesKey`. cyrup's reader has already parsed those bytes away
/// by the time the app sees an event (`cyrup-tui/src/app/input_reader.rs`, crossterm's
/// `event::poll`/`event::read`), so the bytes must be reconstructed before an extension can be
/// offered the key. The in-tree convention is the same one: the dispatcher's own tests feed
/// `host.terminal_input("\x1b[A")` (`crates/cyrup-ext/src/tests/native_dispatch.rs`).
///
/// # Canonical forms
///
/// ONE form per keystroke, so that a round trip through [`decode_terminal_key`] is the identity:
///
/// | keystroke | emitted |
/// |---|---|
/// | `Up`/`Down`/`Right`/`Left`, no modifiers, press | `\x1b[A` / `\x1b[B` / `\x1b[C` / `\x1b[D` |
/// | an arrow WITH modifiers, or a release | `\x1b[1;<mod+1>A` / `\x1b[1;<mod+1>:3A` (kitty) |
/// | `Escape`, no modifiers, press | `\x1b` |
/// | `Enter`, no modifiers, press | `\r` |
/// | `Char(c)`, no modifiers, press, `c` not a control char | `c` |
/// | everything else | `\x1b[<codepoint>;<mod+1>u` / `…;<mod+1>:3u` (kitty CSI u) |
///
/// The unmodified-press forms are the LEGACY ones — the first entry of pi's own
/// `LEGACY_KEY_SEQUENCES` (`keys.ts:369-372`), `data === "\x1b"` (`:841`), `data === "\r"`
/// (`:921`) and `data === key` (`:1194`) — because those are what a terminal without the kitty
/// protocol actually sends, and what every existing cyrup test and doc already writes.
///
/// # What it refuses
///
/// `None` for anything outside [`TerminalKey`], and for `Char(c)` where `c` is a control
/// character (it would collide with `\r`, `\x1b` or a raw ctrl byte and break the round trip).
/// A caller that gets `None` must deliver the keystroke to the editor WITHOUT consulting the
/// seam: not offering a key is safe, dropping one is not.
#[must_use]
pub fn encode_terminal_key(ev: TerminalKeyEvent) -> Option<String> {
    let legacy = ev.modifiers.is_empty() && !ev.release;
    if let Some(final_byte) = arrow_final_byte(ev.key) {
        if legacy {
            return Some(format!("\x1b[{}", final_byte as char));
        }
        return Some(format!(
            "\x1b[1;{}{}{}",
            ev.modifiers.bits() + 1,
            if ev.release { ":3" } else { "" },
            final_byte as char
        ));
    }
    if legacy {
        return match ev.key {
            TerminalKey::Escape => Some("\x1b".to_string()),
            TerminalKey::Enter => Some("\r".to_string()),
            TerminalKey::Char(c) if !c.is_control() => Some(c.to_string()),
            _ => None,
        };
    }
    if let TerminalKey::Char(c) = ev.key
        && c.is_control()
    {
        return None;
    }
    let codepoint = csi_u_codepoint(ev.key)?;
    Some(format!(
        "\x1b[{codepoint};{}{}u",
        ev.modifiers.bits() + 1,
        if ev.release { ":3" } else { "" }
    ))
}

/// Parse one raw terminal chunk back into a [`TerminalKeyEvent`] — the DECODE half of the same
/// table (UW-7), and the port of the six `matchesKey` arms [`TerminalKey`] names.
///
/// `None` means "not a key in this vocabulary". That is NOT "not a key": a consumer that models
/// pi's catch-all (`fleet-status.ts:752-753`, *"any other key deactivates the roster"*) must treat
/// `None` as "some other key", not as "nothing happened".
///
/// # Alternates accepted, and why each one
///
/// The encoder emits one form; the decoder accepts every form a real terminal plausibly sends for
/// the same key, because the chunk may not have come from [`encode_terminal_key`] at all — a WASM
/// guest, a test, or a future reader that forwards bytes verbatim can all put one on this wire.
/// Each alternate below is one pi accepts in the corresponding `matchesKey` arm:
///
/// * **`\x1b[A`-family (CSI)** — the canonical legacy arrow, `LEGACY_KEY_SEQUENCES` first entry
///   (`keys.ts:369-372`).
/// * **`\x1bOA`-family (SS3)** — the same arrow with the terminal in *application cursor key*
///   mode (DECCKM), which `less`, `vim` and tmux all leave set; pi lists it as the second entry of
///   the same table and `matchesLegacySequence` accepts either.
/// * **`\x1b[1;<mod>A` and `\x1b[1;<mod>:<event>A` (kitty arrows)** — pi's `arrowMatch` regex
///   (`keys.ts:612`). This is the ONLY arrow form that can carry a modifier or a release, which
///   is why the encoder falls back to it.
/// * **`\x1b[<cp>u`, `\x1b[<cp>;<mod>u`, `\x1b[<cp>;<mod>:<event>u` (kitty CSI u)** — pi's
///   `csiUMatch` regex (`keys.ts:598`), the form any terminal with the kitty keyboard protocol
///   negotiated sends for Escape, Enter and printables. The alternate-key fields
///   (`\x1b[<cp>:<shifted>:<base>;<mod>u`, flag 4) are parsed and the *shifted*/*base* members
///   discarded, exactly as `matchesKittySequence`'s primary comparison does — cyrup has no
///   non-Latin-layout remapping to resolve with them, so accepting and ignoring them is what keeps
///   a kitty-flag-4 terminal working rather than silently unmatched.
/// * **`\x1b[27;<mod>;<cp>~` (xterm `modifyOtherKeys`)** — pi's `parseModifyOtherKeysSequence`
///   (`keys.ts:696-702`), the pre-kitty fallback terminals use when `modifyOtherKeys=2` is set.
/// * **`\n` for Enter** — pi `(!_kittyProtocolActive && data === "\n")` (`keys.ts:921`).
/// * **`\x1bOM` for Enter** — SS3 M, numpad Enter on some terminals; pi `:922`.
/// * **kitty codepoint 57414 for Enter** — `CODEPOINTS.kpEnter` (`keys.ts:306`), numpad Enter
///   under the kitty protocol; pi matches it beside `CODEPOINTS.enter` at `:924-925`.
///
/// # What is NOT accepted, deliberately
///
/// `\x1b[a`/`\x1bOa` (the legacy shift/ctrl arrow tables, `keys.ts:394-419`) and the raw control
/// bytes for `ctrl+<letter>`. Both encode a MODIFIED key, and the only consumer of this table
/// treats every modified key as pi's catch-all anyway, so decoding them would add two tables to
/// reach the same answer `None` already gives.
///
/// Bracketed-paste content (`\x1b[200~…`) is refused outright, for pi's own stated reason
/// (`keys.ts:531-535`): pasted text can contain byte patterns that look like key sequences.
#[must_use]
pub fn decode_terminal_key(data: &str) -> Option<TerminalKeyEvent> {
    // pi `isKeyRelease`'s first guard (`keys.ts:531-535`) — and the same reasoning applies to the
    // whole parse, not just the release test: a paste is content, never a keystroke.
    if data.contains("\x1b[200~") {
        return None;
    }
    if let Some(ev) = decode_legacy(data) {
        return Some(ev);
    }
    if let Some(ev) = decode_kitty_arrow(data) {
        return Some(ev);
    }
    if let Some(ev) = decode_csi_u(data) {
        return Some(ev);
    }
    decode_modify_other_keys(data)
}

/// The no-modifier legacy forms: CSI and SS3 arrows, bare `\x1b`, `\r`/`\n`/`\x1bOM`, and a lone
/// printable character.
fn decode_legacy(data: &str) -> Option<TerminalKeyEvent> {
    let key = match data {
        // pi `LEGACY_KEY_SEQUENCES.up` … `.left` (`keys.ts:369-372`), both entries each.
        "\x1b[A" | "\x1bOA" => TerminalKey::Up,
        "\x1b[B" | "\x1bOB" => TerminalKey::Down,
        "\x1b[C" | "\x1bOC" => TerminalKey::Right,
        "\x1b[D" | "\x1bOD" => TerminalKey::Left,
        // pi `data === "\x1b"` (`keys.ts:841`).
        "\x1b" => TerminalKey::Escape,
        // pi `data === "\r" || (!_kittyProtocolActive && data === "\n") || data === "\x1bOM"`
        // (`keys.ts:921-922`).
        "\r" | "\n" | "\x1bOM" => TerminalKey::Enter,
        // pi `data === key` (`keys.ts:1194`) — one printable char and nothing else.
        _ => {
            let mut chars = data.chars();
            let c = chars.next()?;
            if chars.next().is_some() || c.is_control() {
                return None;
            }
            TerminalKey::Char(c)
        }
    };
    Some(TerminalKeyEvent::press(key))
}

/// pi's `arrowMatch`: `^\x1b\[1;(\d+)(?::(\d+))?([ABCD])$` (`keys.ts:612`).
fn decode_kitty_arrow(data: &str) -> Option<TerminalKeyEvent> {
    let body = data.strip_prefix("\x1b[1;")?;
    let (body, final_byte) = split_last_byte(body)?;
    let key = match final_byte {
        b'A' => TerminalKey::Up,
        b'B' => TerminalKey::Down,
        b'C' => TerminalKey::Right,
        b'D' => TerminalKey::Left,
        _ => return None,
    };
    let (modifiers, release) = parse_mod_and_event(body)?;
    Some(TerminalKeyEvent {
        key,
        modifiers,
        release,
    })
}

/// pi's `csiUMatch`: `^\x1b\[(\d+)(?::(\d*))?(?::(\d+))?(?:;(\d+))?(?::(\d+))?u$` (`keys.ts:598`).
/// The two alternate-key fields are parsed off and discarded; see [`decode_terminal_key`].
fn decode_csi_u(data: &str) -> Option<TerminalKeyEvent> {
    let body = data.strip_prefix("\x1b[")?.strip_suffix('u')?;
    let (codepoint_part, mod_part) = match body.split_once(';') {
        Some((cp, m)) => (cp, m),
        None => (body, "1"),
    };
    // `<cp>`, `<cp>:<shifted>`, `<cp>:<shifted>:<base>` or `<cp>::<base>` — only the first field
    // is consulted, matching `matchesKittySequence`'s primary comparison (`keys.ts:670`).
    let codepoint: u32 = codepoint_part.split(':').next()?.parse().ok()?;
    let (modifiers, release) = parse_mod_and_event(mod_part)?;
    let key = key_from_csi_u_codepoint(codepoint)?;
    Some(TerminalKeyEvent {
        key,
        modifiers,
        release,
    })
}

/// pi's `parseModifyOtherKeysSequence`: `^\x1b\[27;(\d+);(\d+)~$` (`keys.ts:696-702`). It carries
/// no event type, so it is always a press.
fn decode_modify_other_keys(data: &str) -> Option<TerminalKeyEvent> {
    let body = data.strip_prefix("\x1b[27;")?.strip_suffix('~')?;
    let (mod_part, codepoint_part) = body.split_once(';')?;
    let modifiers = parse_modifier_field(mod_part)?;
    let codepoint: u32 = codepoint_part.parse().ok()?;
    let key = key_from_csi_u_codepoint(codepoint)?;
    Some(TerminalKeyEvent {
        key,
        modifiers,
        release: false,
    })
}

/// `27` → Escape, `13`/`57414` → Enter (pi `CODEPOINTS.enter`/`.kpEnter`, `keys.ts:304,306`),
/// anything else → the printable it denotes. Control codepoints other than those two are refused,
/// so `\x1b[9;1u` (Tab) reads as "not in this vocabulary" rather than as `Char('\t')`.
fn key_from_csi_u_codepoint(codepoint: u32) -> Option<TerminalKey> {
    match codepoint {
        27 => Some(TerminalKey::Escape),
        13 | 57414 => Some(TerminalKey::Enter),
        _ => {
            let c = char::from_u32(codepoint)?;
            (!c.is_control()).then_some(TerminalKey::Char(c))
        }
    }
}

/// `<mod>` or `<mod>:<event>` — pi's modifier field is 1-INDEXED (`keys.ts:604`, `:616`:
/// `modifier: modValue - 1`), and its event field is `1=press, 2=repeat, 3=release`
/// (`keys.ts:501-504`). A repeat is a press for every purpose this table serves, matching pi,
/// whose `isKeyRelease` tests only for `:3` (`keys.ts:539-548`).
fn parse_mod_and_event(field: &str) -> Option<(TerminalKeyModifiers, bool)> {
    match field.split_once(':') {
        Some((m, event)) => Some((parse_modifier_field(m)?, event == "3")),
        None => Some((parse_modifier_field(field)?, false)),
    }
}

/// The 1-indexed modifier field on its own.
fn parse_modifier_field(field: &str) -> Option<TerminalKeyModifiers> {
    let value: u16 = field.parse().ok()?;
    let bits = value.checked_sub(1)?;
    Some(TerminalKeyModifiers::from_bits(u8::try_from(bits).ok()?))
}

/// Split the trailing ASCII byte off a sequence body. `char_indices` rather than `as_bytes`
/// because the body is `&str`; a multi-byte final char is refused, which is correct — every CSI
/// final byte is ASCII.
fn split_last_byte(body: &str) -> Option<(&str, u8)> {
    let (idx, last) = body.char_indices().next_back()?;
    let byte = u8::try_from(last as u32).ok()?;
    Some((&body[..idx], byte))
}
