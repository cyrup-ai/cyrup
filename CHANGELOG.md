# Changelog

All notable changes to cyrup are recorded here. Each released version gets a `## x.y.z` section;
the newest is first. `/changelog` renders this file inside the agent, and a new entry raises the
"What's New" notice on the next start.

## 0.1.0 - 2026-10-11

The first versioned pre-release. Development began 2026-08-27; this entry covers 521 commits
across 149 merged pull requests.

cyrup is a coding agent in Rust: one static binary, 40 model providers, and extensions that run as
sandboxed WebAssembly components. It follows the design of the
[Pi](https://github.com/earendil-works/pi) agent harness — a minimal core, everything-is-an-extension,
an agent that can extend itself — rebuilt on a Rust backbone across 33 crates.

### Working end to end

- The agent loop, the provider layer and the tool set.
- The session tree, with fork, clone and resume.
- The terminal interface, in both the inline viewport and the alternate screen.
- The extension host, with native and WebAssembly guests.
- All five run modes.
- The MCP client and the ACP adapter.
- The `workflowScript` runtime.
- Background and remote subagent delegation.
- The herdr integration.

### Parity method

cyrup is a port, so its behaviour is adjudicated against its upstreams rather than designed from
scratch. Divergences are tracked as a ledger of behavioural differences under
[`docs/gap-analysis/`](docs/gap-analysis), each entry carrying the upstream citation it was measured
against and the test that proves its closure. 1,124 entries are closed. Where cyrup and an upstream
disagree, the upstream is treated as correct unless the divergence is recorded, with a reason, as a
`[CYRUP-DELTA]` in the source.

Upstream citations in the source name the exact TypeScript file and line each behaviour came from,
so a reader can check the port against the original.
