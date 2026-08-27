---
stage: qa
status: completed
updated: 2026-08-23 01:10
---

# Document api.rs's 38 Undocumented Public Items, Starting With The 33 on_* Event Subscribers

**Severity:** medium · **Effort:** L · **Crate:** `crates/cyrup-ext-sdk`

## What is wrong

`crates/cyrup-ext-sdk/src/api.rs` has 38 public items with no `///` doc comment, and the dominant block is the crate's single most important author-facing surface: the 33 `ExtensionApi::on_*` event subscribers, `api.rs:736` (`on_tool_call`) through `api.rs:1030` (`on_session_tree`). They are headed by ONE non-doc `//` section comment at `api.rs:733-734` — which rustdoc drops — so every one of them renders on docs.rs as a bare signature with no prose.

Count the subscribers: `grep -cE '^    pub fn on_' crates/cyrup-ext-sdk/src/api.rs` → 35, minus `on_terminal_input` (:660) and `on_bus` (:729) = 33.

Count the undocumented items across the crate (attributes skipped when walking back):

```
cd crates/cyrup-ext-sdk/src && python3 -c "import re,glob;t=0️
for f in ['api.rs','autocomplete.rs','descriptor.rs','provider.rs','widget.rs','tool_factory.rs','events.rs']+sorted(glob.glob('ctx/*.rs')):
 ls=open(f).read().split(chr(10))
 for i,l in enumerate(ls):
  if not re.match(r'^pub (fn|struct|enum|trait|type) ',l.strip()): continue
  j=i-1
  while j>=0 and ls[j].strip().startswith('#['): j-=1
  if j<0 or not ls[j].strip().startswith('///'): t+=1
print(t)"
```

(remove the stray marker after `t=0`) → **89** total: api.rs 38, ctx/command.rs 14, ctx/ui.rs 10, provider.rs 7, descriptor.rs 5, ctx/session.rs 5, ctx/models.rs 4, autocomplete.rs 2, ctx/tool_call.rs 2, ctx/base.rs 1, ctx/with_session.rs 1. **This task covers only the api.rs 38**; the remaining 51 and the `missing_docs` lint are a separate task (EXT_SDK_CTX_MISSING_DOCS), which must land after this one.

## Why it matters

docs.rs shows an `ExtensionApi` page where 33 of ~40 methods carry no prose — no statement of which host event each maps to, and no explanation of why some return `Outcome` (vetoable) and others return `()` (notify-only). That distinction is the whole tier model, and it is invisible in the rendered docs. The pi citations that justify each wrapper already exist in the `//` section comments; they simply do not reach the reader.

Nothing currently flags this: `missing_docs` is enabled nowhere in the repo (`grep -rn 'missing_docs' /home/user/cyrup` hits only generated files under `target/`), and `[workspace.lints.clippy]` in `/home/user/cyrup/Cargo.toml:97-101` lists only `unwrap_used`/`expect_used`/`panic`/`indexing_slicing`. So the 7-warning `cargo doc` baseline says nothing about these 89 items, and `.flux/todo/CARGO_DOC_WARNINGS.md` will not touch them.

## Fix

Convert each `// --- section ---` heading in api.rs into `///` docs on its members: promote the section citation onto the first member of the group, and give each remaining member a one-line `///` naming its pi counterpart and stating whether it can veto (returns `Outcome`) or is notify-only (returns `()`). Do the same for the other 5 undocumented api.rs items outside the `on_*` block.

Do not enable `#![warn(missing_docs)]` in this task — it would fire on the other 51 items in `ctx/`, `provider.rs`, `descriptor.rs` and `autocomplete.rs`. That switch belongs to EXT_SDK_CTX_MISSING_DOCS.

## Acceptance Criteria

- [ ] Re-running the counting script above with the file list narrowed to `['api.rs']` prints 0
- [ ] `grep -cE '^    pub fn on_' crates/cyrup-ext-sdk/src/api.rs` still returns 35, and every one of those lines is immediately preceded (attributes aside) by a `///` line
- [ ] Each of the 33 event subscriber docs states whether the handler can veto (returns `Outcome`) or is notify-only (returns `()`)
- [ ] `cargo doc -p cyrup-ext-sdk --no-deps` emits no new warnings relative to the 7-warning baseline (or 0, if EXT_SDK_RUSTDOC_LINK_WARNINGS has landed)
- [ ] `cargo test -p cyrup-ext-sdk` passes and `cargo clippy -p cyrup-ext-sdk --all-targets` reports 0 warnings
