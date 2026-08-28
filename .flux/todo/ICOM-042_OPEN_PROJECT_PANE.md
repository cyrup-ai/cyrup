---
stage: aug
status: done
updated: 2026-08-27 22:32
---

# Port openProjectPane and the Herdr client path

> **Upstream parity gap.** `cyrup-intercom` is a port of `pi-intercom` **v0.9.2**; upstream is now
> **v0.12.0** (`ef95f19`, 2026-08-22) at [`nicobailon/pi-intercom`](https://github.com/nicobailon/pi-intercom).
> Reference checkout: `./tmp/pi-intercom`. Gap analysis: `docs/gap-analysis/11-cyrup-intercom.md` — **ICOM-042 — partially closed**.
>
> `project-agent.ts` is **byte-identical at `v0.10.1` and `v0.12.0`** (`sha1 bb336e38`), so every
> `project-agent.ts` citation below is valid against either tag. `index.ts` citations are `v0.12.0`.

---

## 1. Core objective

`intercom({action:"send"|"ask", cwd:"/other/repo"})` addressed at a directory with **no live peer**
is a dead end. `resolve_cwd_delivery_target` resolves `Missing` and the tool returns the bare
sentence `No other intercom sessions are connected in /other/repo.` — true, but it names no next
step. Upstream's same sentence continues `Pass openProjectPaneIfMissing: true to open a Herdr
project pane and start Pi there.`, and that flag really does launch a session in that directory.

Two separable pieces of work fall out of that, and this brief keeps them separable:

- **Part A (§4) — mechanical, fully specified, independently shippable.** The `openProjectPaneIfMissing`
  / `focus` request flags, the schema, the `DeliveryTarget.project_pane` plumbing through both
  `send` and `ask`, the confirm-ordering split, the six error codes, and the correction of every
  place in the crate that quotes a flag the build does not honour.
- **Part B (§5) — one product decision, marked, with a recommendation.** *Which launcher backend
  actually opens the pane.* Everything above Part B is backend-agnostic; Part B is one trait impl.

**Non-negotiable regardless of the §5 answer:** after this task, no string in `crates/cyrup-intercom`
— emitted, or quoted in a doc comment as though it were emitted — may name
`openProjectPaneIfMissing` unless [`parameters_schema`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs#L263)
actually advertises it. Today two doc comments quote it while the schema rejects it. That must end.

---

## 2. What upstream does

### 2.1 `project-agent.ts` — the half that is NOT ported

[`../../tmp/pi-intercom/project-agent.ts`](../../tmp/pi-intercom/project-agent.ts) (324 lines) is two
things bolted together. cyrup ported the pure resolver (`:188-226`, `:298-302`). The launcher half
is `:1-186` plus `:227-296`.

**The error union (`:10-24`)** — six codes, a `Result`-shaped envelope, and a one-method client:

```ts
export type HerdrErrorCode =
  | "HERDR_UNAVAILABLE"
  | "HERDR_UNSUPPORTED_VERSION"
  | "PANE_GONE"
  | "NOT_FOUND"
  | "TIMEOUT"
  | "VALIDATION_ERROR";

export type HerdrResult<T> =
  | { ok: true; data: T }
  | { ok: false; error: { code: HerdrErrorCode; message: string; details?: unknown } };

export interface HerdrClient {
  run<T = unknown>(args: string[], options?: { timeoutMs?: number; signal?: AbortSignal; textOk?: boolean }): Promise<HerdrResult<T>>;
}
```

**Where each code really comes from** — this matters, because "represent the codes" without their
conditions is theatre:

| code | upstream condition |
|---|---|
| `HERDR_UNAVAILABLE` | `spawn` threw, or `child.on("error")`, with `ENOENT` (`:79-82`, `:109-114`) → *"Herdr is not installed or is not on PATH. Install Herdr 0.7.5+ or set HERDR_BIN."* |
| `HERDR_UNSUPPORTED_VERSION` | `--version` parsed but `supportsRawPanes` false (`:150-152`, `:160`) |
| `TIMEOUT` | the `setTimeout` kill, or an aborted `AbortSignal` (`:95-103`) |
| `PANE_GONE` | split succeeded but returned no pane id (`:243`); also any launcher code containing `gone` (`:63`) |
| `NOT_FOUND` | launcher code containing `not_found` / `not-found` / `no_such_pane` (`:64`) |
| `VALIDATION_ERROR` | the default (`:65`), an unparseable version (`:159`), or a non-zero exit with no JSON error envelope (`:133-134`) |

**`openProjectPane` (`:227-253`)** — the whole launch, verbatim:

```ts
export async function openProjectPane(input: {
  cwd: string; focus?: boolean; client?: HerdrClient; signal?: AbortSignal;
}): Promise<ProjectPaneLaunch> {
  const projectRoot = resolveProjectRoot(input.cwd);
  const client = input.client ?? createHerdrClient();
  const detected = await detectHerdr(client, input.signal);
  if (detected.ok === false) throw new Error(formatHerdrError(detected.error));

  const splitArgs = ["pane", "split", "--current", "--direction", "right", "--cwd", projectRoot];
  if (input.focus !== false) splitArgs.push("--focus");
  const split = await client.run(splitArgs, { timeoutMs: 15_000, signal: input.signal });
  if (split.ok === false) throw new Error(formatHerdrError(split.error));
  const paneId = extractPaneId(split.data);
  if (!paneId) throw new Error("Herdr project pane error (PANE_GONE): pane split returned no pane id.");

  const command = shellQuote(process.env.PI_INTERCOM_PI_BIN?.trim() || process.env.PI_BIN?.trim() || "pi");
  const started = await client.run(["pane", "run", paneId, command], { timeoutMs: 15_000, signal: input.signal });
  if (started.ok === false) {
    await client.run(["pane", "close", paneId], { timeoutMs: 5_000 });
    throw new Error(formatHerdrError(started.error));
  }

  return { paneId, projectRoot, command, herdrVersion: detected.data.versionText };
}
```

Four load-bearing details: `focus` defaults to **true** (`input.focus !== false`); the pane is
**closed again** if the agent fails to start in it (`:248`), so a failed launch leaves no orphan;
`resolveProjectRoot` (`:179-186`) `stat`s the directory and **throws `Project target '<p>' is not a
directory.`** before anything is spawned; and `formatHerdrError` (`:141-143`) is the single error
shape — `` `Herdr project pane error (${code}): ${message}` ``.

**`waitForProjectSession` (`:255-296`)** — the launch is only half the delivery. The new agent has
to *register with the broker* before it is addressable:

```ts
  const startedAt = Date.now();
  const timeoutMs = input.timeoutMs ?? DEFAULT_PROJECT_AGENT_TIMEOUT_MS;   // 20_000  (:7)
  const pollMs = input.pollMs ?? DEFAULT_PROJECT_AGENT_POLL_MS;            //    250  (:8)

  while (Date.now() - startedAt < timeoutMs) {
    if (input.signal?.aborted) throw new Error("Cancelled");
    const sessions = await client.listSessions({ timeoutMs: Math.min(5_000, timeoutMs) });

    if (input.to?.trim()) { /* re-run resolveTargetInCwd until it says `found` */ }

    const newInProject = sessions.filter(
      (session) => !input.beforeSessionIds.has(session.id) && sameCwd(session.cwd, input.projectRoot),
    );
    if (newInProject.length === 1) return newInProject[0]!;
    if (newInProject.length > 1) {
      throw new Error(`Multiple new intercom sessions registered in ${input.projectRoot}: ${formatSessionRefs(newInProject)}. Address one explicitly.`);
    }
    await sleep(pollMs, input.signal);
  }
  throw new Error(`Timed out waiting for a Pi intercom session to register in ${input.projectRoot}. The Herdr pane may still be starting, or pi-intercom may not be loaded there.`);
```

The `beforeSessionIds` snapshot is taken **before** the launch (`index.ts:1532`) — the wait
identifies the new session by *difference*, not by cwd alone, so a peer that happened to already be
starting in that directory is not mistaken for the one just launched.

### 2.2 `index.ts` — the call sites

**`resolveCwdDeliveryTarget` (`:1500-1542`)** — cyrup ported everything except the last 11 lines:

```ts
    if (!options.openProjectPaneIfMissing) {
      throw new Error(`${existing.reason ?? `No intercom session is connected in ${targetCwd}.`} Pass openProjectPaneIfMissing: true to open a Herdr project pane and start Pi there.`);
    }

    const beforeSessionIds = new Set(sessions.map((session) => session.id));
    const projectPane = await openProjectPane({ cwd: targetCwd, focus: options.focus, signal: options.signal });
    const session = await waitForProjectSession(activeClient, {
      projectRoot: projectPane.projectRoot,
      currentSessionId,
      beforeSessionIds,
      ...(options.to ? { to: options.to } : {}),
      signal: options.signal,
    });
    return { id: session.id, label: session.name || session.id, projectPane };
```

**The schema (`index.ts:2175-2180`)**, descriptions verbatim:

```ts
      openProjectPaneIfMissing: Type.Optional(Type.Boolean({
        description: "For send/ask with cwd, open a visible Herdr project pane and launch Pi there when no matching live session is connected.",
      })),
      focus: Type.Optional(Type.Boolean({
        description: "For openProjectPaneIfMissing, focus the new Herdr pane. Defaults to true.",
      })),
```

**The `send` arm (`:2322-2401`)** — note the **confirm split**, which is not cosmetic:

```ts
            if (openProjectPaneIfMissing && !cwd) { /* "openProjectPaneIfMissing requires a target cwd." */ }
            const confirmSend = !replyTo && config.confirmSend && ctx.hasUI;
            const attachmentText = attachments?.length ? formatAttachments(attachments) : "";
            if (confirmSend && cwd && openProjectPaneIfMissing) {
              const confirmed = await ctx.ui.confirm("Send message", `Send to "${to ?? cwd}":\n\n${message}${attachmentText}`);
              if (!confirmed) { /* "Message cancelled by user" */ }
            }
            const target: DeliveryTarget = cwd
              ? await resolveCwdDeliveryTarget(connectedClient, { to, cwd, openProjectPaneIfMissing, focus, signal: _signal })
              : { id: await resolveSessionTarget(connectedClient, to) ?? to, label: to };
            const sendTo = target.id;
            const targetDisplay = target.projectPane ? target.label : to ?? target.label;
            …
            if (confirmSend && !(cwd && openProjectPaneIfMissing)) { /* the ordinary post-resolution confirm */ }
```

A pane launch is a **side effect the human must approve before it happens**, so when a launch is
possible the confirm moves *ahead* of resolution and labels the dialog with `to ?? cwd` (there is no
resolved peer name yet). The ordinary confirm is then suppressed so nobody is asked twice.

The result (`:2390-2401`):

```ts
                text: target.projectPane
                  ? `Opened Herdr project pane ${target.projectPane.paneId} for ${target.projectPane.projectRoot} and sent message to ${targetDisplay}`
                  : inferredAsk ? `Reply sent to ${targetDisplay} (inferred from pending ask)` : `Message sent to ${targetDisplay}`,
              details: {
                ...deliveryDetails(result),
                ...(effectiveReplyTo ? { replyTo: effectiveReplyTo } : {}),
                ...(target.projectPane ? { openedProjectPane: true, paneId: target.projectPane.paneId, projectRoot: target.projectPane.projectRoot } : {}),
              },
```

**The `ask` arm (`:2437-2524`)** is the same guard, the same `resolveCwdDeliveryTarget` call, the
same `targetDisplay` (`:2457`), and `details: target.projectPane ? { openedProjectPane, paneId, projectRoot } : {}` (`:2524`). `ask` has **no** confirm dialog at all, so it needs no split.

---

## 3. What already exists in the port and MUST be reused

Nothing below gets rewritten. Every one of these is the thing the new code calls.

| Existing | Reuse it for |
|---|---|
| [`project_target::resolve_target_in_cwd`](../../crates/cyrup-intercom/src/project_target.rs#L91) | unchanged. The launch path calls it **again** inside the wait loop when `to` is set — exactly as upstream `:272-282` does. |
| [`project_target::format_session_refs`](../../crates/cyrup-intercom/src/project_target.rs#L62) | the `Multiple new intercom sessions registered in …` wait-loop error. |
| [`project_target::ProjectTargetResolution`](../../crates/cyrup-intercom/src/project_target.rs#L33) | `Missing { reason, target_cwd }` is already the exact branch point the flag gates. |
| [`cwd::same_cwd`](../../crates/cyrup-intercom/src/cwd.rs#L54) | the `newInProject` filter in the wait loop. Do not byte-compare. |
| [`cwd::resolve_path`](../../crates/cyrup-intercom/src/cwd.rs#L65) | `resolveProjectRoot`'s `resolve(cwd)` half. |
| [`tools::intercom::resolve_target_cwd`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs#L108) | unchanged — the target cwd is already computed before the flag is consulted. |
| [`tools::intercom::resolve_cwd_delivery_target`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs#L117) | **the single seam.** Both `send` and `ask` already route through it; the launch belongs inside it, as upstream. |
| [`transport::client::IntercomClient::list_sessions_with_timeout`](../../crates/cyrup-intercom/src/transport/client.rs#L532) | the wait loop's `listSessions({ timeoutMs: Math.min(5_000, timeoutMs) })`. Already exists — do not add a second lister. |
| [`transport::spawn`'s `wait_until` poll ladder](../../crates/cyrup-intercom/src/transport/spawn.rs#L387) | the shape to copy for the 250 ms poll. It is `Result<()>`-shaped, so the session wait needs its own loop, but the `Instant::now()` / `elapsed() < timeout` / `sleep` idiom is the house style. |
| [`transport::spawn::spawn_detached_broker`](../../crates/cyrup-intercom/src/transport/spawn.rs#L156) | the crate's process-spawn idiom: `tokio::process::Command`, `process_group(0)` on unix, `creation_flags(DETACHED_PROCESS \| CREATE_NO_WINDOW)` on windows, stderr piped for the failure reason. A launcher backend copies this shape. |
| [`SharedIntercomState::set_host_services` / `host_services`](../../crates/cyrup-intercom/src/session_state.rs#L292) | the exact late-bound-`Arc<dyn Trait>` slot idiom (`Mutex<Option<Arc<dyn …>>>`, `unwrap_or_else(\|e\| e.into_inner())`) the launcher slot copies. |
| [`SharedIntercomState::has_ui`](../../crates/cyrup-intercom/src/session_state.rs#L314) + [`host_services().confirm`](../../crates/cyrup-intercom/src/tools/intercom/send.rs#L94) | the confirm gate already in `send.rs`. The split reorders it; it does not reimplement it. |
| [`error::IntercomError`](../../crates/cyrup-intercom/src/error.rs#L6) | do **not** add a variant. The launcher's error is a tool-surface string, like every other error in `tools/intercom/`, and reaches the model through `ToolError`. |
| `CancelToken` (= `tokio_util::sync::CancellationToken`, [`cyrup-core/src/cancel.rs`](../../crates/cyrup-core/src/cancel.rs)) | **the `AbortSignal`.** `ask` already receives it; `send` does not yet. |

Two files pin the *current* contract and will contradict the new one — they are part of the change,
not collateral:

- [`resources.rs:164`](../../crates/cyrup-intercom/src/resources.rs#L164) asserts the shipped skill
  body does **not** contain `openProjectPaneIfMissing`, with the rationale "the skill must not
  document a parameter the schema rejects". That rationale inverts the moment the schema advertises it.
- [`resources/skills/pi-intercom/SKILL.md:9-16`](../../crates/cyrup-intercom/resources/skills/pi-intercom/SKILL.md#L9)
  carries a `[CYRUP-DELTA]` naming the exact upstream passages (`:145-157`, `:241-248`, the bullet at
  `:26`, the clause at `:228`) and says *"Restore upstream's text verbatim the moment the Herdr pane
  launcher lands."* That instruction is now due.

---

## 4. Part A — the mechanical change (prescriptive, backend-agnostic)

### 4.1 New module `crates/cyrup-intercom/src/project_pane.rs`

Declare it in [`lib.rs`](../../crates/cyrup-intercom/src/lib.rs#L37) next to `pub mod project_target;`.
This module is upstream's `HerdrClient` surface with the vendor name lifted out of the *types* and
pushed into the *impl*, so §5's answer changes one `impl` block and nothing else.

```rust
//! `HerdrErrorCode` / `HerdrResult` / `HerdrClient` / `ProjectPaneLaunch` — the launcher half of
//! `pi-intercom/project-agent.ts` (`v0.12.0`, `:10-186`, `:227-253`), with upstream's Herdr-specific
//! client generalized to a trait so the backend is one `impl`, not a dependency baked into the types.

use std::path::{Path, PathBuf};

use cyrup_core::CancelToken;

/// `DEFAULT_PROJECT_AGENT_TIMEOUT_MS` (`project-agent.ts:7`).
pub const DEFAULT_PROJECT_AGENT_TIMEOUT_MS: u64 = 20_000;
/// `DEFAULT_PROJECT_AGENT_POLL_MS` (`project-agent.ts:8`).
pub const DEFAULT_PROJECT_AGENT_POLL_MS: u64 = 250;

/// `HerdrErrorCode` (`project-agent.ts:10-16`), one variant per upstream member.
///
/// The names drop the vendor prefix; [`PaneErrorCode::as_str`] keeps upstream's **wire spelling**,
/// so a launcher named `Herdr` renders `formatHerdrError`'s string byte for byte.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PaneErrorCode {
    /// `HERDR_UNAVAILABLE` — the launcher binary is absent, not on PATH, or refused to start
    /// (`:79-82`, `:109-114`).
    Unavailable,
    /// `HERDR_UNSUPPORTED_VERSION` — present but too old to split a raw pane (`:150-152`, `:160`).
    UnsupportedVersion,
    /// `PANE_GONE` — the split reported success but named no pane (`:243`), or the launcher said
    /// `gone` (`:63`).
    PaneGone,
    /// `NOT_FOUND` — the launcher said `not_found` / `not-found` / `no_such_pane` (`:64`).
    NotFound,
    /// `TIMEOUT` — the command's own deadline elapsed, or the [`CancelToken`] fired (`:95-103`).
    Timeout,
    /// `VALIDATION_ERROR` — upstream's default (`:65`): an unparseable version (`:159`) or a
    /// non-zero exit carrying no error envelope (`:133-134`).
    ValidationError,
}

