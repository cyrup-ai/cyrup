---
title: The :offset-limit header range disappears when offset/limit arrive as JSON floats
priority: LOW
tool: read
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# The :offset-limit header range disappears when offset/limit arrive as JSON floats

## What pi does

read.ts:73-78 `formatReadLineRange` tests only `args?.offset === undefined && args?.limit === undefined`; any defined number renders, so `{"offset": 2.0}` produces the `:2` suffix on the `read <path>` header (read.ts:80-83).

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tui/src/transcript/tool_args.rs:58-69 `read_line_range` reads both values with `Value::as_i64`, which returns `None` for a `serde_json` float (`json!(2.0)`), so both come back `None` and the function returns `None` — no range is appended. This contradicts the execute path, which deliberately accepts floats (/home/user/cyrup/crates/cyrup-tools/src/tools/read.rs:21-22 and the test at /home/user/cyrup/crates/cyrup-tools/src/tests/tools.rs:2404-2419).

## User-visible impact

A model emitting `{"path":"f.txt","offset":2.0,"limit":3.0}` — which cyrup executes correctly — is displayed as `read f.txt` with no line range, while pi displays `read f.txt:2-4`. The transcript hides which window was actually read.

## Parity action

In `read_line_range`, read the numbers with `Value::as_f64` and fold them with the same `jsnum` truncation the execute path uses, so integral floats render identically to their integer spelling.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Verified and not refutable by finding it elsewhere. read_line_range at /home/user/cyrup/crates/cyrup-tui/src/transcript/tool_args.rs:58-69 is the ONLY implementation of the range suffix (rg for read_line_range/formatReadLineRange yields just its two call sites: tool_args.rs:224 compact path and tool_builtin.rs:22 plain `read <path>` header). It reads both values with Value::as_i64, and serde_json-1.0.150/src/number.rs:143 has `N::Float(_) => None` (with arbitrary_precision it would parse "2.0" as i64 and also fail), so json!(2.0) yields None for both and the function returns None. No float-tolerant alternative exists: cyrup-tui has no numeric-coercion helper (rg as_i64|as_u64|as_f64 over cyrup-tui/src shows only raw as_* calls), and the correct primitive — jsnum::to_integer/to_count (ECMA-262 ToIntegerOrInfinity) at /home/user/cyrup/crates/cyrup-tools/src/jsnum.rs — is pub(crate) to cyrup-tools and never reaches the renderer. There is also no fallback source: ReadDetails (/home/user/cyrup/crates/cyrup-tools/src/details.rs:8-11) carries only `truncation`, not the resolved offset/limit, and ToolRun::args (entry.rs:174-176) is documented and used as the raw unnormalized model arguments. Caveat on framing: the CAPABILITY (rendering :start-end) does exist in Rust — only the float-typed input class diverges, and the same as_i64 pattern governs grep/ls/find limit suffixes at tool_builtin.rs:329,356,375, so it is a consistent display-layer choice rather than a one-off omission.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
