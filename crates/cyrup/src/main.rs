//! cyrup — the CLI binary (arch-11 §2.4). The sole `anyhow` boundary and the only binary in the
//! workspace.
//!
//! This file holds three things and nothing else: the entry point, the process-identity syscall,
//! and [`run`] — the **ordered startup sequence**. Every phase of that sequence is a call into the
//! `cyrup` library, so the whole boot reads as one page:
//!
//! | phase | module |
//! | --- | --- |
//! | pre-clap argv routing (internal hops, package/config, credential print) | [`cyrup::predispatch`] |
//! | tracing, HTTP proxy, dirs, settings, first-time setup, `models.json`, catalogs | [`cyrup::bootstrap`] |
//! | arg leniency, extension-flag capture, the `Cli` surface | [`cyrup::cli`] |
//! | run-and-exit actions (`--export`, `--list-models`) | [`cyrup::actions`] |
//! | which session, and under what project trust | [`cyrup::prelaunch`] |
//! | factory + native extensions + runtime launch + post-build knobs | [`cyrup::session_launch`] |
//! | the TTY front end | [`cyrup::interactive`] |
//! | the non-interactive PRINT/JSON/RPC dispatchers | [`cyrup::run`] |
//!
//! What stays here is the ORDER, and the order is the behaviour. Each phase carries the pi
//! `main.ts` line it corresponds to, because several of them are correct only where they sit —
//! PROV-047 above every path that can egress, SEAM-106's `--export` position, DRIFT-007's two
//! catalog phases straddling the `--list-models` exit, `apply_post_build` before pi's `:852`. Read
//! top to bottom, this function is the audit trail for all of it.
//!
//! [`set_process_name`] cannot move into the library: it needs `unsafe` (`prctl(PR_SET_NAME)` /
//! `pthread_setname_np`) and `cyrup`'s lib root is `#![forbid(unsafe_code)]`. That is also why
//! [`cyrup::predispatch`] *classifies* the three internal subcommands and this file dispatches them
//! — each one re-labels the process first (SEAM-070).

use std::io::{self, IsTerminal};
use std::ops::ControlFlow;
use std::process::ExitCode;
use std::sync::Arc;

use clap::Parser;
use cyrup::predispatch::Internal;
use cyrup::session_launch::PostBuild;
use cyrup::{
    AppMode, Cli, Diagnostic, DiagnosticLevel, actions, apply_arg_leniency, bootstrap,
    build_inputs, diagnostics, interactive, migrations, normalize_short_aliases,
    partition_extension_flags, predispatch, prelaunch, render_help, resolve_app_mode,
    run_json_dispatch, run_print_dispatch, run_rpc_dispatch, select_provider, session_launch,
    should_take_over_stdout, spawn_abort_on_signal, timings,
};
use cyrup_config::{AuthStore, EnvVars};
use cyrup_sdk::core::CancelToken;

#[tokio::main(flavor = "multi_thread")]
async fn main() -> ExitCode {
    match run().await {
        Ok(code) => ExitCode::from(code.clamp(0, 255) as u8),
        Err(err) => {
            // The single anyhow boundary: report the full cause chain to stderr, exit non-zero.
            eprintln!("cyrup: {err:#}");
            ExitCode::from(1)
        }
    }
}

/// Set this process's name — pi's `process.title` (`bun/cli.ts:5` and `src/cli.ts:12`
/// `process.title = APP_NAME`; `rpc-entry.ts:6` `process.title = `${APP_NAME}-rpc``). SEAM-070.
///
/// The BASE title is already satisfied for cyrup by accident — a Rust binary's argv\[0\] is `cyrup`,
/// where Node's is `node`, which is the whole reason pi needs the assignment at all. What was
/// genuinely lost is the **role suffix**: pi advertises an RPC-mode process as `pi-rpc`, so an
/// operator can `pkill pi-rpc` or spot a stuck RPC child in `ps` without touching an interactive
/// session. In cyrup an rpc-mode process, a `__subagent-runner` child and an `__intercom-broker`
/// child all appeared as plain `cyrup`.
///
/// This is a syscall against the CURRENT process, not a mutation of the shared environment, so the
/// `std::env::set_var`-is-`unsafe` rationale that used to gate the whole identity block here does
/// not cover it — that rationale applies only to the `PI_CODING_AGENT` half of the same pi
/// statement, which is TOOL-031 / PARITY-GAPS PB-5 and is deliberately still not done here.
///
/// CYRUP-DELTA — reach. On Linux `prctl(PR_SET_NAME)` is what `ps -o comm=` reads, so the item's
/// verification works verbatim. On macOS there is no supported way to change what `ps -o comm=`
/// prints (it reports the executable path); `pthread_setname_np` is the closest equivalent and is
/// what shows up in Activity Monitor, `ps -M` and a debugger. Best-effort on both: a failure is
/// silent, exactly as a `process.title` assignment that the OS truncates is silent upstream.
fn set_process_name(name: &str) {
    let Ok(c_name) = std::ffi::CString::new(name) else {
        return;
    };
    #[cfg(target_os = "linux")]
    // SAFETY: `prctl(PR_SET_NAME, ptr)` reads at most 16 bytes from `ptr`, which points at a live
    // NUL-terminated `CString` owned by this frame, and mutates only this process's own name.
    unsafe {
        libc::prctl(libc::PR_SET_NAME, c_name.as_ptr());
    }
    #[cfg(target_os = "macos")]
    // SAFETY: the macOS `pthread_setname_np` takes only the name and applies it to the CALLING
    // thread; `c_name` is a live NUL-terminated buffer owned by this frame.
    unsafe {
        libc::pthread_setname_np(c_name.as_ptr());
    }
    #[cfg(not(any(target_os = "linux", target_os = "macos")))]
    let _ = c_name;
}