impl PaneErrorCode {
    /// Upstream's wire spelling. `HERDR_*` is preserved for the two launcher-identity codes because
    /// [`PaneLaunchError`]'s `Display` already names the backend, and changing the token would make
    /// a cyrup log unmatchable against an upstream one.
    #[must_use]
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable => "HERDR_UNAVAILABLE",
            Self::UnsupportedVersion => "HERDR_UNSUPPORTED_VERSION",
            Self::PaneGone => "PANE_GONE",
            Self::NotFound => "NOT_FOUND",
            Self::Timeout => "TIMEOUT",
            Self::ValidationError => "VALIDATION_ERROR",
        }
    }
}

/// The `{ ok: false, error }` arm of `HerdrResult<T>` (`project-agent.ts:18-20`). The `ok: true` arm
/// is Rust's `Ok`, so the union is a `Result` and no envelope type is needed.
#[derive(Clone, Debug)]
pub struct PaneLaunchError {
    /// The backend that produced it — `"Herdr"`, `"tmux"`, … Renders in [`Display`].
    pub backend: &'static str,
    /// `error.code`.
    pub code: PaneErrorCode,
    /// `error.message`.
    pub message: String,
}

/// `formatHerdrError` (`project-agent.ts:141-143`):
/// `` `Herdr project pane error (${code}): ${message}` ``. With `backend == "Herdr"` this is byte
/// identical to upstream.
impl std::fmt::Display for PaneLaunchError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{} project pane error ({}): {}", self.backend, self.code.as_str(), self.message)
    }
}

