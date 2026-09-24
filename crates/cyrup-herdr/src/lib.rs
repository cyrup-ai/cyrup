//! `cyrup-herdr` — the workspace's one client for herdr's newline-delimited JSON socket API.
//!
//! Pinned to herdr **v0.9.1** (`tmp/herdr` @ `d59d060`, `Cargo.toml:3`). Every claim in this crate's
//! docs cites herdr's own source, not a consumer's use of it.
//!
//! # The shape of the protocol, in one place
//!
//! **Envelope.** `{"id":"req_1","method":"ping","params":{}}` — `Request`
//! (`tmp/herdr/src/api/schema.rs:35-39`) is an `id` plus a `#[serde(flatten)]`ed `Method`, and
//! `Method` is adjacently tagged `#[serde(tag = "method", content = "params")]` (`:40-47`). So
//! **`params` is mandatory on every method, `ping` included**: the published schema's `ping`
//! variant lists `"required": ["method","params"]`, and `{"id":"x","method":"ping"}` is rejected.
//!
//! **Answer.** Either `{"id":…,"result":{"type":…,…}}` (`schema/response.rs:24-28`, internally
//! tagged over 65 variants) or `{"id":…,"error":{"code":String,"message":String}}` (`:30-39`).
//! `code` is a bare string on the wire — not an enum — and is interpreted after decoding by
//! [`ApiErrorCode::from_wire`].
//!
//! **Framing.** One JSON value per `\n`-terminated line, both directions
//! (`tmp/herdr/src/api/client.rs:185-190`, `tmp/herdr/src/api/server.rs:539-583`).
//!
//! **Connection model — one request per connection.** `handle_connection_with_stop`
//! (`tmp/herdr/src/api/server.rs:156-317`) reads one line, dispatches, writes one line, and
//! returns; there is no read loop. Exactly three things hold a connection open past the first
//! answer: `events.subscribe`, `pane.graphics.stream`, and the four in-band waits. See
//! [`transport`] for why every independent client agrees, and what follows from it.
//!
//! **The one long-lived connection.** `events.subscribe` acknowledges and then pushes events on
//! the same connection ([`stream`]). Because a snapshot cannot share that connection and a
//! subscription does not replay
//! (`tmp/herdr/docs/preview/website/src/content/docs/socket-api.mdx:118-130`, `:824-826`), pairing
//! a cache with a stream has exactly one correct order — so [`HerdrClient::bootstrap`] performs it
//! as one call, once, rather than leaving every consumer to re-derive it. That is a
//! `[CYRUP-EXCEEDS-UPSTREAM]`; the premise and pi's contrasting shape are on the method. It is a
//! ready-made correct order, **not** a surface that makes the wrong one unreachable:
//! [`HerdrClient::session_snapshot`] and [`HerdrClient::subscribe`] are both public and compose
//! into exactly the losing order, which is why [`HerdrClient::subscribe`] says so on itself.
//!
//! # The three states this crate keeps distinguishable
//!
//! | state | detection | what happens |
//! |---|---|---|
//! | not in a herdr pane | [`HerdrPane::discover`] → `None` | **nothing runs**: no client, no connect, no task, no log above `trace` |
//! | in a pane, server gone | first call → `ECONNREFUSED`/`ENOENT` | [`Unavailable::NoSocket`] carrying the path, surfaced to the caller |
//! | a method this build lacks | `invalid_request` on that one method | [`HerdrError::is_unsupported_method`] → that one feature off, the client stays live |
//!
//! A fourth state is not the socket's at all: **the resolved socket path does not connect, but a
//! `herdr` binary does.** (Not "no path resolves": [`env::resolve_socket_path`] is infallible and
//! always answers a `PathBuf` — `env.rs:156-158` says so in the same crate — so that trigger does
//! not exist.) [`cli::HerdrCli`] shells out to the binary, and herdr's CLI is itself a socket
//! client (`tmp/herdr/src/cli.rs:769-775`) that knows where herdr lives even when this crate's
//! replication of that ladder guessed the wrong config dir.
//!
//! **This crate does not make that hop for you.** [`HerdrClient`] never constructs a
//! [`cli::HerdrCli`]; every socket verb ends at [`transport::request`] and stops at
//! [`Unavailable::NoSocket`]. The two routes are offered side by side and a consumer composes
//! them — `cyrup-intercom`'s `HerdrLauncher` takes the CLI route outright. An automatic
//! socket-to-CLI degradation is the seam AUG §11 assigns to step 5, and it is not shipped here.
//! One verb could never take it in any case: there is no `events` module in
//! `tmp/herdr/src/cli/`, so an event consumer genuinely requires the socket and says so
//! ([`Unavailable::NoSocket`]) rather than pretending. With no binary either, that is
//! [`Unavailable::BinaryMissing`] — the honest end of the ladder, and the state of this container.
//!
//! No fabricated success anywhere: a result type this client does not recognise is
//! [`schema::ResponseResult::Unrecognised`] and every typed accessor turns it into
//! [`HerdrError::UnexpectedResult`], never a default.
//!
//! # What is ported, and what is deliberately not
//!
//! herdr publishes 105 renamed methods (`tmp/herdr/src/api/schema.rs:47-271`). [`HerdrClient`]
//! carries **28** of them (every variant of [`schema::Method`]; 27 go through
//! [`HerdrClient::call`] and the twenty-eighth, `events.subscribe`, keeps its connection and is
//! driven by [`HerdrClient::subscribe`]). The remaining **77** are absent, and each block below
//! says what would have to exist first — "large" is never the reason, because the envelope is
//! generic and each method is one enum variant.
//!
//! The four newest — `workspace.create`, `tab.create`, `agent.start` and `agent.prompt` — are
//! saved-machine placement's (`crates/cyrup-ext-subagents/src/placement/`): it opens a
//! herdr-owned pane on a remote machine through a forwarded socket ([`remote`]) and launches the
//! placed child there, exactly the verbs pi-subagents drives
//! (`src/runs/shared/herdr-placed-run.ts:135-175` @v0.68.0).
//!
//! **28 + 77 = 105, and the counts below add up to 77.** That is the point of stating them: a
//! family whose size is guessed hides a method nobody decided about. `pane.rename`,
//! `pane.send_text` and `pane.send_keys` were exactly that — absent from every row of this table
//! while being absent from the client too.
//!
//! | not ported | count | what would need it |
//! |---|---|---|
//! | `events.wait`, `agent.wait` | 2 | two of the methods that hold a connection open past the first answer (`tmp/herdr/src/api/server.rs:251-288`); the third, `agent.prompt`, is ported for placement and waits through its own `wait` option. Unlike `events.subscribe` they answer **once** and then close, so they need no stream — they need a *sibling-agent* caller, which no batch has yet. |
//! | `pane.graphics.{info,set,clear,stream}`, plus the 4 `#[serde(skip)]` frame variants that are not among the 105 | 4 | the wire contract is a raw-byte side channel after the JSON header (`socket-api.mdx:204-236`). **cyrup has no image producer**, so these methods would have no argument to carry. |
//! | `plugin.*` (11), `integration.*` (3) | 14 | require shipping a `herdr-plugin.toml` package (`socket-api.mdx:499-560`) — a distinct deliverable with its own install story. |
//! | `worktree.*` | 4 | herdr worktrees create **herdr workspaces**. cyrup owns its worktrees through `gix` (`crates/cyrup-ext-subagents/src/spawn/worktree.rs`); adopting herdr's would move ownership of a feature that already works. |
//! | pane geometry, scrollback and raw input — `swap` `move` `zoom` `resize` `neighbor` `edges` `focus_direction` `scroll` `copy_motion` `copy_search` `selection.read` `edit_scrollback` `clear` `input.set` `link.activate` `link.resolve` `layout` `rename` `send_text` `send_keys` + `layout.{export,apply,set_split_ratio}` | 23 | **no consumer arranges the user's terminal.** Every batch in flight splits, reads, writes and closes; none moves panes around. `send_text`/`send_keys` are the raw halves of `pane.send_input`, which is the one this client sends because it is what `herdr pane run` is (`tmp/herdr/src/cli/pane.rs:1046-1052`). Add on first caller. |
//! | `server.stop`, `server.live_handoff` | 2 | destroy or restart the user's whole terminal session. |
//! | `workspace.*` except `create` (8), `tab.{list,focus,move,close}` (4), `agent.{read,explain,send_keys,rename,focus}` (5), `command.invoke`, `popup.close`, `notification.show`, `client.window_title.{set,clear}` (2), `product_announcement.dismiss`, `release_notes.dismiss`, `server.reload_config`, `server.{agent_manifests,reload_agent_manifests}` (2) | 27 | no caller yet. |
//! | `client_shell.surface.set` | 1 | **cannot be ported.** The raw socket refuses it by construction — `connection_local_only`, `tmp/herdr/src/api/server.rs:370-376`. It is absent, not deferred. |
//!
//! # A leaf, deliberately
//!
//! This crate depends on no `cyrup-*` crate. The reason is the **build target**, not the
//! dependency graph: `cyrup-ext-subagents` carries `nix`/`libc` ungated, so it cannot target
//! Windows today, while herdr's socket is explicitly Windows-capable
//! (`tmp/herdr/src/ipc.rs:44-51`).
//!
//! The graph argument would not have held, and is not made: `cyrup-intercom` already depends on
//! `cyrup-ext-subagents` (`crates/cyrup-intercom/Cargo.toml:56`, one-directional), so hosting
//! this client inside `cyrup-ext-subagents` would have forced no new edge at all. Both layers now
//! consume it: `cyrup-intercom` (`src/project_pane.rs`'s `HerdrLauncher`, the CLI route) and
//! `cyrup-ext-subagents` (`src/herdr/`, the status bridge). The note that used to stand here —
//! that the second grep was empty — became false when that bridge landed.
//! See this crate's `Cargo.toml` for the full statement.
//!
//! No-panic policy (arch-00 §8). The first four lints below are denied workspace-wide via
//! `[workspace.lints]` and restated here; `unreachable`/`todo`/`unimplemented` are denied at this
//! crate root only.
#![deny(
    clippy::unwrap_used,
    clippy::expect_used,
    clippy::panic,
    clippy::indexing_slicing,
    clippy::unreachable,
    clippy::todo,
    clippy::unimplemented
)]
#![forbid(unsafe_code)]

