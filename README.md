# cyrup

A from-scratch **Rust** port of the [Pi](https://github.com/earendil-works/pi) agent harness —
the same design philosophy (minimal core, everything-is-an-extension, the agent extends itself),
built to be faster and more memory-efficient.

> **Status:** scaffold. The crate graph and conventions are in place; subsystems are stubs.
> The authoritative design lives in [`../spec`](../spec):
> - `spec/functionality/*.md` — language-agnostic conformance targets (`R-*` requirements).
> - `spec/architecture/*.md` — the Rust design (`arch-00`…`arch-12`), ADR-0001 (ratatui), ADR-0002 (WASM), and `REVIEW.md`.

## Workspace layout (arch-00 §2)

| Crate | Role | arch |
|-------|------|------|
| `cyrup-core` | shared substrate: ids, `Content`/`Message`, `EventStream<T>`, `CancelToken`, `Tool` trait, errors | arch-00 |
| `cyrup-provider` | vendor-neutral LLM layer (providers, auth, streaming, handoff) | arch-01 |
| `cyrup-agent` | turn-based agent loop, events, tool execution, hooks | arch-02 |
| `cyrup-tools` | built-in tools (`read`/`write`/`edit`/`bash`/`grep`/`find`/`ls`) + `FsOps`/`ProcOps` seam | arch-03 |
| `cyrup-session` | JSONL session tree, compaction, system-prompt/context assembly | arch-04/05/06 |
| `cyrup-config` | layered settings, project trust, auth store, model resolution | arch-07 |
| `cyrup-ext` | WASM Component Model host (Wasmtime) + native built-in extensions | arch-08 |
| `cyrup-resources` | skills, prompt templates, themes, packages | arch-09 |
| `cyrup-tui` | ratatui + crossterm front-end | arch-10 |
| `cyrup-modes` | print / json / rpc adapters | arch-11 |
| `cyrup-sdk` | public embeddable API | arch-11 |
| `cyrup-session-svc` | the `AgentSession` facade wiring everything (the one seam) | arch-11 |
| `cyrup` | the CLI binary | arch-11 |
| `cyrup-test-support` | faux provider + headless harnesses (dev) | arch-00 §11 |
| `cyrup-ext-sdk` | guest SDK for authoring Rust extensions (`wasm32-wasip2`) | arch-08 |

Dependencies point downward only; `cyrup-core` depends on nothing in-workspace; `cyrup-ext` does
not depend on `cyrup-tui` (extension UI crosses as serializable commands).

## Build

```sh
cargo check        # builds the host workspace (default-members)
cargo test         # unit/integration tests
cargo clippy       # enforces the no-panic policy (arch-00 §8)
```

`cyrup-ext-sdk` is a wasm guest crate, excluded from default-members:
`cargo build -p cyrup-ext-sdk --target wasm32-wasip2` (after `rustup target add wasm32-wasip2`).

## License

MIT
