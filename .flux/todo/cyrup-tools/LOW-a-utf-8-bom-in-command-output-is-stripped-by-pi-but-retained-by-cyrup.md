---
title: A UTF-8 BOM in command output is stripped by pi but retained by cyrup
priority: LOW
tool: bash
source: pi-parity-audit (workflow wf_e427a266-e16)
stage: new
status: done
updated: 2026-08-27
---

# A UTF-8 BOM in command output is stripped by pi but retained by cyrup

## What pi does

`/home/user/cyrup/tmp/pi/packages/coding-agent/src/core/tools/output-accumulator.ts:40,70,85` decodes every chunk through `new TextDecoder()` with `{ stream: true }`. With the default `ignoreBOM: false`, the WHATWG decoder removes a leading UTF-8 BOM (EF BB BF), and `:155,196-203` build the snapshot text from that decoded string — so the BOM never reaches `snapshot.content` and never counts toward `totalDecodedBytes`.

## What cyrup-tools does

`/home/user/cyrup/crates/cyrup-tools/src/output.rs:206-208` `tail_string()` returns `String::from_utf8_lossy(&self.buf)` over the *raw* rolling byte buffer, and `:76-107` `decode_into_counters` uses `std::str::from_utf8`, neither of which strips a BOM. `/home/user/cyrup/crates/cyrup-tools/src/tools/bash.rs:393-394` feeds that tail straight into `truncate_tail` and into the returned content.

## User-visible impact

A command whose output starts with a UTF-8 BOM (common on Windows tooling, `iconv -t UTF-8//BOM`, some generated files) returns a leading U+FEFF character to the model in cyrup and not in pi, which can break exact string matching on the first line and shifts the reported `totalBytes` by 3.

## Parity action

Strip a leading UTF-8 BOM from the first decoded chunk in `OutputAccumulator` (and exclude it from `total_decoded_bytes`), matching `TextDecoder`'s default BOM handling.

## Why this gap is real

An adversary agent was tasked with **refuting** this finding by locating the capability in the Rust under another name. It could not:

> Could not refute. Searched all of cyrup-tools/src and cyrup-core/src for bom/feff/EF BB BF/ignoreBOM/encoding_rs: the only BOM-aware code is edit_diff.rs:26-31 strip_bom, used exclusively by edit/edit_diff to PRESERVE a file's BOM across an edit, and never reached from the bash output path (other hits are cyrup-permission-system/ext_config.rs:529 for config JSON and cyrup-mcp/ui.rs:403, both unrelated). Read the real functions: output.rs:76-107 decode_into_counters uses plain std::str::from_utf8 with no BOM branch, output.rs:205-208 tail_string is String::from_utf8_lossy over the raw rolling buffer, and both consumers (bash.rs:393-394 final result, bash.rs:573 stream update) feed that straight into truncate_tail, which also does no character filtering. There is no encoding_rs dependency, so nothing else performs WHATWG-style BOM removal. Confirmed the pi side does strip it: output-accumulator.ts:36 constructs `new TextDecoder()` (default ignoreBOM:false) and bash.ts:410-412 handleData passes the raw Buffer straight to output.append, so the BOM is gone before tailText and totalDecodedBytes exist. Capability genuinely absent. Severity corrected to low: nothing is silently wrong — cyrup returns a character that really was in the byte stream (arguably the more faithful rendering); the totalBytes delta is a fixed 3 bytes against a limit in the tens of thousands and cannot flip a truncation decision; pi is itself inconsistent, since its totalRawBytes (output-accumulator.ts:114) still counts the BOM and its spilled temp file is written from raw rawChunks so the full-output file keeps the BOM its preview dropped; and the trigger surface is only a command whose very first emitted bytes are a BOM, affecting one invisible zero-width character at the head of line 1.

## Definition of done

1. The capability described under *Parity action* is implemented in `crates/cyrup-tools`.
2. A test pins the new behaviour against the pi semantics quoted above.
3. `cargo check --workspace --all-targets` and `cargo clippy` stay clean.
4. Behaviour that pi does NOT have is not introduced — this is a parity task, not a redesign.
