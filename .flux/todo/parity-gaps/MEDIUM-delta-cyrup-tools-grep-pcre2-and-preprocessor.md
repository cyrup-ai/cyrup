---
stage: aug
status: done
updated: 2026-08-28 21:00
---

# grep: pcre2 (`-P`) and the preprocessor flags (`--pre`, `--pre-glob`, `-z`)

Split out of
[MEDIUM-delta-cyrup-tools-src-tools-grep-rs-1.md](./MEDIUM-delta-cyrup-tools-src-tools-grep-rs-1.md)
§9, which wired `$RIPGREP_CONFIG_PATH` into `grep`'s three builders.

State today: `RgFlags::read(self.opts.rg_config_path.as_deref())` runs on every call
(`grep.rs:539`) and its result reaches `build_matcher` (`grep.rs:644`). `-P`, `--engine`, `--pre`,
`--pre-glob` and `-z` reach `RgFlags::apply_long` / `apply_short` and fall through the catch-all
`_ => {}` arm (`rgconfig.rs`), so a config containing them still searches. That is a safe resting
state, and this task keeps it — but it also leaves one live defect and three smaller ones that the
source already answers.

**This augmentation is prescriptive.** Everything in §2 is determined by ripgrep 14.1.0 and by
cyrup's own module contracts; implement it as written. §3 is the single call that is genuinely a
human's.

---

## 1. What the source settles (verify, do not re-litigate)

### 1.1 The PCRE2 hint is a property of the BUILD, not of the message

`suggest_pcre2`'s first statement is the gate:

```rust
// ripgrep 14.1.0, crates/core/flags/hiargs.rs:1416-1419
fn suggest_pcre2(msg: &str) -> Option<String> {
    if !cfg!(feature = "pcre2") {
        return None;
    }
```

`suggest_other_engine` (`hiargs.rs:1404-1409`) only forwards to it. So ripgrep compiled **without**
the `pcre2` feature emits the bare `grep-regex` error and **no suggestion at all** — it does not
reword the hint, it does not print a different hint, it prints nothing extra.

cyrup links `grep-regex` and not `grep-pcre2` (`crates/cyrup-tools/Cargo.toml:26-28`). cyrup **is**
a build without the feature. The question "what should the hint say instead?" is therefore not
open: ripgrep in cyrup's configuration says nothing, and matching that is byte-parity with a real
`rg`, not an invention.

Two supporting facts, both verified:

- cyrup's current doc comment (`grep.rs:103-106`) justifies emitting the hint on the grounds that
  Pi runs an official release binary built with `--features pcre2`. That premise is about **Pi's
  binary**, not about Pi's source, and it is already conditional even under Pi:
  `getToolPath` (`tmp/pi/.../utils/tools-manager.ts:85-104`) prefers the downloaded release asset
  but falls back to whatever `rg` is on `PATH`, which may be a distro build without PCRE2. The
  hint's presence tracks the binary in both projects.
- Diverging from Pi here is not new ground in this file. `build_matcher` already refuses a raw NUL
  in the pattern on cyrup's own authority (`grep.rs:141-143`: *"The choice to refuse it is CYRUP'S
  OWN"*), and `rgconfig.rs` already refuses to honour `-q` because honouring Pi would import a
  defect (`RgFlags::QUIET_IS_REFUSED`). This is the same shape: a marked, reasoned `[CYRUP-DELTA]`.

Why it matters and why it is the actionable part: under cyrup the advice is actionable **nowhere**.
The `grep` tool schema (`grep.rs:180-192`) has no engine parameter, so the model cannot act on it;
and a human who follows it and writes `--pcre2` into `$RIPGREP_CONFIG_PATH` hits the catch-all arm
and gets the *identical error with the identical hint* on the next call. cyrup emits a retry
instruction that provably cannot change the outcome.

### 1.2 What `-P` / `--engine=pcre2` do in a build without the feature

`matcher_pcre2`'s `#[cfg(not(feature = "pcre2"))]` arm (`hiargs.rs:447-452`) returns
`"PCRE2 is not available in this build of ripgrep"`, and both flags document it
(`defs.rs:1684-1686` for `--engine`, `defs.rs:5267-5269` for `-P/--pcre2`). Real ripgrep **errors
and exits**.

