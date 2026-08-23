---
stage: exec
status: done
updated: 2026-08-22 19:57
---

# Verify The Build Under Non-Default Feature Combinations

## Objective

Produce a **committed, runnable gate** that type-checks every feature combination this
workspace actually ships or tests, and fix the combinations that are red today.

`cargo check --workspace --all-targets` ([README.md:144](../../README.md)) builds exactly ONE
point in the feature space. Nine crates declare `[features]`; six crates silently depend on a
feature being default-on; one combination is red right now and nothing in the repo would say so.

---

## Three corrections to the original task — read these first

### 1. "Enforced in CI" is not achievable. There is no CI.

There is no `.github/` directory in this repository. [README.md:149-150](../../README.md) states
it outright:

> `cargo clippy` is not optional. […] **There is no CI in this repository, so nothing runs these
> for you.**

`docs/TEST-ARCHITECTURE.md` §9.3 ([TEST-ARCHITECTURE.md:1120-1132](../../docs/TEST-ARCHITECTURE.md))
writes its guardrails G1–G5 as a shell block addressed to a CI that does not exist. **Do not add
`.github/workflows/`** as part of this task — a workflow nobody runs is worse than no workflow,
and this repo has deliberately chosen local gates.

The correct deliverable is a **first-class command in the repo's own tooling**:
`cargo run -p xtask -- feature-matrix`, added to the gate list in
[README.md:142-146](../../README.md). `xtask` already exists, already shells out to subprocesses,
already has a `--check` drift mode, and is already dependency-free
([xtask/Cargo.toml:10-15](../../xtask/Cargo.toml)). That is where this belongs.

### 2. `--all-features` across the workspace is FORBIDDEN as written, and would not be a type-check.

The acceptance criterion "`--all-features` is checked across the workspace" collides head-on with
guardrail **G3** ([TEST-ARCHITECTURE.md:1123-1125](../../docs/TEST-ARCHITECTURE.md)):

```bash
# G3 — --all-features silently re-arms the whole suite. It must not appear anywhere.
! rg -q -- '--all-features' .github/ Makefile* justfile* docs/ CLAUDE.md \
  || { echo "ERROR: --all-features re-arms cyrup-it; use --features it deliberately"; exit 1; }
```

And the hazard is real, not theoretical. `--all-features` turns on `cyrup-it`'s `it` feature
([crates/cyrup-it/Cargo.toml:53](../../crates/cyrup-it/Cargo.toml)), which un-no-ops that crate's
build script ([crates/cyrup-it/build.rs:95-97](../../crates/cyrup-it/build.rs)):

```rust
if std::env::var_os("CARGO_FEATURE_IT").is_none() {
    return;
}
```

Past that line, `build.rs` runs a nested `cargo build` of **five** workspace binaries
([build.rs:70-82](../../crates/cyrup-it/build.rs)) and a `wasm32-wasip2` guest component
([build.rs:165-180](../../crates/cyrup-it/build.rs)), and **hard-fails** if the wasm target is not
installed. A "cheap type-check" it is not.

**Prescription — use `--exclude`, which gets the coverage without arming the suite:**

```bash
cargo check --workspace --exclude cyrup-it --all-features --all-targets
```

Feature selection is per-package: with `cyrup-it` deselected, `--all-features` cannot reach
`cyrup-it/it`, `build.rs` stays a no-op, and every *other* crate's features are still jointly
enabled — which is the only thing `--all-features` was ever wanted for here. `cyrup-it` is checked
separately and deliberately as its own matrix row.

G3's grep scans `.github/ Makefile* justfile* docs/ CLAUDE.md`. The string lives in
`xtask/src/features.rs` and `README.md`, neither of which G3 scans, so this does not trip it — but
**encode the `--exclude cyrup-it` in the matrix data itself** so a future reader cannot drop it.

### 3. `cyrup-ext`'s MCP-037a bug is already FIXED. Only the second run is missing.

MCP-037a's verify line ([13a-mcp-activation.md:1895-1899](../../docs/gap-analysis/13a-mcp-activation.md))
does demand a double run. But the task treats the bug itself as live. It is not: the fix landed,
and `docs/gap-analysis/13-cyrup-mcp-STATUS.md:532` already records `MCP-037a` as **implemented**.
[crates/cyrup-ext/src/facade.rs:631-650](../../crates/cyrup-ext/src/facade.rs):

```rust
pub fn refresh_tools(&self) -> Result<bool, ExtError> {
    if !self.registry.take_tools_dirty() {
        return Ok(false);
    }
    self.materialize_guest_tools()?;
    // The FLAG is the answer, not the materializer's own bookkeeping. […]
    Ok(true)
}
```

