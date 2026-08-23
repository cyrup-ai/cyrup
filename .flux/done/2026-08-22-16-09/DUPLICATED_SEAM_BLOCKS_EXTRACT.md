---
stage: new
status: done
updated: 2026-08-23 00:00
---

# Extract Three Copy-Pasted Blocks In Factory, Runtime And Compaction

**Crate:** `crates/cyrup-session-svc` · **Severity:** medium · **Effort:** small

## Description

Three concrete duplications, each behind a different public entry point, each small enough to fix in one sitting. `src/factory.rs:156-174` and `:187-206` are a verbatim builder-replay sequence (SessionBuilder::new, settings_store, cli_settings, four `if let Some(..)` arms for provider_resolver/auth/trust_store/trust_prompt, the native-extensions loop, build) differing only by the chained `.with_manager(manager)` at :190; adding a factory-carried input costs four coordinated edits (field at :27-35, setter at :58-109, and both replay blocks), and forgetting the second silently mis-configures only the fork path. `src/runtime.rs:522-534` and `:703-714` are a character-identical `SessionBeforeSwitch` veto block, and `:536-548` / `:724-737` repeat the resolve-cwd, `MissingSessionCwd` pre-flight, build and install tail — but the two functions are NOT congruent: the import path interleaves `previous`/`drop(current)`/`std::fs::copy` at :715-723, so a single five-step `resume_into` would move the veto after the copy and leave a copied file behind on a vetoed import. Two narrower helpers are the safe extraction. Finally `src/session/compaction.rs:153-159`, `:297-303` and `src/session/auto_compaction.rs:243-249` are three identical `CompactionEnd { aborted: true, .. }` emissions.

## Acceptance Criteria

- [x] `src/factory.rs` has one `seed_builder(&self, cfg) -> SessionBuilder`; both build methods end in a single chained line and `rg -c 'SessionBuilder::new' crates/cyrup-session-svc/src/factory.rs` returns 1.
- [x] `src/runtime.rs` has `vetoed_resume` and `resume_build_and_install` helpers; the veto block and the resolve-cwd/exists-assert/build/install tail each appear once.
- [x] The import path still performs the veto BEFORE `std::fs::copy` (ordering unchanged) — stated explicitly in the PR with the before/after line references.
- [x] One `emit_compaction_cancelled(reason)` helper serves all three call sites; `rg -c 'aborted: true' crates/cyrup-session-svc/src/session/` returns 1.
- [x] `cargo test -p cyrup-session-svc` still reports 311 passing and `cargo clippy --all-targets` gains no warnings.

## Ordering note (acceptance criterion 3)

The import path still vetoes BEFORE `std::fs::copy`. Before: veto at `src/runtime.rs:703-714`, copy
at `:719-723`. After: `self.vetoed_resume(&current, &destination).await` at `src/runtime.rs:728-730`,
copy at `:735-738`, then `resume_build_and_install(destination, ..)` at `:740`. The extracted tail
holds only resolve-cwd / exists-assert / build / install — the veto was deliberately left at each
call site, so no vetoed import can leave a copied file behind. `switch_session_with` reaches the same
tail at `:571`, after its own veto (`:566`) and `previous`/`drop(current)`.

## Findings

Each was produced by a survey agent and then adversarially checked by a separate verifier that tried to refute it. `OVERSTATED` means the finding is real but the verifier corrected its scope or severity — the corrected values are the ones below.

### Extract `SessionFactory`'s duplicated builder-replay block; it is copy-pasted twice and mirrors seven `SessionBuilder` fields

`CONFIRMED` · severity **medium** · effort **small** · dimension `consistency`

**Evidence.** src/factory.rs:156-174 and src/factory.rs:187-206 are the same sequence verbatim (SessionBuilder::new -> .settings_store -> .cli_settings, four `if let Some(..) = &self.<field>` arms for provider_resolver/auth/trust_store/trust_prompt, the `for ext in &self.native_extensions` loop, `builder.build().await`), differing only by `.with_manager(manager)` at :190. Seven factory fields at :27-35 mirror src/builder.rs:445-454.

