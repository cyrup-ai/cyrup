---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_07 — `cyrup-ext-flux` crate: scaffold + state model + `/flux/status` + wiring

## OBJECTIVE

Create the native extension crate `crates/cyrup-ext-flux` in the cyrup workspace, port
[`flux_status.py`](../../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_status.py)'s data
model and renderer to Rust, register `/flux/status` as a native command, and wire the
extension into the binary. This is the first Phase 2 task and the crate's foundation —
FLUX_08/09/10/11 all add to it.

Parent spec: [§3.4 Phase 2](../flux.md), [§3.4.1 skeleton](../flux.md),
[§3.4.2 status port](../flux.md), [§3.4.5 wiring](../flux.md).

## SUBTASKS

### SUBTASK 1: Crate scaffold

```
crates/cyrup-ext-flux/
├── Cargo.toml
└── src/
    ├── lib.rs            # pub fn flux_extension() -> Arc<FluxExtension>
    ├── extension.rs      # NativeExtension impl (this task: flux/status only)
    ├── state.rs          # FLUX_BASE resolution + frontmatter/task/done/review model
    └── render_status.rs  # the table renderer
```

`Cargo.toml`: `name = "cyrup-ext-flux"`, workspace-inherited `edition`/`license`; deps
`cyrup-core`, `cyrup-ext`, `async-trait`, `serde_json`, `tokio` (needed by FLUX_10 — add now).
Add the crate to the workspace `members` in the root `Cargo.toml` and to
`crates/cyrup/Cargo.toml` dependencies.

### SUBTASK 2: `state.rs` — the shared state model

Port function-for-function from
[`flux_status.py`](../../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_status.py)
(`flatten_cwd`, `derive_base`, `parse_frontmatter`, `collect_todos`, `collect_done`,
`format_timestamp`, `collect_reviews`):

- `flatten_cwd(cwd: &str) -> String` — runs of non-ASCII-alphanumerics → one `-`; the exact
  implementation is in spec §3.4.2 (copy it).
- `derive_base(explicit: Option<&Path>) -> PathBuf` — explicit `--base` equivalent is NOT
  needed (commands take section args only); resolve `FLUX_ROOT` env first
  (`${FLUX_ROOT:-$HOME/.flux}` semantics), then `<root>/<flattened current_dir>`.
- `parse_frontmatter(path) -> BTreeMap<String,String>` — port the Python tolerance exactly:
  file must START with `---`; read until the next `---`; split each line on the FIRST `:`;
  missing/malformed → empty map; never error (spec §5.6 — one renderer serves code-puppy and
  cyrup state trees).
- `collect_todos(base) -> Vec<(String, String, String)>` — sorted `todo/*.md` →
  (stem, stage, status).
- `collect_done(base) -> Vec<(String, Vec<(String,String,String)>)>` — `done/<ts>` dirs
  reverse-sorted; `format_timestamp`: 5-part `YYYY-MM-DD-HH-MM` → `YYYY-MM-DD HH:MM`, anything
  else passed through; status defaults to `completed` when absent (Python parity).
- `collect_reviews(base) -> Vec<(String, String)>` — FIXED severity order
  `critical, high, medium, low`; sorted `*.md` within each.

### SUBTASK 3: `render_status.rs` — the table

Port `render()` with the Python layout constants and glyphs (spec §3.4.2):
`name_w = min(max_name_len + 2, 50)` (floor at `len("TODO-FILE")`), `stage_w = 8`,
`_SECTION_PAD = 18`, `_MIN_PANEL_W = 48`; sections TODO → COMPLETED → REVIEW; `𝕱 FLUX STATUS`
header; `═`/`─` rules; status cells `🔄 in-progress` / `✅ done|completed` / `🔁 needs-rework` /
`(unknown)`; review grid with fixed columns `{critical:10, high:6, medium:8, low:5}` and `●`
dots; trailing `═` rule.