The two tests that pin it —
`refresh_tools_reports_a_late_native_registration`
([seam_liveness.rs:242](../../crates/cyrup-ext/src/tests/seam_liveness.rs)) and
`refresh_tools_reports_a_replaced_guest_descriptor`
([seam_liveness.rs:265](../../crates/cyrup-ext/src/tests/seam_liveness.rs)) — carry **no**
`#[cfg(feature = "wasm-host")]`, so they already run on whichever arm is compiled. Every
genuinely host-dependent test module in that crate self-gates correctly:
[`tests/wasm_host.rs:6`](../../crates/cyrup-ext/src/tests/wasm_host.rs) is `#![cfg(feature =
"wasm-host")]`, and so are `trust_gate_order.rs:16`, `capability_handle_ownership.rs:20`,
`native_ctx_state.rs:11`, with per-item gates at `loader_direct_file.rs:122`,
`malformed_manifest.rs:172`, `extension_name_conflicts.rs:272`,
[`seam_liveness.rs:118,143`](../../crates/cyrup-ext/src/tests/seam_liveness.rs) and
[`payload_and_seam_parity.rs:702,884`](../../crates/cyrup-ext/src/tests/payload_and_seam_parity.rs).

So `-p cyrup-ext --no-default-features` is expected **green**. Its matrix row is a regression
lock, not a bug hunt. The real red combination is somewhere else — see Finding A.

---

## The actual feature surface

Nine crates, verified line by line. `cyrup-ext` is the hub: everything downstream is about
whether its `wasm-host` is on.

| Crate | Declaration | What it really controls |
|---|---|---|
| `cyrup-ext` | `default = ["wasm-host"]` ([Cargo.toml:75-76](../../crates/cyrup-ext/Cargo.toml)) | `pub mod caps` / `host` / `host_runtime` ([lib.rs:147-152](../../crates/cyrup-ext/src/lib.rs)) and the re-exports at [lib.rs:191-215](../../crates/cyrup-ext/src/lib.rs). Pulls wasmtime, wasmtime-wasi, reqwest, bytes, async-compression, tokio-util. |
| `cyrup-session-svc` | `default = ["wasm-host"]`, `wasm-host = ["cyrup-ext/wasm-host"]` ([Cargo.toml:19,23](../../crates/cyrup-session-svc/Cargo.toml)) | **Structurally inert — see Finding B.** |
| `cyrup-tui` | `default = ["wasm-host", "scrolling-regions"]`, `scrollback-accumulator` ([Cargo.toml:19-33](../../crates/cyrup-tui/Cargo.toml)) | `scrolling-regions` forwards to `ratatui/scrolling-regions`, which adds two **required** `Backend` trait methods. `wasm-host` is inert in `src/` — see Finding B. |
| `cyrup-tools` | `default = ["inline-images"]`, `inline-images = ["dep:image"]` ([Cargo.toml:41-42](../../crates/cyrup-tools/Cargo.toml)) | Correctly two-armed: [read.rs:284,330](../../crates/cyrup-tools/src/tools/read.rs) and a dedicated `#[cfg(not(feature = "inline-images"))]` test at [tests/tools.rs:210](../../crates/cyrup-tools/src/tests/tools.rs). This is the crate that already does it right. |
| `cyrup-provider` | `faux` ([Cargo.toml:53](../../crates/cyrup-provider/Cargo.toml)) | `#[cfg(any(test, feature = "faux"))]` at [lib.rs:45](../../crates/cyrup-provider/src/lib.rs). PROV-052 territory. |
| `cyrup` | `faux = ["cyrup-provider/faux"]` ([Cargo.toml:48](../../crates/cyrup/Cargo.toml)) | The `Some("faux")` arm at [provider.rs:525](../../crates/cyrup/src/provider.rs). |
| `cyrup-it` | `default = []`, `it`, `wasm-host` ([Cargo.toml:36,53,61](../../crates/cyrup-it/Cargo.toml)) | `it` arms every `[[test]]` via `required-features` **and** un-no-ops `build.rs`. |
| `cyrup-ext-subagents` | `test-fixtures` ([Cargo.toml:21](../../crates/cyrup-ext-subagents/Cargo.toml)) | Two `[[bin]]` fixtures behind `required-features` ([Cargo.toml:92-112](../../crates/cyrup-ext-subagents/Cargo.toml)). |
| `cyrup-intercom` | `test-fixtures` ([Cargo.toml:23](../../crates/cyrup-intercom/Cargo.toml)) | One `[[bin]]` fixture ([Cargo.toml:62-72](../../crates/cyrup-intercom/Cargo.toml)). |

---

## Findings — verified by reading the source, not by running cargo

### Finding A — ONE combination is red today: `cyrup-tui --no-default-features --features scrollback-accumulator`

