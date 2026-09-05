# cyrup-acp

The Agent Client Protocol adapter: cyrup's fourth front-end, beside the terminal interface,
`--mode rpc` and print/json. An editor — Zed is the reference client — speaks ACP JSON-RPC 2.0 over
stdio, and this crate answers it by driving the same `AgentSession` the other three drive.

Users reach it as `cyrup --acp` (or `--mode acp`). See
[Zed and other ACP editors](../../docs/guide/guides/zed-acp.md) for the editor-side setup.

The crate runs under the workspace no-panic policy: `unwrap`, `expect`, `panic` and slice indexing
are denied in non-test code.

## Quickstart

`serve_stdio` is the whole entry point. Supply an `AcpHost` — the trait that knows how to build a
session — and it serves until the client hangs up.

```rust
use std::sync::Arc;

use cyrup_acp::{AcpError, AcpHost, AgentSessionRuntime, BoxFuture, RuntimeRequest, SessionsRoot};

struct Host {
    runtime: Arc<AgentSessionRuntime>,
    root: SessionsRoot,
}

impl AcpHost for Host {
    fn build_runtime<'a>(
        &'a self,
        _req: &'a RuntimeRequest,
    ) -> BoxFuture<'a, Result<Arc<AgentSessionRuntime>, AcpError>> {
        let runtime = Arc::clone(&self.runtime);
        Box::pin(async move { Ok(runtime) })
    }

    fn runtime_ready(&self, _runtime: &Arc<AgentSessionRuntime>) {}

    fn sessions_root(&self) -> SessionsRoot {
        self.root.clone()
    }
}

# async fn run(host: Arc<dyn AcpHost>) -> Result<(), AcpError> {
cyrup_acp::serve_stdio(host).await
# }
```

`serve` takes any transport rather than stdio, which is how `tests/end_to_end.rs` drives the real
connection over a pair of in-memory channels with no process to spawn.

## In-process, which is the inversion of upstream

Upstream `pi-acp` is a separate npm package that cannot link into pi, so it spawns `pi --mode rpc`
as a child and bridges two wires: ACP on its own stdio, and newline-delimited JSON on the child's.
Every fact it holds about the agent arrives as an untyped `Record<string, unknown>`.

`cyrup-acp` is a workspace crate and binds to `AgentSession` directly, so it consumes
`AgentSessionEvent` typed. The subprocess layer has no counterpart here and is not ported: no
child process, no ANSI prelude scraping, no UUID correlation map, and none of the defensive
key-probing those require.

Two things decided it. `cyrup_modes::is_upstream_wire_event` deliberately keeps `SessionReplaced`,
`ModelChanged`, `SessionStart` and `SessionShutdown` off the RPC wire, so an out-of-process adapter
would have no source for ACP's `current_mode_update` or `available_commands_update`. And the
permission path only closes in-process: the sink receives the typed `UiRequest`/`UiKind` with its
`oneshot::Sender<UiReply>`, where pi-acp infers the dialog from its title and synthesises
`allow_once` for every select.

The cost is one live session per connection. `NativeExtension::set_host_services` stashes host
services in first-write-wins `OnceLock` slots, so the limit is structural rather than a policy.

## What a client gets

`initialize`, `session/new`, `session/prompt`, `session/cancel`, `session/load`, `session/list`,
`session/delete`, `session/set_mode` and `session/set_config_option`. Turns stream back as
`session/update` notifications, and permission requests go out as `session/request_permission`.

Four behaviours are worth knowing before reading the code:

- **A prompt resolves only on `AgentSettled`.** Several `TurnEnd`/`AgentEnd` events arrive per
  prompt during retry, compaction and queue drain. Settling on either returns `EndTurn` while the
  run is still streaming, and the client closes the turn and then watches text arrive into a
  session it believes is idle. The `Turn` enum owns the responder, so it cannot be answered twice
  or dropped unanswered.
- **Tool-call status is monotonic**, and the first emission for an id is always `tool_call`. A
  client that saw `in_progress` regress to `pending` would hide its progress UI.
- **Sessions are the same JSONL tree the terminal interface reads.** `cyrup_session::listing` is
  the only source for the session-id-to-file mapping, which is why there is no sidecar index.
- **Stdin EOF is the normal way an editor exits.** `serve` calls `SessionManager::shutdown` on
  every exit path, so the session is disposed, `session_shutdown` is emitted, and tracked bash
  process groups are cancelled rather than orphaned.

## Module map

| Module | |
|---|---|
| `connection` | Transport bootstrap, the `Agent` builder chain, handler registration |
| `sessions` | The one-live-session manager and the `session/*` entry points |
| `turn` | The ACP turn: sole owner of a `session/prompt`'s responder, and the actor driving it |
| `translate` | The pure core: `AgentSessionEvent` → `Vec<SessionUpdate>`, over an explicit ledger |
| `ledger` | The tool-call ledger and terminal appender — the state the translator mutates |
| `permission` | `UiRequest` in, `session/request_permission` out |
| `commands` | Slash-command translation, the catalog projection, the headless dispatcher |
| `config_options` | The `initialize` advertisement, modes and config options |
| `startup` | The markdown startup prelude and the modelless banner |
| `ids` | The three client-supplied strings that become filesystem authorities |
| `error` | How a cyrup failure is presented to the client, and the crate's error type |

`translate` is a pure function over an explicit ledger, so the event-to-notification mapping is
table-testable without a connection. `connection` and `sessions` hold everything impure.

## Tests

`cargo test -p cyrup-acp` runs 243 unit tests and 5 end-to-end tests. The end-to-end file builds a
real `AgentSessionRuntime` over the scripted `FauxProvider`, installs it behind `AcpHost`, and runs
the shipped `serve()` over an in-memory transport — so the frames it asserts are the bytes a client
receives. No network, no credentials, no spawned process.

`the_frame_sequence_is_stable` pins the exact ordered sequence for a tool-calling turn, so a change
to what a client sees is named rather than discovered downstream.

## Provenance

Ported from [`svkozak/pi-acp`](https://github.com/svkozak/pi-acp) v0.0.33, MIT © Sergii Kozak. Read
upstream with `git -C tmp/pi-acp show v0.0.33:<path>`; the clone's working tree is two README-only
commits ahead of the tag.

The wire types are `agent-client-protocol` 2.1 with default features. Do not enable
`unstable_protocol_v2`: it arms a version guard that hard-errors on an unknown `protocolVersion`,
where pi-acp and this port downgrade gracefully.

The port plan and its unit table are in
[`docs/gap-analysis/15-cyrup-acp.md`](../../docs/gap-analysis/15-cyrup-acp.md); the type-design
decisions are in [`ADR-0028`](../../docs/adr/ADR-0028-cyrup-acp-type-design.md). Behaviour that
deliberately differs from upstream is marked `CYRUP-DELTA` in the source, each naming the upstream
line and the reason.
