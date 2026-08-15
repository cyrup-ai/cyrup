# REPRO-LOG — the first execution of this binary

Every other document in this directory is a **reading**. This one is a **measurement**. It records
what happened when the cyrup binary was built, launched, and driven — through a real pty where the
surface needed one, headless where it did not — against seventeen items the backlog had ranked
without anyone ever watching them happen.

> **Read this before trusting a severity.** The backlog's own caveat says "nothing here was built,
> run, tested or reproduced… every `Verify` line is a design, not an observation." That is no longer
> true of the seventeen rows below, and it is still true of the other 431. The difference between
> those two populations is the subject of the closing section.

---

## 0a. AMENDMENT 2026-08-14 — what two parity sweeps did to these seventeen rows

> **Superseded in part by §0b below, which covers sweeps 3-6. Read §0b first.**

> **This log remains a measurement and nothing below has been re-measured.** What follows records
> which of its seventeen rows have had the *code under them changed* since 2026-08-13, so nobody
> reads a transcript as current behaviour. cyrup HEAD is now **`380c713`** (this log ran at
> `0c76986`). **Every row marked FIXED below is fixed by reading, not by re-running** — the two
> sweeps were restricted to `cargo check` and the orchestrator ran only the gate.

**The suite numbers moved again, and this time for a structural reason.** §1's central finding — that
"3932" was carried forward by citation and the real figure was **6387** — holds and was the right
correction. It is now **6440 tests / 6440 passed / 8 skipped in 16.4 s**, because the integration
tests were relocated into their crates as unit tests (`63d729a` / `c3982b5` / `d973906`): **310
integration binaries → 6 + 8 gated**, behind a new `cyrup-it` harness crate. **Two consequences for
this log specifically:**

1. Every path of the form `crates/<crate>/tests/<x>.rs` in §2 and §3 is stale unless it names
   `cyrup-it`. The `one_shot_parity.rs` hang recorded in §1 is in a file that no longer exists at
   that path.
2. **`cyrup-it` is `required-features = ["it"]`, so the gate does not build or run it.** The 16.4 s
   figure buys no coverage of the broker-socket seam tests at all — filed as **structural defect J**
   in `00-residual-ledger.md`, with four `cyrup-it` assertions currently contradicting production as
   the evidence that it is not theoretical.

### Row-by-row status of the seventeen

| row | 2026-08-13 verdict | status at `380c713` |
|---|---|---|
| `SEAM-051` | CONFIRMED | **FIXED** — `--tui-mode` is parsed instead of rejected with exit 1. Closes `DRIFT-022`'s flag half. |
| `SEAM-064` | CONFIRMED | **FIXED** — the pre-launch trust prompt carries both "(this session only)" rows. The one-character `includeSessionOnly: true` fix, exactly as filed. |
| `SEAM-047` | CONFIRMED | **FIXED** — first SIGTERM/SIGHUP tears down and exits 143/129. `SEAM-008` and `SEAM-059` both landed on its back and are closed. |
| `SEAM-063` | CONFIRMED | **FIXED (sweep 1)**, with a residual this log's transcript would still show: the pre-launch status lines print AFTER teardown, not in the picker header with pi's 2 s/3 s dwell, because `SessionSelector` has no status channel. **The live re-run in its Verify block is still owed.** |
| `SEAM-062` | CONFIRMED | **FIXED (sweep 1)** via the "preferred full fix" route — the rename now persists — rather than the parity route of disabling rename. |
| `SEAM-061` | CONFIRMED | **STILL OPEN, and now the top of the whole backlog.** The transcript in §3 is current behaviour. What changed is only that the missing piece narrowed: `SessionScope`, `set_scope` and `scope()` exist; `SessionAction::ToggleScope`, its handler and `show_path`-follows-scope do not. **Two sweeps split it across areas 07 and 08 and neither took it.** |
| `TUI-042` | CONFIRMED | **FIXED** (pre-sweep) — the paste registry is carried on the undo snapshot. |
| `TUI-043` | CONFIRMED | **FIXED** (pre-sweep) — word motion is paste-marker atomic. |
| `TUI-044` | CONFIRMED | **FIXED** (pre-sweep). |
| `TUI-027` | CONFIRMED | **FIXED (sweep 1)** — `/tree` has a real text search; the digit-filter arm and `FilterMode::from_digit` are deleted, and pi's help row and standing `Type to search:` line were ported with it. **The data-loss path this log measured is closed.** |
| `TUI-016` | CONFIRMED (corrected headline) | **FIXED** — and this row is the one that most deserves re-reading: it was **already fixed at HEAD by `c8c86bc`** while both its status row and its item body still said still-open/regressed. Same for `TUI-045`, `TUI-052` and `TUI-055`. |
| `PERM-009` | CONFIRMED | **FIXED (sweep 1)** — the cyrup-only `bash` bypass in `should_expose_tool` is deleted. The permission bypass this log demonstrated headlessly is gone. |
| `AGENT-020` | **REFUTED** | **FIXED anyway, and the refutation stands.** The guard is now the first statement of `Agent::continue_run` and both drain sites restore via `PendingQueue::push_front` on `Err(RunActive)`. The row's finding — that the predicted loss does not occur on the normal path — is unaffected and is still why the item is `low`. **`AGENT-034` later added the same guard at `Agent::prompt`, which had none at all**; read the two as one pattern, not two. |
| `EXT-054` | CONFIRMED (stronger than filed) | **FIXED** (pre-sweep) with `EXT-055`. `EXT-059` is the named residual and is still open (`AgentSession::load_wasm_extension` is a full-authority manifest-less load). |
| `SESS-040` | CONFIRMED, mechanism half REFUTED | **STILL OPEN, and it is now the cheapest of the three remaining highs.** Both siblings closed: `SESS-041` (auto-compaction token — refuted at HEAD, `abort_compaction` cancels both tokens) and `SESS-042` (the `aborted: true` payload — present at both `compaction_end` failure sites). **040, 041 and 042 now differ only in wiring: the moment 040 lands a dispatch site the abort takes effect.** `TUI-055` is fixed, so the band renders — **but nobody has watched it, and this row's mechanism correction (no indicator rendered at all) was measured against a build where it could not**. Re-run before trusting either half. |
| `TUI-045` | CONFIRMED | **FIXED** by `c8c86bc`, with the same stale-record caveat as `TUI-016`. |

~~**Fifteen of the seventeen are now fixed; two — `SEAM-061` and `SESS-040` — are open and are two of
the three remaining highs in the entire backlog.**~~ **CORRECTED by §0b: sixteen are fixed and one —
`SESS-040` — is open. `SEAM-061` was already fixed when this sentence was written.** Both are blocked on coordination across crates
rather than on analysis, which is the same thing this log's §5 concluded about the method.

### What §5's argument looks like a year later, in one paragraph

§5 asked whether the backlog's confidence was justified and answered "only 3 of 17 items survived a
live run unchanged". Two static sweeps have now produced a comparable figure from the other
direction: **≈12% of the items they worked were refuted** — already fixed, wrongly diagnosed, or
resting on a premise that was false at the tag. **The two numbers are measuring the same thing from
opposite ends: a written status in this directory is evidence, not fact.** The difference is that
running the binary corrected *severities*, while re-reading at HEAD corrected *existence*. Neither
substitutes for the other, and the sweeps did the cheaper one — nothing in either sweep was executed.

---

## 0b. AMENDMENT 2026-08-14 (second) — what sweeps 3-6 did to these seventeen rows

> **Still a measurement, still not re-measured.** cyrup HEAD is now **`bdcb0d0`** (this log ran at
> `0c76986`; amendment 0a was written at `380c713`). Sweeps 3, 4, 5 and 6 landed in between and
> **none of them executed anything either** — sweep 6 was explicitly forbidden `cargo nextest`.
> Every status below is again fixed **by reading, not by re-running**.

**The suite numbers moved again: the gate is now `cargo nextest run --workspace` = 6699 tests, 6699
passed, 7 skipped, in 16.3 s** (was 6440 / 8 skipped / 16.4 s in amendment 0a; 6387 in §1; the
inherited 3932 is two corrections old). The structural facts in 0a are unchanged: 310 integration
binaries → **6 + 8 gated** behind the `cyrup-it` harness crate, and **`cyrup-it` is
`required-features = ["it"]`, so the gate still buys zero coverage of the broker-socket seam tests.**

