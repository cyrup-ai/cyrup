# Test architecture

Status: DESIGN. Nothing in this document has been built. It describes the target state, the
migration that reaches it, and the guardrails that hold it.

Written against branch `david/cyrup` at `c8c86bc`. All repo claims below are `file:line` at that
commit; all external claims carry a URL.

---

## 0. The decision this serves, and the one thing it does not claim

The maintainer has decided: **every crate keeps unit tests only (`#[cfg(test)]` inside `src/`);
integration tests move to a single separate crate, excluded from the default `cargo test` path and
run deliberately.** That decision is not re-litigated here. It is also the documented upstream
recommendation — Cargo's own reference says *"Each integration test results in a separate executable
binary, and cargo test will run them serially. In some cases this can be inefficient… If you have a
lot of integration tests, you may want to consider creating a single integration test, and split the
tests into multiple modules"*
([cargo-targets](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#integration-tests)) —
and it is what cargo, deno, ripgrep, rust-analyzer, uv, ruff, diesel and tokio all do in some form.

**What this design does NOT claim: that it fixes the 4h39m.** The cause of that number has not been
isolated. A previous confident attribution ("99% linking") was measured to be false —
`cargo test --workspace --no-run` completes in 0.98s when built. External evidence rules out binary
count as a sufficient explanation on its own: nextest's published benchmarks measure tokio, a suite
of ~170 process-spawning integration binaries, at **1138 tests in 24.27s** end to end
([nexte.st/docs/benchmarks](https://nexte.st/docs/benchmarks/)). 310 binaries do not produce four
and a half hours by existing. Something in this repo stalls. §8 lists the three live hypotheses and
the cheap probes that would settle them.

The restructuring is justified on maintenance, isolation and documented-best-practice grounds. If it
also removes hours, that will be because it cut binary count and eliminated 24 nested `cargo build`
invocations — both directly measurable claims, and both stated as predictions to verify rather than
as results.

---

## 1. Recommendation, with numbers

The maintainer put deletion of the whole set on the table. **Do not take it.** The triage read all
310 files and the honest answer is that almost none of them are waste — they are mostly *misfiled*.

| Verdict | Files | What happens |
|---|---:|---|
| `unit-able` — reaches no seam a unit test cannot reach | **224** | Move into the owning crate's `src/` as `#[cfg(test)]` modules. Zero new binaries. |
| `truly-integration` — needs a spawned binary, a broker socket, a WASM guest, or a cross-crate assembly | **84** | Move into `crates/cyrup-it/`, grouped into **7** `[[test]]` targets. |
| `delete-redundant` — every assertion is strictly covered elsewhere | **2** | Delete. |
| **Total** | **310** | **310 test binaries → 7.** |

Plus **six individual tests** to delete at sub-file granularity (§6.4). Everything else is carried.

**So: carry 308 of 310 files' assertions; delete 2 files and 6 tests; go from 310 integration
binaries to 7.**

### Why deletion is the wrong instinct here, stated plainly

matklad's rule — *"if the project is a library, and it is not published, you don't need integration
tests"* ([Delete Cargo Integration
Tests](https://matklad.github.io/2021/02/27/delete-cargo-integration-tests.html)) — is correct, and
16 of these 18 crates are internal. It is an argument for **relocation, not deletion**, and the
triage applies it precisely: 224 files become unit tests because that is what they always were.

The 84 that survive as integration tests are the parity proofs. They are not incidental:

- **31 of the 310 files are the named `Verify` step of an item in `docs/gap-analysis/`.** Deleting
  those removes the only evidence a parity claim was ever true. Examples the triage surfaced:
  `crates/cyrup-session-svc/tests/settings_resolve.rs` is named twice in
  `docs/gap-analysis/05-cyrup-config-and-resources.md:194` and `:208` as the site of the fix for two
  OPEN items; `crates/cyrup-modes/tests/modes.rs` is the named Verify step for three open items
  (SEAM-028 at `08-cyrup-session-svc-and-modes.md:471`, the `GetAvailableThinkingLevels` verb at
  `:499`, SEAM-069 at `:789`); `crates/cyrup-intercom/tests/presence_context_usage.rs` and
  `session_info_context_fields.rs` are both named in ICOM-041's Verify at `11-cyrup-intercom.md:649`;
  `crates/cyrup-ext/tests/wit_world_sync.rs` closed DRIFT-034 and is prescribed as the *extension
  site* for the open EXT-028 residual at `06-cyrup-ext.md:775` and `:803`.
- **Several are the RED→GREEN test for a shipped security defect.**
  `crates/cyrup-permission-system/tests/gate_integration.rs` is named at
  `10-cyrup-permission-system.md:51` and `:139` as the test for PERM-009 — a configured `tools.bash`
  deny that was being defeated. `crates/cyrup-ext/tests/ext_fail_closed.rs` proves extension budget
  exhaustion fails **closed** by flipping an `AtomicBool` as the first statement of every tool's
  `execute` and asserting it is still false.
  `crates/cyrup-provider/tests/faux_not_in_normal_build.rs` was RED before its fix, when the shipped
  binary answered a real prompt with *"No more faux responses queued"*.
- **Some assert things that are physically unobservable in-process.**
  `crates/cyrup/tests/signal_shutdown.rs` sends a real SIGTERM and asserts exit 143.
  `crates/cyrup-ext-subagents/tests/child_stderr_drain_integration.rs` proves the parent does not
  deadlock when a real child overfills an OS pipe buffer.
  `crates/cyrup-ext-subagents/tests/run_state_signal_and_stop_parity.rs` reports the signal that
  killed a real child, and its own module doc records that replacing that field with `None` left the
  entire suite green — it is the only guard.

The set is expensive because of **how it is packaged**, not because of what it asserts. Repackaging
is the fix.

### Where the 224 and the 84 sit

| Crate | files | unit-able | truly-integration | delete |
|---|---:|---:|---:|---:|
| cyrup-tui | 80 | 77 | 1 | 2 |
| cyrup-session-svc | 50 | 40 | 10 | 0 |
| cyrup-ext-subagents | 47 | 20 | 27 | 0 |
| cyrup-intercom | 20 | 2 | 18 | 0 |
| cyrup-ext | 21 | 8 | 13 | 0 |
| cyrup-tools | 12 | 12 | 0 | 0 |
| cyrup-permission-system | 13 | 8 | 5 | 0 |
| cyrup-agent | 11 | 11 | 0 | 0 |
| cyrup | 16 | 7 | 9 | 0 |
| cyrup-provider | 8 | 7 | 1 | 0 |
| cyrup-session | 7 | 7 | 0 | 0 |
| cyrup-modes | 5 | 5 | 0 | 0 |
| cyrup-sdk | 4 | 4 | 0 | 0 |
| cyrup-ext-sdk | 2 | 2 | 0 | 0 |
| cyrup-config | 1 | 1 | 0 | 0 |
| cyrup-resources | 1 | 1 | 0 | 0 |
| cyrup-test-support | 1 | 1 | 0 | 0 |
| **Total** | **310** | **224** | **84** | **2** |

Two arithmetic notes, so nobody re-derives them: the five triagers reported 311 `filesExamined`
against 310 files on disk (one slice double-counted its own header), but the **verdict** counts sum
to exactly 310, so the verdicts are the authoritative column. Per-crate splits above are
reconstructed from the per-file classifications.

**`cyrup-tui` is the headline.** 80 binaries — 26% of the corpus — of which **77 are unit-able, 1
needs the WASM guest, and 2 are redundant.** Folding it into `src/` is the cheapest, safest, highest
-yield single action available: no new crate, no manifest gate, no runner change, and no parity
seam touched. Do it first after the harness work (§6.2), and it also removes the largest confound
from any subsequent measurement of the 4h39m.

---

## 2. The harness crate

### 2.1 Identity and location

```
crates/cyrup-it/
├── Cargo.toml
├── build.rs                    # builds every binary + the WASM guest ONCE for the whole suite
└── tests/
    ├── cli/main.rs             # 8  — the cyrup binary's argv/exit-code/stdio seam
    ├── broker/main.rs          # 18 — cyrup-intercom over a real Unix socket
    ├── subagents/main.rs       # 27 — spawned subagent children, detachment, signals
    ├── wasm/main.rs            # 23 — live wasm32-wasip2 guests
    ├── harness/main.rs         #  5 — cyrup-test-support-assembled AgentSessions
    ├── toolchain/main.rs       #  3 — tests that drive real `cargo`/`git`
    └── api/main.rs             #  N — public-API-surface tests (see §7)
```

Name `cyrup-it` follows matklad's convention (`it` = integration test) and the workspace's
`cyrup-*` prefix. It is a workspace **member** (`crates/cyrup-it` added to `[workspace] members`)
and is **not** in `default-members`.

### 2.2 `Cargo.toml`

```toml
# crates/cyrup-it/Cargo.toml
#
# The workspace's ONE integration-test crate. It has no `[lib]` and no `[dependencies]` —
# see the note below, this is load-bearing, not an oversight.
[package]
name          = "cyrup-it"
version.workspace    = true
edition.workspace    = true
license.workspace    = true
repository.workspace = true
publish       = false     # hygiene only. `publish` is NOT a gate — see §2.4.
autotests     = false     # no auto-discovery; every target is declared explicitly below.

[lints]
workspace = true

[features]
default   = []
it        = []            # arms every [[test]] below. OFF by default. THIS is the gate.
wasm-host = ["cyrup-ext/wasm-host", "cyrup-session-svc/wasm-host"]

# NO [dependencies] SECTION. Deliberate.
#
# A `[lib]` here would need real `[dependencies]` (dev-deps are invisible to a lib), and
# `cargo build --workspace` unifies features per package across everything it builds. That is
# exactly the PROV-052 trap the root Cargo.toml documents at lines 25-48: cyrup-test-support is
# the only crate whose `[dependencies]` enables `cyrup-provider/faux`, and leaving it in
# default-members compiled the test double into the shipping binary. cyrup-it must not reopen
# that hole. All shared harness code therefore lives in `cyrup-test-support`, which already owns
# that edge and already carries the exclusion rationale.

[dev-dependencies]
cyrup-test-support      = { workspace = true }
cyrup-core              = { workspace = true }
cyrup-provider          = { workspace = true }
cyrup-agent             = { workspace = true }
cyrup-tools             = { workspace = true }
cyrup-session           = { workspace = true }
cyrup-config            = { workspace = true }
cyrup-resources         = { workspace = true }
cyrup-ext               = { workspace = true }
cyrup-session-svc       = { workspace = true }
cyrup-modes             = { workspace = true }
cyrup-sdk               = { workspace = true }
cyrup-tui               = { workspace = true }
cyrup-intercom          = { workspace = true }
cyrup-permission-system = { workspace = true }
cyrup-ext-subagents     = { workspace = true }
tokio       = { workspace = true, features = ["rt-multi-thread", "macros", "process", "net", "time", "io-util", "fs", "sync"] }
serde_json  = { workspace = true }
tempfile    = { workspace = true }
anyhow      = { workspace = true }

[build-dependencies]
serde_json = { workspace = true }

[[test]]
name              = "cli"
path              = "tests/cli/main.rs"
required-features = ["it"]

[[test]]
name              = "broker"
path              = "tests/broker/main.rs"
required-features = ["it"]

[[test]]
name              = "subagents"
path              = "tests/subagents/main.rs"
required-features = ["it"]

[[test]]
name              = "wasm"
path              = "tests/wasm/main.rs"
required-features = ["it", "wasm-host"]

[[test]]
name              = "harness"
path              = "tests/harness/main.rs"
required-features = ["it"]

[[test]]
name              = "toolchain"
path              = "tests/toolchain/main.rs"
required-features = ["it"]

[[test]]
name              = "api"
path              = "tests/api/main.rs"
required-features = ["it"]
```

### 2.3 The gate: `required-features`, and why it beats the alternatives

**`required-features = ["it"]` on every `[[test]]` target.** Cargo: *"The `required-features` field
specifies which features the target needs in order to be built. If any of the required features are
not enabled, the target will be skipped."*
([cargo-targets](https://doc.rust-lang.org/cargo/reference/cargo-targets.html#the-required-features-field))
It is the **only** manifest lever that skips a target under *any* package-selection flag.

Rejected alternatives, each for a specific reason:

| Mechanism | Why not |
|---|---|
| `default-members` exclusion | **Provably inert against this repo's gate.** `--workspace` overrides `default-members` entirely: *"the default-members field specifies paths of members to operate on when in the workspace root and the package selection flags are not used"* ([workspaces](https://doc.rust-lang.org/cargo/reference/workspaces.html)). The merge gate here is `cargo test --workspace`. The root `Cargo.toml:41-48` already concedes exactly this shape of subtlety for `cyrup-test-support`. Set it anyway as a convenience for bare local `cargo test` — never as the mechanism. |
| `publish = false` | A registry flag. It has no effect on package selection or test execution. Set it for hygiene; it must not appear in the gating story. |
| `workspace.exclude` (crate outside the workspace) | Buys a second `Cargo.lock`, breaking the lockstep 0.0.0 discipline at `Cargo.toml:84-99`; needs a rust-analyzer `linkedProjects` entry or the whole suite goes dark in the editor; loses `workspace.lints` and `workspace.dependencies` inheritance. `required-features` gets the same isolation for none of it. |
| `test = false` per target | Works, but it is a per-target flag repeated 7×, and it makes "how do I run these?" unanswerable to a contributor. A named feature answers it. Acceptable as belt-and-braces, not as the mechanism. |
| `#[ignore]` | Still compiles, still links, still **starts every binary** — which is precisely the cost being paid, since build/link is already measured at 0.98s. It also scatters the decision across 5,145 call sites instead of one manifest field. |

**The one hazard, written down because it is realistic here:** `--all-features` silently re-arms the
entire suite. Given this workspace's feature discipline around `cyrup-provider/faux`
(`Cargo.toml:27-38`), a stray `--all-features` in CI or a contributor's muscle memory is a live risk.
§9 makes it a CI check.

### 2.4 The literal commands

**Everyday (the merge gate — must stay fast). Unchanged from today:**

```bash
cargo test --workspace
```

`cyrup-it`'s seven targets are skipped: feature `it` is off, so Cargo does not build them. The
build script no-ops too (§3.3), so no nested `cargo build` is paid either.

Once a full-workspace nextest run has been validated, the gate becomes **two commands, not one** —
nextest does not run doctests, and this is a stable-Rust limitation, not an oversight
([nextest-rs/nextest#16](https://github.com/nextest-rs/nextest/issues/16)). `.config/nextest.toml:19-22`
already states this; carry it forward verbatim:

```bash
cargo nextest run --workspace && cargo test --workspace --doc
```

**Deliberate (the integration suite):**

```bash
# everything
cargo nextest run -p cyrup-it --features it,wasm-host

# one seam
cargo nextest run -p cyrup-it --features it -E 'binary(broker)'

# skip the second link of the workspace binaries by pointing at an existing build
cargo build --workspace --bins
CYRUP_IT_BIN_DIR="$PWD/target/debug" cargo nextest run -p cyrup-it --features it

# plain cargo still works — nothing about correctness depends on nextest
cargo test -p cyrup-it --features it
```

That last line matters. The design deliberately **does not** use nextest setup scripts for the
binary-path problem. Setup scripts would make `$NEXTEST_ENV` injection load-bearing for
*correctness*, after which `cargo test -p cyrup-it` stops working entirely. A `build.rs` keeps the
suite runner-agnostic; nextest is then a pure performance and safety upgrade, adopted for
process-per-test isolation, `leak-timeout` and test groups — not depended on to make tests pass.

---

## 3. Internal structure

### 3.1 Seven targets, not one, and not 310

Consolidate — but not to a monolith. **uv is the cautionary trajectory: 91 → 1 → 12.** PR
[astral-sh/uv#8093](https://github.com/astral-sh/uv/pull/8093) collapsed 91 integration binaries into
one, citing matklad; `crates/uv/tests/` today holds **twelve** directory targets (`it/`,
`pip_install/`, `lock/`, `sync/`, …) because a single target's source grew past 500 KB and became one
serial codegen unit plus one link on the critical path of every edit.

Wasmtime encodes the other half of the rule: consolidate by default
([`tests/all/main.rs`](https://raw.githubusercontent.com/bytecodealliance/wasmtime/main/tests/all/main.rs),
~58 modules, one binary) but keep standalone binaries for tests that own process-global state
(`rlimited-memory.rs`, `disable_host_trap_handlers.rs`).

Applied here, the grouping is **by seam**, so that a `process::exit`, an abort, or a segfault in one
seam cannot take the others down with no report — a real risk in a suite that spawns detached
subagents, sends SIGKILL, and instantiates WASM guests:

| Target | Files | Seam it owns | Notes |
|---|---:|---|---|
| `cli` | 8 | the `cyrup` binary's argv, exit codes, stdio | `one_shot_parity`, `signal_shutdown`, `tui_mode_flag`, `unknown_flag_exit`, `extension_load_failure_exit`, `piped_stdin_trim`, `auth_credential_print`, `list_models_overlay` |
| `broker` | 18 | `cyrup-intercom-broker` over a real Unix socket | includes the two hostile-`UnixListener` protocol files and the two kill-the-broker files |
| `subagents` | 27 | spawned children, detachment, pgids, signals | the 25 fixture-bin files plus `forwarding_subprocess` / `forwarding_spawn_env`, which re-exec the test binary as a child |
| `wasm` | 23 | live wasm32-wasip2 guests | 12 in cyrup-ext + 10 in session-svc + `cyrup-tui/wasm_renderer_screen`. Gated `required-features = ["it", "wasm-host"]` |
| `harness` | 5 | multi-crate `AgentSession` assembly via `cyrup-test-support` | `gate_integration`, `human_dialog`, `empty_command_truthiness`, `management_actions_tool_dispatch`, `wait_tool_registration` |
| `toolchain` | 3 | real `cargo` / `git` subprocesses | `build_tier1`, `package_update_check`, `faux_not_in_normal_build` |
| `api` | new | public-API surface as an external consumer | see §7 |

Two files split at the module boundary rather than moving whole:
`crates/cyrup-session-svc/tests/install_noop.rs` (test 1 is pure, `mod wasm_ext` is not) and
`crates/cyrup-ext-subagents/tests/prompt_workflow_commands_integration.rs` (4 of 7 tests are pure).
`crates/cyrup-ext-subagents/tests/subagent_tool_renderer_integration.rs` splits 6/1 the same way.

### 3.2 Module layout inside a target

Directory targets with `main.rs` are auto-discovered by Cargo
([project-layout](https://doc.rust-lang.org/cargo/guide/project-layout.html)), which is how
rust-lang/cargo's own `tests/testsuite/` gets 154 files into **one** target. We declare them
explicitly anyway (`autotests = false` + `[[test]] path`) — ripgrep's form
([Cargo.toml](https://raw.githubusercontent.com/BurntSushi/ripgrep/master/Cargo.toml)) — because
explicit `required-features` per target is the gate, and a manifest that lists its targets is
greppable.

```
tests/broker/
├── main.rs              # module list + the target's own `mod support;`
├── support/mod.rs       # target-local: spawn_broker, RawClient, wait_until wrappers
├── protocol/
│   ├── mod.rs
│   ├── number_domain.rs
│   ├── array_payload_rejection.rs
│   ├── explicit_null_rejection.rs
│   └── forward_compat.rs
├── lifecycle/           # runtime_claim, startup_fail_fast, reconnect, registers_under_session_id
├── surface/             # human_surface, dismiss_incoming_ask, shared_human_lock
└── commands/            # intercom_command_transcript, intercom_id_command, presence_context_usage
```

`main.rs` is nothing but `mod` declarations, ripgrep-style. Group by **kind of behaviour**, not by
which source file the test happens to touch — grouping by source file is how 310 files accumulated.
ripgrep even carries curation policy in module doc comments (*"misc"* is annotated with a note to
stop adding to it); copy that habit.

### 3.3 Fixtures, builders and helpers

**Shared helpers live in `crates/cyrup-test-support`, not in `cyrup-it`.** That crate already
contains `harness.rs`, `golden.rs`, `tui.rs`, `scripted.rs`, `differential.rs`, `tempdir.rs`,
`interop.rs`, `auth.rs`, `messages.rs`, `response.rs`, `tool_ext.rs`, `tree.rs` — and only four
crates dev-depend on it. Meanwhile the corpus redefines the same helpers by hand:

| Helper | Duplicate definitions | Where |
|---|---:|---|
| `fn fixture()` | 66 | 43 in cyrup-session-svc alone |
| `fn base_config()` | 41 | cyrup-session-svc |
| `fn buf_text(app)` | 22 | cyrup-tui, verbatim x/y loop over the `TestBackend` buffer |
| `fn fixture_binary_path()` | 21 | cyrup-ext-subagents |
| `static ENV_MUTATION_LOCK` | 33 | cyrup-ext-subagents (29 + 4 named `ENV_LOCK`) |
| `fn fixture_component()` | 22 | 12 in cyrup-ext, 10 in cyrup-session-svc — byte-identical, 8 of them literally commented *"mirrors wasm_slash_command.rs"* |
| `struct RawClient` | 6 | cyrup-intercom |
| `fn spawn_broker` / `fn broker_bin` | 6 each | cyrup-intercom |
| `fn wait_until` / `fn within` | 6 | cyrup-intercom |
| hermetic child `Command` builder | 8 | crates/cyrup/tests — and `11-cyrup-intercom.md:791` records one env-scrub bug that had to be fixed in **four** copies separately |
| `fn spawn_child` / `fn wait_child` | 2 | cyrup-permission-system, with **divergent timeout constants** — which is exactly how open item PERM-022 (8_000 vs 20_000) came to exist |

That last row is the argument in one line: duplicated helpers do not merely cost lines, they
manufacture defects. **De-duplicate into `cyrup-test-support` BEFORE moving anything** (§6.1). It
shrinks the thing being moved and makes the move mechanical.

Target-local helpers that genuinely belong to one seam (e.g. `broker/support/mod.rs`) stay inside
that target's directory.

### 3.4 Finding the built binary and the WASM component

This is the hardest constraint on the whole design, and it must be answered up front rather than
discovered mid-migration.

**`CARGO_BIN_EXE_<name>` does not cross packages.** Cargo: *"The absolute path to a binary target's
executable. This is only set when running an integration test or benchmark"*
([environment-variables](https://doc.rust-lang.org/cargo/reference/environment-variables.html)) —
and only for binaries in the *same* package. **51 of the 310 files use it** (8 in `cyrup`, 25 in
`cyrup-ext-subagents`, 18 in `cyrup-intercom`); the repo documents the rule in its own source at
`crates/cyrup-ext-subagents/tests/background_runner_main_integration.rs:103`. Every one of those 51
files fails to compile the day it lands in `cyrup-it`. Cargo's own fix — artifact dependencies,
which set `CARGO_BIN_FILE_<DEP>_<NAME>` and explicitly work for dev-dependencies — is nightly-only
behind `-Z bindeps` ([unstable](https://doc.rust-lang.org/cargo/reference/unstable.html#artifact-dependencies)),
and `rust-toolchain.toml` pins `channel = "stable"`.

**The answer: one `build.rs` in `cyrup-it`.** It resolves every binary path and the WASM component
**once for the whole suite**, and re-exports them as compile-time env for every target in the
package.

```rust
// crates/cyrup-it/build.rs
//
// Resolves, ONCE for the whole suite:
//   - every workspace binary an integration test needs to spawn
//   - the wasm32-wasip2 guest component
// and re-exports them via `cargo::rustc-env=` so `env!()` works in every [[test]] target of this
// package. This replaces 51 `env!("CARGO_BIN_EXE_…")` sites (which cannot cross a package
// boundary) and 24 per-test-binary nested `cargo build` invocations.

const BINS: &[(&str, &str)] = &[
    ("cyrup",                          "cyrup"),
    ("cyrup-intercom-broker",          "cyrup-intercom"),
    ("cyrup-intercom-child-fixture",   "cyrup-intercom"),
    ("cyrup-subagent-fixture",         "cyrup-ext-subagents"),
    ("cyrup-subagent-orchestrator-sim","cyrup-ext-subagents"),
];

fn main() {
    println!("cargo::rerun-if-env-changed=CYRUP_IT_BIN_DIR");
    println!("cargo::rerun-if-env-changed=CYRUP_EXT_FIXTURE_COMPONENT");

    // No-op unless the suite is armed. Cargo sets CARGO_FEATURE_<NAME> for build scripts, so a
    // plain `cargo test --workspace` pays NOTHING here.
    if std::env::var_os("CARGO_FEATURE_IT").is_none() {
        return;
    }

    // 1. Binaries. Honour a pre-built directory first so CI can point at target/debug and skip
    //    the second link entirely.
    if let Some(dir) = std::env::var_os("CYRUP_IT_BIN_DIR") {
        for (bin, _) in BINS { emit_bin(bin, Path::new(&dir).join(bin)); }
    } else {
        // MUST use a target dir distinct from the outer build's. A nested cargo sharing the
        // workspace target dir contends for its build lock; crates/cyrup-ext/Cargo.toml:47-52
        // records what that costs here (a leaked ~213MB artifact cache filled a 16GB /tmp tmpfs
        // and made `ld` die with SIGBUS while linking unrelated doctests).
        let td = out_dir().join("it-bins");
        for (bin, pkg) in BINS {
            emit_bin(bin, cargo_build_bin(pkg, bin, &td));  // --message-format=json-render-diagnostics
        }                                                    // read `"reason":"compiler-artifact"` → `executable`
    }

    // 2. WASM guest. ONE build for the whole suite.
    let component = match std::env::var_os("CYRUP_EXT_FIXTURE_COMPONENT") {
        Some(p) => PathBuf::from(p),
        None => cargo_build_component("cyrup-ext-sdk", "wasm32-wasip2", &out_dir().join("it-wasm")),
    };
    // Hard-fail with an actionable message. NEVER silently skip: build_tier1.rs:13-17 currently
    // returns green when the toolchain is absent, which is a pass that proves nothing.
    assert!(component.exists(), "wasm guest not built. `rustup target add wasm32-wasip2`, or set \
                                 CYRUP_EXT_FIXTURE_COMPONENT to a prebuilt component.");
    println!("cargo::rustc-env=CYRUP_IT_COMPONENT={}", component.display());
}
```

Tests then read:

```rust
const CYRUP_BIN:   &str = env!("CYRUP_IT_BIN_cyrup");
const BROKER_BIN:  &str = env!("CYRUP_IT_BIN_cyrup-intercom-broker");
const COMPONENT:   &str = env!("CYRUP_IT_COMPONENT");
```

**What this buys, concretely:**

- The 51 `CARGO_BIN_EXE` sites become 5 constants. The cross-package blocker is gone on stable.
- **24 nested `cargo build` invocations collapse to 1.** Today 13 files in cyrup-ext, 10 in
  cyrup-session-svc and 1 in cyrup-tui each unconditionally shell out to
  `cargo build -p cyrup-ext-sdk --target wasm32-wasip2`. Ten of them share one fixed path
  (`std::env::temp_dir()/cyrup-session-svc-fixture-target`) and five share another
  (`std::env::temp_dir()/cyrup-ext-fixture-target`), so those groups **serialize on each other's
  cargo build lock**. Four more —
  `crates/cyrup-ext/tests/discover_load.rs:25`, `guest_host_mode.rs:36`,
  `manifest_capabilities.rs:39`, `wasm_provider.rs:25` — pass no `--target-dir` at all and contend
  for the **workspace** target dir, which is the exact contention their eight siblings were written
  to avoid (`wasm_component.rs:31-32` says so verbatim). This is hypothesis (c) in §8.
- Neither fixed `/tmp` path is ever cleaned. `$OUT_DIR` is.
- `CYRUP_EXT_FIXTURE_COMPONENT` keeps working as the escape hatch, at one place instead of 22.

**The cost, stated honestly:** without `CYRUP_IT_BIN_DIR`, the build script relinks the `cyrup`
binary and four fixture binaries into a private target dir — a second compile of those graphs. It is
paid once per suite invocation, not per test, and the suite is deliberate rather than per-commit.
`CYRUP_IT_BIN_DIR` removes it in CI. If that trade turns out to be worse than expected, the fallback
is the wasmtime hybrid: leave the 51 binary-seam files in their owning packages, consolidated into
one `tests/it/main.rs` directory-target per crate (3 extra binaries: `cyrup`, `cyrup-intercom`,
`cyrup-ext-subagents`). That is a 3-binary deviation from "one crate", and it is the position both
external reviews independently recommended. **Build the `cli` target first as a pilot (§6.3) and let
the measurement decide.**

---

## 4. Isolation rules

Stated as rules a reviewer can enforce, with the mechanism that makes each one checkable. **310
processes were silently providing isolation of cwd, env, HOME, temp dirs, socket paths and ports. 7
binaries are not.** Cargo's own single-binary testsuite only works because `#[cargo_test]` gives
every test a filesystem sandbox (*"injects code which does some setup before starting the test,
creating a filesystem 'sandbox'… for each test"*,
[doc.crates.io/contrib](https://doc.crates.io/contrib/tests/writing.html)). Build the equivalent
before the first file moves, not after.

### R1 — Tempdir per test. No fixed paths.

Every test owns a `tempfile::TempDir`; nothing writes to a shared, statically-named path. 185 files
already use `tempfile`, so this is mostly already true. The violations to fix on the way through:

- `std::env::temp_dir()/cyrup-session-svc-fixture-target` (10 files) and
  `std::env::temp_dir()/cyrup-ext-fixture-target` (5 files) — eliminated by the build.rs (§3.4).
- `crates/cyrup-ext/tests/loader.rs:9-18` — `unique_dir()` names a directory
  `cyrup-ext-loader-{tag}-{nanos}` with **no pid suffix and no `TempDir` cleanup**. Three siblings
  (`loader_direct_file.rs:19-27`, `malformed_manifest.rs:31-38`, `manifest_cache.rs:161`) do add
  `std::process::id()` — which stops disambiguating anything the moment they share a binary. All
  four take a `TempDir`.

Reviewer check: `rg 'env::temp_dir\(\)' crates/` must return nothing outside `cyrup-test-support`.

`CARGO_TARGET_TMPDIR` is deliberately **not** used. It is *"only set when building integration test
or benchmark code"*, so it would break the 224 files moving into `src/`. Zero files use it today;
keep it that way.

### R2 — No `std::env::set_var` / `remove_var` in test code.

`set_var`/`remove_var` became `unsafe` in edition 2024 (Rust 1.85.0;
[edition-guide](https://doc.rust-lang.org/edition-guide/rust-2024/newly-unsafe-functions.html)) and
std's conclusion is unambiguous: *"the only sound option is to not use set_var or remove_var in
multi-threaded programs at all"* ([std::env::set_var](https://doc.rust-lang.org/std/env/fn.set_var.html)).
The workspace is edition 2024 (`Cargo.toml:70`). **45 files call it.**

Today each is its own process, so the practice is defensible, and several files say so in their own
doc comments (`crates/cyrup-ext-subagents/tests/cyrup_home_env_sandboxed_tests.rs:1-14`,
`subagents_optin_gate_integration.rs:1-8`, `verify_redaction_inherited_env.rs:11-15`,
`crates/cyrup-tools/tests/shell_interpreter.rs:20-21`,
`crates/cyrup-permission-system/tests/forwarding_persist.rs:92-93`). Every one of those
justifications dies on consolidation.

Worse: **the 33 per-file `static ENV_MUTATION_LOCK: Mutex<()>` declarations will silently stop
working.** They are distinct statics in distinct files today only because each file is a distinct
process. Merged into one binary they remain 33 different mutexes guarding one shared environment —
i.e. no mutual exclusion at all. Do not carry them across unchanged.

Three tiers of fix, in order of preference:

1. **Delete the env write.** Where the env var is merely how production *happens* to read config,
   make production read it through an injectable resolver and pass the value. This is the right
   answer for most of the 45, and it is already the in-repo idiom:
   `crates/cyrup-tui/tests/theme_fidelity.rs:19` injects env via a closure (`env_of`), and
   `image_capabilities.rs` does the same — neither touches the process.
2. **Where the env var IS the mechanism under test** (e.g. `CYRUP_SUBAGENT_CHILD` arming a spawned
   child; `crates/cyrup-tools/tests/bash_env_scrub.rs` asserting pi's unconditional `PI_*`/`CYRUP_*`
   delete), set it **on the child's `Command`**, not on the process.
   `crates/cyrup-tools/tests/bash_session_env.rs:204` already documents this exact distinction.
3. **Where neither is possible**, one shared guard in `cyrup-test-support`:

   ```rust
   // crates/cyrup-test-support/src/env.rs
   // ONE process-wide lock for the whole workspace. Replaces 33 per-file statics that become
   // no-ops the moment two files share a binary.
   static ENV_LOCK: Mutex<()> = Mutex::new(());
   #[allow(unsafe_code)]                 // the single audited unsafe in this crate
   pub fn scoped(pairs: &[(&str, Option<&str>)]) -> EnvGuard { /* set, restore on Drop */ }
   ```

   This requires relaxing `crates/cyrup-test-support/src/lib.rs:23` from `#![forbid(unsafe_code)]`
   to `#![deny(unsafe_code)]` — `forbid` cannot be locally overridden, `deny` can — with one
   documented `#[allow]` at that function and nowhere else. `cyrup-test-support` is `publish = false`
   and never reaches the shipping binary, so the blast radius is the test layer only.

   Tier 3 is also what **unblocks a large slice of the 224**. Several `unit-able` files exist
   outside `src/` for exactly one reason: `crates/cyrup-ext-subagents/src/lib.rs:25` is
   `#![forbid(unsafe_code)]`, so their `set_var` cannot live in a `#[cfg(test)]` module.
   `cyrup_home_env_sandboxed_tests.rs`, `subagents_optin_gate_integration.rs` and
   `verify_redaction_inherited_env.rs` say so verbatim. Give the workspace a safe env seam and those
   files go home. **Moving `crates/cyrup-permission-system/tests/prompt_dedup.rs` into `src/` is
   itself the prescribed fix for open item PERM-020** — `10-cyrup-permission-system.md:357` says the
   integration binary *"cannot reach the crate-private `ext_config::env_lock()`"*, and a `src/`
   module can.

   Pair tier 3 with a nextest test group (§5.3) so the lock is enforced across processes too.

Reviewer check: a clippy `disallowed-methods` entry, which fails the build rather than the review:

```toml
# clippy.toml
disallowed-methods = [
  { path = "std::env::set_var",    reason = "use cyrup_test_support::env::scoped, or Command::env" },
  { path = "std::env::remove_var", reason = "use cyrup_test_support::env::scoped, or Command::env" },
]
```

### R3 — No `std::env::set_current_dir`.

**Zero occurrences today** anywhere in `crates/*/tests` or `crates/*/src`. Keep it at zero. The one
near-miss is `crates/cyrup-ext-subagents/tests/discovery_project_root_wiring_integration.rs:78-93`,
an RAII enter/drop guard; convert it to passing an explicit root. Everything that needs a working
directory uses `Command::current_dir(...)`, which is per-child and already the pattern in
`crates/cyrup-resources/tests/resources.rs`.

Reviewer check: `rg 'set_current_dir' crates/` returns nothing.

### R4 — No fixed ports. Bind `:0`, read the assignment back.

**Already correct throughout.** `crates/cyrup-agent/tests/proxy_live_turn.rs:93`,
`crates/cyrup-sdk/tests/embedder_seams.rs:129`, `crates/cyrup-session-svc/tests/wasm_http.rs:67`,
`crates/cyrup-provider/tests/remote_catalog.rs:12` (which calls it *"the established technique in
this workspace"*). Unix sockets go under the test's own `TempDir`. `127.0.0.1:1` appears twice as a
deliberately-dead address, never as a listener. This means the usual reason a consolidated suite
explodes does not apply here.

Reviewer check: `rg '127\.0\.0\.1:[1-9]' crates/*/tests` must match only the two dead-address sites.

### R5 — Ambient credentials are scrubbed, and the scrub is asserted.

`TOGETHER_API_KEY` is exported on this machine and **has already caused a test to make a real
network call.** Three layers, because any one alone has a hole:

1. **Every spawned child gets `Command::env_clear()` plus an explicit allowlist.** The reference
   implementation already exists at `crates/cyrup/tests/auth_credential_print.rs:76`, and
   `11-cyrup-intercom.md:791` cites it as the pattern the other four hermetic-child builders should
   copy — which is precisely what §3.3's de-duplication does, once, in `cyrup-test-support`.
2. **In-process reads go through injected config, never ambient env.** Generalize
   `crates/cyrup-session-svc/tests/model_registry.rs:38-70`'s `ScrubbedProviderEnv` and its
   `SCRUBBED_PROVIDER_ENV_KEYS` list into `cyrup_test_support::env::PROVIDER_KEYS`, and make the
   harness's provider constructors take an explicit key rather than falling back to env.
3. **A guard test per target that fails loudly if the suite's own process has any of them set:**

   ```rust
   #[test]
   fn no_ambient_provider_credentials() {
       let leaked: Vec<_> = cyrup_test_support::env::PROVIDER_KEYS.iter()
           .filter(|k| std::env::var_os(k).is_some()).collect();
       assert!(leaked.is_empty(),
           "ambient credentials in the test environment: {leaked:?}. \
            Unset them before running the integration suite — a test has previously made a real \
            network call because TOGETHER_API_KEY was exported.");
   }
   ```

   Layer 3 is what layers 1 and 2 cannot give you: it turns "a test quietly used a real API" into a
   named red at the top of the run. A nextest setup script is **not** the answer here — a setup
   script can only *append* `KEY=value` lines to `$NEXTEST_ENV`, so it can blank a variable but not
   unset it, and blanking defeats a value check while passing an `is_some()` check.

### R6 — No `process::exit`, `abort`, or global handler installation in test code.

In a 310-binary world these cost one test. In a 7-binary world one of them takes down every
remaining test in its target with no report. If a test genuinely needs process-global state — the
wasmtime `rlimited-memory.rs` case — it earns its own `[[test]]` target and says why in a module
doc. The current candidates: `crates/cyrup-tui/tests/native_shift_enter.rs:138` calls
`set_native_modifier_probe`, a first-writer-wins global at `crates/cyrup-tui/src/native_modifiers.rs:62`
asserted `.is_none()`, so it must remain the only setter in whatever binary it lands in;
`crates/cyrup-tui/src/terminal_progress.rs:84`'s `static PROGRESS_ARMED: AtomicBool` is read and
written by six tests and needs the shared lock.

### R7 — Moved-to-`src/` tests keep importing through the crate root.

The 224 files that become `#[cfg(test)]` modules gain the ability to reach private items. They must
not use it — see §7.

---

## 5. Determinism

Real-time sleeps are **unaffected by both the crate move and the runner choice**. This is a separate
workstream and should be tracked as one. `tokio::time::pause()` freezes `Instant::now()` and
auto-advances to the next pending timer *"if time is paused and the runtime has no work to do"*, and
requires the `current_thread` runtime that `#[tokio::test]` uses by default
([tokio::time::pause](https://docs.rs/tokio/latest/tokio/time/fn.pause.html)). Decisively, it
**cannot** help when the wait is on a real OS event — a socket read, a child exit, a signal —
because that counts as work.

So the technique is chosen per offender by asking one question: *is the wait inside the tokio
runtime, or on the OS?*

### 5.1 Injected clock / injected `Instant` — for in-process countdowns

The in-repo precedent already exists: `crates/cyrup-tui/tests/status_indicator.rs` is fully
deterministic because `StatusIndicator` exposes `spinner_at(Duration)` / `lines_at(Duration)`. Copy
that shape.

| Offender | Cost | Fix |
|---|---:|---|
| `crates/cyrup-tui/tests/extension_dialog_countdown.rs:85` + `:106` + `:135` | 1.34s | Filed as **TUI-N09** (`07-cyrup-tui.md:1250`). Add `tick_extension_dialog_countdown_at(Instant)`; the file's own filed fix is TUI-N09(b), mirroring `status_indicator.rs`. |
| `crates/cyrup-tui/tests/footer_chrome_fidelity.rs:526` | 1.2s | Same technique for c8's retry countdown. |

### 5.2 `#[tokio::test(start_paused = true)]` — for pure in-runtime timeouts

**Exactly one occurrence in the entire 310-file corpus:** `crates/cyrup-tools/tests/tools.rs:1050`.
It is the right tool for any timeout that lives entirely inside the runtime and is under-used by two
orders of magnitude.

| Offender | Cost | Fix |
|---|---:|---|
| `crates/cyrup-session-svc/tests/summarization_retry_events.rs:102` | 50ms × many | Filed as **DRIFT-036** (`12-upstream-drift-pi-core.md:112`, `:679-681`): `settle()` is `yield_now`×10 then a fixed sleep as its *only* synchronization. The correct poll-until-observed pattern already exists in the same file. Replace, don't pause. |
| `crates/cyrup-modes/tests/modes.rs:1277`, `:1289-1293`, `:1139`, `:1154-1158` | wall-clock asserts | Filed as **SEAM-030(a)/(c)** (`08-…:848-850`), which calls (c) *"pure wall-clock margin with no semantic content, the most flake-prone"*. Both sit beside an already-deterministic assertion proving the same thing. **Delete the clock assertions**; keep the deterministic ones. |
| `crates/cyrup-modes/tests/modes.rs:1088` | 50ms | SEAM-030(b), classified *"a smell, not a defect"* — `extension_ui_effect_json` returns `None` for those variants regardless of sleep length. Low priority; fix with the rest of SEAM-030. |
| `crates/cyrup-ext/tests/native_dispatch.rs:827`, `:858` | ~800ms | The dispatch budget is already injectable (80ms at `:705`, `:832`, `:863`). Scale budget and handler wait down 10×. Also drop the wall-clock upper bounds at `:712` (`<2s`) and `:877` (`<300ms`) — they assert the scheduler, not the code. |

### 5.3 Configurable production timeout — for waits on real OS events

**This is the technique the three named offenders in the brief actually need**, and `time::pause`
provably cannot substitute for any of them.

| Offender | Measured | The real-time thing it waits on | Fix |
|---|---:|---|---|
| `crates/cyrup-intercom/tests/protocol_number_domain.rs` (`:162`, `:205`, `:244`, `:508`, `:540`) | **15.04s / 13 tests** | 5s frame-read timeouts + 3s deadlines against a real broker child over a real `UnixListener`, plus a 5s broker shutdown check | Make the broker's shutdown-check interval and the client's frame-read deadline **injectable configuration with production defaults**. Test sets milliseconds. |
| `crates/cyrup-intercom/tests/protocol_array_payload_rejection.rs` (`:346` and siblings) | **12.04s / 7 tests** | same shape | same |
| `crates/cyrup-permission-system/tests/forwarding_spawn_env.rs` (`:326`, `:327`, `:351`, `:384`) | **8.05s / 3 tests** | 1s registration timeout + a re-exec'd real child | Inject the registration timeout. **Also fix open item PERM-022** (`10-cyrup-permission-system.md:375-385`): the child bound at `:326` is 8_000ms while the parent polls 15s at `:327` — the sibling at `:383` uses 20_000. That constant mismatch exists because `spawn_child`/`wait_child` are copy-pasted across two files (§3.3). |

Injecting a **value** preserves the literal mechanism — subprocess, Unix socket, real signals —
which the port-fidelity rule requires. Swapping the socket for an in-memory channel would not, and
must not be done.

Two more in this class, both worth doing:

- `crates/cyrup-session-svc/tests/wasm_ui_dialogs.rs:265` sleeps a **real 6s per dialog reply**,
  twice per guest invocation, deliberately exceeding the ~5s `WASM_EPOCH_BUDGET_TICKS` to prove a
  slow guest does not wedge the extension. Make the epoch budget configurable on the
  `ExtensionHost`; set it to 50ms and sleep 60ms. Currently the most expensive single file measured.
- `crates/cyrup-tools/tests/tools.rs:780-795` (`bash_timeout_kills`) asserts
  `elapsed >= 2300ms` against a real 2.5s kill of a 30s sleep. The timeout is **already** a tool
  parameter — pass 200ms and assert `>= 150ms`. Pure test change, ~2.4s recovered.

### 5.4 Sleeps to KEEP — do not "fix" these

The gap analysis has already adjudicated several of these as sound. Carrying that ruling forward so
it is not re-litigated:

- `crates/cyrup-intercom/tests/reconnect.rs:300` (2500ms) and `shared_human_lock.rs:266` (500ms) —
  `11-cyrup-intercom.md:417` explicitly says **do not** change these; they are sound negative
  assertions after a fixed wait.
- `crates/cyrup-permission-system/tests/forwarding_preserve_location.rs:157` (2s) — explicitly
  ruled a **non-instance** of the timing test-defect class at `10-cyrup-permission-system.md:594`.
- `crates/cyrup-session-svc/tests/wasm_http.rs:79`/`:95` — `:274` documents that these are
  deliberately REAL sleeps on a REAL local server, which is the point of the request-budget test.
- `crates/cyrup-tui/tests/terminal_theme_query.rs:222` — `Duration::from_secs(5)` is a *parameter*
  passed to `NoTerminalProbe`, never waited on. `07-cyrup-tui.md:1621` explicitly **rejected** filing
  this. Likewise `bash_overlay.rs:100` (`"!sleep 9"`) and `bash_elapsed.rs:129` (`"sleep 30"`) are
  **string literals** fed to a non-executing transcript.
- `crates/cyrup/tests/signal_shutdown.rs:108` — the 100ms "still alive" check is a genuine
  scheduling assertion and inherently a little racy, but it is the assertion. Give it a nextest
  per-test override rather than changing it.

Every remaining bound in the corpus that reads `timeout(30s)` / `timeout(20s)` / `timeout(5s)` is a
**wedge detector paid only on failure**, not a cost. Do not "optimize" them away; they are what turns
a hang into a red.

### 5.5 Nextest configuration to add

`.config/nextest.toml` currently sets only `slow-timeout` and `retries`. Three additions, applied
**in this order** (the tightened timeout must come last, after §5.1–5.3 land):

```toml
# 1. NOW — names the exact failure this file's own header describes.
[profile.default]
leak-timeout = { period = "500ms", result = "fail" }
```

The header at `.config/nextest.toml:8-14` records a detached `__intercom-broker` grandchild that
inherited a harness pipe FD above 2 and never exited, so `wait_with_output()` — which reads to EOF,
**not** to child exit — blocked in `crates/cyrup/tests/piped_stdin_trim.rs` until the broker was
killed by hand. nextest names this class *"leaky tests"* and bounds it
([leaky-tests](https://nexte.st/docs/features/leaky-tests/)). The caveat cuts the right way here:
nextest only detects children that inherited stdout/stderr, and the broker did. This converts a
silent 180s stall into a named `LEAK-FAIL` in half a second. 97 files spawn processes; 14 call
`.output()`/`wait_with_output()`. Add this before anything else.

```toml
# 2. WITH THE MIGRATION — replaces 33 per-file mutexes that become no-ops on consolidation.
[test-groups]
env-mutating = { max-threads = 1 }
broker       = { max-threads = 4 }

[[profile.default.overrides]]
filter     = 'binary(subagents) or test(/env_/)'
test-group = 'env-mutating'

[[profile.default.overrides]]
filter     = 'binary(broker)'
test-group = 'broker'
```

nextest's docs name exactly this repo's scenarios: *"tests run against a global system resource that
may fail, or encounter race conditions, if accessed by more than one process at a time"*
([test-groups](https://nexte.st/docs/configuration/test-groups/)).

```toml
# 3. AFTER §5.1-5.3 — a 60s blanket cannot tell a 15s legitimate test from a 15s regression.
[profile.default]
slow-timeout = { period = "5s", terminate-after = 6 }   # report at 5s, kill at 30s

[[profile.default.overrides]]
filter       = 'test(reconnect) or test(shared_human_lock) or test(dismiss_incoming_ask)'
slow-timeout = { period = "30s", terminate-after = 2 }  # the KEEP list from §5.4
```

202 of 229 measured targets finish in under a second. A 5s report threshold with explicit,
*named* exceptions is the version of this config that can actually catch a regression.

`retries = 0` stays, along with its stated rationale — *"an intermittent red is a defect to diagnose,
not to re-roll."*

---

## 6. Migration plan

Six phases. Phases 1–4 are strictly ordered; phase 5 runs in parallel with 3 and 4; phase 0 gates
everything and must not be skipped.

### 6.0 — Measure first (half a day, no code)

The cause of the 4h39m is unknown and one confident guess has already been wrong. Run the probes in
§8 **before** phase 1. If the dominant cost turns out to be something the restructuring does not
touch (a machine setting; a leaked FD), shipping the restructuring as the fix would be the second
wrong diagnosis in a row. The restructuring is still worth doing — just not as "the fix".

Deliverable: a short note appended to §8 recording which hypotheses were falsified.

### 6.1 — Build the shared harness (blocks everything)

In `crates/cyrup-test-support`:

1. `env.rs` — one process-wide `ENV_LOCK`, `scoped()` guard, `PROVIDER_KEYS`. Requires relaxing
   `src/lib.rs:23` from `forbid` to `deny(unsafe_code)` with exactly one audited `#[allow]` (R2).
2. `child.rs` — the hermetic child `Command` builder: `env_clear()` + allowlist, per
   `crates/cyrup/tests/auth_credential_print.rs:76`. Absorbs 8 hand-rolled copies and closes the
   class of bug recorded at `11-cyrup-intercom.md:791`.
3. `broker.rs` — `spawn_broker`, `RawClient`, `wait_until` / `within`. Absorbs 6 copies each.
4. `subagent.rs` — `fixture_binary_path`, `write_script`, `write_fixture_persona`,
   `base_run_options`, `single_step`, `message_end_line`, `spawn_child`/`wait_child` with **one**
   set of timeout constants (closes PERM-022's root cause).
5. `tui.rs` (extend) — `app()`, `buf_text()`, `key()`, `ctrl()`, `submit()`, `rendered()`,
   `screen()`, `session()`. Absorbs ~60 near-identical private helpers across cyrup-tui.
6. `session.rs` — `fixture()`, `base_config()`, `id()`, `faux_with_ok()`. Absorbs ~160 duplicated
   helper definitions in cyrup-session-svc alone.
7. `clippy.toml` at the workspace root with the R2 `disallowed-methods` entries.

Also worth doing here, because it is the same piece of work as a parity fix: give
`cyrup-modes`' tests a real RPC client instead of hand-rolled NDJSON framing. `08-…:785-789` files
this as **SEAM-069** — *"Embedders and cyrup's own tests must hand-roll NDJSON framing and request
correlation… which is exactly how wire-shape divergences like SEAM-011 and SEAM-053 go unnoticed"* —
with the Verify step *"modes.rs tests drive the client instead of raw lines and still pass."*

**Expected effect on its own:** several hundred lines deleted and a materially smaller corpus to
move, before a single file is relocated.

### 6.2 — Move the 224 into `src/`, in dependency order

Leaves first, so a mistake is contained; the crates that need §6.1's env seam last.

| Wave | Crates | Files | Notes |
|---|---|---:|---|
| A | cyrup-core, cyrup-provider, cyrup-session, cyrup-config, cyrup-resources, cyrup-agent | 27 | Zero blockers. `cyrup-resources` and `cyrup-config` have almost no `src/` test layer today (1 `#[cfg(test)]` fn and 157 unit tests respectively), so `resources.rs` (71 tests) essentially *becomes* that crate's suite. |
| B | **cyrup-tui** | 77 | **The big win: 80 binaries → 0.** One blocker: `experimental_marker.rs:31-40` is the crate's only env mutator and needs §6.1's `scoped()`. Two globals need the shared lock (`PROGRESS_ARMED`, `set_native_modifier_probe`). |
| C | cyrup-tools, cyrup-ext, cyrup-ext-sdk | 22 | `cyrup-ext-sdk/tests/ergonomic.rs` (25 tests) is effectively that crate's entire suite — `src/` has one `#[cfg(test)]` module at `widget.rs:63`. |
| D | cyrup-session-svc, cyrup-modes | 45 | `model_registry.rs`'s `ScrubbedProviderEnv` moves onto the shared lock. |
| E | cyrup-sdk, cyrup | 11 | `cyrup-sdk` has **zero** `#[cfg(test)]` modules today; see §7 — some of these belong in `cyrup-it/tests/api/` instead. `crates/cyrup/tests/first_time_setup.rs:382-388` needs `scoped()`. |
| F | cyrup-permission-system, cyrup-ext-subagents, cyrup-intercom, cyrup-test-support | 42 | Last, because these are the `#![forbid(unsafe_code)]` crates. Moving `prompt_dedup.rs` here **closes PERM-020**. `cyrup-test-support/tests/deferred_interop.rs` is misfiled — it tests cyrup-session's types; move it to `cyrup-session/src/`. |

Rule for every moved file: **keep the `use cyrup_x::…` import paths exactly as they are.** See §7.

### 6.3 — Stand up `cyrup-it`, pilot first

1. Create the crate skeleton, `build.rs`, and **only** the `cli` target (8 files). Prove the
   binary-path mechanism and measure the second-link cost before moving 84 files. If the cost is
   unacceptable, take the wasmtime-hybrid fallback named in §3.4 — decided here, on evidence, not
   later under pressure.
2. Then `broker` (18), then `subagents` (27), then `harness` (5), then `toolchain` (3).
3. `wasm` (23) last, because it is where the build.rs earns the most (22 duplicated
   `fixture_component()` and 24 nested cargo builds collapse to one) and therefore where a mistake
   is most expensive.
4. Delete each file's nested `cargo build` as its target adopts `CYRUP_IT_COMPONENT`.
5. Add `crates/cyrup-it` to `[workspace] members`; leave it out of `default-members`.

### 6.4 — Deletions, each with the reason it is safe

**Two whole files:**

| File | Why deleting is safe |
|---|---|
| `crates/cyrup-tui/tests/tree_dag_assembled.rs` (2 tests) | Both assertions (connectors/fold-markers on open; fold toggling the marker) are already made by `tree_selector.rs::renders_connectors_glyphs_and_fold_markers` and `::fold_hides_descendants_and_unfold_restores`. Its only added dimension — the same assertions through the assembled `App` — is independently covered by `tree_label_timestamp.rs` (4 tests, driving `/tree` through `App::handle_input`) and `tree_branch_summary.rs::tree_selector_can_be_reselected_at_a_given_entry`. Its own module doc at `:11-12` concedes the DAG getter itself is proven in `cyrup-session-svc/tests/session_dag.rs`. Nothing is uniquely covered. |
| `crates/cyrup-tui/tests/footer_extensions.rs` (2 tests) | The render half is covered *more strongly* by `footer_chrome_fidelity.rs::c3_extension_status_row_is_unstyled_and_only_its_ellipsis_is_dim` and `::c3_mirror_short_status_row_has_no_ellipsis`, which assert resolved cell colour through the assembled App. The set/clear half is covered by `extension_ui_effects.rs::set_status_reaches_the_footer_and_clears`, which drives the real `UiEffect` path. **Salvage before deleting:** two assertions on the pure fn `StatusLine::extension_status_text()` (BTreeMap key ordering; `\n`/`\t` collapsing to one space) are unique — carry them as a 3-line unit test in `crates/cyrup-tui/src/status.rs`. |

**Six individual tests:**

| Test | Why |
|---|---|
| `cyrup-session-svc/tests/session_branch_dir.rs::finding3_explicit_session_dir_is_literal` (`:68`) | Strict subset of `session_list_dir.rs::list_sessions_reads_the_explicit_session_dir` (`:32`) — byte-identical setup, and the sibling asserts the same `file.parent() == custom` **plus** `session_dir()` **plus** the listing. Keep test 1 of the file. |
| `cyrup-session-svc/tests/wasm_compaction_override.rs::fixture_component_exists` (`:174-178`) | Tautological: it calls `fixture_component()`, which already asserts `wasm.exists()` internally, then asserts the same path exists. Zero code coverage. Also disappears by construction once §3.4's build.rs owns the artifact. |
| `cyrup-ext-subagents/tests/discovery_integration.rs::path_buf_import_is_reachable` (`:581`) | Body is `let _ = PathBuf::from("/tmp")` with no assertion; its own comment admits it exists to quiet an unused-import lint. |
| `cyrup-ext/tests/manifest_cache.rs::toolchain_detection_reports_status_without_crashing` (`:172`) | Spawns `cargo`/`rustc` probes and asserts only "does not crash". Pays a subprocess for nothing. The other 8 tests in the file are the sole coverage of `src/build/cache.rs` and must stay. |
| `cyrup-resources/tests/resources.rs` network test (`:2305-2308`) | Double-gated: `#[ignore]` **and** `CYRUP_GIT_NETWORK_TESTS=1`. It has almost certainly never executed. Delete, or promote it to a real test — do not leave a third state. |
| `cyrup-tui/tests/image.rs` assertions at `:56` and `:67-70` | **Retarget, do not delete the file.** These are filed test-defect **TUI-N08** (`07-cyrup-tui.md:1236`): they pin an invented `🖼 {label} ({w}×{h})` placeholder and a rasterize-anyway fallback that contradict `src/image.rs:353-367`. The item's own Fix is RETARGET. `halfblocks_renderer_is_not_graphical` (`:104`) is separately tautological and can go. The decode and `clear_images` tests are sound. |

**Explicitly NOT deleted, despite looking deletable:**

- `crates/cyrup-ext/tests/guest_host_mode.rs` — 3 tests proving an `ExtMode` enum and a `has_ui` bool
  marshal correctly, paying a full component build. Weakest earner in the wasm set, but no unit test
  covers it. **Merge into the `wasm` target's `component` module; do not delete.**
- `crates/cyrup/tests/image_auto_resize_file_args.rs` and `image_bytecap.rs` — these were test-defects
  under ICOM-051 only as a *consequence* of SEAM-072 (`build_inputs` read process stdin internally
  and hung on an inherited pipe). That production split is fixed and verified
  (`08-…:1265`). The old ICOM-051 row is stale; do not delete on the strength of it.
- Anything named as a `Verify` step for an open gap item. §1 lists the ones found; **re-grep
  `docs/gap-analysis/` for the filename before deleting any test, as a standing rule.**

### 6.5 — Determinism workstream (parallel with 6.2/6.3)

§5, tracked as its own set of items: TUI-N09, DRIFT-036, SEAM-030, PERM-022, the intercom timeout
injection, the WASM epoch-budget injection, and the `bash_timeout_kills` parameter change. It does
not block the move and the move does not block it.

### 6.6 — Guardrails and docs

§9, plus a `docs/TESTING.md` (or a section in `CLAUDE.md`) carrying the decision procedure in §9.4.

---

## 7. What we lose, stated plainly

**A test in `tests/` links the crate as an EXTERNAL consumer. A `#[cfg(test)]` module in `src/` does
not.** It can see private items, and — more insidiously — it compiles even if the item it needs was
never re-exported. Moving 224 files into `src/` gives up that check for all of them. This is a real
loss, not a formality.

### Where it actually matters here

Most of the 18 crates are internal, consumed only by siblings, so the sibling that consumes them
*is* the external-consumer check. Four places are different:

1. **`cyrup-sdk` — the embedder-facing crate.** Its whole purpose is to be consumed from outside the
   workspace. It has **zero `#[cfg(test)]` modules in `src/` today**, so its four test files are
   currently the *only* thing exercising it as an external consumer.
   `crates/cyrup-sdk/tests/runtime.rs`'s constraint — that it uses `cyrup_sdk` re-exports **only** —
   is enforced today by the fact that it is an external crate, and would silently evaporate inside
   `src/`.
2. **`cyrup-ext-sdk` — the guest SDK.** Third-party extension authors compile against exactly this
   surface. `tests/ergonomic.rs` (25 tests) is effectively the crate's whole suite, and `src/` has
   one `#[cfg(test)]` module at `widget.rs:63`.
3. **`cyrup-core`'s `Tool` trait vtable.** `crates/cyrup-tools/tests/pi_schema.rs` reads every
   built-in tool's `parameters()` / `description()` / `prompt_snippet()` **through
   `Arc<dyn cyrup_core::Tool>`** — the object-safe surface, from outside. Reading them through
   concrete types inside `src/` is a weaker assertion.
4. **Re-export completeness generally.** A type used by extension authors but accidentally not
   `pub use`d at the crate root is invisible to every `src/` test and fatal to a consumer.

### How the design compensates

1. **A dedicated `api` target in `cyrup-it`.** `cyrup-it` is, by construction, an external consumer
   of every crate in the workspace. `tests/api/main.rs` holds the surface tests that must stay
   external: `cyrup-sdk`'s `runtime.rs` and `embedding.rs` move **here**, not into `cyrup-sdk/src/`;
   likewise `cyrup-ext-sdk`'s `ergonomic.rs` and `cyrup-tools`' `pi_schema.rs`. It also holds two new
   compile-only cases — a minimal embedder and a minimal extension, each written using **re-exports
   only** — which is the check nothing else performs. This is a deliberate deviation from the
   "224 unit-able" split: roughly 8 files stay external on API-surface grounds even though they
   need no seam.
2. **Doctests on the public items.** Doctests compile as external consumers, and the two-command
   gate (`cargo nextest run --workspace && cargo test --workspace --doc`) already runs them.
   `cyrup-sdk` and `cyrup-ext-sdk` should carry doctests on their headline entry points — cheap, and
   they double as documentation the port currently lacks.
3. **The moved tests keep their import paths (R7).** A file moving from
   `crates/cyrup-tui/tests/editor.rs` into `crates/cyrup-tui/src/…` keeps `use cyrup_tui::…`, not
   `use crate::…` or `use super::…`. That preserves the assertion "this is reachable from the crate
   root" for everything except genuine privacy. Reviewer check, greppable and enforceable: **a moved
   test that introduces a `super::` or `crate::` path to a previously-public item is rejected.**
4. **Optional, if it earns its keep:** a `cargo public-api` diff in CI to catch accidental removals
   from the public surface. Listed as a candidate, not a requirement — it is a maintenance cost and
   the port's API is still moving.

### One more honest loss

310 processes gave crash isolation for free. In a 7-target world, a `process::exit`, an abort or a
segfault takes down the rest of that target with no report. R6 and the seam-based grouping bound the
blast radius; nextest's process-per-test removes it entirely for anyone using nextest
([how-it-works](https://nexte.st/docs/design/how-it-works/)). Under plain `cargo test` the risk is
real and is the price of consolidation.

---

## 8. What is not known, and how to find out

**The 4h39m has NOT been isolated.** Do not attach a speed number to this design.

The arithmetic that frames the problem: ~4h39m to reach ~250 targets is **~67 seconds per target**,
against **2.4 minutes of total in-harness execution across 229 targets** (~0.6s each). libtest reports
only the duration of `#[test]` function bodies. Time in static init, in `main`'s teardown, and above
all in the parent's wait-for-pipe-EOF after the last test returns is **invisible to it**. A
near-constant ~1min/target gap is the signature of a fixed stall on process exit, not of slow tests
and not of slow linking.

Three live hypotheses, in descending order of how cheaply they can be falsified. None is asserted as
the cause.

**(a) Leaked-FD pipe-EOF stall.** A test spawns a child that inherits the harness's captured
stdout/stderr FD and outlives the test; the reader waits for EOF on the pipe, not for the child to
exit ([rust-lang/rust#35136](https://github.com/rust-lang/rust/issues/35136), still open).
**This repo has already hit it and written it down** — `.config/nextest.toml:8-14` records the
detached `__intercom-broker` grandchild blocking `piped_stdin_trim.rs` until killed by hand. 97 files
spawn processes; 56 mention intercom/broker; 14 call `.output()`/`wait_with_output()`.
*Probe:* add `leak-timeout = { period = "500ms", result = "fail" }` (§5.5) and run
`cargo nextest run --workspace`. Offenders are named in the report. **Cost: 5 minutes. Do this first.**

**(b) macOS per-new-binary exec cost.** On macOS, XProtect scans every executable on first run, and
*"the XprotectService daemon runs in a single thread"*; Nethercote measured Rust's ui test suite
(~4,000 test executables) dropping from 9m42s to 3m33s with it disabled
([nnethercote](https://nnethercote.github.io/2025/09/04/faster-rust-builds-on-mac.html), corroborated
at [alacritty#8785](https://github.com/alacritty/alacritty/issues/8785)). The environment is darwin
25.5.0 and the suite is 310 freshly-linked binaries that cargo runs **serially** — the gap has that
shape. *Probe:* `hyperfine --warmup 0 -r 5 <one freshly-linked test binary>`; touch a source file,
re-link, re-time; then add the terminal under System Settings → Privacy & Security → Developer Tools
and repeat. **Cost: 10 minutes.** If this is it, the crate restructuring does not fix it and a machine
setting does.

**(c) Nested `cargo build` lock contention. (Not previously raised; surfaced by the triage.)**
**24 test binaries workspace-wide each unconditionally shell out to `cargo build -p cyrup-ext-sdk
--target wasm32-wasip2`** — 13 in cyrup-ext, 10 in cyrup-session-svc, 1 in cyrup-tui. Ten share one
fixed target dir (`std::env::temp_dir()/cyrup-session-svc-fixture-target`) and five share another
(`.../cyrup-ext-fixture-target`), so those groups serialize on each other's cargo build lock. Four
more — `crates/cyrup-ext/tests/discover_load.rs:25`, `guest_host_mode.rs:36`,
`manifest_capabilities.rs:39`, `wasm_provider.rs:25` — pass **no** `--target-dir` and therefore
contend for the **workspace** target dir, which is precisely what their eight siblings were written
to avoid (`wasm_component.rs:31-32`). This repo has already lost a day to a variant of it:
`crates/cyrup-ext/Cargo.toml:47-52` records `build_tier1.rs` leaking a ~213MB artifact cache 57 times,
filling a 16GB `/tmp` tmpfs, and making `ld` die with SIGBUS while linking unrelated doctests.
*Probe:* time `cargo test -p cyrup-ext -p cyrup-session-svc` with the wasm features **on** vs **off**;
and time it once with `CYRUP_EXT_FIXTURE_COMPONENT` pointed at a prebuilt component, which skips all
24 nested builds. **Cost: 20 minutes.** Unlike (a) and (b), this one *is* fixed by the design — §3.4's
build.rs takes 24 → 1 — so if it dominates, the restructuring is the fix and can be claimed as such.

**Fourth, cheap, and diagnostic rather than causal:** `cargo nextest run -p cyrup-tui` vs
`cargo test -p cyrup-tui` on the same build. Same binaries, different scheduling; isolates how much of
the cost is cargo's documented serial-per-binary execution.

Record the results here when they land. Until then: **the design is justified on maintenance,
isolation and best-practice grounds, and on two directly verifiable claims — 310 test binaries become
7, and 24 nested cargo builds become 1.**

---

## 9. The guardrail

All three: a documented rule, a lint, and CI checks. Any one alone erodes.

### 9.1 The rule (documented)

> **A crate's `tests/` directory stays empty.** Tests live in `src/` under `#[cfg(test)]`, or in
> `crates/cyrup-it/`. A test earns a place in `cyrup-it` only by crossing one of four seams: the
> `cyrup` binary's argv/exit-code/stdio, a spawned child process, the intercom broker socket, or a
> live WASM guest — plus the `api` target's public-surface cases (§7). A test that crosses none of
> those is a unit test and belongs in `src/`.
>
> **Adding an eighth `[[test]]` target to `cyrup-it` requires a written justification in this
> document**, and only two justifications are accepted (tokio's criteria): the target needs a
> crate-level `#![cfg(...)]` the rest of the suite must not get, or it needs process isolation
> because it aborts, panics on unwind, installs a global handler, or mutates process-global state.

### 9.2 The lint

`clippy.toml` at the workspace root, with the R2 `disallowed-methods` entries for
`std::env::set_var` / `remove_var`. These fail the build, not the review.

### 9.3 The CI checks

```bash
# G1 — no crate may grow a tests/ directory back. THE load-bearing check.
if compgen -G 'crates/*/tests/*' > /dev/null; then
  echo "ERROR: integration tests belong in crates/cyrup-it (see docs/TEST-ARCHITECTURE.md)"; exit 1
fi

# G2 — the workspace's [[test]] target count must not grow silently.
n=$(cargo metadata --no-deps --format-version 1 \
      | jq '[.packages[].targets[] | select(.kind[] == "test")] | length')
[ "$n" -le 7 ] || { echo "ERROR: $n integration targets, expected <= 7"; exit 1; }

# G3 — --all-features silently re-arms the whole suite. It must not appear anywhere.
! rg -q -- '--all-features' .github/ Makefile* justfile* docs/ CLAUDE.md \
  || { echo "ERROR: --all-features re-arms cyrup-it; use --features it deliberately"; exit 1; }

# G4 — isolation rules R2/R3 and the dead CARGO_BIN_EXE pattern.
! rg -q 'std::env::set_current_dir' crates/            || exit 1
! rg -q 'env!\("CARGO_BIN_EXE_' crates/                || exit 1   # use CYRUP_IT_BIN_* (§3.4)
! rg -q 'env::temp_dir\(\)' crates/ -g '!crates/cyrup-test-support/**' || exit 1

# G5 — the two-command gate. A single command silently drops doctests.
cargo nextest run --workspace && cargo test --workspace --doc
```

G1 is the one that actually holds the line: `crates/*/tests/*` matches nothing once every crate's
`tests/` directory is gone, and `crates/cyrup-it/tests/` is matched by neither glob because the
pattern is anchored at `crates/*/tests/` with cyrup-it explicitly the exception — write it as an
allowlist of one, not a wildcard.

### 9.4 The decision procedure a contributor follows

```
Writing a new test?
├── Does it spawn the `cyrup` binary, a subagent child, the broker, or a WASM guest?
│   ├── yes → crates/cyrup-it/tests/<seam>/  (cli | broker | subagents | wasm | harness | toolchain)
│   └── no  ↓
├── Does it assert something about the crate's PUBLIC surface as an external consumer
│   would see it (re-exports, object-safe trait vtables, an embedder's import list)?
│   ├── yes → crates/cyrup-it/tests/api/    — or a doctest on the public item
│   └── no  ↓
└── src/, under #[cfg(test)]. Import through the crate root (`use cyrup_x::…`), not `super::`.
```

---

## 10. Summary of commitments

| | Before | After |
|---|---:|---:|
| Integration test binaries | 310 | 7 |
| Files carried | — | 308 of 310 (+ 6 individual tests deleted) |
| Nested `cargo build` invocations per suite run | 24 | 1 |
| `env!("CARGO_BIN_EXE_…")` sites | 51 | 0 (5 build.rs constants) |
| `fixture_component()` definitions | 22 | 0 (1 build.rs) |
| Per-file env mutexes | 33 | 1 + a nextest test group |
| `std::env::set_var` call sites in tests | 45 | 0 |
| Duplicated helper definitions | ~250 | 0 |
| Merge gate | `cargo test --workspace` | `cargo nextest run --workspace && cargo test --workspace --doc` |
| Deliberate suite | — | `cargo nextest run -p cyrup-it --features it,wasm-host` |
| Wall clock | 4h39m, cause unknown | **not promised** — see §8 |