`ratatui-core`'s `Backend` trait declares `scroll_region_up` / `scroll_region_down` as
**required** methods, each behind ratatui's own `scrolling-regions` feature
(`ratatui-core-0.1.2/src/backend.rs:362-367` and `:387-392`; the feature is declared at
`ratatui-core-0.1.2/Cargo.toml:70`). `cyrup-tui/Cargo.toml:33` is the **only** thing in this
workspace that enables it — `ratatui-image` explicitly takes `default-features = false, features
= []` (`ratatui-image-11.0.6/Cargo.toml:116-119`), and no other member asks for it
([cyrup-ext-subagents/Cargo.toml:90](../../crates/cyrup-ext-subagents/Cargo.toml),
[cyrup-mcp/Cargo.toml:166](../../crates/cyrup-mcp/Cargo.toml),
[cyrup-it/Cargo.toml:138](../../crates/cyrup-it/Cargo.toml) all take bare `ratatui = "0.30.2"`).

Three of the four `impl Backend` blocks in this workspace gate those two methods correctly:

* [`src/app/backend.rs:192-200`](../../crates/cyrup-tui/src/app/backend.rs) — production `InlineBackend`
* [`src/tests/inline_stacking.rs:127-134`](../../crates/cyrup-tui/src/tests/inline_stacking.rs)
* [`src/tests/resize_viewport_failure.rs:113-121`](../../crates/cyrup-tui/src/tests/resize_viewport_failure.rs)

The fourth does not.
[`crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs:74-79`](../../crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs):

```rust
    // Required since TUI-092 F5 turned on ratatui's `scrolling-regions` feature (`425ef9f`), which
    // added both methods to `Backend`. […]
    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_up(region, amount)
    }
    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_down(region, amount)
    }
```

The file self-gates on the *other* feature — `#![cfg(feature = "scrollback-accumulator")]` at
[line 4](../../crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs) — so the two combinations behave
differently:

* `-p cyrup-tui --no-default-features` → the whole file is cfg'd out. **Green.**
* `-p cyrup-tui --no-default-features --features scrollback-accumulator` → the file compiles,
  `scrolling-regions` is off, the trait has no such methods. **`E0407: method
  \`scroll_region_up\` is not a member of trait \`Backend\`` (×2).**

This is the exact class of defect the task exists to find, and nothing today catches it: the
default gate never disables `scrolling-regions`, and `cyrup-it` only ever enables
`scrollback-accumulator` *on top of* defaults
([cyrup-it/Cargo.toml:99](../../crates/cyrup-it/Cargo.toml)).

**Fix — two attribute lines, bringing the probe in line with its three siblings:**

```rust
    // Required since TUI-092 F5 turned on ratatui's `scrolling-regions` feature (`425ef9f`), which
    // added both methods to `Backend`. Gated exactly as the production `InlineBackend`
    // (`src/app/backend.rs:192`, `:197`) and the two in-src capture backends are: the methods are
    // declared on `Backend` ONLY under `ratatui/scrolling-regions`, so an ungated impl is `E0407`
    // in a `--no-default-features --features scrollback-accumulator` build.
    #[cfg(feature = "scrolling-regions")]
    fn scroll_region_up(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_up(region, amount)
    }
    #[cfg(feature = "scrolling-regions")]
    fn scroll_region_down(&mut self, region: std::ops::Range<u16>, amount: u16) -> io::Result<()> {
        self.inner.scroll_region_down(region, amount)
    }
```

Do **not** "fix" this by deleting the file, even though its own header calls it a
"THROWAWAY perf probe […] delete after measuring" and it is one of the `crates/*/tests/*` files
guardrail G1 forbids ([TEST-ARCHITECTURE.md:1113-1116](../../docs/TEST-ARCHITECTURE.md)).
Deleting it removes the evidence instead of the defect, and the tests-layout question is a
different task. Two `#[cfg]` lines; nothing else.

### Finding B — two `wasm-host` features cannot do what their manifests claim

**Neither `cyrup-session-svc/wasm-host` nor `cyrup-tui/wasm-host` can produce a wasmtime-free
build, and neither crate compiles at all if `cyrup-ext/wasm-host` is genuinely off.**

The mechanism is one missing attribute in the workspace dependency table. [Cargo.toml:111](../../Cargo.toml):

```toml
cyrup-ext          = { path = "crates/cyrup-ext",          version = "0.0.0" }
```

No `default-features = false`. **No internal crate in this workspace sets it on any `cyrup-*`
edge** (`grep -n 'cyrup-[a-z-]* *= *{[^}]*default-features' crates/*/Cargo.toml` → empty). So every
consumer inherits `cyrup-ext`'s `default = ["wasm-host"]`, and a downstream crate turning off its
*own* `wasm-host` changes nothing about wasmtime.

And the crates are not merely failing to *remove* wasmtime — they **name host-only items
unconditionally**:

* [`crates/cyrup-session-svc/src/lib.rs:30`](../../crates/cyrup-session-svc/src/lib.rs) declares
  `mod host_services;` with no `cfg`, and
  [`host_services.rs:17-19`](../../crates/cyrup-session-svc/src/host_services.rs) opens with
  `use cyrup_ext::caps::http::HttpCaps; use cyrup_ext::caps::proc::ProcCaps; use cyrup_ext::host::{…};`
  — all three modules exist only under `wasm-host`
  ([cyrup-ext/src/lib.rs:147-152](../../crates/cyrup-ext/src/lib.rs)).
* [`crates/cyrup-session-svc/src/builder.rs:957`](../../crates/cyrup-session-svc/src/builder.rs)
  — `let native_services: Arc<dyn cyrup_ext::host::HostServices> = host_services.clone();` sits
  *outside* the `#[cfg]` pair at `:936-938`.
* [`crates/cyrup-tui/src/app/mod.rs:111`](../../crates/cyrup-tui/src/app/mod.rs) —
  `use cyrup_ext::host::HostServices;`, unconditional. `cyrup-tui/Cargo.toml:15-18` says
  *"Nothing in `src/` is gated on it"*, which is true of `cfg` attributes and hides the harder
  fact that `src/` cannot compile without the host.