/// The live-keyboard probe `cyrup_tui::native_modifiers` cannot host: reading it needs `unsafe`, and
/// that crate is `#![forbid(unsafe_code)]`. Here for the same reason [`set_process_name`] is, and it
/// is registered from [`run`] beside it.
///
/// Ports `pi/packages/tui/native/darwin/src/darwin-modifiers.c` (@v0.84.1) — the same four names
/// against the same four masks (`:23-28`), read from the same source state (`:46`). Upstream reaches
/// this through a prebuilt N-API addon and answers "no modifier pressed" whenever the addon is
/// missing (`native-modifiers.ts:39-52`); a direct framework link cannot be missing, so the only
/// hosts without a probe are the ones this module is not compiled for.
///
/// Upstream also skips the helper on any arch that is not `x64`/`arm64` (`native-modifiers.ts:24`).
/// That is a constraint of shipping prebuilt binaries, not a behaviour, and is deliberately not
/// ported.
#[cfg(target_os = "macos")]
mod native_modifier_probe {
    use cyrup_tui::ModifierKey;

    /// `kCGEventSourceStateCombinedSessionState`. `CGEventSourceStateID` is a SIGNED 32-bit enum —
    /// `objc2-core-graphics` generates it as `CGEventSourceStateID(pub i32)`, and its
    /// `kCGEventSourceStatePrivate = -1` settles the question.
    const COMBINED_SESSION_STATE: i32 = 0;
    /// The four `kCGEventFlagMask*` values `darwin-modifiers.c:24-27` selects between.
    const FLAG_MASK_SHIFT: u64 = 0x0002_0000;
    const FLAG_MASK_CONTROL: u64 = 0x0004_0000;
    const FLAG_MASK_ALTERNATE: u64 = 0x0008_0000;
    const FLAG_MASK_COMMAND: u64 = 0x0010_0000;

    #[link(name = "CoreGraphics", kind = "framework")]
    unsafe extern "C" {
        /// `CGEventFlags CGEventSourceFlagsState(CGEventSourceStateID stateID)` — `CGEventFlags` is
        /// `uint64_t`. Bound under a snake-case name so the declaration needs no lint suppression.
        #[link_name = "CGEventSourceFlagsState"]
        fn cg_event_source_flags_state(state_id: i32) -> u64;
    }

    /// `isModifierPressed` — `darwin-modifiers.c:31-55`.
    pub(super) fn probe(key: ModifierKey) -> bool {
        let mask = match key {
            ModifierKey::Shift => FLAG_MASK_SHIFT,
            ModifierKey::Control => FLAG_MASK_CONTROL,
            ModifierKey::Option => FLAG_MASK_ALTERNATE,
            ModifierKey::Command => FLAG_MASK_COMMAND,
        };
        // SAFETY: `CGEventSourceFlagsState` takes one integer state id by value and returns a
        // bitmask by value. It borrows no memory from this frame, mutates nothing in this process,
        // and reads only the window server's live modifier state, so there is no pointer to keep
        // valid and no aliasing to uphold. It is safe to call from any thread at any time.
        let flags = unsafe { cg_event_source_flags_state(COMBINED_SESSION_STATE) };
        flags & mask != 0
    }
}

/// The Windows half of the same seam, ported from
/// `pi/packages/tui/native/win32/src/win32-console-mode.c` (@v0.84.1): `GetAsyncKeyState` against the
/// generic and sided virtual-key codes for each modifier (`:57-61`), tested with pi's own
/// `KEY_PRESSED_MASK` of `0x8000` (`:8`, `:54`).
///
/// **This cannot fire in cyrup's current configuration, and is registered anyway.** The reason it
/// cannot is recorded in full on `cyrup_tui::should_detect_native_shift_enter`: upstream's helper
/// puts the console into `ENABLE_VIRTUAL_TERMINAL_INPUT` mode and so must recover modifiers that
/// crossterm — which never sets that flag — still has, and hands over as `KeyModifiers::SHIFT`
/// before the rescue's `modifiers != NONE` guard. That analysis is a source read performed on Linux,
/// never observed on Windows, and it holds only while nothing enables VT input. If any of that
/// changes, a registered probe is the difference between Shift+Enter inserting a newline and
/// submitting the message; an unregistered one is a silent regression for every Windows user.
#[cfg(windows)]
mod native_modifier_probe {
    use cyrup_tui::ModifierKey;

    /// `KEY_PRESSED_MASK` — the high bit of `GetAsyncKeyState`'s `SHORT` means "currently down".
    const KEY_PRESSED_MASK: u16 = 0x8000;

    /// Virtual-key codes, values corroborated against `windows-sys`'s
    /// `Win32::UI::Input::KeyboardAndMouse`.
    const VK_SHIFT: i32 = 16;
    const VK_CONTROL: i32 = 17;
    const VK_MENU: i32 = 18;
    const VK_LWIN: i32 = 91;
    const VK_RWIN: i32 = 92;
    const VK_LSHIFT: i32 = 160;
    const VK_RSHIFT: i32 = 161;
    const VK_LCONTROL: i32 = 162;
    const VK_RCONTROL: i32 = 163;
    const VK_LMENU: i32 = 164;
    const VK_RMENU: i32 = 165;

    #[link(name = "user32")]
    unsafe extern "system" {
        /// `SHORT WINAPI GetAsyncKeyState(int vKey)` — `WINAPI` is `__stdcall`, which is what
        /// `extern "system"` selects on 32-bit Windows and plain C elsewhere. Bound under a
        /// snake-case name so the declaration needs no lint suppression.
        #[link_name = "GetAsyncKeyState"]
        fn get_async_key_state(v_key: i32) -> i16;
    }

