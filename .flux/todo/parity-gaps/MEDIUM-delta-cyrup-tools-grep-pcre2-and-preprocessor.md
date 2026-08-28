---
stage: task
status: todo
updated: 2026-08-28
---

# grep: pcre2 (`-P`) and the preprocessor flags (`--pre`, `--pre-glob`, `-z`)

Split out of
[MEDIUM-delta-cyrup-tools-src-tools-grep-rs-1.md](./MEDIUM-delta-cyrup-tools-src-tools-grep-rs-1.md)
§9. That task wired `$RIPGREP_CONFIG_PATH` into `grep`'s three builders. These flags were
deliberately left out of it because they are not config wiring — each needs its own decision, and
together they were the reason the original estimate for that task looked large.

They are currently PARSE-AND-IGNORE: they fall through `RgFlags::apply_long` / `apply_short`'s
catch-all arm in [`rgconfig.rs`](../../crates/cyrup-tools/src/tools/rgconfig.rs), so a config
containing them still searches rather than erroring. That is a safe resting state, not a finished
one.

## 1. `-P` / `--engine=pcre2`

pi gets pcre2 for free — its `rg` binary ships with the feature compiled in, so a user who writes
`--pcre2` in their config gets look-around and backreferences. cyrup uses `grep-regex`, which is
`regex`-backed and supports neither.

Closing this means taking `grep-pcre2`/`pcre2-sys`, which is a **C dependency**: it changes the
build for every consumer of `cyrup-tools`, on every platform, and interacts with
`#![forbid(unsafe_code)]` at the crate boundary. That is a project-level call, not a grep-level
one.

### 1.1 The live defect this leaves behind

`grep.rs`'s `suggest_other_engine` tells the model, on any look-around or backreference refusal:

> Consider enabling PCRE2 with the --pcre2 flag

Under pi that advice is **actionable** — the user puts `--pcre2` in their ripgrep config. Under
cyrup it is actionable **nowhere**: there is no `--pcre2` to enable, and now that the config IS
read, a user who follows the advice and writes `--pcre2` into it gets silence rather than either
the feature or an error. cyrup emits advice it cannot honour.

Whatever is decided about the dependency, the hint text has to stop promising a capability that
does not exist. Fixing the message is cheap and is not gated on the pcre2 decision; it should
probably not wait for it.

## 2. `--pre` / `--pre-glob`

`--pre` names a program ripgrep runs over each file, searching its stdout instead of the file's
bytes.

Under a remote or virtual [`FsOps`](../../crates/cyrup-tools/src/ops/mod.rs) this has **no
coherent meaning**: the candidate's bytes do not exist as a local file for a local subprocess to
open, and there is no defensible answer to which host the preprocessor should run on. It also
hands arbitrary program execution to whatever wrote the config file, reached through a tool whose
whole point is that it only reads.

The decision needed is not "how do we implement it" but "does this belong in an in-process
searcher at all". `--pre-glob` is meaningless on its own and follows whatever `--pre` decides.

## 3. `-z` / `--search-zip`

Decompress-then-search. Same shape as `--pre` but narrower and without the arbitrary-execution
problem — it needs a decompression dependency and a decision about which formats, and it is the
one of the three that could plausibly be done in-process against a `read_stream`.

## Definition of done

1. A decision, recorded, on the pcre2 C dependency — taken or refused, with the reason.
2. `suggest_other_engine` no longer advises a `--pcre2` flag that cyrup cannot honour.
3. A decision, recorded, on whether `--pre`/`--pre-glob` belong in an in-process searcher under an
   arbitrary `FsOps`.
4. Whatever is not implemented stays parse-and-ignore rather than becoming an error, so a config
   written for real ripgrep keeps searching.
