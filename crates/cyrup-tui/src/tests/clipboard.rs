//! The clipboard **write** chain — Pi `copyToClipboard` (`coding-agent/src/utils/clipboard.ts:
//! 73-174`), the writer behind `/copy`.
//!
//! # What was broken
//!
//! `app.rs` carried a target-gated pair: a `#[cfg(unix)]` arm that probed `pbcopy`/`wl-copy`/`xclip`
//! and a `#[cfg(not(unix))]` arm that was `fn copy_to_clipboard(_text: &str) {}` — a total no-op.
//! Neither arm returned anything, so `/copy` printed `copied last message (N chars)` either way. On
//! Windows the user pressed copy, was told it worked, and pasted stale content. Pi has a working
//! `win32` arm (`clipboard.ts:109-110`) and *throws* when every branch fails so `handleCopyCommand`
//! can `showError` (`interactive-mode.ts:6016-6018`). The same file already READ the clipboard on
//! every platform through `arboard`, so cyrup could read a Windows clipboard it could not write.
//!
//! # Why these tests take the platform as a parameter
//!
//! The defect was invisible precisely because it lived behind a `cfg` the CI host never compiled.
//! [`clipboard_write_plan`] takes the target and the environment as arguments, so the Windows chain,
//! the macOS chain and all four Linux chains are asserted from whatever host runs the suite — the
//! only shape of test that could have caught the original.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::indexing_slicing, clippy::panic)]

use crate::clipboard::{
    clipboard_write_plan, osc52_sequence, ClipboardEnv, ClipboardWrite, MAX_OSC52_ENCODED_LENGTH,
};

/// A headless, non-remote desktop with no display server at all.
fn bare() -> ClipboardEnv {
    ClipboardEnv::default()
}

/// THE regression: a non-unix target must have a real write chain, not an empty one. The old
/// `#[cfg(not(unix))]` arm's plan was, in effect, this vector — empty.
#[test]
fn windows_has_a_write_chain_and_it_is_pis() {
    let plan = clipboard_write_plan("windows", &bare());
    assert!(!plan.is_empty(), "the not(unix) arm used to be a silent no-op");
    assert_eq!(
        plan,
        vec![ClipboardWrite::Native, ClipboardWrite::Command("clip", &[])],
        "`clipboard.ts:88` native first (p !== \"linux\"), then `:109-110` execSync(\"clip\")",
    );
}

/// macOS: native addon first, `pbcopy` as the fallback (`clipboard.ts:88`, `:107-108`).
#[test]
fn macos_chain_is_native_then_pbcopy() {
    assert_eq!(
        clipboard_write_plan("macos", &bare()),
        vec![ClipboardWrite::Native, ClipboardWrite::Command("pbcopy", &[])],
    );
}

/// Linux **skips** the native path — Pi's `p !== "linux"` guard (`clipboard.ts:88`), justified at
/// `:82-87`: the native backend does not retain X11 selection ownership after the call resolves, so
/// it reports success while the clipboard stays empty. A plan that began with `Native` here would be
/// the silent-success bug wearing a different hat.
///
/// FreeBSD is covered by the documented `[CYRUP-DELTA]` in `clipboard.rs`: `arboard` serves every
/// unix except macOS from that same X11/Wayland backend, so the exclusion is expressed as "macOS or
/// Windows only" rather than as Pi's literal `!== "linux"`.
#[test]
fn no_x11_platform_uses_the_native_backend() {
    for os in ["linux", "freebsd"] {
        for env in [
            bare(),
            ClipboardEnv { x11_display: true, ..bare() },
            ClipboardEnv { wayland_display: true, wayland_session: true, ..bare() },
        ] {
            let plan = clipboard_write_plan(os, &env);
            assert!(
                !plan.contains(&ClipboardWrite::Native),
                "clipboard.ts:82-92 skips the native addon on X11 platforms; {os} got {plan:?}",
            );
        }
    }
}