    /// `is_key_pressed` — `win32-console-mode.c:52-55`, including its cast through `unsigned short`
    /// before the mask.
    fn is_key_pressed(virtual_key: i32) -> bool {
        // SAFETY: `GetAsyncKeyState` takes one integer by value and returns one by value. It borrows
        // no memory from this frame, mutates nothing in this process, and reads only the OS's live
        // keyboard state. It is safe to call from any thread at any time, and on any value of
        // `virtual_key` — an out-of-range code simply reports "not pressed".
        let state = unsafe { get_async_key_state(virtual_key) };
        (state as u16) & KEY_PRESSED_MASK != 0
    }

    /// `is_modifier_name_pressed` — `win32-console-mode.c:57-63`, the same generic-plus-sided groups.
    pub(super) fn probe(key: ModifierKey) -> bool {
        let keys: &[i32] = match key {
            ModifierKey::Shift => &[VK_SHIFT, VK_LSHIFT, VK_RSHIFT],
            ModifierKey::Control => &[VK_CONTROL, VK_LCONTROL, VK_RCONTROL],
            ModifierKey::Option => &[VK_MENU, VK_LMENU, VK_RMENU],
            // Upstream tests only the two sided Win keys here — there is no generic `VK_WIN`.
            ModifierKey::Command => &[VK_LWIN, VK_RWIN],
        };
        keys.iter().copied().any(is_key_pressed)
    }
}

