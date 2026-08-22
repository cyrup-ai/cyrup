---
stage: new
status: done
updated: 2026-08-22 06:00
---

# MCP-115 / F5: A 401 With A JSON-RPC Body Never Becomes An Error

## Description

Measured and recorded at `docs/gap-analysis/13-cyrup-mcp-STATUS.md:251`, deliberately left unfixed
in PR #30 because it is a second, distinct mechanism from the one F5 addressed.

The bare-401 fix works for a 401 with no body. It does **not** work for a 401 carrying
`Content-Type: application/json` and a parseable JSON-RPC error, because rmcp applies its
JSON-RPC-error shortcut to **every** non-success status, not just 400:
`rmcp-3.1.4/src/transport/common/reqwest/streamable_http_client.rs:278-293` returns
`Ok(StreamableHttpPostResponse::Json(..))` for that case, so the
`Err(UnexpectedServerResponse("HTTP {status}: {body}"))` at `:296` — which `runtime.rs:2063`
prefix-matches — is never constructed. `bare_unauthorized` cannot fix it: it is never called.

Measured through the real `ConnectionBuilder::connect_http_client` against a loopback fixture
answering `initialize` with 401 + a JSON-RPC error body: the connect ends as a hard failure and
the OAuth ladder is never reached. **A server that answers this way — which is legal, and which
the MCP spec's own error shape encourages — can never authenticate.**

Fix shape: catch the status before rmcp collapses it, in the client-decorator seam this crate
already occupies (`SessionIdProbe` / `RequestHeadersCommandClient`), raising the unauthorized
shape whenever the response was HTTP 401 regardless of body; or carry the status out of the
decorator into the ladder.

**Do the fixture first.** The ladder tests need a `json_rpc_body` mode on `FixtureOptions`
alongside `challenge: false`. The fixture's inability to produce this shape is exactly why the
gap went unseen, so a fix without the fixture leaves the next one unseen too.

Fails SAFE today — a hard connect error, never a wrongly-authenticated request — so this is
correctness, not a security hole.

## Acceptance Criteria

- [ ] `FixtureOptions` can answer `initialize` with 401 + a JSON-RPC error body
- [ ] A test asserts the OAuth ladder IS reached for that response, and fails before the fix (ablation)
- [ ] The bare-401 case still works — do not regress F5
- [ ] `MCP-115`'s row and the "Still open" section of `13-cyrup-mcp-STATUS.md` are updated
- [ ] `cargo nextest run --workspace` and `cargo clippy --workspace --all-targets` are clean