pub mod cli;
pub mod client;
pub mod env;
pub mod error;
pub mod machine;
pub mod probe;
pub mod reconnect;
#[cfg(unix)]
pub mod relay;
#[cfg(unix)]
pub mod remote;
pub mod schema;
pub mod stream;
pub mod transport;

#[cfg(test)]
mod tests;

pub use cli::{CliError, CliOutput, HerdrCli};
pub use client::{HerdrClient, WAIT_GRACE};
pub use env::{EnvSource, HerdrPane, ProcessEnv, resolve_socket_path};
pub use error::{ApiError, ApiErrorCode, HerdrError, Result, Unavailable};
pub use probe::{Capability, Pong, ping, ping_current_pane, ping_for};
pub use reconnect::{
    Backoff, RECONNECT_ATTEMPTS, RECONNECT_INITIAL_DELAY, RECONNECT_MAX_DELAY, ReconnectingEvents,
    StreamItem,
};
pub use schema::{
    Event, EventData, EventEnvelope, EventKind, Method, Request, ResponseResult,
    ServerCapabilities, Subscription, SubscriptionEventData, SubscriptionEventEnvelope,
    SubscriptionEventKind,
};
pub use stream::{HerdrEvents, MAX_STREAM_BYTES};
pub use transport::{DEFAULT_TIMEOUT, LocalStream, MAX_REQUEST_BYTES, MAX_RESPONSE_BYTES};