cyrup does **not** copy that, and the reason is already written down: `rgconfig.rs`'s module
contract is that a config never makes the tool fail (*"one bad line in a config must not turn every
search into an error"*, and the unrecognised-flag rationale in the module header). A shared
`.ripgreprc` written for a PCRE2-enabled `rg` must keep cyrup searching. Parse-and-ignore stands.
This is a mechanical consequence of an existing contract, not a new decision.

Note `--engine=auto` collapses onto `--engine=default` for cyrup exactly: the `Auto` arm
(`hiargs.rs:375-398`) returns the Rust matcher whenever it builds, and differs only in the text of
the failure it concatenates — which cyrup has no PCRE2 error to produce.

### 1.3 `--no-pcre2-unicode` / `--pcre2-unicode` are NOT pcre2-gated — cyrup can honour them today

`NoPcre2Unicode::update` is one line: `args.no_unicode = v.unwrap_switch();`
(`defs.rs:4711-4714`). It is a DEPRECATED alias of `--no-unicode` (`defs.rs:4701-4707`) that writes
the same engine-independent bool, and `test_no_unicode` (`defs.rs:4851-4863`) proves all four names
— `--no-unicode`, `--unicode`, `--no-pcre2-unicode`, `--pcre2-unicode` — target `args.no_unicode`
with last-occurrence-wins.

cyrup honours exactly one of the four (`"no-unicode" => self.no_unicode = true`) and feeds it to
`builder.unicode(!rg.no_unicode)` (`grep.rs:162`). The other three fall through the catch-all. Two
of them carry `pcre2` in the name and are therefore this task's; the third (`--unicode`) is their
symmetric partner and must land with them or the group is incoherent. Cost: three match arms.

### 1.4 `--pre` cannot be implemented at `grep`'s seam at all

This is structural, not a judgement call:

- `FsOps` (`crates/cyrup-tools/src/ops/mod.rs:437-502`) has no process operation. Execution lives
  on a *different* trait, `ProcOps` (`ops/mod.rs:507`), and `GrepTool` holds only
  `Arc<dyn FsOps>` (`grep.rs:170`). Honouring `--pre` means giving the read-only search tool a
  process capability it does not have.
- A preprocessor subprocess opens the path itself, with the process's own credentials. It therefore
  routes **around** the decorator chain that every `grep` byte currently passes through —
  `TraversalFs`, which confines every operation to a root and denies escapes
  (`isolation/traversal.rs:1-13`), and `ProtectedFs` (`isolation/protected.rs:1-11`).
- `read_stream`'s documented contract (`ops/mod.rs:453-458`) says the fallback exists precisely for
  *"a backend that genuinely cannot stream (a remote/RPC filesystem)"*. For such a backend the
  candidate's bytes are not a local file, so there is no path for `COMMAND PATH` to open and no
  defensible answer to which host runs the command.
- The program is named by whoever wrote `$RIPGREP_CONFIG_PATH`, and ripgrep spawns it
  unconditionally, once per file (`defs.rs:5377-5386`). That is arbitrary code execution reached
  through a tool whose entire contract is that it only reads.

`--pre-glob` has no meaning without `--pre` (`defs.rs:5514`: *"This flag has no effect if the
`--pre` flag is not used"*) and follows it.

### 1.5 Correction: `-z` is subprocess-based in ripgrep too

The pre-augmentation text claimed `-z` is *"the one of the three that could plausibly be done
in-process"* and is free of the arbitrary-execution problem. Half right, and the wrong half matters:

- `-z` *"expects the decompression binaries (such as `gzip`) to be available in your `PATH`"*
  (`defs.rs:5986-5991`). It spawns processes exactly like `--pre`. What differs is that the program
  set is fixed by ripgrep rather than chosen by the config author — so it is not *arbitrary*
  execution, but it is still execution, and §1.4's `FsOps`/`ProcOps` argument applies unchanged.
- `-z` and `--pre` clear each other (`SearchZip::update` sets `args.pre = None`,
  `defs.rs:6002-6006`; `Pre`'s doc: *"This overrides the `--search-zip` flag"*, `defs.rs:5424`).
- An **in-process** `-z` would be a cyrup invention, not parity, and it is not a small one:
  ripgrep's set is gzip, bzip2, xz, LZ4, LZMA, Brotli and Zstd (`defs.rs:5986-5987`), while the
  workspace's `async-compression` pin carries gzip, brotli, deflate and zstd only
  (`Cargo.toml:238`) — 4 of 7, missing bzip2/xz/LZ4/LZMA — and it is a Tokio-async decoder where
  `read_stream` hands back a blocking `std::io::Read` driven under `spawn_blocking`.

`-z` stays parse-and-ignore. If it is ever revisited it is its own task with its own dependency
question, not a rider on this one.

### 1.6 Correction: `unsafe_code` is not an argument against pcre2

The pre-augmentation text said a C dependency *"interacts with `#![forbid(unsafe_code)]` at the
crate boundary."* It does not, and the workspace says so in its own words:
*"the lint is per-crate, not transitive"* (`Cargo.toml:243`). cyrup-tools declares
`#![deny(unsafe_code)]` (`lib.rs:18`) and already carries `#[allow(unsafe_code)]` sites of its own
in `ops/local/`. A dependency's `unsafe` is invisible to that lint. Delete this argument; it makes
the real one look weaker than it is.

### 1.7 The real numbers for the pcre2 dependency

Resolved from the crates.io index, 2026-08-28:

| crate | version | depends on |
| --- | --- | --- |
| `grep-pcre2` | 0.1.10 (2026-07-15) | `grep-matcher ^0.1.8`, `log ^0.4.20`, `pcre2 ^0.2.6` |
| `pcre2` | 0.2.11 | `libc ^0.2.146`, `log ^0.4.19`, `pcre2-sys ^0.2.10` |
| `pcre2-sys` | 0.2.10 | build: `cc ^1.0.73` (feature `parallel`), `pkg-config ^0.3.27`; runtime: `libc` |

- **Exactly three new crates** enter the graph: `grep-pcre2`, `pcre2`, `pcre2-sys`. `grep-matcher`,
  `log`, `libc`, `cc` and `pkg-config` are all already in `Cargo.lock`.
- **"The workspace stops being all-Rust" is false.** `Cargo.lock` already has `cc` as a build
  dependency of `aws-lc-sys`, `ring`, `blake3`, `tree-sitter`, `tree-sitter-bash`, `wasmtime` and
  `cmake`. The workspace already compiles C.
- **What actually changes:** `cyrup-tools`' own subtree is pure Rust today — nothing beneath
  `cyrup-core`, `grep-*`, `ignore`, `globset`, `similar`, `feruca`, `image`, `tokio` invokes `cc`.
  `cyrup-tools` is the crate an embedder takes to get the tool set, and it would stop being
  buildable without a C toolchain (and, on the `pkg-config` path, without a discoverable
  `libpcre2-8`). That, plus cross-compilation and supply surface, is the entire real cost.
- **The mitigation ripgrep itself uses is an off-by-default cargo feature.** `cyrup-tools` already
  has a features table (`crates/cyrup-tools/Cargo.toml:36-41`). A `pcre2 = ["dep:grep-pcre2"]`
  feature, default OFF, leaves every existing consumer's build byte-identical and makes cyrup's
  hint gate literally ripgrep's `cfg!(feature = "pcre2")`.

---

## 2. Prescription

Three edits. None of them is gated on §3.

### 2.1 `grep.rs` — stop emitting the hint (the live defect)

Delete `suggest_other_engine` (`grep.rs:103-117`) and both of its call sites:

- `grep.rs:59` becomes `let msg = suggest_text(suggest_multiline(err.to_string()));`
- `grep.rs:73` becomes `format!("rg: {}", suggest_text(suggest_multiline(seed)))`

Do **not** leave an identity function behind, and do **not** write
`cfg!(feature = "pcre2")` against a feature that is not declared in
`crates/cyrup-tools/Cargo.toml` — that fires rustc's `unexpected_cfgs` warning.

Move the citation into `rg_pattern_error`'s doc comment, replacing layer 2's third bullet. Required
content, in the doc's existing register:

> ripgrep composes `suggest_other_engine(suggest_text(suggest_multiline(msg)))`
> (`hiargs.rs:371` for the outer, `:505-510` for the inner two), but
> `suggest_other_engine` forwards to `suggest_pcre2`, whose first statement is
> `if !cfg!(feature = "pcre2") { return None; }` (`hiargs.rs:1416-1419`). The hint is a property of
> the BUILD. cyrup links `grep-regex` and not `grep-pcre2`, so cyrup IS a build without the feature
> and ripgrep in that build appends nothing — which is what cyrup does. `[CYRUP-DELTA]` against
> Pi's binary, which has the feature; see [`RgFlags::PCRE2_IS_DECLINED`] for why reproducing the
> string would be worse than dropping it.

Then invert the existing guard `lookaround_and_backreferences_carry_ripgreps_pcre2_hint`
(`grep.rs:1812-1851`) — rename it to `lookaround_and_backreferences_carry_no_pcre2_hint`, delete
the `hint` constant, and assert exact equality with the bare blocks:

```text
rg: regex parse error:
    (?:(?=foo))
       ^^^
error: look-around, including look-ahead and look-behind, is not supported
```

```text
rg: regex parse error:
    (?:\0)
       ^^
error: backreferences are not supported
```

Neither `suggest_multiline` nor `suggest_text` fires on either (no `the literal`/`not allowed`, no
`pattern contains "\0"`), so nothing else is appended.

**This is the guard.** It fails today, in the opposite direction, and it fails again the moment
anyone re-adds the hint.

### 2.2 `rgconfig.rs` — recognise-and-decline instead of falling through

The catch-all is for flags this module does not *know*. These are flags it knows and declines —
the same distinction `"quiet" => {}` already draws. Two consts, following `QUIET_IS_REFUSED`'s
shape exactly (same `#[cfg_attr(not(test), allow(dead_code))]`, since nothing in the production
path reads them):

```rust
/// Why `-P`/`--pcre2`/`--engine` are recognised and then dropped.
///
/// cyrup links `grep-regex` and not `grep-pcre2`, so it is a build without ripgrep's `pcre2`
/// feature. Real ripgrep in that build ERRORS on these flags ("PCRE2 is not available in this
/// build of ripgrep", `hiargs.rs:447-452`, documented at `defs.rs:5267-5269`). cyrup does not,
/// because this module's contract is that a config never makes the tool fail: one line in a
/// `.ripgreprc` shared with a PCRE2-enabled `rg` must not break every cyrup search. The same
/// build fact is why `grep.rs` does not emit ripgrep's `--pcre2` suggestion — see
/// `suggest_pcre2`'s `cfg!` gate at `hiargs.rs:1416-1419`.
#[cfg_attr(not(test), allow(dead_code))]
pub const PCRE2_IS_DECLINED: &'static str =
    "-P/--pcre2/--engine=pcre2 need grep-pcre2 (a C dependency cyrup-tools does not take); \
     ripgrep without the feature errors (hiargs.rs:447-452), but this module never fails a \
     search over a config line";

/// Why `--pre`, `--pre-glob` and `-z` are recognised and then dropped.
///
/// All three make ripgrep spawn a program per file — `--pre` one named by the config author
/// (`defs.rs:5377-5386`), `-z` a decompressor found on `PATH` (`defs.rs:5986-5991`). `grep`'s seam
/// is `FsOps` (`ops/mod.rs:437`), which has no process operation at all; execution is `ProcOps`
/// (`ops/mod.rs:507`), which `GrepTool` does not hold. A subprocess would also open the file with
/// its own credentials, bypassing the `TraversalFs`/`ProtectedFs` decorators every other byte
/// `grep` reads passes through — arbitrary execution reached through a read-only tool.
#[cfg_attr(not(test), allow(dead_code))]
pub const PREPROCESSOR_IS_DECLINED: &'static str =
    "--pre/--pre-glob/-z spawn a program per file; grep's seam is FsOps (ops/mod.rs:437), which \
     has no exec, and a subprocess would bypass the isolation decorators";
```

In `apply_long`, above the catch-all:

```rust
// Recognised and declined — see `PCRE2_IS_DECLINED`. `--engine`'s VALUE is consumed here on
// purpose: an unconsumed value-taking flag leaves its argument to be re-read as a top-level
// arg by `parse`, so `--engine\n-i` would silently turn the search case-insensitive.
// `default` and `auto` are what cyrup already does — `Auto` (hiargs.rs:375-398) returns the
// Rust matcher whenever it builds — so all three values land in the same place.
"engine" => {
    take();
}
"pcre2" | "no-pcre2" | "auto-hybrid-regex" | "no-auto-hybrid-regex" => {}

// Recognised and declined — see `PREPROCESSOR_IS_DECLINED`. Same value-consuming reason.
"pre" | "pre-glob" => {
    take();
}
"search-zip" | "no-search-zip" => {}
```

In `apply_short`, above its catch-all: `'P' | 'z' => {}` with a one-line pointer to the two consts.

The `take()` calls are load-bearing, not cosmetic. `parse` advances `i` past the flag before
calling `apply_long`, and only a match arm that calls `take` advances it past the value. A
`--pre` / `--pre-glob` / `--engine` written in ripgrep's two-line form therefore leaves its
argument to be re-examined at the top of the loop today; a bare word is harmlessly ignored, but an
argument that begins with `-` is parsed as a flag and applied.

Guard: `assert_eq!(RgFlags::parse("--engine\n-i\n"), RgFlags::default());` — fails today
(`case == Some(CaseMode::Insensitive)`), passes after.

### 2.3 `rgconfig.rs` — honour the unicode aliases (§1.3)

In `apply_long`, beside the existing `"no-unicode"` arm:

```rust
// `--pcre2-unicode`/`--no-pcre2-unicode` are DEPRECATED aliases of `--unicode`/`--no-unicode`
// (defs.rs:4701-4714) — engine-independent, writing the same `no_unicode` bool that the Rust
// engine reads. Last occurrence wins across all four names (defs.rs:4851-4863).
"no-unicode" | "no-pcre2-unicode" => self.no_unicode = true,
"unicode" | "pcre2-unicode" => self.no_unicode = false,
```

Guard: `assert!(!RgFlags::parse("--no-unicode\n--pcre2-unicode\n").no_unicode);` — fails today.

---

## 3. The one decision that needs a human

Everything above is determined. This is not:

> **May `cyrup-tools` carry an OFF-BY-DEFAULT `pcre2` cargo feature that pulls `grep-pcre2` →
> `pcre2` → `pcre2-sys`, making `cyrup-tools`' dependency subtree — pure Rust today — require a C
> toolchain when the feature is enabled?**

Frame it with §1.7's numbers, not with adjectives: three new crates; `cc`/`pkg-config`/`libc`/`log`
already present; the workspace already compiles C in seven other places; default builds byte-
identical; the cost lands on anyone who *enables* it (C toolchain, cross-compilation, supply
surface) plus a permanent third entry in the workspace's C-dependency ledger, which
`Cargo.toml:240-274` shows has been declined three times running (`syntect` on `default-fancy`
instead of oniguruma; `ratatui-image` without `chafa-dyn`; `rustix` on the `linux_raw` backend).

That standing pattern is the default answer, and refusing costs nothing beyond §2 as written. What
a human owes is the ratification — recorded in `PCRE2_IS_DECLINED` if refused, or as a
`docs/adr/` entry with the feature-flag shape if taken.

If it is TAKEN later, §2 does not get rewritten: declare the feature, and the hint gate returns as
`cfg!(feature = "pcre2")` — ripgrep's own condition, now legal because the feature exists.

---

## 4. Definition of done

1. `suggest_other_engine` is gone from `grep.rs`; neither `rg_pattern_error` nor
   `rg_nul_literal_error` composes it; `rg_pattern_error`'s doc carries the `hiargs.rs:1416-1419`
   gate citation and the `[CYRUP-DELTA]` marker.
2. **The guard**: `lookaround_and_backreferences_carry_no_pcre2_hint` asserts both rg 14.1.0 error
   blocks end at `…is not supported` / `…are not supported` with nothing appended.
3. `-P`, `-z`, `--pcre2`, `--no-pcre2`, `--engine`, `--auto-hybrid-regex`,
   `--no-auto-hybrid-regex`, `--pre`, `--pre-glob`, `--search-zip`, `--no-search-zip` reach
   explicit arms rather than the catch-all; `--engine`, `--pre` and `--pre-glob` consume their
   value; behaviour is unchanged (still no error, still searches).
4. `PCRE2_IS_DECLINED` and `PREPROCESSOR_IS_DECLINED` exist on `RgFlags` beside
   `QUIET_IS_REFUSED`, carrying the reasons — the refusal is recorded in the code, where it cannot
   drift from the arms it explains.
5. `--unicode`, `--pcre2-unicode` and `--no-pcre2-unicode` set `no_unicode` per §1.3.
6. `RgFlags::parse("--engine\n-i\n") == RgFlags::default()` and
   `!RgFlags::parse("--no-unicode\n--pcre2-unicode\n").no_unicode`.
7. The §3 question is answered by a human — recorded in `PCRE2_IS_DECLINED` (refused) or in
   `docs/adr/` (taken).

## 5. Out of scope — do not do

- Do not add `grep-pcre2` before §3 is answered.
- Do not turn any of these flags into an error. §1.2.
- Do not implement `--pre` or `--pre-glob`. §1.4 — it is not an implementation problem, `grep`'s
  seam has no process capability.
- Do not implement an in-process `-z`. §1.5 — it is not parity, and the tree cannot reach parity
  anyway (4 of 7 formats, async decoder against a blocking reader).
- Do not reword the hint into cyrup-specific prose. §1.1 — ripgrep without the feature prints
  nothing, and inventing text diverges from both ripgrep and Pi.
