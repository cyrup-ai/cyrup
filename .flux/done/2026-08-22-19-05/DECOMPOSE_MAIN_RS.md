---
stage: qa
status: completed
updated: 2026-08-22 21:34
---

# Decompose crates/cyrup/src/main.rs — Rework

The decomposition **landed and is accepted**: `main.rs` 2,828 → 643 lines (code 1,614 → 313),
the triplicated native-extension block collapsed to one function, all 51 definitions verified
at single destinations, no code lost, 230 tests green including the PROV-047 and SEAM-106
ordering tests, changes confined to `crates/cyrup/src/`.

**Do not re-do the decomposition.** Two defects remain. Both are edits to lines that already
exist — no new functions, no moved code, no signature changes.

---

## 1. Narrow six over-exposed items

The plan required, verbatim:

> Items with **no** `main.rs` caller after the move stay private to their module: …
> `migrated_credentials_warning`, `trust_store_for` (crate-internal — `pub(crate)`) …

Thirteen of fifteen were done right. These six were left `pub` on the `cyrup` lib's public
API with no caller outside their own module or crate.

### Research: every narrowing is safe

Verified by enumerating **every** reference across `src/` and the three separate
integration-test crates in `tests/`:

| item | file:line | callers | narrow to | why it compiles |
| --- | --- | --- | --- | --- |
| `migrated_credentials_warning` | [`interactive.rs:30`](../../crates/cyrup/src/interactive.rs) | `run_interactive` (`:290`), test mod (`:462`) | `fn` | a child `mod tests` can reach a private parent item through `use super::` |
| `trust_store_for` | [`prelaunch.rs:248`](../../crates/cyrup/src/prelaunch.rs) | `trust_prompt_callback` (`:231`), `session_launch.rs:163` | `pub(crate) fn` | both callers are in this crate |
| `list_models` | [`actions.rs:99`](../../crates/cyrup/src/actions.rs) | `list_models_action` (`:90`) — **the only call** | `fn` | same module |
| `collect_settings_diagnostics` | [`bootstrap.rs:108`](../../crates/cyrup/src/bootstrap.rs) | `load_startup_settings` (`:99`) | `fn` | same module |
| `gather_session_refs` | [`session_resolve.rs:382`](../../crates/cyrup/src/session_resolve.rs) | `prelaunch.rs:48`, tests (`:752`, `:813`) | `pub(crate) fn` | same crate; tests reach it via `super::` |
| `gather_session_scopes` | [`session_resolve.rs:374`](../../crates/cyrup/src/session_resolve.rs) | `prelaunch.rs:153`, test (`:776`) | `pub(crate) fn` | same crate; test reaches it via `super::` |

Two traps checked and cleared:

* **No integration-test crate uses any of the six.** The many `list_models` hits in
  `tests/first_time_setup.rs` and `cli.rs` are the **`Cli::list_models` struct field**, an
  unrelated name — not this function. Narrowing the function cannot affect them.
* **None of the six is re-exported.** `lib.rs`'s `pub use` blocks name none of them, so no
  re-export breaks.

`main.rs` names none of the six, so the sequencer is untouched by this change.

### Required edit

Change only the visibility keyword on each of the six declarations:

```rust
// interactive.rs:30
-pub fn migrated_credentials_warning(providers: &[String]) -> Option<String> {
+fn migrated_credentials_warning(providers: &[String]) -> Option<String> {

// prelaunch.rs:248
-pub fn trust_store_for(dirs: &ConfigDirs) -> Arc<cyrup_config::trust::TrustStore> {
+pub(crate) fn trust_store_for(dirs: &ConfigDirs) -> Arc<cyrup_config::trust::TrustStore> {

// actions.rs:99
-pub fn list_models(models: &[cyrup_provider::Model], search: &str) -> anyhow::Result<i32> {
+fn list_models(models: &[cyrup_provider::Model], search: &str) -> anyhow::Result<i32> {

// bootstrap.rs:108
-pub fn collect_settings_diagnostics(
+fn collect_settings_diagnostics(

// session_resolve.rs:374 and :382
-pub fn gather_session_scopes(dirs: &ConfigDirs) -> (Vec<SessionInfo>, Vec<SessionInfo>) {
+pub(crate) fn gather_session_scopes(dirs: &ConfigDirs) -> (Vec<SessionInfo>, Vec<SessionInfo>) {
-pub fn gather_session_refs(dirs: &ConfigDirs) -> (Vec<SessionRef>, Vec<SessionRef>) {
+pub(crate) fn gather_session_refs(dirs: &ConfigDirs) -> (Vec<SessionRef>, Vec<SessionRef>) {
```

Nothing else changes — no `use` lines, no call sites, no `lib.rs`.

---

## 2. Clear the five rustdoc warnings the move introduced

### Research: the exact ledger

