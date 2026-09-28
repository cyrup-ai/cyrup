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
#![allow(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::indexing_slicing,
    clippy::panic
)]

use crate::clipboard::{
    ClipboardEnv, ClipboardError, ClipboardWrite, MAX_OSC52_ENCODED_LENGTH, WslRoute,
    clipboard_failure, clipboard_write_plan, headless, osc52_required, osc52_sequence, wsl_route,
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
    assert!(
        !plan.is_empty(),
        "the not(unix) arm used to be a silent no-op"
    );
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
        vec![
            ClipboardWrite::Native,
            ClipboardWrite::Command("pbcopy", &[])
        ],
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
            ClipboardEnv {
                x11_display: true,
                ..bare()
            },
            ClipboardEnv {
                wayland_display: true,
                wayland_session: true,
                ..bare()
            },
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
    let env = ClipboardEnv {
        wayland_session: true,
        x11_display: true,
        ..bare()
    };
    let plan = clipboard_write_plan("linux", &env);
    assert!(
        !plan.contains(&ClipboardWrite::Command("wl-copy", &[])),
        "{plan:?}"
    );
    assert_eq!(
        plan.first(),
        Some(&ClipboardWrite::Command(
            "xclip",
            &["-selection", "clipboard"]
        ))
    );
}

/// Termux goes first when `TERMUX_VERSION` is set (`clipboard.ts:113-121`), with the ordinary
/// Wayland/X11 tools still queued behind it as Pi's `if (!copied)` fallthrough.
#[test]
fn linux_termux_is_tried_before_the_display_server_tools() {
    let env = ClipboardEnv {
        termux: true,
        x11_display: true,
        ..bare()
    };
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

/// `if (!osc52Emitted && (isRemoteSession(env) || (!copied && headless)))` (`clipboard.ts:120`).
/// The remote case is the non-obvious one: a successful LOCAL write over SSH put the text on the
/// wrong machine's clipboard, so the escape — which the terminal emulator forwards to the machine
/// the user is actually sitting at — is emitted anyway.
///
/// **TUI-102.** These four assertions previously pinned `osc52_required(remote, copied)` =
/// `remote || !copied`; they are migrated in place to the three-argument gate, and the two
/// `(false, false)` rows now carry the `headless` conjunct that decides them.
#[test]
fn osc52_is_emitted_when_remote_even_after_a_local_success() {
    assert!(
        osc52_required(true, true, false),
        "remote + copied still emits"
    );
    assert!(
        osc52_required(false, false, true),
        "nothing worked locally and there was no local route to begin with"
    );
    assert!(osc52_required(true, false, false));
    assert!(
        !osc52_required(false, true, false),
        "local success, local session"
    );
}

/// **TUI-102**, the headline defect. `DISPLAY` is set and `xclip`/`xsel` are missing, so the plan
/// produced two failing steps. The old gate was `remote || !copied`, which made `!copied` alone
/// enough: the OSC 52 escape went out, `copied` was forced `true`, and `/copy` reported
/// `copied selection (N chars)` over a clipboard nothing had been written to. Upstream refuses
/// precisely this — "OSC 52 cannot be verified, so a desktop session with a display reports the
/// failure instead (#9618)" (`clipboard.ts:115-117`).
///
/// **Red without the change:** `headless` does not exist (so the file does not compile), and
/// `osc52_required(false, false)` returns `true`.
#[test]
fn tui102_desktop_display_failure_does_not_emit_osc52() {
    let env = ClipboardEnv {
        x11_display: true,
        ..bare()
    };
    assert!(!headless("linux", &env));
    assert!(
        !osc52_required(false, false, headless("linux", &env)),
        "a desktop session with a display must report the failure, not emit an unverifiable escape",
    );
}

/// The other half of the same gate: with no display server at all the terminal IS the only clipboard
/// route (containers, WSL without WSLg), so the escape must still go out (`clipboard.ts:116-117`).
/// Guards against over-narrowing the fix into a blanket "never emit on a local failure".
#[test]
fn tui102_headless_linux_still_emits_osc52() {
    let env = bare();
    assert!(headless("linux", &env));
    assert!(osc52_required(false, false, headless("linux", &env)));
    // Each of pi's three conjuncts, removed in turn, takes the box off the headless path.
    for env in [
        ClipboardEnv {
            x11_display: true,
            ..bare()
        },
        ClipboardEnv {
            wayland_display: true,
            ..bare()
        },
        ClipboardEnv {
            termux: true,
            ..bare()
        },
    ] {
        assert!(!headless("linux", &env), "{env:?}");
    }
    // `p === "linux"` is literal upstream (`clipboard.ts:118`).
    for os in ["macos", "windows", "freebsd"] {
        assert!(!headless(os, &bare()), "{os}");
    }
}

/// The remote clause survives the fix. Included so the gate cannot be "corrected" by collapsing it
/// to `!copied && headless` — over SSH the local write went to the wrong machine, and the box may
/// well have a `DISPLAY` of its own (`clipboard.ts:117`).
#[test]
fn tui102_remote_emits_even_after_a_local_success() {
    assert!(osc52_required(true, true, false));
    let desktop = ClipboardEnv {
        remote: true,
        x11_display: true,
        ..bare()
    };
    assert!(osc52_required(
        desktop.remote,
        true,
        headless("linux", &desktop)
    ));
}

/// **TUI-102.** Pi's throw ladder (`clipboard.ts:124-137`) in its exact precedence order, with every
/// message byte-compared. The old code had one string, `"Failed to copy to clipboard"`, for all five
/// outcomes, so a user with no `xclip` installed was told nothing about what to install.
///
/// **Red without the change:** `ClipboardError` does not exist, so this file does not compile.
#[test]
fn tui102_failure_messages_are_pi_verbatim() {
    assert_eq!(
        clipboard_failure("linux", &bare(), true),
        ClipboardError::Oversized
    );
    assert_eq!(
        ClipboardError::Oversized.message(),
        "Clipboard unavailable: text exceeds the OSC 52 size limit"
    );

    // `oversized` is checked BEFORE the platform block (`clipboard.ts:125-126`), so it wins over a
    // display server that would otherwise name a helper.
    assert_eq!(
        clipboard_failure(
            "linux",
            &ClipboardEnv {
                x11_display: true,
                ..bare()
            },
            true
        ),
        ClipboardError::Oversized,
    );

    // termux → Wayland → X11 (`clipboard.ts:127-135`): the first rung that matches wins, so a
    // Termux box with a `DISPLAY` is still a Termux failure.
    assert_eq!(
        clipboard_failure(
            "linux",
            &ClipboardEnv {
                termux: true,
                x11_display: true,
                ..bare()
            },
            false
        ),
        ClipboardError::Termux,
    );
    assert_eq!(
        ClipboardError::Termux.message(),
        "Clipboard unavailable: install the Termux:API app and `termux-api` package"
    );
    assert_eq!(
        clipboard_failure(
            "linux",
            &ClipboardEnv {
                wayland_display: true,
                x11_display: true,
                ..bare()
            },
            false
        ),
        ClipboardError::Wayland,
    );
    assert_eq!(
        ClipboardError::Wayland.message(),
        "Clipboard unavailable: install `wl-clipboard` (`wl-copy`) or check Wayland access"
    );
    assert_eq!(
        clipboard_failure(
            "linux",
            &ClipboardEnv {
                x11_display: true,
                ..bare()
            },
            false
        ),
        ClipboardError::X11,
    );
    assert_eq!(
        ClipboardError::X11.message(),
        "Clipboard unavailable: install `xclip` or `xsel`, or check X11 access"
    );

    // The fallthrough (`clipboard.ts:137`): the whole platform block is `p === "linux"`, so macOS
    // reaches the bare message even with a display set.
    assert_eq!(
        clipboard_failure("macos", &bare(), false),
        ClipboardError::Unavailable
    );
    assert_eq!(
        clipboard_failure(
            "macos",
            &ClipboardEnv {
                x11_display: true,
                ..bare()
            },
            false
        ),
        ClipboardError::Unavailable,
    );
    assert_eq!(
        clipboard_failure("linux", &bare(), false),
        ClipboardError::Unavailable
    );
    assert_eq!(
        ClipboardError::Unavailable.message(),
        "Clipboard unavailable"
    );
}

/// `headless` reads `wayland_display`, NOT the wider `wayland_session` — pi tests
/// `env.WAYLAND_DISPLAY` alone (`clipboard.ts:118`), so an `XDG_SESSION_TYPE=wayland` box with no
/// socket IS headless upstream, and correctly so: [`clipboard_write_plan`] refuses to queue
/// `wl-copy` for it (asserted just above), leaving the terminal as the only route.
///
/// Pinned as its own test because `wayland_session` is the field a reader reaches for by name, and
/// using it here would suppress the escape on exactly the box that needs it.
#[test]
fn tui102_headless_ignores_a_wayland_session_with_no_socket() {
    let env = ClipboardEnv {
        wayland_session: true,
        wayland_display: false,
        ..bare()
    };
    assert!(
        clipboard_write_plan("linux", &env).is_empty(),
        "no local step exists"
    );
    assert!(
        headless("linux", &env),
        "a Wayland session with no WAYLAND_DISPLAY has no local clipboard route",
    );
    assert!(osc52_required(false, false, headless("linux", &env)));
}

/// **TUI-102.** `if (!copied && p === "linux" && isWSL(env))` and its two routes
/// (`clipboard.ts:110-114`): Windows Terminal gets OSC 52, anything else goes through the PowerShell
/// interop write. cyrup had ZERO WSL symbols — `rg -i wsl crates/cyrup-tui/src/clipboard.rs` was
/// empty — so WSL-without-WSLg fell out of the ladder as a plain "Clipboard unavailable".
///
/// Spawning `powershell.exe` is not exercised here; the decision is. That is the same limit
/// [`clipboard_write_plan`] already accepts for `pbcopy` and `clip`, and the reason both are pure
/// functions over a parameterised `os`.
///
/// **Red without the change:** neither `wsl_route` nor `WslRoute` exists.
#[test]
fn tui102_wsl_without_wslg_routes_to_windows_interop() {
    assert_eq!(
        wsl_route(
            "linux",
            &ClipboardEnv {
                wsl: true,
                wt_session: true,
                ..bare()
            }
        ),
        Some(WslRoute::Osc52),
        "`if (env.WT_SESSION) osc52Emitted = emitOsc52(text)` (clipboard.ts:112)",
    );
    assert_eq!(
        wsl_route(
            "linux",
            &ClipboardEnv {
                wsl: true,
                wt_session: false,
                ..bare()
            }
        ),
        Some(WslRoute::PowerShell),
        "`copied = osc52Emitted || copyViaWindowsClipboard(text)` (clipboard.ts:113)",
    );
    assert_eq!(
        wsl_route("linux", &bare()),
        None,
        "an ordinary Linux box takes no interop route"
    );
    // `p === "linux"` is literal (`clipboard.ts:110`): a stray WSLENV on another platform is inert.
    assert_eq!(
        wsl_route(
            "macos",
            &ClipboardEnv {
                wsl: true,
                wt_session: true,
                ..bare()
            }
        ),
        None
    );
    assert_eq!(
        wsl_route(
            "windows",
            &ClipboardEnv {
                wsl: true,
                ..bare()
            }
        ),
        None
    );

    // WSL without WSLg has no display, so it is also headless — which is what makes the interop arm
    // the only thing between it and a failure.
    assert!(headless(
        "linux",
        &ClipboardEnv {
            wsl: true,
            ..bare()
        }
    ));
}

// ------------------------------------------------------------------ the READ side (DRIFT-045) --

use crate::clipboard::{ClipboardRead, clipboard_read_plan};

/// A Wayland session: `WAYLAND_DISPLAY` set (which also makes `isWaylandSession()` true).
fn wayland() -> ClipboardEnv {
    ClipboardEnv {
        wayland_display: true,
        wayland_session: true,
        ..ClipboardEnv::default()
    }
}

/// **DRIFT-045.** Pi `readClipboardText` (`clipboard.ts:52-69` @v0.84.2) tries `wl-paste` before
/// the native backend, and ONLY on Linux, in a Wayland session, with `WAYLAND_DISPLAY` set — the
/// three-way conjunction at `:53`. The branch is new in this delta (pi `bfc679d5e`).
///
/// **Red before the fix:** neither `clipboard_read_plan` nor `ClipboardRead` existed —
/// `grep -rnE 'read_clipboard_text|wl-paste' crates --include='*.rs'` returned 0 across the
/// workspace — so this test did not compile.
#[test]
fn the_wayland_read_branch_is_gated_on_pis_three_way_conjunction() {
    assert_eq!(
        clipboard_read_plan("linux", &wayland()),
        vec![ClipboardRead::WlPaste, ClipboardRead::Native],
        "`platform() === \"linux\" && isWaylandSession() && WAYLAND_DISPLAY` (clipboard.ts:53)",
    );

    // Each conjunct removed in turn drops the branch and leaves the native read alone.
    let x11 = ClipboardEnv {
        x11_display: true,
        ..ClipboardEnv::default()
    };
    assert_eq!(
        clipboard_read_plan("linux", &x11),
        vec![ClipboardRead::Native]
    );
    let session_but_no_socket = ClipboardEnv {
        wayland_session: true,
        ..ClipboardEnv::default()
    };
    assert_eq!(
        clipboard_read_plan("linux", &session_but_no_socket),
        vec![ClipboardRead::Native],
        "a Wayland session with no WAYLAND_DISPLAY has no socket for wl-paste",
    );
    for os in ["macos", "windows", "freebsd"] {
        assert_eq!(
            clipboard_read_plan(os, &wayland()),
            vec![ClipboardRead::Native],
            "{os}: pi's read gate is the literal `platform() === \"linux\"`",
        );
    }
}

/// The READ gate is deliberately NARROWER than the write side's `[CYRUP-DELTA]`, which widened
/// pi's `p !== "linux"` to "macOS or Windows" so the BSDs would not take a silently-failing native
/// write. There is no such hazard on a read: a failed read yields no text, which is exactly what a
/// missing helper yields, so the literal port is also the safe one. Stated as a test so the two
/// gates are not "made consistent" by a later reader.
#[test]
fn the_read_gate_and_the_write_gate_are_allowed_to_disagree_on_freebsd() {
    assert_eq!(
        clipboard_read_plan("freebsd", &wayland()),
        vec![ClipboardRead::Native]
    );
    assert!(
        !clipboard_write_plan("freebsd", &wayland()).contains(&ClipboardWrite::Native),
        "the write side excludes freebsd from the native step; the read side does not",
    );
}

mod clipboard_paste_tests {
    use crate::InputEvent;
    use crate::UiTheme;
    use crate::app::*;
    use cyrup_session_svc::{InputSource, UserInput};
    use ratatui::backend::TestBackend;
    use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    /// Pi's clipboard paste inserts the materialized temp-file PATH into the editor as ordinary text
    /// (`this.editor.insertTextAtCursor(filePath)`, interactive-mode.ts:2552) — NOT an inline image.
    /// This drives the exact `insert_clipboard_image_path` step the Ctrl+V handler calls, then the
    /// real Enter-submit path, then the `UserInput` the run loop builds from that text — proving the
    /// OUTGOING message the LLM receives is text carrying the path with NO image content block
    /// (`AppAction::Submit` → `UserInput::text`, app.rs:3158; Pi `userContent = [{type:text}]` +
    /// (empty) images, agent-session.ts:1117).
    #[test]
    fn clipboard_paste_inserts_path_as_text_with_no_image_block() {
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();

        // The path a clipboard image is materialized to (mirrors the real `cyrup-clipboard-<uuid>.png`
        // under the OS temp dir; it need not exist on disk — insertion is a pure text edit). On macOS
        // this is `/var/folders/…/T/…`, a leading-slash path — the case that must NOT be mistaken for
        // a slash command on submit.
        let path = std::env::temp_dir().join("cyrup-clipboard-0198f000-test.png");
        let path_str = path.to_string_lossy().to_string();

        app.insert_clipboard_image_path(&path);

        // Pi mechanism: the bare path is now editable text in the buffer …
        assert_eq!(
            app.state().editor.text(),
            path_str,
            "path must land in the editor as text"
        );
        // … and is NOT embedded as an inline image (the former `pending_images` embed is gone), so the
        // potentially-huge raster never floods context.
        assert!(
            app.pending_images().is_empty(),
            "clipboard paste must not embed an image block into pending_images"
        );

        // Real submit path: plain Enter routes editor text → `dispatch_submission` → `AppAction::Submit`
        // (a leading-slash temp path fuzzy-matches no command, so it dispatches as a text prompt).
        let enter = InputEvent::Key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        let action = app.handle_input(&enter);
        assert_eq!(
            action,
            AppAction::Submit(path_str.clone()),
            "the pasted path must submit as a text prompt, not a slash command"
        );

        // The run loop turns that submitted text into the outgoing user message via `UserInput::text`
        // (app.rs:3158): the LLM receives the path AS TEXT with an EMPTY image set. `into_agent_message`
        // then yields a single text content block (Pi's `[{type:text,text}]` + no images).
        let outgoing = UserInput::text(path_str.clone(), InputSource::Tui);
        assert_eq!(
            outgoing.text, path_str,
            "outgoing message text is the pasted path"
        );
        assert!(
            outgoing.images.is_empty(),
            "outgoing user message must carry no image content block"
        );
    }

    /// **DRIFT-045.** Pi `handleClipboardPaste` (`interactive-mode.ts:2870-2892` @v0.84.2) is
    /// image-first, text-second, and the text read is LAZY — `:2882` returns before the
    /// `readClipboardText()` at `:2884` can run.
    ///
    /// **Red before the fix:** `paste_from_clipboard` did not exist and
    /// `try_paste_clipboard_image_path` consulted the image clipboard only, so the
    /// `image absent + text present` case inserted nothing and returned `false` — the whole defect.
    /// (`grep -rnE 'read_clipboard_text|wl-paste' crates --include='*.rs'` returned 0.)
    #[test]
    fn clipboard_paste_is_image_first_text_second_and_the_text_read_is_lazy() {
        let path = std::env::temp_dir().join("cyrup-clipboard-0198f001-test.png");
        let path_str = path.to_string_lossy().to_string();

        // (a) An image on the clipboard wins, and the TEXT read never happens (`:2882` returns).
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        let text_read = std::cell::Cell::new(false);
        let pasted = app.paste_from_clipboard(
            || Some(path.clone()),
            || {
                text_read.set(true);
                Some("clipboard text".to_string())
            },
        );
        assert!(pasted);
        assert_eq!(app.state().editor.text(), path_str);
        assert!(
            !text_read.get(),
            "pi returns at `:2882`; reading the text clipboard anyway is a second system call \
             upstream never makes and would clobber the path on a clipboard holding both"
        );

        // (b) No image, text present → the text is inserted (`:2884-2888`). This is the case that
        // used to insert nothing at all.
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        assert!(app.paste_from_clipboard(|| None, || Some("hello from the clipboard".to_string())));
        assert_eq!(app.state().editor.text(), "hello from the clipboard");

        // (c) Neither → nothing inserted and `false`, so the caller lets Ctrl+V fall through to the
        // editor (a terminal that maps Ctrl+V to a bracketed paste still works).
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        assert!(!app.paste_from_clipboard(|| None, || None));
        assert_eq!(app.state().editor.text(), "");

        // (d) `text || null` (`clipboard.ts:66`): an EMPTY string is falsy upstream, so it must not
        // count as a paste — otherwise Ctrl+V over an empty clipboard would swallow the key.
        let mut app = App::new(TestBackend::new(80, 24), UiTheme::dark()).unwrap();
        assert!(!app.paste_from_clipboard(|| None, || Some(String::new())));
        assert_eq!(app.state().editor.text(), "");
    }
}
