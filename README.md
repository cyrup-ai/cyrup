# cyrup

A from-scratch **Rust** port of the [Pi](https://github.com/earendil-works/pi) agent harness —
the same design philosophy (minimal core, everything-is-an-extension, the agent extends itself),
built to be faster and more memory-efficient.

> **Status:** implemented, and under active parity work. 18 crates, ~250k lines of Rust;
> `cargo check --workspace --all-targets` is clean. The agent loop, provider layer, tool set,
> session tree, TUI, extension host and all four run modes are real and wired end to end.
> What remains is *behavioral equivalence* with the upstreams — see
> [Coverage and known gaps](#coverage-and-known-gaps). This is not a released product.

## What it ports

cyrup tracks four TypeScript upstreams. The core is [`earendil-works/pi`](https://github.com/earendil-works/pi);
three optional subsystems port standalone Pi extensions, since Pi core deliberately ships no
permission system of its own.

| upstream | ported by | ported version |
|---|---|---|
| `earendil-works/pi` | most crates | v0.83.0 |
| `nicobailon/pi-subagents` | `cyrup-ext-subagents` | ~v0.33.x–v0.34.0 (crate records no version) |
| `MasuRii/pi-permission-system` | `cyrup-permission-system` | v0.7.1 |
| `nicobailon/pi-intercom` | `cyrup-intercom` | v0.7.0 (its `lib.rs` still says v0.6.0) |

Provenance lives in the code: doc comments cite the upstream `.ts` file — and usually the exact
line range — that each Rust item ports. There are 6,237 such citations across 384 source files.
That index is how parity is audited; preserve and extend it when editing.

## Workspace layout (arch-00 §2)

| Crate | Role | ports from |
|-------|------|------------|
| `cyrup-core` | shared substrate: ids, `Content`/`Message`, `EventStream<T>`, `CancelToken`, `Tool` trait, errors | `pi/packages/ai` (types, event-stream, diagnostics) |
| `cyrup-provider` | vendor-neutral LLM layer (providers, auth, streaming, catalogs, images) | `pi/packages/ai` (whole package) |
| `cyrup-agent` | turn-based agent loop, events, tool execution, hooks, steering/follow-up queues | `pi/packages/agent` |
| `cyrup-tools` | built-in tools (`read`/`write`/`edit`/`bash`/`grep`/`find`/`ls`) + `FsOps`/`ProcOps` seam | `pi` harness + coding-agent tools |
| `cyrup-session` | JSONL session tree, compaction, system-prompt/context assembly | `session-manager.ts`, `compaction.ts` |
| `cyrup-config` | layered settings, project trust, auth store, model resolution | `settings-manager.ts` et al. |
| `cyrup-ext` | WASM Component Model host (Wasmtime) + native built-in extensions | `coding-agent/src/core/extensions/` |
| `cyrup-resources` | skills, prompt templates, themes, packages | `skills.ts`, `package-manager.ts` |
| `cyrup-tui` | ratatui + crossterm front-end | `pi/packages/tui` + `modes/interactive/` |
| `cyrup-modes` | print / json / rpc adapters | `modes/{print-mode,rpc}` |
| `cyrup-sdk` | public embeddable API | `core/sdk.ts` |
| `cyrup-session-svc` | the `AgentSession` facade wiring everything (**the one seam**) | `core/agent-session*.ts` |
| `cyrup` | the CLI binary | `main.ts`, `cli.ts` |
| `cyrup-ext-subagents` | OS-subprocess subagent delegation (the largest crate, 87k LOC) | `pi-subagents` |
| `cyrup-permission-system` | runtime allow / ask / deny policy over every tool call | `pi-permission-system` |
| `cyrup-intercom` | Unix-socket broker for supervisor↔subagent coordination | `pi-intercom` |
| `cyrup-test-support` | faux provider + differential / interop / golden parity harnesses (dev) | cyrup-original |
| `cyrup-ext-sdk` | guest SDK for authoring Rust extensions (`wasm32-wasip2`) | cyrup-original |

Dependencies point downward only; `cyrup-core` depends on nothing in-workspace; `cyrup-ext` does
not depend on `cyrup-tui` (extension UI crosses as serializable commands). `cyrup-session-svc` is
the only crate that depends on all eight lower crates (`core`, `provider`, `agent`, `tools`,
`session`, `config`, `ext`, `resources`), and the only one the front-ends consume.

The three extension crates are **default OFF**. Each activates on its own env flag
(`CYRUP_SUBAGENTS` / `CYRUP_PERMISSION_SYSTEM` / `CYRUP_INTERCOM`) or on the presence of its
config file — note that dropping a policy file into a repo is enough to arm the permission gate.

## Design docs

The authoritative design lives in a **separate `spec/` tree that is not vendored into this
repository**; the code refers to it as `../spec`:

- `spec/functionality/*.md` — language-agnostic conformance targets (`R-*` requirements).
- `spec/architecture/*.md` — the Rust design (`arch-00`…`arch-12`), ADR-0001 (ratatui), ADR-0002 (WASM).

The code cites those identifiers ~2,400 times. Even without the tree checked out the vocabulary is
a usable search index — `grep -rn "R-08-010" crates` finds every site implementing that requirement.

## Build

```sh
cargo check        # builds the host workspace (default-members)
cargo test         # unit/integration tests
cargo clippy       # REQUIRED — the no-panic policy only fires here
```

`cargo clippy` is not optional. The no-panic policy (arch-00 §8) is expressed as
`[workspace.lints.clippy]` denials of `unwrap_used` / `expect_used` / `panic` /
`indexing_slicing`, and **clippy tool lints do not fire under `cargo build` or `cargo test`.**
There is no CI in this repository, so nothing runs these for you.

`cyrup-ext-sdk` is a wasm guest crate, excluded from default-members:
`cargo build -p cyrup-ext-sdk --target wasm32-wasip2` (after `rustup target add wasm32-wasip2`).

Edition 2024, `resolver = "3"`, MSRV 1.96. A cold first build compiles the full dependency graph
(wasmtime, cranelift, gix, reqwest/rustls) — budget a few minutes and don't mistake it for a hang.

## Testing

~3,180 test functions. Roughly three-quarters are inline `#[cfg(test)]` modules under
`crates/*/src/`; the remaining 120 files live in `crates/*/tests/` (`cyrup-tui` and
`cyrup-session-svc` carry the most).

`cyrup-test-support` holds the parity machinery: `differential.rs` (diffs cyrup's emitted event
sequence against the Pi-shaped expected sequence), `interop.rs` (Pi-shaped session JSONL
round-trip), and `golden.rs` (normalized JSONL snapshots — regenerate with `UPDATE_GOLDEN=1`).
Offline coverage comes from `cyrup-provider`'s `faux` feature, a 1:1 port of Pi's faux provider
with a seeded PRNG so snapshots reproduce.

**Tests must not hit real provider APIs or paid tokens.** This is convention plus the faux
provider, not an enforced guard — the handful of live-API tests are gated behind `#[ignore]` and
an env-var check.

## Coverage and known gaps

Honest accounting, so nobody mistakes an unported feature for a bug:

- **Providers**: 31 of Pi's 38 built-ins are registered, over 6 of its 10 text wire APIs.
  Not ported: `amazon-bedrock`, `github-copilot`, `google-vertex`, `openai-codex` (each blocked on
  an unported wire protocol or auth flow), plus `qwen-token-plan{,-cn}` and `radius`.
  Model catalogs ship as embedded JSON; every catalog belongs to a registered provider.
- **Auth**: stored OAuth credentials are honored, but none of Pi's 11 interactive OAuth *login*
  flows are implemented. Credentials must be obtained out of band.
- **Extensions**: Pi loads TypeScript at runtime via `jiti`; cyrup runs extensions as WASM
  components under Wasmtime (the `wasm-host` feature, default-on) plus the three native built-ins
  above. Extension UI crosses the boundary as serializable commands rather than live components.
- **TUI**: the inline viewport is implemented; Pi's alternate-screen/fullscreen mode, mouse
  support and thinking-block rendering are not.
- **Out of scope**: Pi's `packages/{client,server,protocol,storage,evals}` have no counterpart
  here and are not planned — Pi's own coding-agent does not depend on them either.
- Every upstream has landed changes since the version cyrup ported. Diff the ported tag against
  upstream `HEAD` before treating a difference as a defect.

## License

MIT
