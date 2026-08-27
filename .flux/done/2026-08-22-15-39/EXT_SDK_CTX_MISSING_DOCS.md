---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Document The Remaining 51 Undocumented Public Items And Turn On missing_docs For cyrup-ext-sdk

**Severity:** medium · **Effort:** M · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

After api.rs is handled (EXT_SDK_SUBSCRIBER_DOC_COMMENTS), 51 public items across the rest of the crate still have no `///`:

ctx/command.rs 14, ctx/ui.rs 10, provider.rs 7, descriptor.rs 5, ctx/session.rs 5, ctx/models.rs 4, autocomplete.rs 2, ctx/tool_call.rs 2, ctx/base.rs 1, ctx/with_session.rs 1.

Reproduce (attributes skipped when walking back for a `///`):

```
cd crates/cyrup-ext-sdk/src && python3 -c "import re,glob;t=0
for f in ['api.rs','autocomplete.rs','descriptor.rs','provider.rs','widget.rs','tool_factory.rs','events.rs']+sorted(glob.glob('ctx/*.rs')):
 ls=open(f).read().split(chr(10))
 for i,l in enumerate(ls):
  if not re.match(r'^pub (fn|struct|enum|trait|type) ',l.strip()): continue
  j=i-1
  while j>=0 and ls[j].strip().startswith('#['): j-=1
  if j<0 or not ls[j].strip().startswith('///'): t+=1
print(t)"
```

→ 89 today (38 of which are api.rs).

The same non-doc-`//`-heading pattern as api.rs is the cause. Concrete sites:

- `src/ctx/ui.rs:260` `// --- chrome …` heads undocumented `set_header` (:261), `set_footer` (:267), `set_title` (:273)
- `src/ctx/ui.rs:292-301` heads `editor_text` (:303), `set_editor_text` (:311), `paste_editor_text` (:317)
- `src/ctx/ui.rs:324` heads `theme` (:326)
- `src/ctx/command.rs:23-37` heads `new`, `ctx`, `ui`, `session`, `models`

Note rustdoc does not currently see `src/ctx/*` at all — all 13 submodules are private (`src/ctx/mod.rs:36-48`, with pub re-exports at :50-62) — so `cargo doc --no-deps` never link-checks them without `--document-private-items`.

## Why it matters

These are the context surfaces an extension author calls on every event: `ctx.ui().set_header(...)`, `ctx.session()`, `ctx.models()`, the provider stream. `missing_docs` is enabled nowhere in the repo (`grep -rn 'missing_docs' /home/user/cyrup` hits only `target/`), and `[workspace.lints.clippy]` (`/home/user/cyrup/Cargo.toml:97-101`) covers only `unwrap_used`/`expect_used`/`panic`/`indexing_slicing`, so the count regrows unobserved. `.flux/todo/CARGO_DOC_WARNINGS.md` does not cover this class — rustdoc emits no warning for an undocumented item.

## Fix

1. Convert each `// --- section ---` heading into `///` docs on its members across `src/ctx/*`, `src/provider.rs`, `src/descriptor.rs`, `src/autocomplete.rs`: promote the section citation onto the first member, one-line `///` on each remaining member.
2. Once the script prints 0, add `#![warn(missing_docs)]` to `crates/cyrup-ext-sdk/src/lib.rs` so it cannot regrow. Verify it is actually effective on the private ctx submodules or note in `src/ctx/mod.rs`'s `## Submodules` section that `--document-private-items` is required when editing them.

Depends on EXT_SDK_SUBSCRIBER_DOC_COMMENTS landing first (otherwise the lint fires on 38 api.rs items).

## Acceptance Criteria

- [ ] The counting script above prints 0
- [ ] `grep -n 'warn(missing_docs)' crates/cyrup-ext-sdk/src/lib.rs` matches
- [ ] `cargo check -p cyrup-ext-sdk` and `cargo check -p cyrup-ext-sdk --target wasm32-wasip2` both report 0 warnings, 0 errors with the lint enabled
- [ ] Deleting one `///` line from `crates/cyrup-ext-sdk/src/ctx/ui.rs` makes `cargo check -p cyrup-ext-sdk` emit a `missing_docs` warning; restoring it clears the warning
- [ ] `cargo doc -p cyrup-ext-sdk --no-deps --document-private-items 2>&1 | grep -c '^warning'` returns 0
- [ ] `cargo clippy -p cyrup-ext-sdk --all-targets` reports 0 warnings