/// `normalizeCode(raw)` (`project-agent.ts:60-66`) — map a backend's own code/diagnostic onto the
/// union. Substring matching on a lowercased haystack, in upstream's order.
#[must_use]
pub fn normalize_code(raw: &str) -> PaneErrorCode {
    let code = raw.to_lowercase();
    if code.contains("timeout") || code.contains("timed_out") {
        PaneErrorCode::Timeout
    } else if code.contains("gone") {
        PaneErrorCode::PaneGone
    } else if code.contains("not_found") || code.contains("not-found") || code == "no_such_pane" {
        PaneErrorCode::NotFound
    } else {
        PaneErrorCode::ValidationError
    }
}

/// `ProjectPaneLaunch` (`project-agent.ts:28-33`). `herdrVersion` → `launcher_version`.
#[derive(Clone, Debug)]
pub struct ProjectPaneLaunch {
    /// `paneId` — the backend's own pane handle, echoed in the tool result and `details.paneId`.
    pub pane_id: String,
    /// `projectRoot` — the **canonicalized** directory (`resolveProjectRoot`, `:179-186`).
    pub project_root: String,
    /// `command` — what was actually started in the pane.
    pub command: String,
    /// `herdrVersion` — the version string the availability probe reported.
    pub launcher_version: String,
}