`cargo doc -p cyrup --no-deps` reports **21 warnings** (a `grep -c '^warning:'` counts **22**,
because rustdoc's own "generated 21 warnings" summary line also begins with `warning:`).

**Five are new**, and they exist *because* the code moved from the binary — which `cargo doc`
does not document at all — into the library, which it does. Before the refactor these same doc
comments produced zero warnings because nothing ever rendered them.

Two are **unresolved links**: intra-module references that were valid inside `main.rs` and that
the move invalidated. These matter most — an unresolved link means the target does not exist,
so the rendered doc silently drops the reference.

Three are **public-doc-links-to-private-item**.

### Why plain backticks, not repointed links

For all five, replace the bracket link with plain backticks. Do **not** repoint the link and do
**not** widen visibility to satisfy it:

* Repointing `[`session_list_cwd_filter`]` to
  ``[`crate::session_resolve::session_list_cwd_filter`]`` **trades one warning for another** —
  that item is `pub(crate)`, and `print_resume_hint` is `pub`, so rustdoc's
  `private_intra_doc_links` fires on exactly the same line.
* Widening any target to `pub` to make a link resolve would re-open defect 1.
* A link to a private item renders as plain text anyway, so the bracket buys the reader nothing.

House-style note, for scope: this crate already carries **eight** pre-existing
`links to private item` warnings (`read_piped_stdin`→`normalize_piped_stdin`,
`registry_with_credentials`→`composed_registry`, `run_print_dispatch`→`turn_inputs`,
`signals`→`ShutdownSignal`, `run_trust_prompt`→`persist_trust_choice` ×2, and the
`SUBCOMMANDS` / `0` unresolved links in `credential_print.rs`, `intercom_broker_cmd.rs`,
`subagent_runner_cmd.rs`). **Leave every one of them alone** — they are
[`CARGO_DOC_WARNINGS.md`](CARGO_DOC_WARNINGS.md)'s business, not this task's. Fix only the five
listed below.

### Required edits — exact strings

```
crates/cyrup/src/interactive.rs:47   (unresolved link — target moved to session_resolve.rs)
-/// cwd-encoded path [`session_list_cwd_filter`] compares against — so the `--session-dir` argument is
+/// cwd-encoded path `session_resolve::session_list_cwd_filter` compares against — so the `--session-dir`
+/// argument is
```
Name the module so the reader can still find it; the sentence continues on the next line
unchanged (`printed exactly when the session is not where a bare relaunch would look for it.`).
Re-wrap if the line exceeds the file's ~100-column norm.

```
crates/cyrup/src/interactive.rs:77   (links to private BENCHMARK_DRAIN_MS)
-/// real terminal (Pi `interactiveMode.init()`), give the stdin handler [`BENCHMARK_DRAIN_MS`] to drain
+/// real terminal (Pi `interactiveMode.init()`), give the stdin handler `BENCHMARK_DRAIN_MS` to drain
```

```
crates/cyrup/src/prelaunch.rs:227   (unresolved link — `run` is in main.rs, a different crate)
-/// the interactive-only wiring in [`run`] reproduces; every other host leaves it unset and the
+/// the interactive-only wiring in `run` (`main.rs`) reproduces; every other host leaves it unset and the
```

```
crates/cyrup/src/session_launch.rs:141-142   (links to private attach_native_extensions)
-/// Build the [`SessionFactory`] every mode launches from: the shared prefix plus
-/// [`attach_native_extensions`].
+/// Build the [`SessionFactory`] every mode launches from: the shared prefix plus
+/// `attach_native_extensions`.
```
Keep `[`SessionFactory`]` — it is a public re-exported type and resolves cleanly.

```
crates/cyrup/src/session_launch.rs:178   (links to private apply_post_build)
-/// host. See [`apply_post_build`] for the knobs themselves.
+/// host. See `apply_post_build` for the knobs themselves.
```

---

## Not a defect — no action

`main.rs` is **643 lines** against the original DoD's "under ~450". Accepted as a correction to
the task, not a shortfall: 283 of those lines are pi-parity ordering comments, and the same task
mandates that "Every doc comment travels verbatim with the code it documents" and that the
load-bearing ordering citations stay readable as one sequence in `run()`. Reaching 450 would
mean deleting that documentation. The meaningful number — code lines, 1,614 → 313, −81% — is
comfortably met.

Two deviations beyond the plan are accepted as improvements and must be **kept**:

* the factory-build + `launch` for the two non-interactive arms is hoisted out of the `match`
  rather than repeated per arm — the same collapse taken one step further, with each arm's
  subsequent ordering preserved;
* `actions::list_models_action` puts the `--list-models` auth predicate beside the renderer
  instead of in the sequencer.

---

## Definition of done

- [ ] The six declarations in §1 carry the visibility in the "narrow to" column; no call site,
      `use` line or `lib.rs` entry changed.
- [ ] The five doc comments in §2 use plain backticks; no target's visibility was widened to
      make a link resolve.
- [ ] `cargo doc -p cyrup --no-deps` reports **16** warnings
      (`... 2>&1 | grep -c '^warning:'` → **17**), and
      `... 2>&1 | grep -E 'crates/cyrup/src/(actions|bootstrap|interactive|predispatch|prelaunch|session_launch|main)\.rs'`
      returns nothing.
- [ ] The sixteen pre-existing warnings in `credential_print.rs`, `input.rs`,
      `intercom_broker_cmd.rs`, `provider.rs`, `run.rs`, `signals.rs`, `startup_ui.rs` and
      `subagent_runner_cmd.rs` are untouched.
- [ ] `cargo build -p cyrup` clean and `cargo test -p cyrup` still 230 passing.
- [ ] No file outside `crates/cyrup/src/` modified.