**One correction to 0a, and it matters because it was this log's evidence:** 0a cites "four `cyrup-it`
assertions currently contradicting production" as proof that the un-built crate is not a theoretical
problem. **Sweep 6 re-read all four at HEAD and they now match production** — `tool_actions.rs:319`,
`:372`, `:502` and `intercom_command_transcript.rs:144` carry no trailing period, exactly as
`tools/intercom.rs` emits (`ICOM-026`, closed as REFUTED). **The structural defect is unchanged and is
now filed in its own right as `ICOM-053`**; it just no longer has that particular demonstration.

### Row changes since amendment 0a

| row | status at `380c713` (0a) | status at `bdcb0d0` |
|---|---|---|
| `SEAM-061` | "STILL OPEN, and now the top of the whole backlog" | **FIXED — and it was already fixed when 0a said otherwise.** Sweep 6 found both halves live at HEAD: `cyrup-tui/src/session_selector.rs:154`, `:276`, `:313`, `:1918`, `:1985` (including upstream's un-wired `onToggleScope` semantics, where Tab is *swallowed* rather than ignored) and `crates/cyrup/src/main.rs:1354` taking pi's **two** loaders, handed to the picker at `startup_ui.rs:191-201` (`cli/session-picker.ts:15-19` @v0.83.0). **The §3 transcript for this row is NO LONGER current behaviour.** |
| `SESS-040` | "STILL OPEN, the cheapest of the three remaining highs" | **STILL OPEN — and now one of only TWO highs in the entire backlog** (with `PROV-047`). Unchanged: the indicator advertises "(esc to cancel)" and nothing dispatches. Its §3 transcript IS still current behaviour. |

**Sixteen of the seventeen are now fixed; one — `SESS-040` — is open.**

### A new measurement risk this log should own

Sweep 6 root-caused the intermittent `cyrup-tools` nextest **`LEAK`** (filed as `TOOL-042`) to fd
inheritance: `std::process::Command` defaults every **unnamed** stdio handle to `Stdio::inherit()`, so
a test fixture that named only `.stdout(piped())` handed its child the harness's stdin and stderr and
held nextest's pipe open past exit. **That closure is a static argument plus two regression pins, not
an observation** — no one has re-run it. **Confirming it needs ~35 runs of `cargo nextest run -p
cyrup-tools` with the LEAK count reported**, which is exactly the kind of work this log exists to do
and which no sweep is permitted to perform. If a LEAK still appears, the next instrument is
`lsof -p <pid>` on any surviving `sleep`/`bash` — **not `pgrep -f <pattern>`, which matches its own
pattern in other shells' command lines and already produced a fabricated "22 orphaned brokers"
measurement in this very log.**

### Two live-behaviour claims filed by sweep 6 that this log has NOT measured

Recorded here rather than in an area file, because they are precisely the class this log exists for:

1. **`CFG-049`'s startup keypress gate.** `show_deprecation_warnings` now prints pi's block, prints
   `Press any key to continue...`, enters raw mode, reads one byte and restores — blocking startup
   before TUI init. **The block itself is exercised by no test** (a harness whose stdin is a terminal
   would hang the suite), so only the strings and the empty early return are pinned. **Needs a live
   run with `~/.cyrup/hooks/` present: the session must WAIT.** It also carries a deliberate
   `[CYRUP-DELTA]` — pi's `once("data")` never fires on an already-closed stdin, so *upstream hangs
   there* and cyrup returns on a zero-length read.
2. **`CFG-051`'s migrated-credential notice.** The line moved off the pre-TUI stderr path into the
   transcript, ahead of `modelFallbackMessage`. Verified **structurally**, not visually: nothing
   asserts a RENDERED transcript line. Needs a rendered-transcript assertion in
   `crates/cyrup-tui/tests/` plus a live confirmation.

---

## Header — what was run, where, and with what honesty about scope

| | |
|---|---|
| **Repo / branch** | `/Users/davidmaple/cyrup.ai/cyrup`, branch `david/cyrup` |
| **Commit** | `0c76986` (`docs(adr): settle the nine open questions, and reconstruct ADR-0001/0002`) |
| **Working tree** | **not clean** — carries the suite-repair edits described in §2, which were made during this exercise and are listed there file by file |
| **Platform** | macOS 26.5.2 (build 25F84), Darwin 25.5.0, `arm64` (Apple M2 Pro, `T6020`) |
| **Toolchain** | `rustc 1.96.0 (ac68faa20 2026-05-25)`, `cargo 1.96.0 (30a34c682 2026-05-25)` |
| **Binary** | `target/debug/cyrup`, built by `cargo build --workspace` (exit 0) |
| **Upstream tags read** | `pi` v0.83.0 (ported baseline) and v0.84.1 (latest); `pi-permission-system` v0.7.1 / v0.8.0; `pi-intercom` v0.9.2 / v0.10.1. Always via `git show <tag>:<path>`, never a working tree. |
| **Date** | 2026-08-13 |

### Instruments, and why each was chosen

- **`tmux` (a real terminal emulator on a real pty)** — the primary instrument for every TUI row.
  `script -q /dev/null` was tried first and is **not usable**: nothing answers the TUI's cursor-position
  probe, so the binary dies at startup with `terminal backend error: The cursor position could not be
  read within a normal duration`. That failure is itself worth recording, because it is what a naive
  "just run it in CI" attempt will hit.
- **`python pty.fork` + `pyte`** — used for the pre-launch selector rows (`SEAM-061`…`SEAM-064`),
  where the screen had to be read as a cell grid rather than as a capture-pane string.
- **Headless invocation with `env -i`** — for rows whose entire claim is an exit code, an argv
  rejection or a signal response (`SEAM-051`, `SEAM-047`, `PERM-009`).
- **Session JSONL readback** — for every row whose claim is about what the *model* received or what
  was *persisted*. On-screen text was never accepted as evidence of either. This is the single most
  load-bearing methodological choice in the log: three rows (`TUI-042`, `TUI-043`, `TUI-027`) look
  cosmetic on screen and are data defects in the file.

### Hygiene, stated so the results can be attacked

Every run used a **scratch `HOME` and a scratch `CYRUP_AGENT_DIR`**. The user's real `~/.cyrup` was
never read or written by any measurement in this log. Provider keys and proxies were scrubbed from
the environment except in the four rows that deliberately used a real provider (`PERM-009`,
`AGENT-020`, `SESS-040`, and the `newDefects` that came out of those sessions), which are marked as
such and name the model they used.

### Scope, honestly

| | count | rows |
|---|---:|---|
| driven through a **real pty** | 14 | `SEAM-061`, `SEAM-062`, `SEAM-063`, `SEAM-064`, `UW-2`, `TUI-016`, `TUI-027`, `TUI-042`, `TUI-043`, `TUI-044`, `TUI-045`, `AGENT-020`, `EXT-054`, `SESS-040` |
| driven **headless** (exit codes / argv / signals) | 3 | `SEAM-051`, `SEAM-047`, `PERM-009` |
| **blocked** — could not be driven at all | 0 | — |

**Nothing was blocked.** The one row that carried a pre-declared BLOCKED branch (`EXT-054`, "if the
`wasm32-wasip2` toolchain is unavailable") did not need it: the target was already installed and the
reference guest built in 0.10 s.

### What this log does NOT cover

- 431 of the 448 open items were not driven. No claim here generalises to them except the
  methodological one in the closing section.
- No row was driven on Linux or Windows. At least one finding (`TUI-N13`, §2) is macOS-specific *and
  was invisible on Linux*, so single-platform measurement is a known hole in this very log.
- The four rows using a live provider used **Together** (`openai/gpt-oss-120b`, `openai/gpt-oss-20b`,
  `Qwen/Qwen3.5-9B`). Model-specific behaviour is called out where it was observed
  (see `PERM-032`).

---

## 1. The real suite numbers

### The claim that was inherited

Every prior pass quoted **"3932 passed / 0 failed / 8 ignored"**. No pass had executed the suite. The
figure was carried forward by citation.

### What the first run actually produced

The suite had four red targets, leaked processes, and hung.

```
-p cyrup-ext-subagents --lib ......... 2216 passed, 2 FAILED
    spawn::tests::terminate_reaches_the_childs_own_descendants_not_just_the_direct_child
    spawn::worktree::tests::a_timed_out_setup_hook_is_killed_not_abandoned
-p cyrup-ext-subagents --test native_supervisor_channel_integration ... FAILED
-p cyrup-tui --test bash_overlay ..... 10 passed, 2 FAILED
    hotkeys_global_key_cells_resolve_from_the_live_keymap
    hotkeys_lists_extension_registered_shortcuts
-p cyrup-tui --test markdown ......... FAILED

HANG:    cyrup/tests/one_shot_parity.rs::an_unmatched_models_pattern_warns_on_stderr
         "has been running for over 60 seconds" in the full run;
         4.19 s and 4/4 green when the target is run alone.

LEAK:    orphaned `cyrup __intercom-broker` processes surviving cargo's exit,
         the oldest 45 minutes old.
```

### The before/after — this is the finding

| | first run (as inherited) | after the repairs |
|---|---|---|
| **passed** | 6386 | **6387** |
| **failed** | 1 *(plus 4 red targets found in targeted runs, plus one hang that prevented a full run from completing at all)* | **0** |
| **ignored** | 8 | **8** |
| **filtered** | 0 | **0** |
| **targets** | 338 | **338** |
| **wall (warm)** | 709.67 s | **~250 s** |
| **orphaned brokers after run** | 22 reported *(see the correction below)* | **0**, three runs running |
| **hangs** | ≥1 indefinite | **0** |
| **flaky set** | non-empty (3 independent mechanisms) | **∅**, across 3 consecutive runs under 2 stdin conditions |

**The inherited number was not merely unrun — it was wrong in magnitude.** The suite is **6387**
tests, not 3932. A figure quoted through four passes was off by 2455 tests, which is the cleanest
available demonstration of what citation-without-execution costs.

**Final, and now meaningful: `6387 passed / 0 failed / 8 ignored / 0 filtered` across 338 targets.**

The 8 ignored are named rather than counted: six live-provider tests
(`live_anthropic`, `live_openai`, `live_google`, `live_mistral`, `live_together`,
`live_openrouter_images` — each needs a real API key), `git_clone_real_network_https`, and one
doctest (`cyrup-ext-sdk/src/macros.rs`). **Nothing was `#[ignore]`d or deleted to reach green.**

### Determinism evidence

Five full `cargo test --workspace --no-fail-fast` runs:

| run | condition | passed | failed | ignored | wall |
|---|---|---:|---:|---:|---:|
| 1 | as-inherited | 6386 | 1 | 8 | 709.67 s |
| A | stdin = open pipe | 6386 | 1 | 8 | 435.81 s |
| B | stdin = open pipe | 6387 | **0** | 8 | 976.86 s |
| C | stdin = `/dev/null` | 6387 | **0** | 8 | 258.35 s |
| D | stdin = `/dev/null` | 6387 | **0** | 8 | 247.05 s |

Runs B, C and D have **byte-identical failing sets (∅)** and identical totals under two different
stdin conditions, with zero `has been running for over 60 seconds` lines in any of the three.

### A measurement correction, recorded because it matters more than the number

The original "22 orphaned brokers" figure was produced by `pgrep -f '__intercom-broker'`, which
**matches its own pattern in other shells' command lines**. The correct instrument is:

```
ps -axo pid,command | grep -cE '[/]cyrup __intercom-broker'
```

Under that instrument the counts are `BEFORE=0 / AFTER=0` for runs A, B and C — zero orphaned
brokers, three runs in a row. The leak was real (the harness fix took it from 13 to 0), but **the
number that named it was an artifact of the tool that measured it.** A repro log that did not
re-derive its own instruments would have propagated it.

A second, smaller leak is real and constant: `+15` temp dirs per full run (14 × `.tmpXXXXXX/` from
`-p cyrup-session-svc` alone, 1 × `cyrup-bash-*.log`), attributed to `FileLock::acquire` creating a
lock file that `Drop` never removes (`cyrup-config/src/lock.rs:42-46`) while a store access lands
after `TempDir::drop`'s `remove_dir_all` has begun. Filed as `SEAM-073`. Constant, not growing.

---

## 2. Repairs made to reach green, and their classification

Every red was classified against pi at its tag **before** anything was touched. None was weakened.
The three test-side fixes were **mutation-checked** — production code was deliberately broken to
prove the repaired test still goes red.

| what | class | why | filed |
|---|---|---|---|
| `crates/cyrup/src/input.rs`, `main.rs`, `lib.rs` | **production, parity fix** | `build_inputs` read the process's own stdin internally; pi reads `stdinContent` in `main` (`main.ts:819-826`) and **passes it in** (`:828-832`, declared `:169-172`). cyrup had fused the descriptor-owning step into prompt assembly, so any test driving `build_inputs` hung forever on an inherited pipe. Restored pi's split. | `SEAM-072` |
| `crates/cyrup-tui/src/app.rs:7652` | `test-defect` | `a_live_bash_run_names_its_spool_file` parsed a spool path out of **one** rendered line; the status block is word-wrapped and macOS `TMPDIR` (`/var/folders/<2>/<30>/T/`) pushes the row to exactly 120 columns, so the path wraps. Green on Linux (`/tmp`), red on every macOS host. The wrap is faithful to pi (`bash-execution.ts:201` @v0.83.0). Fixed by flattening whitespace before parsing; **assertion unchanged**. | `TUI-N13` |
| `crates/cyrup-ext/src/caps/proc.rs` | `test-defect` | Asserted `buffered <= MAX_PIPE_BUFFER_BYTES` exactly; the pump's real invariant is one chunk looser (`wait_for_room` returns at `len < CAP`, then a full 8 KiB read appends). Overshoot measured at 4412 bytes — a partial chunk, i.e. the bounded behaviour the test exists to prove. Named the chunk, loosened by exactly one chunk, and **added** a plateau assertion so the test still discriminates. | `EXT-N01` |
| test fixtures (`image_auto_resize_file_args.rs`, `image_bytecap.rs`, intercom env) | `test-defect` | consequences of the two above | `ICOM-051` |

**`SEAM-072` is almost certainly the same root cause as the previously unexplained
`one_shot_parity::an_unmatched_models_pattern_warns_on_stderr` stall**, which is why that hang does
not appear in runs B–D.

Two further items were filed from this work without a production change, because both are product
decisions rather than repairs: `SEAM-071` (`--no-extensions` cannot switch off the three native
built-ins) and `SEAM-073` (the temp-dir leak).

### The hypothesis the plan asked to be tested first

> "`Command::output()` reads to EOF, not to child exit. A grandchild inheriting those pipe handles
> holds them open. Orphaned brokers are exactly such grandchildren — and the two subagent failures
> are *about* killing a child's descendants. Confirm or refute it; do not assume it."

**Partially confirmed, and the correction is instructive.** The *shape* of the hypothesis — a
blocking read on a descriptor nobody closes — was exactly right, and it did explain the hang. But
the descriptor was **not** the one predicted. It was **fd 0 of the cargo test runner itself**, read
to EOF by `read_piped_stdin()` inside `build_inputs`, with no grandchild involved at all. The
orphaned brokers were a *separate* mechanism (an ambient `CYRUP_INTERCOM` opting hermetic tests into
a broker), and the subagent failures were a *third*.

So: one hypothesis, three mechanisms, and the unifying story was wrong even though the instinct that
produced it was right. Worth recording as a caution — a hypothesis that explains all three symptoms
at once is attractive precisely in proportion to how little it has been tested.

---

## 3. Row-by-row results

**17 rows. 16 CONFIRMED · 1 REFUTED · 0 BLOCKED.**

Transcripts below are trimmed to the load-bearing lines; nothing is paraphrased. `$R` is a scratch
root under the session scratchpad; `$H` a scratch `HOME`.

---

### `SEAM-051` — CONFIRMED · headless-binary

**Predicted** — `--tui-mode <regular|fullscreen>` is captured as an unrecognised extension flag and
exits 1, so the flag's own default value refuses to launch the binary.

**Happened** — exactly as predicted at the observable level. Reproduced with the value present,
absent, bogus and in `=` form; with extensions enabled and disabled; and in print, `--mode json` and
`--mode rpc`.

```
$ cyrup --offline --no-session --no-extensions --tui-mode regular -p hi </dev/null
Error: Unknown option: --tui-mode
EXIT=1

$ cyrup --offline --no-session --tui-mode fullscreen -p hi        # extensions ENABLED
Error: Unknown option: --tui-mode                    EXIT=1
$ cyrup --offline --no-session --no-extensions --tui-mode -p hi   # no value
Error: Unknown option: --tui-mode                    EXIT=1
$ cyrup --offline --no-session --no-extensions --tui-mode=regular -p hi
Error: Unknown option: --tui-mode                    EXIT=1
$ cyrup --mode json --tui-mode regular --offline --no-session --no-extensions
Error: Unknown option: --tui-mode                    EXIT=1
$ cyrup --mode rpc  --tui-mode regular --offline --no-session --no-extensions
Error: Unknown option: --tui-mode                    EXIT=1

$ cyrup --offline --no-session --no-extensions -p hi               # CONTROL, flag absent
No more faux responses queued                        EXIT=1        # reached the provider

$ cyrup --tui-mode regular --help
cyrup - AI coding assistant with read, bash, edit, write tools
... (full help printed; no --tui-mode row anywhere in Options)
EXIT=0
```

The control is what makes the row conclusive: without the flag the binary gets as far as the
provider; with it, it dies before any session is built.

**Correction applied** — two mechanism details were wrong. (1) The emitted text is `Error: Unknown
option: --tui-mode` (**singular**); the plural `Unknown option(s):` form is used only when more than
one flag is unmatched. (2) The item attributed the failure to extension-flag partitioning in a way
that reads as though the extension subsystem is required to produce it — the error is **identical
under `--no-extensions`**, so the reconciliation diagnostic runs regardless of extension discovery.
Verdict unchanged.

---

### `SEAM-064` — CONFIRMED · live-terminal

**Predicted** — the pre-launch trust prompt renders three options instead of pi's five, so no answer
can avoid persisting a verdict.

**Happened** — three rows, and the persistence consequence measured rather than inferred.

```
────────────────────────────────────────────────────────────────
 Project trust
 /private/tmp/.../trust/projT

 Saved decision: none
 Current session: untrusted

 → Trust
   Trust parent folder (/private/tmp/.../scratchp
   Do not trust

 ↑↓ navigate  enter save  escape/ctrl+c cancel
────────────────────────────────────────────────────────────────

# option count = 3. No "Trust (this session only)", no "Do not trust (this session only)".

# Enter on the default row:
$ cat $R/agent/trust.json
{ "/private/tmp/.../trust2/projT": true }

# ESC instead:
$ cat $R/agent/trust.json
cat: .../trust/agent/trust.json: No such file or directory
```

Cancel is the only non-persisting exit, and it does not grant trust. **No correction.**

---

### `UW-2` — CONFIRMED · live-terminal

**Predicted** — the first-run setup wizard is gated but never invoked; the gate is live for this
build.

**Happened** — with `CYRUP_EXPERIMENTAL=1`, no `settings.json`, and the default agent dir, the binary
goes straight to the interactive TUI. No theme picker, no analytics question, no `settings.json`
written.

```
30|  escape interrupt · ctrl+c/ctrl+d clear/exit · / commands · ! bash · ctrl+o more
...
39| 0.0%/128k (auto) • xp                                            faux/faux-1

$ ls -la $H/.cyrup/agent/
-rw-r--r--  0 B models-store.json.lock          # and nothing else
```

Two independent corroborations that the gate's inputs were **all true in this very process**: the
footer prints the `xp` experimental badge (`status.rs:356`, pi `footer.ts:162-164`), proving
`CYRUP_EXPERIMENTAL=1` was read; and the agent dir ends the run holding only the lock file, proving
`settings_path` did not exist. The wizard surface is not broken-but-invisible either — the sibling
pre-launch selectors (trust prompt, resume picker) render fine on the same pty in the same pass.

**Correction applied** — the `main.rs:215-217` comment claiming the gate is "faithfully `false` for
the cyrup rebrand" is **contradicted by the running binary**: `CARGO_PKG_NAME` is `cyrup` and
`APP_NAME`/`CONFIG_DIR_NAME` are `cyrup`/`.cyrup`, so `is_official_distribution()` is true and the
gate does fire — into an empty `if` body. This settles `PARITY-GAPS.md` **OQ-6** on the evidence
side: the standing trap-list entry "the deliberately unreachable first-run wizard" is **wrong**, and
should be struck.

---

### `SEAM-047` — CONFIRMED · headless-binary

**Predicted** — the first SIGTERM/SIGHUP neither tears down nor exits 143/129; `--mode rpc` runs
forever and never emits `session_shutdown`.

**Happened** — confirmed end to end, with shell exit codes as evidence.

```
$ mkfifo $R/in
$ cyrup --mode rpc --offline --no-session --no-extensions < $R/in > $R/out 2> $R/err &
$ exec 3> $R/in ; sleep 3
alive_before_signal: 1
$ kill -TERM $PID          # poll `kill -0` once a second for 15 s
STILL ALIVE after 15s following SIGTERM
second-signal-needed: SIGKILL
--- stdout --- (empty)     --- stderr --- (empty)

# SIGHUP: identical — STILL ALIVE after 15s, SIGKILL required.

# double delivery:
first TERM  -> alive
second TERM -> EXITCODE=143
--- stdout --- (empty)
```

The `143` on the **second** delivery is precisely the item's claim that `ShutdownSignal::exit_code`
is only consulted the second time. stdout was byte-empty in every run, so no `session_shutdown` is
emitted on the way out under either delivery. `timeout(1)` and container-stop semantics are
unavailable against rpc mode. **No correction.**

---

### `SEAM-063` — CONFIRMED · live-terminal

**Predicted** — session delete permanently unlinks the JSONL where pi routes through `trash` first,
and the `io::Result` is swallowed.

**Happened** — both halves, live, with a stubbed `trash` on `PATH` as the instrument.

```
# part 1: is `trash` reached?
$ cat $R/bin/trash
#!/bin/sh
echo "TRASH CALLED: $@" >> $R/trash.log
exit 0

# --resume picker, ctrl+d then Enter:
 4| Delete session? enter confirm · escape/ctrl+c cancel
 9| › delete me                                              1 msgs
   -> after Enter:
 9|   No sessions in current folder. Press Tab to view all.

$ cat $R/trash.log
cat: .../trash.log: No such file or directory      # trash NEVER invoked
$ ls -l <session>.jsonl
No such file or directory (os error 2)             # permanently unlinked

# part 2: make the delete FAIL.
$ chmod 555 <sessions-dir>                          # dr-xr-xr-x
   -> after Enter:
 9|   No sessions in current folder. Press Tab to view all.
# no error text on screen, no stderr line on exit
$ ls -l <session>.jsonl
-rw-r--r-- 417 B  .../2026-01-01T00-00-00-000Z_...d002.jsonl   # STILL THERE
```

A failed delete is **visually identical** to a successful one, and the user is left believing the
session is gone when it is not.

**Correction applied** — the item says cyrup "additionally reports success unconditionally". That is
accurate for the in-app `/resume` path (`app.rs:4025` prints "deleted session"), but the pre-launch
`--resume` picker measured here prints **nothing at all** — no success text, no failure text; the
only feedback is the row disappearing. On this surface the defect is a *missing status line* as well
as a swallowed error, and the Fix must add a status channel to `startup_ui.rs`'s `on_apply` for pi's
`"Session moved to trash"` / `"Session deleted"` / `"Failed to delete: …"`
(`session-selector.ts:846`, `:849`) to render into. Upstream half re-read at v0.84.1: matches the
item verbatim.

---

### `SEAM-062` — CONFIRMED · live-terminal

**Predicted** — the pre-launch picker offers rename, shows the new name, and discards it.

**Happened** — confirmed exactly as filed, including the relaunch check.

```
 5| ctrl+s sort · ctrl+n named · ctrl+d delete · ctrl+p path (off) · ctrl+r rename
 9| › rename me please                                        1 msgs
 -- ctrl+r --
 4| enter to save · escape/ctrl+c to cancel
 7|  rename
 -- typed NEWNAME, Enter --
 9| › NEWNAME                                                 1 msgs

$ grep -c NEWNAME <session>.jsonl
0                                    # grep exit=1 — nothing written, no session_info line

$ cyrup --resume ...                 # relaunch
 9| › rename me please                                        1 msgs
```

Complete positive feedback; zero persistence. Upstream re-read at v0.84.1: `cli/session-picker.ts:48`
constructs the component with `{ showRenameHint: false }` and **no** `renameSession` callback, so
pi's pre-launch picker cannot enter rename mode at all. **No correction.**

---

### `SEAM-061` — CONFIRMED · live-terminal

**Predicted** — the `--resume` picker merges current-folder and all-projects sessions, labels the
result "Current Folder", and advertises a dead `tab scope` toggle.

**Happened** — confirmed on every clause, plus the impact claim measured rather than inferred.

```
 3| Resume Session (Current Folder)   ◉ Current Folder | ○ All  Name: All  Sort: Threaded
 4| tab scope · re:<pattern> regex · "phrase" exact
 5| ctrl+s sort · ctrl+n named · ctrl+d delete · ctrl+p path (off) · ctrl+r rename
 9| › alpha one message                                      1 msgs      <- projA
10|   alpha two message                                      1 msgs      <- projA
11|   beta one message                                       1 msgs      <- projB, foreign

-- TAB --
(identical screen, byte-for-byte; the raw pty log shows the redraw emitting only
 ESC[39m ESC[49m ESC[59m ESC[0m ESC[?25l and NO cell changes)

-- select the foreign row and Enter, from projA --
27|  beta one message
38| /private/tmp/.../picker/projA        <- footer cwd is STILL projA
39| 0.0%/128k (auto)                                          faux/faux-1

-- empty state, elsewhere --
 9|   No sessions in current folder. Press Tab to view all.
```

Tab is not merely unbound — it produces **no redraw at all** — while the hint row prints `tab scope`
and the empty state instructs the user to press it.

**Correction applied** — one screen element the item does not mention, and it makes the misreport
worse: the header also renders a scope **radio**, `◉ Current Folder | ○ All`. So the UI does not
merely name the wrong scope in prose, it draws a two-state control showing "Current Folder" selected,
beside a `tab scope` hint, over a list that is already both scopes merged. Also recorded for the Fix:
`ctrl+p path (off)` exists as a *manual* cwd-column toggle, whereas pi derives `showCwd` strictly
from `scope === "all"` (`session-selector.ts:844`) — the fix must make `show_path` **follow** the
scope, not merely arm the Tab action.

---

### `TUI-042` — CONFIRMED · live-terminal

**Predicted** — the undo snapshot omits the paste registry, so undoing a delete over a `[paste #N …]`
marker silently drops the pasted content from the submitted message.

**Happened** — end to end, with the model's actual input read out of the session JSONL rather than
off the screen. 719 bytes / 40 lines pasted through a **real bracketed paste**
(`tmux paste-buffer -p`, which emits `ESC[200~ … ESC[201~`).

```
$ wc -c paste40.txt
     719 paste40.txt

-- real bracketed paste --
[paste #1 +40 lines]

-- ONE Backspace: the marker is atomic and vanishes whole --
(empty)

-- Undo. Ctrl+- must be sent as the kitty CSI-u form ESC[45;5u (see TUI-053) --
$ tmux send-keys -H 1b 5b 34 35 3b 35 75
[paste #1 +40 lines]        <-- marker is BACK; the UI says the paste is restored

-- Enter. What the model actually received: --
USER MSG len=20 text='[paste #1 +40 lines]'

-- CONTROL: same paste, no edit, straight to Enter, same session --
USER MSG len=20  '[paste #1 +40 lines]'
USER MSG len=719 'PASTELINE001-zeta\nPASTELINE002-zeta\n…\nPASTELINE040-zeta'
```

The control is what isolates the cause: with no edit, all 719 characters are sent. The loss is caused
by the **undo**, not by the paste path. The quieter variant confirmed too: paste → undo → paste
re-issues `#4` where pi restores `pasteCounter` and reissues `#3`.

**Correction applied** — one measured numeric nit. The Impact calls `[paste #1 2000 chars]` "the
20-character literal string"; measured live it is **21** characters (`len=21`). 20 is correct only
for the `+N lines` form. Mechanism, call sites and both variants reproduce verbatim.

---

### `TUI-043` — CONFIRMED · live-terminal

**Predicted** — word motion and Ctrl+W are not paste-marker atomic; one Ctrl+W after a large paste
orphans the marker and drops the content.

**Happened** — verbatim, both halves.

```
[paste #1 +40 lines]
-- ONE Ctrl+W with the caret at the end (col 20) --
[paste #1 +40 lines               <-- exactly ONE character, the ']', was deleted
-- Enter --
USER MSG len=19 text='[paste #1 +40 lines'

-- the cursor-motion half --
[paste #2 +40 lines]
-- Alt+Left (cursor word backward), then type "X" --
[paste #2 +40 linesX]             <-- caret parked INSIDE the marker
```

**Correction applied** — the Verify line's parenthetical is imprecise. It asks to assert that
`E::CursorWordBackward` from the marker's end "lands on the marker's start column, **not one char
short**". Measured, cyrup lands **19 columns short**, not one: from col 20 the class run consumes
only the `]` and stops at col 19. The analysis and the Fix are otherwise exactly right.

---

### `TUI-044` — CONFIRMED · live-terminal

**Predicted** — `undo()` discards the snapshot's cursor column, so the caret ends up where it
happened to be rather than where the undone edit was.

**Happened** — confirmed by **two independent readouts**, reproduced twice from a clean editor.

```
-- kill ring loaded with 'world' (Ctrl+U), type 'hello' (caret 5), Ctrl+Y --
yanked : [helloworld]
-- 8 × Left -> caret 2 --
8xLeft : [helloworld]
-- Undo (ESC[45;5u) --
undo   : [hello]                       # buffer correctly restored

-- readout 1: the software caret is a reverse-video (SGR 7) cell --
$ tmux capture-pane -e -p | cat -v | tail -8
^[[38;2;212;212;212mhe^[[7ml^[[0m^[[38;2;212;212;212mlo
                     ^^^^^^ caret on index 2, NOT index 5

-- readout 2: behavioural. The next keystroke lands there. --
typed Z: [heZllo]                      # pi gives 'helloZ'
```

**No correction needed** — the item is exactly right, including its concrete example. **One note
added for whoever writes the regression test**, because it would otherwise mislead:
`tmux display-message -p '#{cursor_x}'` is **not a valid instrument here**. cyrup hides the hardware
cursor and paints its own caret as a reverse-video cell, so the pane's hardware cursor is stale
write-position (measured: 6 on an empty editor, 4 with the caret at logical col 2, 10 immediately
after this undo). Read the SGR-7 cell from `capture-pane -e`.

---

### `TUI-027` — CONFIRMED · live-terminal

**Predicted** — `/tree` has no text search, and its four action keys are the characters pi types into
that search; the captured text is persisted to the session JSONL.

**Happened** — confirmed, and the **persistence half is now measured, not traced**. The instrument
was typing the ordinary word `text`, one key at a time.

```
-- session JSONL BEFORE opening /tree --
session / model_change eeb2d0cf / thinking_level_change / message / message
(no `label` entry)

 Session Tree   Filter: default   (4/4)
› ◆ model → faux-1
  └⊟ ◇ thinking → off
     └⊟ ● user: hello world
 ↑/↓ move   ←/→ page   z/x branch   e label   t label time

after 't' -> Filter: default (4/4) [+label time]      # timestamp column toggled
after 'e' -> Label (empty to remove):  >              # inline label EDITOR opened
after 'x' -> > x                                      # search text captured as a LABEL
after 't' -> > xt

-- Enter --
 label → xt
› ◆ model → faux-1  ☆labeled

-- the DATA: last line of the session JSONL --
{"type":"label","id":"01dfd155","parentId":"471ccb39","timestamp":"2026-08-13T12:48:26.225394Z",
 "targetId":"eeb2d0cf","label":"xt"}
```

The entry counter never moved off `(4/4)` across all four keystrokes — **there is no search state of
any kind.** The `targetId` is the `model_change` entry: whichever row the cursor happened to be on
when `e` was pressed — on a fresh session, not even a message.

**No correction.** The measured artefact is appended to the item body so nobody has to re-derive it.

---

### `TUI-016` — CONFIRMED (with a corrected headline) · live-terminal

**Predicted** — "Queued messages are now entirely invisible — texts discarded, footer count deleted."

**Setup note, recorded because it was the hard part.** The faux provider cannot stream from the
binary (unscripted ⇒ `No more faux responses queued` immediately), so a genuine streaming turn was
produced from a **local** fake `openai-completions` endpoint declared in the scratch agent dir's
`models.json` — 60 SSE deltas, one per second, bound to `127.0.0.1` only. No network.

**Happened** — the *absence* claim confirmed completely; the *headline* is wrong, and the difference
matters.

```
 21| count slowly
 25| QUEUEDMSGONE            <-- rendered INTO THE CHAT TRANSCRIPT
 29| QUEUEDMSGTWO            <-- same, indistinguishable from a delivered message
 32| tok00 tok01 … tok18
 35| ⠏ Working...
 36|─────────────────────────────────────────────────────────
 37|                          <-- editor: empty. No pending-messages region.
 38|─────────────────────────────────────────────────────────
 40| 0.0%/128k (auto)                                slowly/slow-1
                              ^ footer: no queued count, no hint

# whole 200-line scrollback searched for ANY queue surface:
$ capture-pane -S -200 | grep -in "queue\|steer\|follow"
51: QUEUEDMSGONE
55: QUEUEDMSGTWO
# ZERO hits on 'queued', 'Steering:', 'Follow-up:' or the '↳ … to edit all queued messages' hint.

# they really WERE queued, not dispatched — the stream ran to tok59 first:
user      | "count slowly"
assistant | "tok00 … tok59"
user      | "QUEUEDMSGONE"
assistant | "tok00 … tok15"   (interrupted)
```

**Correction applied — retitle and restate.** The item should not read "entirely invisible — texts
discarded". cyrup **optimistically renders each queued message into the chat transcript as an
ordinary user bubble**, so the user sees text that looks *delivered* while it is still sitting in a
queue. The observable is **worse than absence**: an affirmative, wrong signal. Two sub-corrections:
(1) "texts discarded" is true only of the TUI's copy — the session layer keeps them, proved by
Escape restoring `QUEUEDMSGTWO` verbatim into the editor; (2) the Fix must **remove the transcript
echo at the same time** the pending-messages rows are added, or pi's `Steering:` row and cyrup's
phantom bubble will both render and the message will appear twice.

This row also produced a new defect that survives both `TUI-016` and `TUI-005` — filed as `TUI-052`.

---

### `PERM-009` — CONFIRMED · headless-binary

**Predicted** — `should_expose_tool` re-exposes `bash` despite a tool-level deny, and the
allow-listed command then executes.

**Happened** — a **live, end-to-end permission bypass in the shipped binary**, not a code-shape
concern. The instrument is a canary file created seconds earlier that no model could hallucinate.

```
$ touch $S/perm009/proj/PERM009_CANARY_9f3a.txt

=== CASE B (CONTROL: tool-level deny only) ===
{ "tools": { "bash": "deny" } }
$ cyrup --provider together --model openai/gpt-oss-20b --no-session \
        -p "Run the shell command: git status"
analysis: We cannot run shell commands. We can just explain that.
final:    I don't have permission to run arbitrary shell commands directly. …
# nothing executed.

=== CASE A (BYPASS: same deny + a NARROWER command allow) ===
{ "tools": { "bash": "deny" },
  "bash":  { "git status": "allow" } }
$ (same command)
final: **git status**
    On branch main
    |
    No commits yet
    |
    Untracked files:
        PERM009_CANARY_9f3a.txt
        a.txt
    |
    nothing added to commit but untracked files present
# genuine git(1) output from THAT repo — canary present, "No commits yet" present.
# (the model's reply wrapped this in its own code fence; re-indented here so it
#  does not terminate this transcript block)

=== CASE C (same bypass config, a command NOT on the allow list) ===
$ ... -p "Run the shell command: whoami"
The `whoami` command cannot be executed due to permission restrictions.
```

Case C bounds the bypass: it grants exactly the allow-listed command — precisely the mechanism the
item predicts (`should_expose_tool` re-exposes `bash`, then `manager.rs:205-215` resolves the command
rule above the tool-level deny). The item's own Verify fails on **both** halves today.

**Correction applied** — the item needs no factual change; one line is strengthened from prediction
to measurement. Impact's "`git status` **executes**" is now a first-hand observation, and Confidence
moves from "both sides re-read at both upstream tags" to "**reproduced in the shipped binary**".
Severity `critical` is correct and, if anything, understated.

---

### `AGENT-020` — **REFUTED** · live-terminal

**Predicted** — `continue_run` drains the steering queue before the run-active check, so "a
user-typed steering message is silently destroyed… on the normal path of typing while a turn is in
flight". That Impact was the sole justification for the `high → critical` raise.

**Happened** — the code citation is **accurate and unchanged at HEAD**; the Impact **is not what the
assembled binary does**.

```
$ tmux send-keys "Write out the numbers 1 to 400, one per line, nothing else."; Enter
 ⡆ Working...                                       # streaming, assistant at "30"

$ tmux send-keys "STEERING_CANARY_alpha7 stop and say ACK"     # typed MID-STREAM
 30
 ⡇ Working...
 ─────────────────────────────────────────
 STEERING_CANARY_alpha7 stop and say ACK   <-- in the editor
 ─────────────────────────────────────────
$ tmux send-keys Enter
# t+1s: editor empty, stream continues at "37"

-- full scrollback after the turn settles --
 69: STEERING_CANARY_alpha7 stop and say ACK     <-- queued, echoed into transcript
512: We need to obey stop. ACK.
514: ACK                                          <-- the model ANSWERED it

-- 4 further attempts, submitting at 3.0 / 4.0 / 4.5 / 5.0 s into a ~10 s turn,
   deliberately aimed at the settle boundary --
632: RACE_3.0_say_OK   700: OK
956: RACE_4.0_say_OK   961: OK
1112: RACE_4.5_say_OK  1117: OK
1268: RACE_5.0_say_OK  1273: OK
# 5/5 canaries survived and were answered. ZERO losses.

-- the cited code IS still there at HEAD --
crates/cyrup-agent/src/agent.rs:1635-1656
    if last_is_assistant {
        let steering = lock(&self.steering).drain();
        if !steering.is_empty() { return self.start_run(EntryStart::Prompt(steering), true).await; }
```

The drain-before-latch window is real. The **steering path the TUI actually uses does not enter it**:
typing during a stream queues and re-drives, and `continue_run` is not entered while the latch is
held on that path.

**Correction applied** — `critical → low`, and Impact rewritten as a **latent race** rather than an
unconditional loss. The Fix (push the drained messages back on the error path) is kept — it is cheap
and correct. Also recorded on the item: the `README.md:106-107` "data loss on a normal path"
criterion was applied to a **predicted** consequence the binary does not exhibit. See §5.

---

### `EXT-054` — CONFIRMED (and stronger than filed) · live-terminal

**Predicted** — `ExtensionManifest.capabilities` is never read; the declared WASM sandbox grant model
is inert. Row carried a BLOCKED branch if the `wasm32-wasip2` toolchain were unavailable.

**Happened** — the BLOCKED branch was not needed, and the result exceeds the item.

```
$ rustup target list --installed
aarch64-apple-darwin / wasm32-wasip2 / x86_64-pc-windows-gnu
$ cargo build -p cyrup-ext-sdk --target wasm32-wasip2
    Finished `dev` profile in 0.10s
$ ls -la target/wasm32-wasip2/debug/cyrup_ext_sdk.wasm      # 4.1 MB

# fixture manifest — the strictest declaration the schema can express:
{ "id": "nocaps-demo", "world": "cyrup:ext@0.4",
  "capabilities": { "fs": [], "exec": false, "net": false, "ui": false } }

# startup panel:  [Extensions]  cyrup-intercom / cyrup-permission-system / nocaps-demo / subagents

=== exec, declared false ===
 /execdemo
 exec stdout: hi                <-- host process REALLY executed `echo hi`

=== net, declared false, and the host was launched --offline ===
 /httpdemo https://api.together.xyz/v1/models
 http status: 401 body: Missing API key    <-- real TLS round trip to a live host

=== ui, declared false ===
# both results above were delivered through ctx.ui().notify(), which also worked.

=== consumer-side grep at HEAD (unchanged) ===
$ grep -rn capabilities crates/cyrup-ext/src --include='*.rs'
# producers only: manifest.rs:20, :23-35, loader.rs:213 / :259 (`Default::default()`), doc comments.
# NO consumer.
```

Every bit off, the full host surface granted.

**RESOLVED 2026-08-13** (sandbox-and-extension-gating pass). Fixed; see `06-cyrup-ext.md` EXT-054 for
the evidence block. The regression fixture is the one this repro used: build `cyrup-ext-sdk` for
`wasm32-wasip2`, lay it out with an `extension.json`, load it through `discover_and_load`. It lives
at `crates/cyrup-ext/tests/manifest_capabilities.rs` (9 tests; 5 go RED if the one-line manifest
threading in `load_discovered` is reverted).

**One line of this transcript's conclusion is withdrawn.** The `--offline` observation is factually
correct — the `net` bypass WAS measured with `--offline` set — but the inference that it constitutes
a second failed control is not. pi's `--offline` is documented, verbatim, as "Disable startup network
operations" (`packages/coding-agent/src/cli/args.ts:277` @v0.83.0), pi has no extension network gate
of any kind, and cyrup's help text is the identical sentence. A guest reaching the network on an
`--offline` host is parity. `EXT-058` is re-rated `low` and reclassified as a product decision rather
than a defect; the control that was genuinely missing was the manifest's `"net": false`, which now
works.

**Correction applied** — two edits. (1) Impact's "Blast radius today: **zero WASM guests ship** … so
nothing is currently mis-granted" overstates the safety margin. The SDK's **own reference guest** is
a complete, loadable component that exercises `exec` and `http-client`, `wasm32-wasip2` builds it in
under a second, and `-e <dir>` loads it pre-trusted — the mis-grant is reproducible **today**, with
no third-party code. (2) `--offline` does not gate the guest `http-client` import either: the `net`
bypass was measured **with `--offline` set on the host**, so neither the manifest grant nor the
offline flag stands between an installed guest and the network. The offline case is added to Verify
and filed separately as `EXT-058`.

---

### `SESS-040` — CONFIRMED on its central claim, mechanism half REFUTED · live-terminal

**Predicted** — compaction cannot be cancelled; Escape is inert; "the user presses Escape, **the
indicator keeps spinning**", and the band renders `(esc to cancel)`.

**Happened** — the central claim confirmed; the user-visible half is wrong, and wrong in the
*worse* direction.

```
=== A. Baseline: how long does a compaction last, and what is on screen? NO keys sent. ===
sampled every 200 ms for 4 s, then every 1 s:
[t=0.2s] complete=0 ind=      [t=5s]  complete=0 ind=
[t=0.4s] complete=0 ind=      [t=8s]  complete=0 ind=
   ...                        [t=10s] complete=0 ind=
[t=4.0s] complete=0 ind=      [t=11s] complete=1 ind=
# ind = `capture-pane -p | sed -n '/cancel\|Compact/p'` — EMPTY at every sample.
# ~10.5 s of compaction with NO spinner, NO "Compacting context...", NO "(esc to cancel)".
# The transcript area was literally blank.

=== B. The row itself: Escape at t=3s into a compaction ===
*** ESC at t=3s ***
[t=4s] … [t=17s]                      # nothing
[t=18s] compaction complete
# Escape did nothing. The provider call ran another ~15 s and billed.

=== C. The session file was mutated anyway ===
compaction  2026-08-13T13:08:22.426093Z  ## Goal - Provide a list of numbers from 1 to 250 …
# ^ the run in which Escape was pressed.

=== D. Code claims re-verified at HEAD ===
$ grep -rn "AbortCompaction" crates/
crates/cyrup-session-svc/src/command.rs:32:    AbortCompaction,
crates/cyrup-session-svc/src/command.rs:116:  C::AbortCompaction => { self.abort_compaction(); }
# still exactly two lines. Zero production dispatch sites.
```

**Correction applied** — Impact rewritten. "cyrup shows a cancel key that does nothing. The user
presses Escape, the indicator keeps spinning" becomes: *cyrup shows **nothing at all** — for the full
10–18 s the status band is empty, so there is no `(esc to cancel)` suffix either*. The in-tree
citations at `app.rs:4615-4639` and `app.rs:6044` are accurate **as source** but describe a band that
never reaches the screen. That non-render is a **separate defect** (`TUI-055`) and is cross-referenced
on the item, because `SESS-040`'s Fix landing alone still leaves the user with an unlabelled blank
screen. Verify gains an assertion that the band actually renders during compaction.

---

### `TUI-045` — CONFIRMED · live-terminal

**Predicted** — an escape sequence split at the ESC byte across `read(2)` boundaries is not
reassembled; a fragmented arrow key while a turn streams aborts the run and types `[A` into the
prompt.

**Happened** — deterministically, first attempt, in **both** states. Separate `tmux send-keys -H`
calls with a sleep force separate `write(2)`s on the pty, i.e. separate `read(2)`s in crossterm.

```
=== CONTROL: intact \x1b[A in ONE write, at idle ===
$ tmux send-keys -H 1b 5b 41
 Count from 1 to 60, one number per line, nothing else.     <-- history recall: Up WORKED

=== SPLIT at idle: 0x1b, 60 ms gap, 0x5b 0x41 ===
 [ACount from 1 to 60, one number per line, nothing else.   <-- literal "[A" typed in

=== SPLIT MID-STREAM (the row as written) ===
$ ... "Count from 1 to 300 …"; Enter; sleep 6      # confirmed streaming: " ⠸ Working..."
$ tmux send-keys -H 1b; sleep 0.06; tmux send-keys -H 5b 41
 265
 266
 267
 Operation aborted                       <-- the run was ABORTED at token 267 of 300
 ─────────────────────────────────────
 [A                                      <-- and "[A" typed in
```

The item's exact sentence is now a measurement rather than a code-path argument, and the control
isolates fragmentation as the sole cause.

**Correction applied** — Confidence raised to "reproduced in a live terminal, both idle and
mid-stream". More importantly, **the item's reachability hedge is too conservative**: it says "on a
local PTY a keypress is normally one write and one read, so the exposure is over SSH/mosh/tmux". No
SSH, mosh or throttled pipe was needed — **a 60 ms gap between two writes on a local pty is
sufficient**, which is well within what tmux, a busy multiplexer, or any remote transport produces
routinely. Any input source that does not deliver a sequence in a single write is exposed.

---

## 4. `newDefectsObserved`

Eleven candidate defects were observed that no item describes. **Nine were filed. Two were struck
after auditing them against pi** — recorded here in full, because a struck candidate is evidence too
and the next pass should not re-derive them.

### Filed

| new ID | area | severity | what |
|---|---|---|---|
| `PROV-052` **FIXED 2026-08-13** | 01 | **critical** (raised on the fix pass) | The shipped binary's **default model was the in-process faux TEST provider**. A bare `cyrup -p hi` with no credentials fails with the internal string `No more faux responses queued`, and the interactive footer advertises `faux/faux-1` as the live model — while `--help` documents `--provider <name> (default: google)`. **Fixed 2026-08-13** — the `faux` feature edge moved out of every `[dependencies]` section (`cargo tree -p cyrup -e features --edges normal` now reports no `faux`, guarded by `crates/cyrup-provider/tests/faux_not_in_normal_build.rs`), and the no-credential path now resolves to a zero-model provider ⇒ pi's `formatNoModelsAvailableMessage()` on stderr + exit 1 (`main.ts:852-855` @v0.83.0). NB the `(default: google)` help line is a **stale string in pi itself** — `args.ts:87-88` applies no default — so it was correctly left alone; see the item body. |
| `TUI-052` | 07 | **high** | A queued message dequeued by Escape **stays in the transcript forever** as a phantom user message that was never sent and is not in the session JSONL. Survives both `TUI-016`'s and `TUI-005`'s fixes. |
| `TUI-053` | 07 | **high** | `Ctrl+-` (`editor.undo`) is **unreachable from any terminal without the kitty keyboard protocol**. pi maps the legacy byte explicitly (`keys.ts:1277`); cyrup relies on crossterm, which decodes `0x1F` as `Ctrl+'7'`. |
| `TUI-054` | 07 | **high** | A **failed or aborted** compaction is announced as `compaction complete`. `CompactionEnd` is destructured `{ .. }`, discarding `aborted` and `error_message`. |
| `TUI-055` | 07 | **high** | **No status indicator renders for the entire duration of a compaction** — the screen is blank for 10–20 s. |
| `TUI-056` | 07 | low | The context-usage meter **resets to `0.0%` after an aborted turn** while the conversation is still in the transcript. |
| `TUI-057` | 07 | low | Slash-command palette submission is **inconsistent** — sometimes one Enter, sometimes two, sometimes a trailing space suppresses it. Filed with its instrument caveat intact. |
| `EXT-058` | 06 | medium | Guest WASM `http-client` is **not gated by `--offline`**. |
| `PERM-032` | 10 | low | A permission-**denied** tool result breaks the next provider request on `together/openai/gpt-oss-20b` (3/3), while the same denial is handled fine by two other models. Filed at **low confidence** as a lead. |
| `ICOM-052` | 11 | low → **medium** | The intercom broker socket path has **no `SUN_LEN` guard**; on a long agent-dir path the broker dies and the only trace is a WARN that names neither the cause nor the path. **INDEPENDENTLY REPRODUCED 2026-08-13** (cross-group verification pass), as a by-product of the `SEAM-071` live check below — and the reproduction is a clean A/B on path length alone, so it is no longer a lead. Same binary, same model, same flags, only `CYRUP_AGENT_DIR` differs. **107-char agent dir** (`/private/tmp/claude-501/-Users-davidmaple-cyrup-ai/<uuid>/scratchpad/live4/agent`): the turn still succeeds (`Ok.`, exit 0) but stderr carries exactly the WARN the item describes — `intercom: startup connect failed; scheduling reconnect error=intercom broker error: intercom broker exited before startup with code 1` — naming neither `SUN_LEN`, nor the path, nor the length, and **0 brokers** are left running, so intercom is silently dead for the whole session. **10-char agent dir** (`/tmp/cy1/a`): no WARN at all, and the broker comes up and stays up (`38135 …/cyrup __intercom-broker`). Raised low → medium: the failure is total (no intercom for the session), silent (one WARN that misdirects toward a connect/retry problem), and triggered by an ordinary long path — every run under a temp dir with a UUID in it hits it. |

### Struck after audit — do not re-file

**"Raw provider HTTP error JSON is dumped verbatim into the TUI transcript."** Observed and real:
full pretty-printed `http 503: { "id": "…", "error": { … } }` bodies appear inline in the transcript
and as the entire output of `--print`. **But it is a faithful port.** pi's `formatProviderError`
composes exactly `"<status>: <body>"` from the raw body — cyrup's own
`crates/cyrup-provider/src/utils/error_body.rs:1-15` documents this and ports pi's 4000-char cap, and
`stream/sse.rs:347` is `format!("http {code}: {body}")` implementing it. pi shows the same wire JSON.
There is no parity gap. If the *product* wants a one-line human message, that is an upstream-first
change, not a cyrup defect.

**"Context-usage meter shows `?` after a compaction."** Observed, and **also a faithful port**:
`crates/cyrup-tui/src/status.rs:377` implements pi's `contextPercent === "?"` branch
(`footer.ts:151-152`) verbatim, and `:364-365` documents `?/200k (auto)` as the intended rendering
"when a compaction has left it unknown". Struck. The *other* half of that observation — the meter
reading `0.0%` after an **aborted** turn — is a different state (window known, percent computed as
zero) and is **not** covered by pi's `?` branch, so it was filed alone as `TUI-056`.

Both strikes came from reading the Rust and the TypeScript after the observation. **Two of eleven
observed "defects" were the code working correctly** — which is the same error rate, in the opposite
direction, as the backlog's own.

---

## 5. What this exercise proved about the method

The plan asserted that **an item is not ranked until someone has watched it happen**. Seventeen rows
is a small sample, but it is the only sample that exists, and it is decisive on three points.

### The rates, stated plainly

| | count | rate |
|---|---:|---:|
| rows driven | 17 | — |
| **CONFIRMED** | 16 | **94%** |
| **REFUTED** | 1 | **6%** |
| **BLOCKED** | 0 | **0%** |
| rows needing a **substantive** mechanism or severity correction | 10 | **59%** |
| rows needing a smaller annotation | 4 | 24% |
| rows that survived **completely unchanged** | 3 | **18%** |

### Is the backlog's confidence justified?

**On existence: yes, emphatically.** Sixteen of seventeen items describe something the binary really
does. The two-sided-evidence method — read the Rust at HEAD, read the TypeScript at a named tag,
default to rejection — does not invent defects. A 94% confirmation rate on a corpus assembled
entirely by reading is a strong result and should be said so.

**On mechanism and severity: no.** Only **3 of 17 rows survived untouched.** 59% needed a
substantive correction, and the corrections were not pedantry — three of them changed what the fix
has to do:

- `TUI-016` was filed as *absence* ("entirely invisible"). It is *affirmative misinformation*: the
  queued message is echoed into the transcript as though delivered. A fix written to the item as
  filed would have **added** the pending-messages rows and left the phantom bubble, rendering the
  message twice.
- `SESS-040` was filed as "a cancel key that does nothing… the indicator keeps spinning". There is no
  indicator. A fix written to the item would have wired Escape correctly and left the user staring at
  a blank screen for 18 seconds, still with no way to know a compaction was running.
- `SEAM-063` was filed as "reports success unconditionally". On the pre-launch surface it reports
  *nothing*. The fix needs a status channel that the item did not know was missing.

In each case the item's *verdict* was right and its *picture of the screen* was wrong — and the
picture is what a fix is written against. **This is the specific failure mode of reading-only
analysis: it recovers what the code does and not what the user sees.** It is worst exactly where the
consequence is a rendering, which is to say across the whole of area 07 and every pre-launch surface
in area 08.

### The severity scale did not survive contact

Two of the four `high → critical` raises made in the 2026-08-12 repair pass are now known to have
been made on **predicted** consequences:

- `AGENT-020` was raised to `critical` on "data loss, on the normal path of typing while a turn is in
  flight". **Five for five, including four attempts deliberately aimed at the settle boundary, no
  message was lost.** The path the raise named does not reach the window the code contains. Now
  `low`.
- `EXT-054` was raised correctly, but its *blast radius* note ("zero WASM guests ship, so nothing is
  currently mis-granted") was wrong in the reassuring direction — the in-tree SDK guest reproduces
  the mis-grant in under a second.

The raise procedure applied README §severity to an item's **own Impact prose**. When that prose was a
prediction, the procedure faithfully propagated the prediction into a rating. **Severity raises must
cite an observation or say that they do not.** That is now recorded on both items.

### One thing the exercise proved that nobody planned

**The suite that was supposed to be the safety net was itself unmeasured**, and wrong by 2455 tests.
The same citation-without-execution habit that produced the `3932` figure is what produced the
mechanism errors above. They are not two problems.

The `TUI-N13` case is the sharpest instance: a **deterministic, 5-out-of-5 macOS-only failure** that
every prior pass missed because the first measurement was piped through `tail`. The instrument hid
it. Two other measurements in this very log had the same disease — `pgrep -f` inventing 22 orphaned
brokers, and `tmux display-message '#{cursor_x}'` reporting a stale hardware cursor while the real
caret is an SGR-7 cell. **Three instrument errors in one exercise.** Any future repro pass should
budget for validating its instruments as a first-class step, and record them, as this log does.

### Batch 1 stop condition

The plan's rule: **≥2 rows REFUTED or BLOCKED ⇒ re-plan.**

- **Batch 1 (the 7 `SEAM-*` / `UW-2` rows): 7 CONFIRMED, 0 REFUTED, 0 BLOCKED.** Not met.
- **All 17 rows: 1 REFUTED, 0 BLOCKED = 1.** Not met, on either reading.

**The stop condition is NOT met, and it should not be treated as the all-clear it looks like.** It
counts the wrong thing. It is tuned to catch a backlog that is *hallucinating defects*, and this
backlog is not doing that — it is describing real defects *inaccurately*. A 59% correction rate
passed a gate that a 12% refutation rate would have failed. See the return note for the specific
re-plan recommendation.

---

## Appendix — residual risks in the green suite

Three qualifications the plan should carry, so "cargo test --workspace is green" is not
over-trusted as a per-batch gate:

1. **`cargo test` still has no per-test timeout.** `SEAM-072` removed the one known indefinite
   blocker, but the gate's failure mode for any future one is a silent stall that names nothing.
   `.config/nextest.toml` exists (untracked; `slow-timeout 60s, terminate-after 3, retries = 0`) and
   was validated, but it applies only under `cargo nextest run`, which is **not** the gate. Moving
   the gate would need **both** `cargo nextest run --workspace` **and** `cargo test --workspace
   --doc`, because nextest never runs doctests.
2. **`SUBA-069` remains open and is the one credible residual flake source.** Three worktree
   setup-hook tests are wall-clock-budgeted on pi's own 5000 ms production default and go red under a
   *competing cargo build*. None of the five runs reproduced it — the tree was quiet — so it is not
   in the measured flaky set, but a batch script that builds and tests concurrently can trip it.
3. **Run the gate with `< /dev/null`.** The `SEAM-072` fix makes this unnecessary for the known case;
   it costs nothing and removes a whole class.

`cargo clippy --workspace --all-targets` is exit 0 with 11 unique warnings, all `nonminimal_bool` in
pre-existing production code, none in any test touched this round, and the count is unchanged by the
production edits above.