This is EXT-026's open residual, already recorded at
[06-cyrup-ext.md:263](../../docs/gap-analysis/06-cyrup-ext.md) ("A wasmtime-free
cyrup-session-svc build cannot be produced") and
[06-cyrup-ext.md:882](../../docs/gap-analysis/06-cyrup-ext.md).

**Scope decision — do NOT try to close EXT-026 here.** Making `cyrup-session-svc` genuinely
wasmtime-free means gating a 3000-line `host_services.rs` and every `LiveHostServices` call site.
That is its own task. What this task must do is stop the matrix from *claiming* a proof it does
not have:

* Keep the `-p cyrup-session-svc --no-default-features` row (it is a real, distinct compilation of
  the `#[cfg(not(feature = "wasm-host"))]` arms at
  [builder.rs:936,1158,2056](../../crates/cyrup-session-svc/src/builder.rs) and
  [session.rs:1180](../../crates/cyrup-session-svc/src/session.rs), which the default gate never
  compiles).
* Label the row honestly in the matrix data: **"compiles the native arms — does NOT remove
  wasmtime (EXT-026)."**
* Correct the two manifest comments to say so, replacing
  `crates/cyrup-session-svc/Cargo.toml:20-22`'s *"Disable for a native-only build (no wasmtime)"*
  with the truth:

```toml
# Selects the NATIVE arms of `builder.rs`/`session.rs` (`#[cfg(not(feature = "wasm-host"))]`).
# It does NOT produce a wasmtime-free build and must not be described as one: `cyrup-ext` enters
# this crate through the workspace table with default features ON (root `Cargo.toml:111`), and
# `src/host_services.rs:17-19` + `src/builder.rs:957` name `cyrup_ext::{caps, host}::*`
# unconditionally through the ungated `mod host_services;` (`src/lib.rs:30`). Closing that is
# EXT-026 (`docs/gap-analysis/06-cyrup-ext.md:263`), not this flag.
wasm-host = ["cyrup-ext/wasm-host"]
```

### Finding C — a workspace-wide `--no-default-features` is a guaranteed hard failure today

Six crates require `cyrup-ext/wasm-host` and none of them says so in its manifest. They get it
for free from `default = ["wasm-host"]`, which `--workspace --no-default-features` removes.

| Crate | Unconditional host-only reference |
|---|---|
| `cyrup-mcp` | [`src/runtime.rs:47`](../../crates/cyrup-mcp/src/runtime.rs) `use cyrup_ext::HostServices;`; `src/owner.rs:313,373,527`; `src/extension.rs:85,323`; `src/ui.rs:4806,4827`; `src/proxy.rs:1696` |
| `cyrup-ext-subagents` | [`src/tui/fleet_overlay.rs:43`](../../crates/cyrup-ext-subagents/src/tui/fleet_overlay.rs) `use cyrup_ext::{InteractiveOverlay, OverlayKey, OverlayKeyCode, OverlayLine, OverlayOutcome};`; `src/tui/fleet_theme.rs:36`; `src/tui/notices.rs:173,179`; `src/background/watch.rs:658,664` |
| `cyrup-intercom` | [`src/session_state.rs:13`](../../crates/cyrup-intercom/src/session_state.rs) `use cyrup_ext::HostServices;`; `src/extension.rs:35`; `src/seams.rs:37` |
| `cyrup-session-svc` | Finding B |
| `cyrup-tui` | Finding B |
| `cyrup-modes` | test-only ([`src/rpc.rs:1683`](../../crates/cyrup-modes/src/rpc.rs), inside `#[cfg(test)] mod tests`) — still breaks `--all-targets` |
| `cyrup` | test-only (`src/tests/dispatch.rs:547,555,589`), on the `[dev-dependencies]` edge at [Cargo.toml:121](../../crates/cyrup/Cargo.toml) |

`cyrup-mcp` is the sharpest case, because it *documents* the dependency it does not declare
([crates/cyrup-mcp/Cargo.toml:16-19](../../crates/cyrup-mcp/Cargo.toml)):

> `cyrup-ext`'s `wasm-host` feature is DEFAULT-ON and `HostServices` lives behind it, so this
> crate names the trait unqualified exactly as `cyrup-ext-subagents` does.

There is a second, subtler break in the same crate: `NativeExtension::set_host_services` is
itself a **feature-gated trait method**
([cyrup-ext/src/native.rs:682-683](../../crates/cyrup-ext/src/native.rs) — `#[cfg(feature =
"wasm-host")] fn set_host_services(&self, _services: Arc<dyn crate::host::HostServices>) {}`),
and `crates/cyrup-mcp/src/extension.rs:776` implements it with no gate. With the feature off,
that is `E0407` independently of the import errors.

**Prescription — declare the requirement on the edge.** In each of the six manifests, replace
`cyrup-ext = { workspace = true }` with:

```toml
# `wasm-host` is NOT optional for this crate: it names `cyrup_ext::host::HostServices` (and the
# feature-gated `NativeExtension::set_host_services` trait method) unconditionally. It used to ride
# on `cyrup-ext`'s `default = ["wasm-host"]`, which made `--workspace --no-default-features` a hard
# compile error with nothing in any manifest explaining why. Stating it here costs nothing in the
# shipped graph (the feature is default-on anyway) and makes the requirement survive a
# `--no-default-features` sweep, which is what `cargo run -p xtask -- feature-matrix` runs.
cyrup-ext = { workspace = true, features = ["wasm-host"] }
```

This is **not** the PROV-052 hazard shape. That defect was a `[dependencies]` edge enabling a
*test double* (`cyrup-provider/faux`) into the shipped binary
([crates/cyrup-provider/Cargo.toml:15-52](../../crates/cyrup-provider/Cargo.toml)). `wasm-host` is
the production capability host, already on in every shipping configuration; the edge changes the
resolved graph in exactly zero builds and only pins a truth that is currently implicit.

With those six edges in place, `cargo check --workspace --no-default-features --all-targets`
becomes a **meaningful, passing** row: `cyrup-ext/wasm-host` stays on because six crates truthfully
require it, while `cyrup-tools/inline-images`, `cyrup-tui/scrolling-regions`,
`cyrup-tui/scrollback-accumulator` and `cyrup-it/it` all go off — proving those four are genuinely
optional. Without the edges, that row can only ever be a wall of `E0432`/`E0407`.

### Finding D — `wasm32-wasip2` stays; the criterion resolves in its favour

`setup.sh:18-24` ([setup.sh](../../setup.sh)) installs it, and it is load-bearing in two places:

* [`crates/cyrup-ext-sdk/Cargo.toml:13-19`](../../crates/cyrup-ext-sdk/Cargo.toml) — `crate-type =
  ["cdylib", "rlib"]`, the guest SDK, built with
  `cargo build -p cyrup-ext-sdk --target wasm32-wasip2` ([README.md:164](../../README.md)).
* [`crates/cyrup-it/build.rs:165-180`](../../crates/cyrup-it/build.rs) — the armed suite builds the
  guest component and **asserts** rather than skipping: *"Run `rustup target add wasm32-wasip2`,
  or set CYRUP_EXT_FIXTURE_COMPONENT."*

The toolchain file deliberately does not auto-install it
([rust-toolchain.toml:4-7](../../rust-toolchain.toml)); `setup.sh` does, for the dev container.
Both are correct. **Do not remove the `setup.sh` line.** The matrix covers the target with a
`check` (not a `build`) row, which is the cheap half of the guarantee:

```bash
cargo check -p cyrup-ext-sdk --target wasm32-wasip2
```

---

## The gate: `cargo run -p xtask -- feature-matrix`

`cargo-hack` is **not installed** in this environment (`ls ~/.cargo/bin` → `cargo-nextest` only)
and `cargo install cargo-hack` is a from-source compile — `setup.sh:26-40` already documents that
`get.nexte.st` is blocked by the egress policy and that installs here are the slowest step by far.
More importantly, `--feature-powerset` is the **wrong tool for this workspace**: it would generate
`cyrup-ext --no-default-features` for the whole graph (Finding C), it would arm `cyrup-it/it`
(correction 2), and it cannot express "`--exclude cyrup-it`" or "this row is expected to compile
the native arms but not remove wasmtime" (Finding B). **Write the curated matrix as data in
`xtask`.** Every row carries the obligation it discharges, and the obligation is printed when the
row fails.

### New file: `xtask/src/features.rs`

Matches `xtask`'s existing idioms — `Result<(), String>`, `std::process::Command`,
`workspace_root()`, zero dependencies ([xtask/Cargo.toml:10-15](../../xtask/Cargo.toml)) — and
obeys `[workspace.lints.clippy]`'s `unwrap_used` / `expect_used` / `panic` / `indexing_slicing`
denials ([Cargo.toml:97-101](../../Cargo.toml)): no `unwrap()`, no `expect()`, no `panic!`, no
indexing.

```rust
//! `cargo run -p xtask -- feature-matrix` — the non-default feature gate.
//!
//! `cargo check --workspace --all-targets` (README "Build") builds ONE point in the feature space.
//! Nine crates declare `[features]`, and the combinations that are NOT that point are where
//! compilation errors hide in this workspace — the `#[cfg(not(feature = "wasm-host"))]` arms of
//! `cyrup-ext`/`cyrup-session-svc` are never compiled by the everyday gate, and neither is any
//! build with `ratatui/scrolling-regions` off.
//!
//! # Why a curated list and not `cargo hack --feature-powerset`
//!
//! Three things the powerset cannot express, each load-bearing here:
//!
//! * **`cyrup-ext/wasm-host` is not optional for six crates.** `cyrup-mcp`, `cyrup-intercom`,
//!   `cyrup-ext-subagents`, `cyrup-session-svc`, `cyrup-tui` and `cyrup-modes` name
//!   `cyrup_ext::host::*` unconditionally. Each declares `features = ["wasm-host"]` on its
//!   `cyrup-ext` edge so the requirement survives `--no-default-features`; a powerset that turned
//!   it off anyway would report a manifest fact as a compile failure, every run.
//! * **`--all-features` must never select `cyrup-it`.** It sets `it`, which un-no-ops
//!   `crates/cyrup-it/build.rs` into a nested five-binary + wasm-guest build (`build.rs:95`), and
//!   re-arms every seam test. `docs/TEST-ARCHITECTURE.md` §9.3 G3 exists for exactly this.
//!   `--exclude cyrup-it` is encoded in the row below, not left to the caller.
//! * **A row can be worth running without proving what it looks like it proves.** The
//!   `cyrup-session-svc --no-default-features` row compiles the native arms; it does NOT remove
//!   wasmtime (EXT-026). `why` says so, so a green run cannot be over-read.

use std::path::PathBuf;
use std::process::Command;

/// One row: the cargo verb, everything after it, and the obligation it discharges.
struct Combo {
    /// `check` for every row but one — MCP-037a's verify line requires cyrup-ext's tests to RUN on
    /// both arms of `wasm-host`, not merely to type-check (13a-mcp-activation.md:1895-1899).
    verb: &'static str,
    args: &'static [&'static str],
    /// Printed when the row fails. Name the obligation, not the command.
    why: &'static str,
    /// Rows that cost minutes: excluded by `--fast`.
    slow: bool,
}