**Why it matters.** Adding one more factory-carried construction input costs four coordinated edits (field, setter, and both replay blocks). Forgetting the second block silently mis-configures only the fork path — `build_from_manager` is the runtime's fork entry, reached by a different public API — and no type or test shape catches the divergence. SEAM-065's trust_store/trust_prompt pair is the most recent thing that had to be threaded through all four spots.

**Fix.** Add `fn seed_builder(&self, cfg: SessionConfig) -> SessionBuilder` holding src/factory.rs:156-173's body; reduce `build_with_parent`'s tail to `self.seed_builder(cfg).build().await` and `build_from_manager`'s to `self.seed_builder(cfg).with_manager(manager).build().await`. Removes ~18 duplicated lines and makes the field list one-place.

### Factor the duplicated resume-switch tail in `runtime.rs` (veto → resolve cwd → assert exists → build+install)

`OVERSTATED` · severity **low** · effort **small** · dimension `consistency`

**Evidence.** src/runtime.rs:522-534 and src/runtime.rs:703-714 are a character-identical `if self.vetoed(&current, HostEvent::SessionBeforeSwitch { reason: "resume".to_string(), target_session_file }).await { return Ok(SwitchResult { cancelled: true }); }`. src/runtime.rs:536-548 and :724-737 repeat the cwd-resolve match, the `if !cwd.exists() { return Err(SessionServiceError::MissingSessionCwd(..)) }` pre-flight (#42 / Pi `assertSessionCwdExists`), the `factory.build(SessionTarget::Resume(..), Some(cwd))` and `self.install(next, "resume", previous)`. The import path interleaves `previous`/`drop(current)`/`std::fs::copy` at :715-723 between the two, so the sequences are not congruent.

**Why it matters.** The exists-check is a load-bearing pre-flight that must run before teardown, and it is written twice, 180 lines apart, behind two different public APIs. A future fix to the ordering or the veto payload can land on one resume entry point and not the other — and the import path is the one people forget.

**Fix.** Do NOT extract one five-step `resume_into` — that reorders the import path's veto relative to its `std::fs::copy`. Extract two narrower helpers instead: `async fn vetoed_resume(&self, current: &SharedSession, target: &Path) -> bool` for the identical veto block, and `async fn resume_build_and_install(&self, path: PathBuf, cwd_override: Option<PathBuf>, previous: Option<String>) -> Result<SwitchResult, SessionServiceError>` for the resolve-cwd / assert-exists / build / install tail, which both call sites reach in the same relative position. Separately collapse src/session/compaction.rs:153-159, :297-303 and src/session/auto_compaction.rs:243-249 into one `emit_compaction_cancelled(reason)` helper.

**Verifier correction.** The duplication is real but the two functions do NOT run the same sequence, and the proposed fix would change behaviour. Verified: the 11-line veto block IS character-identical at src/runtime.rs:522-534 and :703-714, and the cwd-resolve / exists-assert / build+install tail is duplicated at :536-548 and :724-737. But the ORDER differs — `switch_session_with` runs veto -> resolve cwd -> assert exists -> `previous` -> `drop(current)` -> build, whereas the import path runs veto -> `previous` -> `drop(current)` -> **copy the file into the sessions dir** (:719-723) -> resolve cwd -> assert exists -> build. A single `resume_into` holding 'steps 1-5' called from the import path 'after its copy step' would therefore move the veto to AFTER `std::fs::copy`, so a vetoed import would leave a copied file behind — an observable semantic change, not a refactor. Scope corrected: what is safely liftable is two smaller helpers, not one five-step function. The finding's secondary item is fully CONFIRMED — src/session/compaction.rs:153-159, src/session/compaction.rs:297-303 and src/session/auto_compaction.rs:243-249 are three identical `CompactionEnd { reason, result: None, aborted: true, will_retry: false, error_message: None }` emissions, and the first and third are each preceded by `cancel_slot.clear();` exactly as stated.
