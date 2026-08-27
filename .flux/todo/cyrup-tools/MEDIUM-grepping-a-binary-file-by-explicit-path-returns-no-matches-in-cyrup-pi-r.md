---
title: Grepping a binary file by explicit path returns no matches in cyrup; pi returns the matching lines
priority: MEDIUM
tool: grep
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# Grepping a binary file by explicit path returns no matches in cyrup; pi returns the matching lines

## What pi does

pi passes `searchPath` positionally to ripgrep (/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/grep.ts:224) with no `--text`/`--binary`, so ripgrep's explicit-path rule applies: `BinaryDetection::convert(b'\x00')`, which searches the file and reports matches. Verified with rg 14.1.0 on `bin.dat` = `hello NEEDLE\n\0\x01\x02binary NEEDLE\n`: `rg --json --line-number -- NEEDLE bin.dat` emits two match events (line 1 and line 2).

## What cyrup-tools does

/home/user/cyrup/crates/cyrup-tools/src/tools/grep.rs:120-123 builds the searcher with `BinaryDetection::quit(b'\x00')` for every file, including the `meta.is_file` explicit-path branch at :342-360; the divergence is acknowledged in the comment at :104-108. Verified by running the tool with `{"pattern":"NEEDLE","path":"bin.dat"}`: result is `No matches found`.

## User-visible impact

A user who points grep directly at a file containing NUL bytes (minified bundle with embedded binary, .pack/.pyc/.class, a log with control bytes, a UTF-16 file without BOM) is told there are no matches even though the text is present — an answer that is wrong rather than merely truncated.

## Parity action

Use `BinaryDetection::convert(b'\x00')` in the explicit-`path`-is-a-file branch (keeping `quit` for traversal-discovered files) and derive the printed block from the same converted stream so line numbers stay consistent, instead of re-reading the raw file.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Not refutable. cyrup has exactly one grep implementation and one searcher construction: /home/user/cyrup/crates/cyrup-tools/src/tools/grep.rs:120-123 builds every searcher with BinaryDetection::quit(b'\x00'), and search_one is shared by BOTH the explicit-file branch (meta.is_file, :342-360) and the walk branch (:367-433). Exhaustive ripgrep over crates/cyrup-tools/src and crates/cyrup-core/src for BinaryDetection, SearcherBuilder, search_reader/search_slice, and \x00 returns hits only inside grep.rs; there is no convert() path, no --text/-a/binary/text input field (schema at :44-56 is pattern/path/glob/ignoreCase/literal/context/limit only), no GrepOpts knob (config.rs:272-284 has just limit and max_bytes), and only one registration site (registry.rs:88) with no decorator that could substitute detection. The divergence is explicitly acknowledged as [CYRUP-DELTA] in the comment at grep.rs:104-108 — an implementation rationale (convert renumbers lines at each NUL, which would desync from the raw \n-split re-read used by the formatBlock path), not an alternate expression of the capability. I verified pi's side (grep.ts:219-225 pushes searchPath positionally with no --text) and both ripgrep behaviours with rg 14.1.0: on an explicit path, `rg --json --line-number -- NEEDLE bin.dat` emits two match events (binary_offset:13); the same file reached by directory traversal emits nothing at all, not even the pre-NUL line 1, matching cyrup's own committed expectation in tests/tools.rs:858-907. So the capability of searching an explicitly-named NUL-containing file is genuinely absent, not merely implemented differently.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