impl Combo {
    fn label(&self) -> String {
        format!("cargo {} {}", self.verb, self.args.join(" "))
    }
}

const MATRIX: &[Combo] = &[
    Combo {
        verb: "check",
        args: &["--workspace", "--all-targets"],
        why: "the default point — README \"Build\". Every other row is a departure from it.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-ext", "--no-default-features", "--all-targets"],
        why: "the `#[cfg(not(feature = \"wasm-host\"))]` arms of facade.rs (:697, :1306, :1333, \
              :1421, :2002, :2047) compile in NO other row. EXT-026 found a hard build error here \
              once already.",
        slow: false,
    },
    Combo {
        verb: "nextest",
        args: &["run", "-p", "cyrup-ext", "--no-default-features"],
        why: "MCP-037a's verify line: `refresh_tools` must report a late NATIVE registration on \
              BOTH arms. The two tests (src/tests/seam_liveness.rs:242, :265) are deliberately \
              feature-agnostic; this is the run that exercises the arm the default gate skips.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-session-svc", "--no-default-features", "--all-targets"],
        why: "compiles the native arms (builder.rs:936, :1158, :2056; session.rs:1180). It does \
              NOT remove wasmtime — `cyrup-ext` still enters with default features on \
              (root Cargo.toml:111) and `mod host_services;` is ungated (lib.rs:30). EXT-026.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-tools", "--no-default-features", "--all-targets"],
        why: "`inline-images` off: read.rs:330's fallback arm and the \
              `#[cfg(not(feature = \"inline-images\"))]` test at src/tests/tools.rs:210.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-tui", "--no-default-features", "--all-targets"],
        why: "`scrolling-regions` off removes two REQUIRED methods from ratatui's `Backend`; every \
              `impl Backend` in the crate must gate them to match.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &[
            "-p", "cyrup-tui",
            "--no-default-features", "--features", "scrollback-accumulator",
            "--all-targets",
        ],
        why: "THE row that was red. tests/zzz_scratch_perf_probe.rs self-gates on \
              `scrollback-accumulator` but implemented `scroll_region_{up,down}` UNGATED, so this \
              is the only combination where the file compiles without the trait methods existing \
              (E0407 x2). Its three sibling impls gate correctly.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-tui", "--features", "scrollback-accumulator", "--all-targets"],
        why: "defaults + the accumulator — the shape cyrup-it's dev edge creates \
              (crates/cyrup-it/Cargo.toml:99).",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-provider", "--features", "faux", "--all-targets"],
        why: "the scripted double compiles standalone, not only via cyrup's self-dev-dependency.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup", "--features", "faux", "--all-targets"],
        why: "src/provider.rs:525's `#[cfg(feature = \"faux\")]` arm — the five spawn-the-binary \
              tests in cyrup-it depend on this compiling.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-ext-subagents", "--features", "test-fixtures", "--all-targets"],
        why: "the two `required-features` fixture bins (Cargo.toml:92-112); cyrup-it's build.rs \
              builds them by name and panics if a target stops existing.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-intercom", "--features", "test-fixtures", "--all-targets"],
        why: "same, for cyrup-intercom-child-fixture (Cargo.toml:62-72).",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["--workspace", "--no-default-features", "--all-targets"],
        why: "every default OFF at once. Meaningful only because the six crates that truly need \
              `cyrup-ext/wasm-host` now declare it on their edge; it proves `inline-images`, \
              `scrolling-regions`, `scrollback-accumulator` and `it` are genuinely optional.",
        slow: false,
    },
    Combo {
        verb: "check",
        // `--exclude cyrup-it` is NOT optional and NOT the caller's business: `--all-features`
        // sets `it`, which un-no-ops crates/cyrup-it/build.rs into a nested build and re-arms
        // every seam test (docs/TEST-ARCHITECTURE.md §9.3 G3).
        args: &["--workspace", "--exclude", "cyrup-it", "--all-features", "--all-targets"],
        why: "two features that are individually fine and jointly contradictory fail HERE and \
              nowhere else.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-ext-sdk", "--target", "wasm32-wasip2"],
        why: "the guest crate is excluded from default-members, so nothing else type-checks it for \
              the target setup.sh installs. Needs `rustup target add wasm32-wasip2`.",
        slow: false,
    },
    Combo {
        verb: "check",
        args: &["-p", "cyrup-it", "--features", "it,wasm-host", "--all-targets"],
        why: "the deliberate suite's own type-check. SLOW: build.rs runs a nested cargo build of \
              five binaries plus the wasm guest (build.rs:95-180). Set CYRUP_IT_BIN_DIR and \
              CYRUP_EXT_FIXTURE_COMPONENT to skip the relink.",
        slow: true,
    },
];

