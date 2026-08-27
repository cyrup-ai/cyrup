---
stage: qa
status: needs-rework
updated: 2026-08-27 05:20
---

# Project-trust gating — remaining work

> **QA verdict 9/10.** The gate itself is landed, correct and upstream-faithful (commit
> `8ffd145`): `manager_paths_for(agent_dir, Option<&Path>)`, `project_trusted`,
> `warn_project_untrusted` + `UNTRUSTED_PROJECT_MESSAGE`, both lifecycle arms, the three
> constructors, and four regression tests in `src/extension/tests/project_trust.rs`. **203 passed,
> 0 failed**; clippy at exactly its pre-existing baseline with zero warnings in any touched file.
> Do not re-do any of that. One item is outstanding.

## Verified sound — do not change these

- **No rebuild site was missed.** `extension/paths.rs` is the only production `ManagerPaths`
  builder; all seven `manager_paths_for` call sites and both `refresh_config_and_manager` call
  sites are updated. `sanitize/skills.rs:315` is a test-module helper that already passes
  `project_global_config_path: None`.
- **No hidden integration regression.** The exec run only exercised `--lib`, and `cyrup-it` is a
  separate package — so this was re-checked: those targets are `required-features = ["it"]` (off
  by default, deliberately excluded from `cargo test --workspace`), and no test under
  `cyrup-it/tests/permission/` writes a `.cyrup/agent` policy file, so none depends on
  project-scoped policy. `layers_wired.rs`'s "projectAgent" prose refers to the layer model, not
  to a project file on disk.
- **The tests are well-formed.** The untrusted/trusted pair is what gives them force: a lone
  `Block` assertion could pass for an unrelated reason, and the trusted twin rules that out.

---

## 1. The `[CYRUP-DELTA]` rationale overstates its guarantee (BLOCKER)

`src/extension/config.rs:85-93` currently reads:

> `host_services.get()` resolves it **exactly** for THIS crate … It is **exact rather than a
> heuristic** because: this crate holds `Arc<dyn HostServices>` unconditionally while
> `cyrup-ext`'s `host` module is `cfg(feature = "wasm-host")`, so it cannot build on the arm where
> the two could diverge; and on the arm it does build, `ExtensionHost::load_native_with_services`
> (`cyrup-ext/src/facade.rs:354-366`) sets the backend and the ctx source in one body.

Both premises are true. The conclusion does not follow from them, because it needs an unstated
third premise — *that every attachment of `host_services` goes through the facade*.

**It does not, and the counterexamples are in this repository.**
`NativeExtension::set_host_services` is a public trait method, and **26 in-repo sites call it
directly with a hand-built `HostCtx`**:

| location | sites |
| --- | --- |
| `crates/cyrup-permission-system/src/extension/tests/` | 15 |
| `crates/cyrup-it/tests/permission/` | 11 |

Each one puts the extension in the state the doc says cannot arise: backend attached, **no**
`HostCtxSource`, `HostCtxRich::default()` → `is_project_trusted = false` → the project scope is
silently withheld. That is precisely the "silent narrowing" this `[CYRUP-DELTA]` exists to
prevent, occurring in the configuration it claims is impossible. One existing test
(`config_reload::session_start_rebuilds_manager_from_current_session_cwd`) hit it during exec and
had to be given an explicit trusted ctx.

**This is not a behavioural defect.** Production goes through `load_native_with_services`
(`cyrup-session-svc/src/builder.rs:1010`), so the shipped gate is correct, and the failure
direction is conservative (less permission, not more). It is a **documentation defect in the one
comment the whole design rests on** — the durable record of *why* the gate keys on backend
attachment. A maintainer touching the gate, or anyone embedding this extension without the facade,
would be misled by "exact".

The exec run reached this same conclusion in its own report and commit message ("`exact` was too
strong"). The code was not updated to match.

### What to do

Rewrite the second paragraph of `project_trusted`'s doc so the guarantee it states is the one that
actually holds. It must say, in whatever wording fits the file's voice:

- `host_services.get()` is **exact for the facade wiring** — `load_native_with_services` attaches
  the backend and the `HostCtxSource` in one body, and that is the path
  `cyrup-session-svc/src/builder.rs:1010` takes for every native built-in, so in production the
  two are inseparable.
- It is **not** exact for a host that calls `NativeExtension::set_host_services` directly without
  `ExtensionHost::set_ctx_source`. Such a host gets `HostCtxRich::default()`, so this gate reads
  `is_project_trusted = false` and withholds the project scope.
- That residual case is **accepted and conservative** — it withholds policy rather than granting
  it — and it is the reason `src/extension/tests/support.rs::trusted_event_ctx` exists: a test
  whose subject is project-scoped policy has to state trust explicitly.
- Keep the `cfg(feature = "wasm-host")` premise; it is true and still rules out the
  `--no-default-features` hazard the parity index warned about.

Do not weaken the gate itself and do not change `project_trusted`'s body — the behaviour is right.
This is a comment correction.

While there, check the same overstatement has not been copied into
`src/extension/tests/support.rs::trusted_event_ctx`'s doc, which describes the same mechanism.

---

## 2. The `project_trust.skipped` review entry is unpinned (recommended, judge the cost)

DoD item 5 names two observable effects of an untrusted session; only one is pinned.
`an_untrusted_session_announces_the_reduced_scope` covers the **notification**. Nothing covers the
**review entry** — `write_review_entry("project_trust.skipped", { cwd, phase })` — which is the
durable, security-relevant half and the one that survives a dropped notify sink.

The exec run disclosed this and judged the cost disproportionate: asserting it needs the
`CYRUP_PERMISSION_SYSTEM_LOGS_DIR` override plus JSONL read-back, held under the crate env lock.
That is a defensible call, and `crates/cyrup-permission-system/src/tests/forwarding_audit_trail.rs`
already establishes the pattern if it is taken up (`entries()` / `events()` helpers over
`debug_path(logs_dir)`).

Either pin it using that existing pattern, or record in `project_trust.rs`'s module doc that the
review half is deliberately unpinned and why — so the gap is a decision on the record rather than
an omission. Both `phase` values (`session_start`, `resources_discover`) should be covered if it
is pinned, since only the first arm is exercised by any current test.

---

## Definition of done for this rework

1. `project_trusted`'s `[CYRUP-DELTA]` states a guarantee that holds for every in-repo caller —
   exact for the facade path, conservative-and-accepted for a direct `set_host_services` — and no
   longer claims the divergent configuration cannot arise.
2. `trusted_event_ctx`'s doc does not repeat the overstatement.
3. Item 2 is either pinned or recorded as a deliberate gap.
4. `cargo check -p cyrup-permission-system --all-targets` and
   `cargo clippy -p cyrup-permission-system --all-targets` stay clean, and the lib suite still
   passes at 203.

> Reference checkout for upstream citations: `tmp/pi-packages/packages/pi-permission-system`
> (gitignored — re-clone with
> `git clone https://github.com/gotgenes/pi-packages tmp/pi-packages` if absent).