/// `openProjectPane`'s `input`, minus the injected client (`project-agent.ts:227-232`).
pub struct ProjectPaneRequest<'a> {
    /// The already-canonicalized project root (see [`resolve_project_root`]).
    pub project_root: PathBuf,
    /// `input.focus !== false` (`:239`) — **already defaulted to `true`** by the caller.
    pub focus: bool,
    /// Upstream's `AbortSignal` (`:230`).
    pub cancel: &'a CancelToken,
}

/// `HerdrClient` (`project-agent.ts:22-24`) collapsed with `openProjectPane` (`:227-253`).
///
/// Upstream exposes a generic `run(args)` and keeps the pane choreography in a free function
/// because there is exactly one backend. Here the choreography IS the contract — split a pane at
/// `project_root`, start the agent in it, close the pane again if the agent fails to start
/// (`:248`) — so the trait is the whole operation and a backend cannot get the cleanup wrong.
#[async_trait::async_trait]
pub trait ProjectPaneLauncher: Send + Sync {
    /// The backend's display name, used in [`PaneLaunchError`]'s prefix and in the
    /// `Opened {name} project pane …` result line.
    fn name(&self) -> &'static str;

    /// `openProjectPane(input)` (`:227-253`).
    ///
    /// # Errors
    /// A [`PaneLaunchError`] whose `code` is the real condition, never a catch-all.
    async fn open(&self, request: ProjectPaneRequest<'_>) -> Result<ProjectPaneLaunch, PaneLaunchError>;
}

/// The launcher bound when no backend is configured. Every call answers
/// [`PaneErrorCode::Unavailable`] — the same code upstream returns for a missing binary — so the
/// flag is honoured with a true statement rather than silently ignored.
pub struct UnavailableLauncher {
    /// Why. Named so the message can say *which* backend is missing when one is expected.
    pub reason: String,
}

#[async_trait::async_trait]
impl ProjectPaneLauncher for UnavailableLauncher {
    fn name(&self) -> &'static str {
        "Project pane"
    }
    async fn open(&self, _request: ProjectPaneRequest<'_>) -> Result<ProjectPaneLaunch, PaneLaunchError> {
        Err(PaneLaunchError {
            backend: self.name(),
            code: PaneErrorCode::Unavailable,
            message: self.reason.clone(),
        })
    }
}