**No ANSI** — the TUI strips it from external text (spec §0.4,
[`../../crates/cyrup-tui/src/ansi.rs`](../../crates/cyrup-tui/src/ansi.rs)). The glyphs carry
the semantics. Color lives in the FLUX_09 overlay.

Arg handling: positional section filter — `/flux/status`, `/flux/status todo`,
`/flux/status todo review`. Invalid section name → self-issued Error notify + `Ok(None)`
(the `execute_command` output-channel contract, spec §0.2). Missing base dir → return the
`(no flux state at <base>)` line.

### SUBTASK 4: `extension.rs` + `lib.rs`

Follow the spec §3.4.1 skeleton exactly (it is the approved shape), minus the parts belonging
to later tasks — this task registers ONLY:

- `api.register_command("flux/status", …)` (NOT cheatsheet/about — FLUX_08; NOT the shortcut —
  FLUX_09; NOT the tool — FLUX_10).
- `execute_command` routes `"flux/status"` → `Ok(Some(render_status::render(args)))`; anything
  else → the `ExtError::Component("native extension has no handler …")` default.
- Keep the `host_services: Arc<OnceLock<Arc<dyn HostServices>>>` field +
  `set_host_services` now (FLUX_09/10 need it; the pattern is
  [`../../crates/cyrup-ext-subagents/src/extension.rs`](../../crates/cyrup-ext-subagents/src/extension.rs)
  lines 139, 751–757).
- Do NOT subscribe to `ResourcesDiscover` yet (FLUX_11).

`lib.rs`: `pub fn flux_extension() -> Arc<FluxExtension>`.

### SUBTASK 5: Wire into the binary

In [`../../crates/cyrup/src/main.rs`](../../crates/cyrup/src/main.rs), after the subagents
attach block (`main.rs:692-717`, spec §3.4.5):

```rust
factory_builder = factory_builder.with_native_extension(cyrup_ext_flux::flux_extension());
```

### SUBTASK 6: Build + behavioral check

```bash
cargo build -p cyrup-ext-flux && cargo build -p cyrup
```

Hand-build a fixture and render it:

```bash
BASE=~/.flux/-tmp-flux-scratch   # reuse the FLUX_02–05 scratch state, or hand-write one
mkdir -p "$BASE/todo" "$BASE/done/2026-08-15-20-30" "$BASE/review/critical"
# one todo with stage: exec / status: in-progress; one done file; one review file
```

In the TUI: `/flux/status` prints the aligned glyph table (todo row, `── 2026-08-15 20:30 ──`
done group, review grid with the `●` under CRITICAL); `/flux/status todo` prints only the TODO
section; `/flux/status bogus` produces an Error notify; an empty `FLUX_ROOT` (run with
`FLUX_ROOT=/tmp/empty-flux cyrup`) prints `(no flux state at …)`.

## RESEARCH NOTES

- `NativeExtension` trait surface: [`../../crates/cyrup-ext/src/native.rs`](../../crates/cyrup-ext/src/native.rs)
  (`init` :461, `execute_command` :580, `set_host_services` :683).
- `CommandDescriptor` is `{ description, completions }`:
  [`../../crates/cyrup-ext/src/registry.rs`](../../crates/cyrup-ext/src/registry.rs) :94-98.
- Command names with `/` route fine (dispatch splits on first space — spec §0.2).
- Native commands shadow same-named templates (spec §0.2) — irrelevant here (no `flux/status`
  template exists) but load-bearing for the family.

## DEFINITION OF DONE

- [ ] `cargo build -p cyrup-ext-flux` and the full `cyrup` binary build cleanly.
- [ ] `/flux/status` renders the fixture table with correct alignment, glyphs, done-grouping,
      and severity grid; section filters and the invalid-section Error notify behave as above.
- [ ] `/flux/status` appears in the TUI command list; the extension shows up in whatever
      extension-listing surface the binary exposes.
- [ ] No other `/flux/*` native commands registered yet (templates from the package still
      handle the rest).

No tests to be written. No benchmarks to be written.