fn cargo_bin() -> std::ffi::OsString {
    std::env::var_os("CARGO").unwrap_or_else(|| "cargo".into())
}

/// `feature-matrix [--fast]`. Runs every row, reports every failure — it does NOT stop at the
/// first, because the point of a matrix is to learn how many combinations are broken, not one.
pub fn run_matrix(flags: &[String], root: PathBuf) -> Result<(), String> {
    let mut fast = false;
    for flag in flags {
        match flag.as_str() {
            "--fast" => fast = true,
            other => return Err(format!("unknown flag {other:?} — feature-matrix takes `--fast`")),
        }
    }

    let mut failed: Vec<String> = Vec::new();
    let mut ran = 0usize;
    for combo in MATRIX {
        if fast && combo.slow {
            println!("SKIP  {} (--fast)", combo.label());
            continue;
        }
        ran += 1;
        println!("\n──── {}", combo.label());
        let status = Command::new(cargo_bin())
            .current_dir(&root)
            .arg(combo.verb)
            .args(combo.args)
            .status()
            .map_err(|e| format!("cannot run cargo: {e}"))?;
        if !status.success() {
            eprintln!("FAIL  {}\n      {}", combo.label(), combo.why);
            failed.push(combo.label());
        }
    }

    if failed.is_empty() {
        println!("\nfeature-matrix: {ran} combination(s) green");
        return Ok(());
    }
    Err(format!(
        "{} of {ran} combination(s) failed:\n  {}",
        failed.len(),
        failed.join("\n  ")
    ))
}
```

### Edit: `xtask/src/main.rs`

Today `run()` ([main.rs:376](../../xtask/src/main.rs)) is the `gen-catalogs` body and
`parse_args()` ([main.rs:339-374](../../xtask/src/main.rs)) rejects anything that is not
`gen-catalogs` at `:353-358`. Add a dispatcher above it and rename the existing body; nothing
inside `gen-catalogs` changes.

```rust
mod features;
mod tsdata;