/// Wayland with a socket: `wl-copy`, then the X11 pair as the fallback Pi drops to when `wl-copy`
/// exits non-zero (`clipboard.ts:132-149` → `copyToX11Clipboard`, `:12-18`).
#[test]
fn linux_wayland_prefers_wl_copy_then_falls_back_to_x11() {
    let env = ClipboardEnv {
        wayland_display: true,
        wayland_session: true,
        x11_display: true,
        ..bare()
    };
    assert_eq!(
        clipboard_write_plan("linux", &env),
        vec![
            ClipboardWrite::Command("wl-copy", &[]),
            ClipboardWrite::Command("xclip", &["-selection", "clipboard"]),
            ClipboardWrite::Command("xsel", &["--clipboard", "--input"]),
        ],
    );
}

/// A Wayland *session* with no `WAYLAND_DISPLAY` has no socket to talk to, so Pi requires BOTH
/// (`clipboard.ts:126` `if (isWayland && hasWaylandDisplay)`) and otherwise goes straight to X11.
#[test]
fn linux_wayland_session_without_a_socket_does_not_run_wl_copy() {
    let env = ClipboardEnv { wayland_session: true, x11_display: true, ..bare() };
    let plan = clipboard_write_plan("linux", &env);
    assert!(!plan.contains(&ClipboardWrite::Command("wl-copy", &[])), "{plan:?}");
    assert_eq!(plan.first(), Some(&ClipboardWrite::Command("xclip", &["-selection", "clipboard"])));
}

/// Termux goes first when `TERMUX_VERSION` is set (`clipboard.ts:113-121`), with the ordinary
/// Wayland/X11 tools still queued behind it as Pi's `if (!copied)` fallthrough.
#[test]
fn linux_termux_is_tried_before_the_display_server_tools() {
    let env = ClipboardEnv { termux: true, x11_display: true, ..bare() };
    assert_eq!(
        clipboard_write_plan("linux", &env),
        vec![
            ClipboardWrite::Command("termux-clipboard-set", &[]),
            ClipboardWrite::Command("xclip", &["-selection", "clipboard"]),
            ClipboardWrite::Command("xsel", &["--clipboard", "--input"]),
        ],
    );
}

/// A headless Linux box has no local tool at all — the plan is empty and the OSC 52 fallback
/// (`clipboard.ts:166-169`) is the only thing that can copy, which is exactly why the caller must
/// not assume a plan step ran.
#[test]
fn headless_linux_has_no_local_step() {
    assert!(clipboard_write_plan("linux", &bare()).is_empty());
}

/// `emitOsc52` (`clipboard.ts:26-32`): base64 between `ESC ] 52 ; c ;` and `BEL`.
#[test]
fn osc52_wraps_base64_in_the_pi_sequence() {
    let seq = osc52_sequence("hi").unwrap();
    assert_eq!(seq, "\u{1b}]52;c;aGk=\u{7}");
}

/// Past `MAX_OSC52_ENCODED_LENGTH` Pi emits nothing at all (`clipboard.ts:28-30`) — a huge payload
/// desynchronizes terminal rendering.
#[test]
fn osc52_refuses_an_oversized_payload() {
    // 3 raw bytes encode to 4 base64 chars, so this is comfortably over the cap.
    let big = "x".repeat(MAX_OSC52_ENCODED_LENGTH);
    assert!(osc52_sequence(&big).is_none());
    // …and something just under it still encodes.
    let ok = "x".repeat(MAX_OSC52_ENCODED_LENGTH / 8);
    assert!(osc52_sequence(&ok).is_some());
}

/// `if (remote || !copied)` (`clipboard.ts:166`). The remote case is the non-obvious one: a
/// successful LOCAL write over SSH put the text on the wrong machine's clipboard, so the escape —
/// which the terminal emulator forwards to the machine the user is actually sitting at — is emitted
/// anyway.
#[test]
fn osc52_is_emitted_when_remote_even_after_a_local_success() {
    assert!(crate::clipboard::osc52_required(true, true), "remote + copied still emits");
    assert!(crate::clipboard::osc52_required(false, false), "nothing worked locally");
    assert!(crate::clipboard::osc52_required(true, false));
    assert!(!crate::clipboard::osc52_required(false, true), "local success, local session");
}