/// `resolveProjectRoot(cwd)` (`project-agent.ts:179-186`): `resolve()`, then **reject a non-directory**,
/// then `realpathSync`. Runs BEFORE anything is spawned, so a typo'd path costs no process.
///
/// [`crate::cwd::resolve_path`] is the ported `resolve()`; `std::fs::canonicalize` is `realpathSync`.
/// Unlike [`crate::cwd::normalize_cwd`], the failure here is NOT swallowed — upstream throws.
///
/// # Errors
/// `Project target '<path>' is not a directory.` (`:183`, verbatim).
pub fn resolve_project_root(base: &Path, cwd: &str) -> Result<PathBuf, String> {
    let resolved = crate::cwd::resolve_path(base, cwd);
    let is_dir = std::fs::metadata(&resolved).map(|m| m.is_dir()).unwrap_or(false);
    if !is_dir {
        return Err(format!("Project target '{}' is not a directory.", resolved.display()));
    }
    Ok(std::fs::canonicalize(&resolved).unwrap_or(resolved))
}
```

### 4.2 `project_target.rs` — add `wait_for_project_session`, delete the stale delta note

Upstream keeps the wait beside the resolver in one file; so does the port. Add to
[`project_target.rs`](../../crates/cyrup-intercom/src/project_target.rs):

```rust
/// `waitForProjectSession(client, input)` (`v0.12.0 project-agent.ts:255-296`).
///
/// A launched pane is not yet addressable: the agent inside it has to connect and `register` before
/// the broker lists it. This polls the roster until it does.
///
/// `before_session_ids` is snapshotted BEFORE the launch (`index.ts:1532`), so the new session is
/// identified by DIFFERENCE. A cwd-only filter would happily return a peer that was already
/// starting there for its own reasons.
///
/// # Errors
/// - `"Cancelled"` when `cancel` fires (`:269`).
/// - the ambiguity string at `:289` when more than one new session registers there.
/// - the timeout string at `:295`.
pub async fn wait_for_project_session(
    client: &crate::transport::client::IntercomClient,
    project_root: &str,
    current_session_id: &str,
    before_session_ids: &std::collections::HashSet<String>,
    to: Option<&str>,
    cancel: &cyrup_core::CancelToken,
    launcher_name: &str,
) -> Result<SessionInfo, String> {
    use std::time::Duration;
    let timeout = Duration::from_millis(crate::project_pane::DEFAULT_PROJECT_AGENT_TIMEOUT_MS);
    let poll = Duration::from_millis(crate::project_pane::DEFAULT_PROJECT_AGENT_POLL_MS);
    // `Math.min(5_000, timeoutMs)` (`:270`) — one roster fetch may not outlive the whole wait.
    let list_timeout = timeout.min(Duration::from_secs(5));
    let started = tokio::time::Instant::now();

    while started.elapsed() < timeout {
        if cancel.is_cancelled() {
            return Err("Cancelled".to_string());
        }
        let sessions = client
            .list_sessions_with_timeout(list_timeout)
            .await
            .map_err(|e| e.to_string())?;

        // `:272-282` — with an explicit `to`, reuse the SAME resolver the non-launch path uses, so
        // the id/name/prefix ladder cannot drift between the two.
        if let Some(to) = to.map(str::trim).filter(|t| !t.is_empty()) {
            if let Ok(ProjectTargetResolution::Found { session, .. }) =
                resolve_target_in_cwd(&sessions, current_session_id, project_root, Some(to))
            {
                return Ok(*session);
            }
        } else {
            let new_in_project: Vec<&SessionInfo> = sessions
                .iter()
                .filter(|s| !before_session_ids.contains(&s.id) && same_cwd(&s.cwd, project_root))
                .collect();
            match new_in_project.as_slice() {
                [only] => return Ok((*only).clone()),
                [] => {}
                many => {
                    return Err(format!(
                        "Multiple new intercom sessions registered in {project_root}: {}. Address one explicitly.",
                        format_session_refs(many)
                    ));
                }
            }
        }
        tokio::select! {
            () = tokio::time::sleep(poll) => {}
            () = cancel.cancelled() => return Err("Cancelled".to_string()),
        }
    }

    // `:295`, with the vendor noun parameterized and the product name substituted.
    Err(format!(
        "Timed out waiting for a cyrup intercom session to register in {project_root}. \
         The {launcher_name} pane may still be starting, or cyrup-intercom may not be loaded there."
    ))
}
```

Then **delete** the `# [CYRUP-DELTA] — the Herdr pane launcher is not ported` block at
[`project_target.rs:4-21`](../../crates/cyrup-intercom/src/project_target.rs#L4) — including the
sentence at `:19` that quotes `Pass openProjectPaneIfMissing: true …`. Replace the module doc's
first line with one naming both halves as ported. Also fix the two test doc comments at
[`:220`](../../crates/cyrup-intercom/src/project_target.rs#L220) ("upstream: offer the Herdr pane;
cyrup: report it") — that claim is now false.

### 4.3 `session_state.rs` — the launcher slot

Copy the `host_services` idiom exactly ([`:107`](../../crates/cyrup-intercom/src/session_state.rs#L107),
[`:171`](../../crates/cyrup-intercom/src/session_state.rs#L171),
[`:292-301`](../../crates/cyrup-intercom/src/session_state.rs#L292)):

```rust
    /// The project-pane launcher backend, late-bound like [`Self::host_services`]. `None` until a
    /// backend is bound; the tool then answers `openProjectPaneIfMissing` with
    /// [`crate::project_pane::UnavailableLauncher`] rather than ignoring the flag.
    project_pane_launcher: Mutex<Option<Arc<dyn crate::project_pane::ProjectPaneLauncher>>>,
```

with `set_project_pane_launcher(&self, launcher: Arc<dyn ProjectPaneLauncher>)` and
`project_pane_launcher(&self) -> Option<Arc<dyn ProjectPaneLauncher>>`, both one line, both using
`unwrap_or_else(|e| e.into_inner())`.

### 4.4 `tools/intercom/mod.rs` — params, schema, `DeliveryTarget`, the seam

**Params** — add to [`IntercomParams`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs#L48)
after `cwd` (`#[serde(rename_all = "camelCase")]` already maps the names):

```rust
    /// `openProjectPaneIfMissing` (`v0.12.0 index.ts:2175-2177`) — for `send`/`ask` with `cwd`,
    /// open a visible project pane and launch cyrup there when no matching live session is
    /// connected. Rejected without a `cwd` (`:2322-2326`, `:2437-2441`).
    #[serde(default)]
    open_project_pane_if_missing: Option<bool>,
    /// `focus` (`v0.12.0 index.ts:2178-2180`) — focus the new pane. **Defaults to true**
    /// (`project-agent.ts:239` is `input.focus !== false`, so only an explicit `false` unfocuses).
    #[serde(default)]
    focus: Option<bool>,
```

**Schema** — add both properties to
[`parameters_schema`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs#L263) after `"cwd"`,
descriptions from `index.ts:2176`/`:2179` with the vendor noun replaced by the chosen backend, and
`Pi` → `cyrup`. Then **remove** the comment at
[`mod.rs:272`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs#L272) that says the `cwd`
description is carried "minus the sentence about `openProjectPaneIfMissing`" — it is no longer minus
anything.

**`DeliveryTarget`** ([`:85`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs#L85)) gains
upstream's third member, and its `[CYRUP-DELTA]` doc block ([`:80-84`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs#L80))
is deleted:

```rust
pub(super) struct DeliveryTarget {
    pub(super) id: String,
    pub(super) label: String,
    /// `projectPane?: ProjectPaneLaunch` (`v0.12.0 index.ts:75`). `Some` only when THIS call
    /// launched the pane — every result and `details` branch keys off it.
    pub(super) project_pane: Option<crate::project_pane::ProjectPaneLaunch>,
}
```

**The seam.** Give `resolve_cwd_delivery_target` upstream's options object rather than a fifth
positional argument, and hand it the state so it can read the launcher slot:

```rust
/// `resolveCwdDeliveryTarget`'s `options` (`v0.12.0 index.ts:1500-1506`).
pub(super) struct CwdDeliveryOptions<'a> {
    pub(super) to: Option<&'a str>,
    pub(super) cwd: &'a str,
    pub(super) open_project_pane_if_missing: bool,
    /// Already defaulted: `params.focus.unwrap_or(true)`.
    pub(super) focus: bool,
    pub(super) cancel: &'a CancelToken,
}

pub(super) async fn resolve_cwd_delivery_target(
    state: &SharedIntercomState,
    client: &crate::transport::client::IntercomClient,
    options: CwdDeliveryOptions<'_>,
) -> Result<DeliveryTarget, ToolError> {
    // … unchanged through `resolve_target_in_cwd` …
    match existing {
        ProjectTargetResolution::Found { session, .. } => {
            // … unchanged label logic …
            Ok(DeliveryTarget { id: session.id.clone(), label, project_pane: None })
        }
        ProjectTargetResolution::Missing { reason, .. } => {
            let launcher = state.project_pane_launcher();
            // `v0.12.0 index.ts:1529-1530`. The sentence naming the flag is emitted ONLY because
            // the schema now advertises it — this is the line ICOM-042 exists to make honest.
            if !options.open_project_pane_if_missing {
                return Err(ToolError::new(format!(
                    "{reason} Pass openProjectPaneIfMissing: true to open a {} project pane and start cyrup there.",
                    launcher.as_ref().map_or("project", |l| l.name())
                )));
            }
            // `resolveProjectRoot` FIRST (`project-agent.ts:233`): a non-directory is refused
            // before any backend is consulted, so a typo costs no process.
            let project_root = crate::project_pane::resolve_project_root(
                std::path::Path::new(&current_session.cwd),
                &target_cwd,
            )
            .map_err(ToolError::new)?;

            // `const beforeSessionIds = new Set(sessions.map(s => s.id))` (`index.ts:1532`) — the
            // snapshot is taken from the roster ALREADY fetched above, before the launch.
            let before: std::collections::HashSet<String> =
                sessions.iter().map(|s| s.id.clone()).collect();

            let launcher = launcher.unwrap_or_else(|| {
                Arc::new(crate::project_pane::UnavailableLauncher {
                    reason: "No project pane launcher is configured for this session.".to_string(),
                })
            });
            let launch = launcher
                .open(crate::project_pane::ProjectPaneRequest {
                    project_root: project_root.clone(),
                    focus: options.focus,
                    cancel: options.cancel,
                })
                .await
                .map_err(|e| ToolError::new(e.to_string()))?;

            let session = crate::project_target::wait_for_project_session(
                client,
                &launch.project_root,
                &current_session_id,
                &before,
                options.to,
                options.cancel,
                launcher.name(),
            )
            .await
            .map_err(ToolError::new)?;

            // `{ id: session.id, label: session.name || session.id, projectPane }` (`:1542`) —
            // JS `||`, so a blank name falls through to the id. NOTE the label deliberately does
            // NOT consider `to` here, unlike the `found` arm.
            let label = session.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| session.id.clone());
            Ok(DeliveryTarget { id: session.id, label, project_pane: Some(launch) })
        }
    }
}
```

**Dispatch** — `send` now needs the cancel token
([`mod.rs:183`](../../crates/cyrup-intercom/src/tools/intercom/mod.rs#L183)):

```rust
            "send" => self.action_send(&params, &client, cancel).await,
```

### 4.5 `tools/intercom/send.rs`

Four edits to [`send.rs`](../../crates/cyrup-intercom/src/tools/intercom/send.rs):

1. **Signature** gains `cancel: &CancelToken`.
2. **The guard**, immediately after the `to`/`cwd`/`message` guard (`index.ts:2322-2326`):

```rust
        let open_pane = params.open_project_pane_if_missing.unwrap_or(false);
        // `v0.12.0 index.ts:2322-2326` — verbatim, and BEFORE the confirm, so a flag typo never
        // costs a dialog.
        if open_pane && cwd.is_none() {
            return Err(ToolError::new("openProjectPaneIfMissing requires a target cwd."));
        }
```

3. **The confirm split** ([`send.rs:92-114`](../../crates/cyrup-intercom/src/tools/intercom/send.rs#L92)).
   Hoist `confirm_send` and `attachment_text` above the resolution, add the pre-resolution branch,
   and gate the existing one:

```rust
        // `const confirmSend = !replyTo && config.confirmSend && ctx.hasUI` (`:2328`), hoisted
        // above the resolution because a pane LAUNCH is a side effect the human approves BEFORE it
        // happens, not after.
        let confirm_send = params.reply_to.is_none() && self.state.config.confirm_send && self.state.has_ui();
        let attachment_text = params.attachments.as_deref().filter(|a| !a.is_empty())
            .map(format_attachments).unwrap_or_default();
        let launch_possible = cwd.is_some() && open_pane;

        // `v0.12.0 index.ts:2330-2341`: the label is `to ?? cwd` — there is no resolved peer name
        // yet, and there may never be one.
        if confirm_send && launch_possible
            && let Some(services) = self.state.host_services()
        {
            let label = to.clone().or_else(|| cwd.clone()).unwrap_or_default();
            if !services.confirm(
                "Send Message",
                &format!("Send to \"{label}\":\n\n{message}{attachment_text}"),
                &cyrup_ext::DialogOptions::default(),
            ) {
                return Ok(text_result("Message cancelled by user"));
            }
        }

        let delivery = match cwd.as_deref() {
            Some(cwd) => resolve_cwd_delivery_target(&self.state, client, CwdDeliveryOptions {
                to: to.as_deref(),
                cwd,
                open_project_pane_if_missing: open_pane,
                focus: params.focus.unwrap_or(true),
                cancel,
            }).await?,
            None => { /* unchanged */ }
        };
        let DeliveryTarget { id: target, label, project_pane } = delivery;
        // `target.projectPane ? target.label : to ?? target.label` (`:2346`) — a launched session's
        // OWN name wins over the caller's `to`, because `to` may have been a bare filter.
        let target_display = if project_pane.is_some() { label } else { to.clone().unwrap_or(label) };
```

   …and the existing confirm at `:94` becomes `if confirm_send && !launch_possible && let Some(services) = …`,
   reusing the hoisted `attachment_text`.

4. **The result** ([`send.rs:179-192`](../../crates/cyrup-intercom/src/tools/intercom/send.rs#L179)),
   `index.ts:2390-2401`:

```rust
        if let Some(pane) = &project_pane
            && let Some(map) = details.as_object_mut()
        {
            map.insert("openedProjectPane".to_string(), serde_json::json!(true));
            map.insert("paneId".to_string(), serde_json::json!(pane.pane_id));
            map.insert("projectRoot".to_string(), serde_json::json!(pane.project_root));
        }
        Ok(detailed_result(
            match (&project_pane, &inferred_ask) {
                // The pane branch OUTRANKS the inferred-reply branch upstream (`:2392-2396`): a
                // freshly launched session cannot have a pending ask to infer against anyway.
                (Some(pane), _) => format!(
                    "Opened {} project pane {} for {} and sent message to {target_display}",
                    launcher_name, pane.pane_id, pane.project_root
                ),
                (None, Some(_)) => format!("Reply sent to {target_display} (inferred from pending ask)"),
                (None, None) => format!("Message sent to {target_display}"),
            },
            details,
        ))
```

   (`launcher_name` comes from `self.state.project_pane_launcher()`; with a Herdr backend the
   sentence is upstream's verbatim.)

### 4.6 `tools/intercom/ask.rs`

[`ask.rs`](../../crates/cyrup-intercom/src/tools/intercom/ask.rs) already has the `CancelToken`.
Three edits, no confirm split (`ask` has no dialog):

1. The same `open_project_pane_if_missing requires a target cwd.` guard (`index.ts:2437-2441`).
2. [`:37-38`](../../crates/cyrup-intercom/src/tools/intercom/ask.rs#L37) passes `CwdDeliveryOptions`,
   and [`:56-59`](../../crates/cyrup-intercom/src/tools/intercom/ask.rs#L56) takes `project_pane`
   out of the destructure and applies the same `targetDisplay` rule (`:2457`).
3. [`:134`](../../crates/cyrup-intercom/src/tools/intercom/ask.rs#L134) becomes a `detailed_result`
   when a pane was opened (`:2524`), keeping `text_result` otherwise:

```rust
        let text = format!("**Reply from {to}:**\n{reply_text}{reply_attachments}");
        Ok(match &project_pane {
            Some(pane) => detailed_result(text, serde_json::json!({
                "openedProjectPane": true,
                "paneId": pane.pane_id,
                "projectRoot": pane.project_root,
            })),
            None => text_result(text),
        })
```

### 4.7 The shipped skill and its pin

- [`SKILL.md`](../../crates/cyrup-intercom/resources/skills/pi-intercom/SKILL.md): restore upstream's
  `:145-157`, `:241-248`, the Pattern-6 bullet at `:26` and the closing clause of `:228` from
  [`../../tmp/pi-intercom/skills/pi-intercom/SKILL.md`](../../tmp/pi-intercom/skills/pi-intercom/SKILL.md),
  with `pi` → `cyrup` and the vendor noun matching the chosen backend, and drop delta note **2** from
  the front matter (delta **1**, the `CYRUP_INTERCOM_ASK_TIMEOUT_MS` rename, stays). The passage at
  `SKILL.md:175` — *"`cwd` addresses a session that is already live in that directory; it never starts
  one."* — is now **false** and must go.
- [`resources.rs:141-142`](../../crates/cyrup-intercom/src/resources.rs#L141) and the assertion at
  [`:164`](../../crates/cyrup-intercom/src/resources.rs#L164) encode the opposite invariant. The
  assertion's *intent* — the skill must not tell the model to pass a parameter the schema rejects —
  survives; only its polarity changes, and it should be re-expressed against
  `parameters_schema()["properties"]` so it tracks the schema instead of a hard-coded name.

---

## 5. ⚠️ THE ONE DECISION — do not make it alone

> **Which backend opens the pane?** Everything in §4 is settled and shippable without this answer.
> This section is the only part of the task that a human must resolve, and it is exactly one
> `impl ProjectPaneLauncher` block plus one binding site.

Upstream's launcher is Herdr-specific: it shells out to `process.env.HERDR_BIN ?? "herdr"` and its
messages name *"Herdr 0.7.5+"*. cyrup ships no Herdr integration, and
`cyrup-ext-subagents` already carries a **deliberate** Herdr divergence
([`tui/fleet.rs:58-60`](../../crates/cyrup-ext-subagents/src/tui/fleet.rs#L58) — no Herdr inspector,
`H` answers *"Herdr inspector controls are unavailable in this context."*). Adding a Herdr
*dependency* here would contradict a decision this workspace already made in the other direction.

| | A — Herdr | B — tmux | C — no launcher |
|---|---|---|---|
| new third-party dep | `herdr` ≥ 0.7.5 | `tmux` (already the terminal for most sessions) | none |
| upstream fidelity | byte-identical | same contract, different vendor noun | flag never exists |
| works headless / IDE / cloud | no | no | n/a |
| contradicts `cyrup-ext-subagents` | yes | no | no |
| §4 work needed | all of it | all of it | §4 minus the flag |

**Recommendation: B (tmux), with the trait left in place so A can be added later as a second impl.**
Three reasons:

1. **Upstream is already moving there.** `v0.11.0` (`4af53db`) added a `tmuxPane` envelope field and
   renders `· tmux ${session.tmuxPane}` in the roster ([`index.ts:527-530`, `:551`, `:891-900`](../../tmp/pi-intercom/index.ts#L527)),
   and [`types.ts:36-42`](../../tmp/pi-intercom/types.ts#L36) says the pane id exists so a supervisor
   "can drive that pane via tmux" — explicitly listing Herdr sessions as the ones that *lack* it.
   **ICOM-058 is the sibling task that ports exactly that field.** A tmux launcher and ICOM-058's
   roster column are the same surface seen from both ends.
2. **No new dependency and no contradiction.** `tmux split-window -h -c <root> -P -F '#{pane_id}'`
   (plus `-d` when `focus == false`) does the whole job in one command, returns the pane id on
   stdout, and needs nothing cyrup does not already have. `$TMUX` unset is a clean
   `Unavailable`; `tmux -V` below 1.9 (no `-c`) is a clean `UnsupportedVersion`; a `can't find pane`
   stderr normalizes to `NotFound` through [`normalize_code`](#41-new-module-crates-cyrup-intercom-src-project_panears).
   All six codes get a real condition, from a backend cyrup can actually reach.
3. **A is not foreclosed.** The trait is the whole point: a `HerdrLauncher` is additive, and the
   selection can later become config (`IntercomConfig`) without touching §4.

**If the answer is C (no launcher):** §4 still ships, minus §4.4's params/schema and minus the
launch arm of `resolve_cwd_delivery_target`. In that case the `Missing` arm must return a *real*
contract instead of today's dead end, and the two stale `[CYRUP-DELTA]` quotations must still go:

```rust
        ProjectTargetResolution::Missing { reason, target_cwd } => Err(ToolError::new(format!(
            "{reason} cyrup cannot start a session in {target_cwd}; start one there, or use \
             intercom({{action:\"list-cwd\"}}) to find a directory that has one."
        ))),
```

Under C the shipped skill keeps its adapted passages, `SKILL.md:175` stays true, and
`resources.rs:164` stays as written — but the front-matter promise *"Restore upstream's text
verbatim the moment the Herdr pane launcher lands"* must be rewritten to record that the launcher is
declined, not pending, so the next sweep does not re-open this.

**Sketch of the recommended impl** (Part B only — everything it touches is inside one file):

```rust
/// `createHerdrClient` + `openProjectPane` (`project-agent.ts:68-139`, `:227-253`) against tmux.
pub struct TmuxPaneLauncher {
    /// `PI_INTERCOM_PI_BIN ?? PI_BIN ?? "pi"` (`:245`) — cyrup's own binary, resolved like
    /// `transport::spawn::resolve_broker_command` does, via `current_exe()`.
    agent_command: std::ffi::OsString,
}

#[async_trait::async_trait]
impl ProjectPaneLauncher for TmuxPaneLauncher {
    fn name(&self) -> &'static str { "tmux" }

    async fn open(&self, request: ProjectPaneRequest<'_>) -> Result<ProjectPaneLaunch, PaneLaunchError> {
        // `detectHerdr` (`:154-162`): availability THEN version, each its own code.
        //   $TMUX unset            → Unavailable        ("Not running inside tmux; …")
        //   `tmux -V` unparseable  → ValidationError    (`:159`)
        //   tmux < 1.9 (no `-c`)   → UnsupportedVersion (`:160`)
        // Then ONE command does upstream's split+run, so `:248`'s compensating `pane close` has no
        // window to leak in — tmux closes the pane itself when the command exits.
        //   tmux split-window -h -c <project_root> -P -F '#{pane_id}' [-d] -- <agent_command>
        // Empty stdout → PaneGone (`:243`); non-zero exit → normalize_code(stderr).
        // The whole spawn races `request.cancel.cancelled()` → Timeout (`:95-98`).
        todo!("Part B — gated on §5")
    }
}
```

Bind it where `HostServices` is bound — [`IntercomExtension`](../../crates/cyrup-intercom/src/extension.rs)'s
`set_host_services` path — calling `state.set_project_pane_launcher(...)` only when the backend
reports itself available, so a non-tmux session simply has no launcher and §4.4's
`UnavailableLauncher` answers with the truth.

---

## 6. Definition of Done

Observable behavior, in a live two-session broker.

**Independent of §5:**

1. `grep -rn openProjectPaneIfMissing crates/cyrup-intercom` returns **no** occurrence that
   describes the string as something cyrup emits or would emit, unless
   `parameters_schema()["properties"]` contains the key. The two `[CYRUP-DELTA]` blocks at
   `project_target.rs:4-21` and `tools/intercom/mod.rs:80-101` are gone or rewritten to describe what
   the build now does.
2. `intercom({action:"send", cwd:"<dir with a peer>", message:"hi"})` and the `ask` equivalent behave
   **exactly as today** — same target, same label, same result string, same `details`. The `Found`
   path is untouched.
3. All five `resolve_target_in_cwd` outcomes (sole peer, zero peers, the three ambiguity strings) are
   byte-for-byte unchanged.
4. A `send`/`ask` at a directory with no live peer returns a message that names a **real** next
   action. It never names a parameter the schema rejects.

**If §5 answers A or B:**

5. `intercom({action:"send", openProjectPaneIfMissing:true})` **without** `cwd` returns exactly
   `openProjectPaneIfMissing requires a target cwd.` — and does so before any dialog or process.
6. `intercom({action:"send", cwd:"<empty dir>", openProjectPaneIfMissing:true, message:"hi"})` opens a
   visible pane in that directory, starts cyrup in it, waits for it to register, delivers, and
   returns `Opened <backend> project pane <id> for <root> and sent message to <peer>` with
   `details.openedProjectPane == true`, `details.paneId`, `details.projectRoot`.
7. The same call with `focus:false` opens the pane **unfocused**; with `focus` omitted it opens
   **focused**.
8. `cwd` pointing at a file or a non-existent path returns `Project target '<abs path>' is not a
   directory.` and starts no process.
9. With `confirmSend` enabled and a UI, the launch variant asks for confirmation **once, before the
   pane is opened**, labelled with `to` or the `cwd`; declining leaves no pane behind and returns
   `Message cancelled by user`. The non-launch variant still confirms after resolution, labelled with
   the resolved peer. Nobody is ever asked twice.
10. Each of the six codes is reachable from its own real condition and renders as
    `<backend> project pane error (<CODE>): <message>` — in particular, no backend installed yields
    `HERDR_UNAVAILABLE` with a message naming what to install, and a pane that never registers within
    20 s yields the timeout sentence naming the project root.
11. Cancelling the tool call during a launch or during the registration wait returns `Cancelled`
    rather than blocking to the 20 s deadline.
12. `ask` with a launch returns the `**Reply from <peer>:**` body **and** the `openedProjectPane`
    details; `ask` without one returns the body with no details, as today.
13. The shipped `SKILL.md` documents `openProjectPaneIfMissing` / `focus` again, no longer claims
    `cwd` "never starts one", and every parameter it names is advertised by `parameters_schema()`.

**If §5 answers C:**

14. `openProjectPaneIfMissing` is absent from the schema and from `IntercomParams`, and passing it is
    rejected as an unknown field — the same refusal as today, but now the missing-peer error states
    the limitation in its own words instead of pointing at a flag.
15. `SKILL.md`'s front-matter delta note records the launcher as **declined**, with the reason, so the
    next parity sweep reads a decision rather than an open question.