async fn run() -> anyhow::Result<i32> {
    // Process identity, first half — pi `cli.ts:12` `process.title = APP_NAME` (SEAM-070). The
    // rpc-mode suffix (pi's separate `rpc-entry.ts:6`) is applied once the mode is resolved, below.
    //
    // The SECOND half of pi's statement, `process.env.PI_CODING_AGENT = "true"` (`cli.ts:13`), is
    // still NOT replicated: `std::env::set_var` is `unsafe` under edition 2024 because the env is
    // not thread-safe to mutate once the runtime has spawned threads. That is a real hazard and a
    // real gate; it never applied to the process NAME, which is a syscall on this process alone —
    // conflating the two is how SEAM-070 stayed unfiled. The env half is TOOL-031 / PARITY-GAPS
    // PB-5 (area 04).
    set_process_name("cyrup");

    // The Shift+Enter rescue's one missing link (UW-1). `cyrup_tui` wires the whole chain —
    // `app/input_reader.rs` calls `rescue_native_shift_enter` with the live platform and
    // `TERM_PROGRAM` — but `is_native_modifier_pressed` answers `false` until something registers a
    // probe, so on a terminal that encodes nothing on Enter (Apple Terminal sends one `\r` for both
    // Enter and Shift+Enter) the rescue could never fire and Shift+Enter submitted the message.
    //
    // Registration is NOT gated on `TERM_PROGRAM`: that gate is upstream's and lives inside
    // `should_detect_native_shift_enter`, which reads the env per keystroke. Gating here as well
    // would freeze the decision to whatever the variable held at process start.
    #[cfg(any(target_os = "macos", windows))]
    cyrup_tui::set_native_modifier_probe(native_modifier_probe::probe);

    // Pi `resetTimings()` at the top of `main()` (main.ts:474). The namespace is explicit here
    // because the table is process-global, exactly as pi's module-level Map is (AGENT-027).
    timings::reset_timings(timings::TimingLabel::Main);

    // Pi rewrites its multi-char short aliases in its hand-rolled parser; clap cannot express them as
    // native shorts, so normalize them up front (`-nt` ⇒ `--no-tools`, …).
    let raw: Vec<String> = normalize_short_aliases(std::env::args());
    let argv: Vec<String> = raw.iter().skip(1).cloned().collect();

    // The three internal, never-advertised subcommands — `__subagent-runner --config <path>`
    // (arch-SA §2.2/§6.5), `__intercom-broker` (cyrup-intercom-port.md §7.3) and
    // `__mcp-keyring-helper` (13f-mcp-credentials MCP-260). All three MUST be recognized before ANY
    // user-facing arg leniency/clap parsing and before the package/config pre-dispatch below, which
    // has no knowledge of them. `cyrup::predispatch` classifies; the naming + dispatch is here
    // because `set_process_name` is `unsafe` and cannot live in the library (SEAM-070: a distinct
    // role name is what makes a stuck child findable in `ps`, the same reason pi gives its rpc child
    // its own `process.title`).
    //
    // The keyring-helper arm is load-bearing *where it sits*, not merely early: its whole contract
    // is one line of JSON in on stdin and one line of JSON out on stdout, and it "must not
    // initialise the cache, the config or tracing" (`cyrup_mcp::credentials::run_keyring_helper`).
    // Returning from here keeps it above the bootstrap HTTP-proxy install and above
    // `run_predispatch`, so nothing can log a secret or put a stray byte on the stdout the parent
    // process is parsing. It is also the one arm that is synchronous — there is nothing to await.
    match predispatch::classify_internal(&raw) {
        Some(Internal::SubagentRunner) => {
            set_process_name("cyrup-subagent");
            return Ok(cyrup::subagent_runner_cmd::dispatch(&raw).await);
        }
        Some(Internal::IntercomBroker) => {
            set_process_name("cyrup-broker");
            return Ok(cyrup::intercom_broker_cmd::dispatch().await);
        }
        Some(Internal::McpKeyringHelper) => {
            // `PR_SET_NAME` caps a name at 16 bytes including the NUL, so `ps -o comm=` shows this
            // one as `cyrup-mcp-keyri` — still distinct from `cyrup`, `cyrup-broker` and
            // `cyrup-subagent`, which is all SEAM-070 asks of it. Silent truncation is the
            // documented contract of `set_process_name`, not a defect to route around.
            set_process_name("cyrup-mcp-keyring");
            return Ok(cyrup::mcp_keyring_helper_cmd::dispatch());
        }
        None => {}
    }

    // PROV-047 — the BOOTSTRAP `httpProxy` install (pi main.ts:536-538). It sits HERE, above the
    // package/config pre-dispatch (:541), above the credential-print pre-dispatch (:557) and above
    // `parseArgs` (:562), because every one of those can egress before a session exists.
    bootstrap::install_bootstrap_http_proxy();

    // Package/config subcommands (pi main.ts:486) then `auth print-api-key|print-bearer-token` (pi
    // main.ts:557-559) — both before `parseArgs`, in pi's order.
    if let Some(code) = predispatch::run_predispatch(&argv).await? {
        return Ok(code);
    }

    // Pi-faithful arg leniency (args.ts:80-82,131-139,202-203) BEFORE clap: a bad `--mode` is
    // silently dropped, a bad `--thinking` warns + drops, and an unknown single-dash option becomes a
    // Pi `Unknown option` error (exit 1) rather than a clap usage error (exit 2).
    let (lenient_argv, parse_diagnostics) = apply_arg_leniency(&argv);

    // Capture unknown `--flag[=val]` as extension flags before clap (Pi args.ts:188-201), then parse
    // the cleaned argv and stitch the captured flags back onto the struct.
    let (clean_argv, extension_flags) = partition_extension_flags(&lenient_argv);
    let mut clap_argv = vec![raw.first().cloned().unwrap_or_else(|| "cyrup".to_string())];
    clap_argv.extend(clean_argv);
    let mut cli = Cli::parse_from(&clap_argv);
    cli.extension_flags = extension_flags;
    // Trim each comma-split segment of `--models`/`--tools`/`--exclude-tools` and drop empty
    // tool/exclude-tool names, matching Pi's post-split normalization (args.ts:114,120-129). clap's
    // `value_delimiter = ','` splits but never trims, so `--tools "read, grep"` would otherwise keep
    // `" grep"` and silently drop the tool. Run before any consumer reads these Vecs.
    cli.normalize_list_flags();
    // SEAM-107: put back the literal spelling of a `-p ---…` escape-hatch message (pi
    // args.ts:140-146). Must run before anything reads `positionals` — `--export`'s output path and
    // the RPC `@file` guard both do, below.
    cli.restore_escaped_positionals();
    bootstrap::init_tracing(cli.verbose);
    timings::time("parseArgs", timings::TimingLabel::Main);

    // Report parse diagnostics (Pi main.ts:504-512): warnings + errors to stderr, any error exits 1.
    diagnostics::report(&parse_diagnostics);
    if parse_diagnostics
        .iter()
        .any(|d| d.level == DiagnosticLevel::Error)
    {
        return Ok(1);
    }

    // `--version` (Pi main.ts:573-576): `console.log(VERSION); process.exit(0);` — a BARE semver,
    // no program name — and, decisively, AFTER the diagnostics gate above, so `cyrup -x --version`
    // reports `Unknown option: -x` and exits 1 exactly as pi does. SEAM-052.
    if cli.version {
        println!("{}", env!("CARGO_PKG_VERSION"));
        return Ok(0);
    }

    // `--export <session> [out]` (Pi main.ts:578-590) — a standalone action that runs and exits.
    //
    // SEAM-106: this must stay UPSTREAM of four guards pi runs it upstream of —
    // `validateForkFlags`/`validateSessionIdFlags` (pi `:603-604`), the RPC `@file` guard
    // (`:598-601`) and the `--api-key requires a model` bail (`:757-761`). Export is what a user
    // reaches for when the session is ALREADY in a bad state, so those guards fired on exactly the
    // invocations that need it most. Upstream's only predecessors are `parseArgs` + the diagnostics
    // gate (`:562-570`) and the `--version` exit (`:573-576`), which are the two blocks above.
    //
    // The optional output path is pi's `parsed.messages[0]` (`:580`) — the MESSAGE list, from which
    // `@file` tokens have already been partitioned away (args.ts:186-187).
    if let Some(export) = &cli.export {
        let messages = cyrup::split_positionals(&cli.positionals).1;
        return actions::export_session_html(export, messages.first().map(String::as_str)).await;
    }

    // Rich `--help` body (Pi printHelp, args.ts:212). Loaded-extension flags are the outer extension
    // tier; the bin injects an empty set today (the injection point is preserved 1:1).
    if cli.help {
        print!("{}", render_help(&[]));
        return Ok(0);
    }

    // Conflicting-session-flag diagnostics (Pi `validateForkFlags`/`validateSessionIdFlags`).
    if let Err(msg) = cli.validate_session_flags() {
        anyhow::bail!("{msg}");
    }

    let stdin_tty = io::stdin().is_terminal();
    let stdout_tty = io::stdout().is_terminal();
    let mode = resolve_app_mode(&cli, stdin_tty, stdout_tty);
    // pi gives its RPC host its OWN entry point with its own title — `process.title =
    // `${APP_NAME}-rpc`` (rpc-entry.ts:6) — so an rpc child is distinguishable from an interactive
    // session in `ps`. cyrup has one entry point, so the suffix is applied here, at the first point
    // the resolved mode is known. SEAM-070.
    if mode == AppMode::Rpc {
        set_process_name("cyrup-rpc");
    }

    // Stdout takeover (Pi main.ts:535-537): for a non-interactive run that is not a plain-metadata
    // command, install the guard so every *incidental* stdout write between here and the protocol
    // stream — a `runMigrations` notice, `createSessionManager`'s cross-project "Session found in
    // different project" hint — is rerouted to stderr (via `emit_stray_line`) instead of corrupting
    // the PRINT/JSON/RPC stream on stdout. The protocol writers keep writing to real stdout (their
    // injected `io::stdout()` sink is the analog of Pi's `writeRawStdout`).
    if should_take_over_stdout(&cli, mode) {
        cyrup::output_guard::take_over_stdout();
    }

    // `@file` is unsupported in RPC mode (Pi main.ts:540-543).
    if mode == AppMode::Rpc && !cyrup::input::split_file_args(&cli).is_empty() {
        anyhow::bail!("@file arguments are not supported in RPC mode");
    }

    // Resolve directories (CLI > env > default; the only place env is read). `--session-dir`,
    // `--offline`, `--api-key`, `--model(s)` thread through `CliConfigOverrides`.
    let env = EnvVars::from_process();
    let (overrides, dirs) = bootstrap::resolve_dirs(&cli, &env)?;

    // One-time startup migrations (Pi `runMigrations(cwd)`, main.ts:549): legacy auth/session/tools
    // moves + extension-system deprecation warnings.
    let migration = migrations::run_migrations(&dirs);
    timings::time("runMigrations", timings::TimingLabel::Main);

    // Pi's `startupSettingsManager` (main.ts:610-611), created after the migrations and used for
    // exactly two things: surfacing settings load/parse errors as warnings, and the `sessionDir`
    // lookup below. One manager, both jobs — as upstream.
    let (mut startup_settings, settings_diagnostics) = bootstrap::load_startup_settings(&dirs);
    diagnostics::report(&settings_diagnostics);

    // Experimental first-time setup — Pi main.ts:615-617, verbatim position: between
    // `startupSettingsManager` (`:610`) and the `sessionDir` tier chain (`:625-630`), for pi's stated
    // reason at `:613-614` — "Runs before any runtime services are created so the chosen settings
    // apply everywhere". Wired per ADR-0011.
    if bootstrap::maybe_run_first_time_setup(mode, &cli, &dirs, &env, &mut startup_settings).await?
    {
        timings::time("firstTimeSetup", timings::TimingLabel::Main);
    }

    // `sessionDir` tier 3 (Pi main.ts:625-630): CLI `--session-dir` > `$CYRUP_SESSION_DIR` >
    // `startupSettingsManager.getSessionDir()` (settings-manager.ts:670-673). `ConfigDirs::resolve`
    // folded in the first two tiers; the settings tier has to be applied out here because the
    // settings file lives under the `agent_dir` that `resolve` itself computes — the same reason Pi
    // builds its startup manager only after the dirs exist. A settings-derived dir counts as
    // EXPLICIT: Pi hands it to `createSessionManager(parsed, cwd, sessionDir, …)` (main.ts:630)
    // through the same argument slot as `--session-dir`, so it is used literally rather than
    // cwd-encoded, and `session_list_layout`/`Cli::to_session_config` key off that flag.
    let dirs = cyrup::apply_settings_session_dir(dirs, &startup_settings);

    // `--name` must be non-empty after trim (Pi main.ts:586-592).
    let session_name = cli.validated_name().map_err(|m| anyhow::anyhow!("{m}"))?;

    // `--api-key` requires a resolvable model spec (Pi main.ts:701-710): without any of
    // `--model`/`--provider`/`--models` there is no provider to attach the key to.
    if cli.api_key.is_some()
        && cli.model.is_none()
        && cli.provider.is_none()
        && cli.models.is_empty()
    {
        anyhow::bail!(
            "--api-key requires a model to be specified via --model, --provider/--model, or --models"
        );
    }

    // (`--export` is the other standalone run-and-exit action; it dispatches far above, at pi's own
    // position immediately after the `--version` exit — SEAM-106.)

    // `<agent_dir>/models.json` — the user's custom-provider / custom-model file (CFG-002). Loaded
    // HERE, before `--list-models` and before provider selection, or a declared provider is
    // unlistable and unlaunchable. Never fatal (pi model-config.ts:251).
    let (models_json, models_json_warnings) = bootstrap::load_models_json(&dirs);
    if !models_json_warnings.is_empty() {
        diagnostics::report(&models_json_warnings);
    }

    // Runtime model-catalog overlay, phase 1 (DRIFT-007) — Pi's cache-only restore
    // `await modelRuntime.refresh({ allowNetwork: false })` (agent-session-services.ts:180). Disk
    // only, NO network I/O. It sits HERE, beside the `models.json` load and above the
    // `--list-models` exit, because that is where Pi has it (its listing exit is main.ts:816,
    // downstream of runtime creation), so `cyrup --list-models` renders the persisted overlay.
    // Phase 2 (the network revalidation) stays downstream at its own site — a listing run therefore
    // shows the cache and issues no request.
    cyrup::provider::restore_model_catalog(&dirs).await;

    // `--list-models` enumerates the multi-provider registry (Pi `modelRuntime.getAvailable()`,
    // list-models.ts:35) — independent of `--provider`/`--model`, and resolved BEFORE provider
    // selection, so a `--provider <unknown>` does not gate the listing (matching Pi).
    if let Some(search) = &cli.list_models {
        return actions::list_models_action(&dirs, &models_json, search);
    }

    let mut provider = select_provider(
        cli.provider.as_deref(),
        cli.model.as_deref(),
        cli.api_key.as_deref(),
        &models_json,
    )?;

    // Unknown-model diagnostic (Pi `resolveCliModel`, main.ts:377-378 / model-resolver.ts:494-500):
    // a `--model` on a *known* provider whose id is not in the catalog warns (the build still proceeds
    // with a custom-id model). An *unknown provider* already errored in `select_provider` above.
    if let Some(warning) = cyrup::unknown_model_warning(
        cli.provider.as_deref(),
        cli.model.as_deref(),
        &cyrup::provider::all_available_models(&models_json),
    ) {
        diagnostics::report(&[Diagnostic::warning(warning)]);
    }

    // Map CLI → SessionConfig. The diagnostics half is Pi's `resolvePromptInput` warning channel
    // (resource-loader.ts:60-63): a `--system-prompt`/`--append-system-prompt` token that names an
    // EXISTING but unreadable file warns and falls back to being used as literal text — never fatal.
    let (mut config, prompt_diagnostics) = cli.to_session_config_with_diagnostics(&dirs, mode);
    diagnostics::report(&prompt_diagnostics);
    // CFG-003: a package declared in `settings.json` whose working tree is missing is CLONED during
    // session assembly, exactly as Pi's resource loader does — `packageManager.resolve()` with no
    // `onMissing` (resource-loader.ts:403,549 @v0.83.0) reaches `installMissing`
    // (package-manager.ts:1260-1271), which installs unless `isOfflineModeEnabled()` (`:42-46`).
    // That predicate is `PI_OFFLINE` upstream and `--offline`/`CYRUP_OFFLINE`/`PI_OFFLINE` here,
    // already folded into `overrides.offline` above. It is the ONLY gate upstream has, so no
    // settings key or extra flag is invented for it.
    config.install_missing_packages = !overrides.offline;

    // Non-interactive session-resolution depth (Pi `createSessionManager`, main.ts:254-350): a
    // `--session`/`--fork` partial-UUID prefix match, a global cross-project search, a
    // `--session-id` create-if-missing-by-exact-id, the plain-stdin fork-into-cwd confirm, and the
    // non-interactive missing-session-cwd guard. Engaged only when a session ref is supplied — the
    // bare `New`/`Continue` target from `to_session_config` stands otherwise (no needless listing).
    if (cli.fork.is_some() || cli.session.is_some() || cli.session_id.is_some())
        && let Some(code) = prelaunch::resolve_session(&cli, &dirs, mode, &mut config).await?
    {
        return Ok(code);
    }

    // Pre-launch startup-UI orchestration (Pi `cli/startup-ui.ts` + `cli/session-picker.ts`): the
    // `--resume` picker runs over the cyrup-tui selectors BEFORE the runtime is built.
    // Interactive-only (it needs a real TTY); the one-shot/RPC live path is untouched. Returns
    // `Some(0)` when the user cancels the picker.
    if mode == AppMode::Interactive
        && let Some(code) = prelaunch::resolve_startup_ui(&cli, &dirs, mode, &mut config).await?
    {
        return Ok(code);
    }

    // AGENT-027 — pi's `time("createSessionManager")` (main.ts:652) closes the block that resolves
    // WHICH session this run attaches to (its `createSessionManager`, `:254-350`, plus the `--name`
    // append at `:650`). cyrup's counterpart is the pair above, after which `config.target` is final.
    timings::time("createSessionManager", timings::TimingLabel::Main);

    let deprecation_warnings = migration.deprecation_warnings.clone();
    let settings_store = cyrup::file_settings_store(&dirs);

    // Runtime model-catalog overlay, phase 2 (DRIFT-007) — pi's detached, mode-gated
    // `void modelRuntime.refresh()` (main.ts:863-866). Downstream of the `--list-models` exit, like
    // pi's, so a listing run issues no request. Nothing is awaited.
    bootstrap::maybe_spawn_catalog_refresh(mode, &dirs, &env, &overrides, &settings_store);

    let cancel = CancelToken::new();

    // Default-launch model (Pi `findInitialModel`, model-resolver.ts:527-607): with NEITHER
    // `--provider` nor `--model` (nor a `--models` scope) on a FRESH session, upgrade the zero-model
    // `UnconfiguredProvider` that `select_provider` yielded to the resolved default, and set the
    // matching `model_pattern` so the builder launches on that exact model. A no-op when nothing is
    // configured — the empty catalog then stands and `resolve_model` yields `model: None` +
    // `modelFallbackMessage` (SEAM-075).
    if let Some((launch_provider, launch_pattern)) =
        bootstrap::resolve_default_launch_model(&cli, &dirs, &config, &models_json, &settings_store)
    {
        provider = select_provider(Some(&launch_provider), None, None, &models_json)?;
        config.model_pattern = Some(launch_pattern);
    }

    // `--api-key` installs a RUNTIME credential on the same credential store the session's model
    // runtime reads (Pi `modelRuntime.setRuntimeApiKey(...)`, main.ts:764 → model-runtime.ts:400-418).
    // cyrup handed the key only to `select_provider`'s throwaway `InMemoryCredentialStore`, so the
    // session's `AuthStore` — the one behind `hasConfiguredAuth`, `getProviderAuthStatus` and
    // `/logout`'s `listCredentials()` — never saw it. Building the store here (instead of letting
    // `SessionBuilder` default it) is what lets the key be installed on it.
    //
    // The default-launch block above cannot have swapped `provider` out from under this: `--api-key`
    // is rejected earlier unless one of `--model`/`--provider`/`--models` is present, and that block
    // runs only when all three are absent.
    let auth_store = Arc::new(AuthStore::at(dirs.agent_dir.join("auth.json")));
    if let Some(api_key) = cli.api_key.as_deref() {
        auth_store.set_runtime_api_key(provider.id().clone(), api_key.to_string());
    }

    // Interactive mode drives the **multi-session** `AgentSessionRuntime` (arch-11 §3.4) so the
    // session-swap commands rebuild the active session in place and the TUI re-binds to it. The
    // one-shot/RPC modes keep the single fixed `AgentSession` seam unchanged.
    if let AppMode::Interactive = mode {
        let target = config.target.clone();
        let fresh = cyrup::session_resolve::is_fresh_target(&target);
        // SEAM-065: trust is resolved INSIDE the build, in pi's tier order, so the extension
        // `project_trust` hook runs BEFORE the human is asked (project-trust.ts:54-70 vs :90-94).
        // The prompt callback is supplied for the interactive host only — pi's `hasUI` gate (:86-88).
        let factory = session_launch::build_factory(
            provider,
            config,
            settings_store.clone(),
            auth_store.clone(),
            &dirs,
            models_json.clone(),
            Some(prelaunch::trust_prompt_callback(&dirs)),
        )?;
        // SEAM-075: `require_model: false`. pi gates its modelless stop on the MODE
        // (main.ts:852-855), so a credential-less first run still gets a TUI to type `/login` and
        // then `/model` into; the banner is shown inside `run_interactive`.
        let (runtime, session) = match session_launch::launch(
            factory,
            target,
            PostBuild {
                session_name: session_name.as_deref(),
                cli: &cli,
                fresh,
                require_model: false,
            },
        )
        .await?
        {
            ControlFlow::Break(code) => return Ok(code),
            ControlFlow::Continue(pair) => pair,
        };
        // pi marks the scoped-`--models` resolution separately (`time("resolveModelScope")`,
        // main.ts:842); in cyrup that work happens inside `session_launch::apply_post_build`.
        timings::time("resolveModelScope", timings::TimingLabel::Main);
        // SEAM-033 — announce NOW, after `--name`/`--models`, which is where pi's interactive host
        // does it (its own `bindExtensions` inside `InteractiveMode`, the sibling of
        // print-mode.ts:73 / rpc-mode.ts:319). Idempotent per session.
        session.bind_extensions().await;
        // (The migrated-credential notice and the `modelFallbackMessage` warning are NOT printed
        // here — pi renders both INSIDE the running UI, interactive-mode.ts:874-876 and :883-884. A
        // pre-TUI `eprintln!` put them exactly where the first frame paints over them. See
        // `cyrup::interactive::run_interactive`. CFG-051.)
        // Show extension-system deprecation warnings and BLOCK on a keypress before the TUI takes
        // the terminal — Pi main.ts:838-840. This arm is already the interactive one, so pi's mode
        // guard is structural here. Printing without the gate (CFG-049) put the only notice that a
        // legacy `hooks/` directory has stopped loading one frame ahead of the paint that erases it.
        migrations::show_deprecation_warnings(&deprecation_warnings);
        let _signals = spawn_abort_on_signal(runtime.clone(), cancel.clone(), AppMode::Interactive);
        // `PI_STARTUP_BENCHMARK` interactive run path (Pi main.ts:819-835): init the TUI, let stdin
        // drain terminal query replies for ~150ms, stop, then print timings — never the event loop.
        if timings::startup_benchmark_enabled() {
            interactive::run_interactive_benchmark().await?;
            timings::time("interactiveMode.init", timings::TimingLabel::Main);
            timings::print_timings();
            return Ok(0);
        }
        // Pi `prepareInitialMessage(parsed, settingsManager.getImageAutoResize(), stdinContent)`
        // (main.ts:828-832): the `images.autoResize` setting decides whether an `@image.png`
        // positional is downsampled to 2000px or inlined at full resolution.
        let auto_resize_images = session.services().settings.effective().image_auto_resize();
        // Pi main.ts:819-826 reads piped stdin in `main` (never in `prepareInitialMessage`, and
        // never for RPC mode, which owns stdin for JSON-RPC) and passes the string in at :831.
        let piped_stdin = cyrup::read_piped_stdin().await?;
        // AGENT-027 — pi's `time("readPipedStdin")` (main.ts:826) and `time("prepareInitialMessage")`
        // (`:833`) bracket exactly this pair. `initTheme` (`:835`) has no separate mark here: cyrup's
        // theme boot happens inside `run_interactive`, downstream of the print below, so a mark
        // placed here would time nothing. Recorded rather than silently dropped.
        timings::time("readPipedStdin", timings::TimingLabel::Main);
        let inputs = build_inputs(&cli, &dirs.cwd, auto_resize_images, piped_stdin).await?;
        timings::time("prepareInitialMessage", timings::TimingLabel::Main);
        // Pi prints immediately before entering the mode's own loop (`printTimings()` at
        // main.ts:899, after every `main` mark has been taken).
        timings::print_timings();
        // The startup package-update check (Pi `interactive-mode.ts:850-856`): DETACHED, gated on
        // `NetworkPolicy::allow_update_check()` (`--offline` / `CYRUP_OFFLINE` /
        // `CYRUP_SKIP_VERSION_CHECK`), and delivered to the run loop over a channel so nothing here
        // is awaited before the first frame. `None` when the gate declines.
        let update_policy = cyrup_config::policy::NetworkPolicy::resolve(
            session.services().settings.effective(),
            &env,
            &overrides,
        );
        let package_updates = cyrup::update_check::spawn_package_update_check(
            dirs.package_dir.clone(),
            Some(dirs.cwd.clone()),
            update_policy,
        );
        let result = interactive::run_interactive(
            runtime.clone(),
            session.clone(),
            inputs,
            cli.verbose,
            cancel,
            package_updates,
            migration.migrated_auth_providers.clone(),
            // `--tui-mode` reaches the renderer here and nowhere else (ADR-0005 §B-14); the
            // setting fallback is applied inside `run_interactive`, which is where the session's
            // effective settings are in scope.
            cli.tui_mode.map(|m| match m {
                cyrup::TuiMode::Regular => cyrup_config::settings::TuiMode::Regular,
                cyrup::TuiMode::Fullscreen => cyrup_config::settings::TuiMode::Fullscreen,
            }),
        )
        .await;
        // Quit is a normal exit here too: Pi disposes the runtime on every host teardown path
        // (agent-session-runtime.ts:397-404), emitting `session_shutdown{reason:"quit"}` so
        // extensions can flush/deregister. Runs even when the TUI loop errored out.
        runtime.dispose().await;
        // …and then, on a clean quit, the ONE line Pi prints after disposing
        // (interactive-mode.ts:3594-3597): the exact invocation that returns here. Under an explicit
        // `--session-dir` this is the only surfaced route back to the session — the picker a bare
        // relaunch offers only ever lists the session's own directory.
        if result.is_ok() {
            interactive::print_resume_hint(&dirs, &session).await;
        }
        result?;
        return Ok(0);
    }

    // `PI_STARTUP_BENCHMARK` is interactive-only (Pi main.ts:800-804).
    if timings::startup_benchmark_enabled() {
        anyhow::bail!("PI_STARTUP_BENCHMARK only supports interactive mode");
    }

    // The two non-interactive hosts. Both take pi's modelless hard stop (`require_model: true`,
    // main.ts:852-855) and neither supplies a trust PROMPT — SEAM-065: pi's `resolveProjectTrusted`
    // reads the saved-decision tier for every host (project-trust.ts:72-75) and only the prompt is
    // behind `hasUI` (:86-88), so the store is wired and the prompt deliberately is not.
    let target = config.target.clone();
    let fresh = cyrup::session_resolve::is_fresh_target(&target);
    let factory = session_launch::build_factory(
        provider,
        config,
        settings_store.clone(),
        auth_store.clone(),
        &dirs,
        models_json.clone(),
        None,
    )?;
    let (runtime, session) = match session_launch::launch(
        factory,
        target,
        PostBuild {
            session_name: session_name.as_deref(),
            cli: &cli,
            fresh,
            require_model: true,
        },
    )
    .await?
    {
        ControlFlow::Break(code) => return Ok(code),
        ControlFlow::Continue(pair) => pair,
    };

    match mode {
        AppMode::Rpc => {
            timings::print_timings();
            let _signals = spawn_abort_on_signal(runtime.clone(), cancel.clone(), AppMode::Rpc);
            let reader = tokio::io::BufReader::new(tokio::io::stdin());
            let mut writer = tokio::io::stdout();
            run_rpc_dispatch(&runtime, reader, &mut writer).await?;
            // Restore stdout at teardown (Pi `finally { restoreStdout() }`, main.ts:848).
            cyrup::output_guard::restore_stdout();
            Ok(0)
        }
        AppMode::Print | AppMode::Json => {
            // SEAM-006: print/json run on the RUNTIME host, exactly like interactive and RPC. Pi's
            // entry point is `runPrintMode(runtimeHost: AgentSessionRuntime, options)`
            // (print-mode.ts:32) — it has no bare-session host. Building a bare `AgentSession` here
            // left every loaded extension's `ctx.newSession()`/`ctx.fork()`/`ctx.switchSession()`/
            // `ctx.reload()` with nothing to act on (`SessionServiceError::NoRuntimeHost`, warned
            // and swallowed), and since this arm is what a spawned subagent child re-execs into,
            // EVERY subagent run inherited the missing host.
            //
            // `settingsManager.getImageAutoResize()` for the `@file` image path (Pi main.ts:830),
            // read before `session` moves into the signal guard.
            let auto_resize_images = session.services().settings.effective().image_auto_resize();
            // `mode` here is `Print` or `Json`; both are pi's `runPrintMode` host, whose handler
            // exits 143/129 on the first SIGTERM/SIGHUP (print-mode.ts:48-64).
            let _signals = spawn_abort_on_signal(runtime.clone(), cancel.clone(), mode);
            // NO prompt-required guard here: Pi has none. `buildInitialMessage` answers
            // `initialMessage: undefined` for a run with no stdin/`@file`/message
            // (initial-message.ts:36-42) and `runPrintMode` simply skips its send loops
            // (print-mode.ts:121-127), falling through to the terminal output block and returning 0.
            // The `ensure_prompt` bail that used to sit here inverted the exit code of every
            // prompt-less one-shot invocation — `cyrup -c -p`, `cyrup --session <id> --mode json` —
            // and suppressed JSON mode's session header entirely. See `run::turn_inputs`.
            // Pi main.ts:819-826 / :831 — the read happens here, in `main`, and the content is
            // passed into prompt assembly.
            let piped_stdin = cyrup::read_piped_stdin().await?;
            // AGENT-027 — pi's `time("readPipedStdin")` / `time("prepareInitialMessage")`
            // (main.ts:826/:833) are on the shared path, so they cover this arm too.
            timings::time("readPipedStdin", timings::TimingLabel::Main);
            let inputs = build_inputs(&cli, &dirs.cwd, auto_resize_images, piped_stdin).await?;
            timings::time("prepareInitialMessage", timings::TimingLabel::Main);
            // Pi prints once every `main` mark has been taken, immediately before entering the mode
            // (main.ts:902).
            timings::print_timings();
            let mut out = io::stdout();
            let dispatch = if let AppMode::Json = mode {
                run_json_dispatch(&runtime, &inputs, &mut out).await
            } else {
                run_print_dispatch(&runtime, &inputs, &mut out).await
            };
            // Restore stdout at teardown (Pi `finally { restoreStdout() }`, main.ts:848).
            cyrup::output_guard::restore_stdout();
            dispatch
        }
        AppMode::Interactive => unreachable!("interactive mode is handled before this match"),
    }
}