fn run() -> Result<(), String> {
    let mut argv = std::env::args().skip(1);
    let cmd = argv.next().unwrap_or_default();
    match cmd.as_str() {
        // `parse_args` re-reads `std::env::args()` and validates the command itself, so this arm
        // hands it nothing and stays a pure rename of the old `run` body.
        "gen-catalogs" => run_gen_catalogs(),
        "feature-matrix" => features::run_matrix(&argv.collect::<Vec<_>>(), workspace_root()),
        other => Err(format!(
            "unknown command {other:?} — commands are `gen-catalogs` and `feature-matrix` \
             (see each one's module docs for flags)"
        )),
    }
}

fn run_gen_catalogs() -> Result<(), String> {
    let args = parse_args()?;
    // …the existing body of `run`, unchanged…
}
```

Also update the `feature-matrix` half of the module doc at
[main.rs:1-31](../../xtask/src/main.rs), whose `# Usage` block currently names `gen-catalogs` as
the only command.

### Edit: `README.md`

Add one line to the Build block at [README.md:142-146](../../README.md), so the gate is
discoverable where the other two live:

```sh
cargo check --workspace --all-targets   # type-check everything, including tests
cargo clippy --workspace --all-targets  # REQUIRED — the no-panic policy only fires here
cargo nextest run --workspace           # the everyday gate: 6,855 tests (7 skipped), ~18s
cargo run -p xtask -- feature-matrix    # the non-default feature combinations (add --fast to skip cyrup-it)
```

Follow it with a sentence in the prose at [README.md:148-150](../../README.md): the everyday gate
builds one point in the feature space, `feature-matrix` builds the rest, and the
`cyrup-session-svc --no-default-features` row compiles the native arms without removing wasmtime
(EXT-026).

---

## Order of work

1. **Finding A** — the two `#[cfg(feature = "scrolling-regions")]` attributes in
   [`crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs:74,79`](../../crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs).
   This is the only known-red combination; fix it before the gate that would report it.
2. **Finding C** — `features = ["wasm-host"]` on the `cyrup-ext` edge in `cyrup-mcp`,
   `cyrup-intercom`, `cyrup-ext-subagents`, `cyrup-session-svc`, `cyrup-tui`, `cyrup-modes`, and on
   `cyrup`'s dev edge ([crates/cyrup/Cargo.toml:121](../../crates/cyrup/Cargo.toml)), each with the
   comment from Finding C. Without this the workspace `--no-default-features` row cannot pass.
3. **Finding B** — correct the two misleading manifest comments
   ([cyrup-session-svc/Cargo.toml:20-22](../../crates/cyrup-session-svc/Cargo.toml),
   [cyrup-tui/Cargo.toml:15-18](../../crates/cyrup-tui/Cargo.toml)).
4. `xtask/src/features.rs` + the `main.rs` dispatcher + the README line.
5. Run `cargo run -p xtask -- feature-matrix` and fix whatever else it turns up. Expect surprises
   only in the two rows nothing has ever compiled: `-p cyrup-session-svc --no-default-features`
   and `--workspace --no-default-features`. Record any row you cannot fix as a `slow: false` entry
   whose `why` names the blocking ledger item, rather than deleting the row.

## Definition of done

- [ ] `crates/cyrup-tui/tests/zzz_scratch_perf_probe.rs` gates both `scroll_region_*` methods on
      `scrolling-regions`, matching `src/app/backend.rs:192,197`.
- [ ] The seven crates in Finding C declare `features = ["wasm-host"]` on their `cyrup-ext` edge.
- [ ] `cyrup-session-svc`'s and `cyrup-tui`'s `wasm-host` comments no longer claim a wasmtime-free
      build.
- [ ] `xtask/src/features.rs` exists with the 16-row `MATRIX`; `xtask/src/main.rs` dispatches
      `feature-matrix`; `--exclude cyrup-it` is in the `--all-features` row's data, not left to a
      caller.
- [ ] `cargo run -p xtask -- feature-matrix` exits 0, and its final line reports every row green.
- [ ] `README.md`'s Build block lists the command.
- [ ] `setup.sh`'s `rustup target add wasm32-wasip2` is unchanged (Finding D settled it), and the
      matrix covers the target via `cargo check -p cyrup-ext-sdk --target wasm32-wasip2`.

No new tests, benchmarks or docs pages are required by this task. The matrix runs one existing
test suite (`-p cyrup-ext --no-default-features`) because MCP-037a's verify line asks for a run on
both arms; everything else is a type-check.
