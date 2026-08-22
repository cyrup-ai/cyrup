---
stage: aug
status: done
updated: 2026-08-22 18:25
---

# Tighten Visibility After the Bedrock Decomposition

## Context — what is already done

[`crates/cyrup-provider/src/api/bedrock_converse_stream/`](../../crates/cyrup-provider/src/api/bedrock_converse_stream/)
is the finished decomposition of the old 4,721-line `bedrock_converse_stream.rs`: 16 modules plus a
10-file `tests/` tree. QA confirmed it is a pure code move (194 item names in, 194 out; the only
differing lines are three wrapped signatures) and that every gate holds. **None of that is in scope
here. Do not re-split anything, do not move code between modules.**

The split widened every moved item to `pub(super)` uniformly — 121 occurrences: 97 items (73 `fn`,
11 `const`, 7 `struct`, 1 `enum`, 5 `async fn`) and 24 struct fields. That grants exactly the scope
these items had inside the old single file, so nothing is wrong. But a quarter of them never cross a
module boundary, and the boundaries are the entire point of the split. **Tighten every item that
does not need to be seen outside its own file back to private.**

## Why this is compiler-driven, not a hand list

A name-based scan cannot answer this. Ripgrep says `EventStreamDecoder::push` is used in
`capabilities.rs`, `convert.rs`, `sigv4.rs` and `url.rs` — those are all `Vec::push`. It says
`EnvSource::new` is used in seventeen files — most are `Self::new()` on unrelated types. It says the
`EventFrame.headers` field is read in `driver.rs` and `sigv4.rs` — those are local `let mut headers`
bindings and `resp.headers()`. Generic names (`new`, `get`, `push`, `json`, `header`, `index`,
`message`, `status`, `region`, `profile`) make every such verdict unreliable in the "still needed"
direction.

The compiler is not fooled. **Strip every `pub(super)`, then restore only what rustc demands back.**
The end state is then provably minimal: every surviving `pub(super)` exists because removing it was
a compile error.

Verified safe: `pub(super)` appears in no comment or doc string anywhere in the tree, so a global
strip touches only declarations.

## Procedure

### 1. Strip

```bash
cd crates/cyrup-provider/src/api/bedrock_converse_stream
sed -i 's/pub(super) //g' *.rs tests/*.rs
grep -rc 'pub(super)' . | grep -v ':0' || echo "all 121 stripped"
```

### 2. Restore what the compiler demands, until the build is clean

Privacy shows up as four errors — `E0603` (private item named through a path), `E0616` (private
field read), `E0451` (private field in a struct literal) and `E0624` (private method) — and every
one of them carries a secondary span pointing at the *declaration*. Drive off those spans, not off
the message text:

```bash
cd /home/user/cyrup
python3 - <<'PY'
import json, re, subprocess, collections
DIR = 'crates/cyrup-provider/src/api/bedrock_converse_stream/'
DECL = re.compile(r'^(\s*)((?:async )?(?:fn|struct|enum|const|type)\s+\w+|\w+\s*:\s*[^=]+,?\s*$)')
for round_no in range(1, 8):
    out = subprocess.run(
        ['cargo', 'build', '-p', 'cyrup-provider', '--all-targets', '--message-format=json'],
        capture_output=True, text=True).stdout
    targets = collections.defaultdict(set)          # file -> {line numbers}
    for line in out.splitlines():
        try: msg = json.loads(line).get('message') or {}
        except json.JSONDecodeError: continue
        if (msg.get('code') or {}).get('code') not in ('E0603','E0616','E0451','E0624'): continue
        stack = list(msg.get('spans', [])) + [s for c in msg.get('children', []) for s in c.get('spans', [])]
        for sp in stack:
            if sp['file_name'].startswith(DIR):
                targets[sp['file_name']].add(sp['line_start'])
    if not targets:
        print(f"round {round_no}: build clean"); break
    fixed = 0
    for f, lines in targets.items():
        src = open(f).read().split('\n')
        for n in sorted(lines):
            ln = src[n-1]
            if 'pub(super)' in ln or 'pub ' in ln: continue
            m = DECL.match(ln)
            if m:
                src[n-1] = m.group(1) + 'pub(super) ' + ln.lstrip(); fixed += 1
        open(f, 'w').write('\n'.join(src))
    print(f"round {round_no}: restored {fixed}")
PY
```

A span can land on a *use* site rather than a declaration; the `DECL` guard skips those, so a round
that restores nothing means the remaining spans need reading by hand — inspect them rather than
loosening something at random.

### 3. Catch the trap the strip sets

Restoring only what `E0603`/`E0616`/`E0451`/`E0624` demand can still leave a **new warning**:
`private_interfaces` fires when a `pub(super) fn` returns or takes a type that is now private to its
own file — e.g. `resolve_client_config` in `config.rs` returning `BedrockClientConfig` if that struct
went private while the function stayed `pub(super)`. That is a signature/type mismatch, not a
missing use, so no error names it. After the loop:

```bash
cargo build -p cyrup-provider --all-targets 2>&1 | grep -A4 'private_interfaces\|more private than'
```

Fix by restoring `pub(super)` on the **named type**, never by narrowing the function.

### 4. Report, do not suppress, any `dead_code`

A `pub(super)` item that nothing uses raises no lint; the same item private raises `dead_code`. If
the tightening surfaces one, that item is genuinely unreachable — a real finding. Report it. Do not
re-widen it to silence the lint, and do not add `#[allow(dead_code)]`.

## Expected outcome

Roughly 30 of the 121 occurrences should end up removed. These 30 are known file-local by a
conservative check (their names appear in no other file at all, so the verdict cannot be a
false positive) — every one of them **must** be private when the loop settles. If the compiler
restores any of them, the analysis was wrong somewhere and is worth understanding before moving on:

| File | Now private |
|---|---|
| `blocks.rs` | `blocks_to_content`, `now_millis`, `Block::index` |
| `capabilities.rs` | `model_match_candidates`, `supports_native_xhigh_effort` |
| `config.rs` | `SKIP_AUTH_SECRET_KEY`, `arn_region`, `parse_ini_profile`, `should_use_explicit_bedrock_endpoint` |
| `convert.rs` | `cache_point`, `convert_tool_result_content`, `create_image_block`, `non_blank_text_block`, `required_text_block` |
| `driver.rs` | `EVENT_STREAM_MEDIA_TYPE` |
| `events.rs` | `handle_content_block_start`, `handle_content_block_delta`, `handle_content_block_stop`, `handle_metadata` |
| `framing.rs` | `MAX_EVENT_FRAME_BYTES`, `be_u32`, `EventStreamDecoder::buffer`, `EventFrame::headers`, `EventFrame::payload` |
| `headers.rs` | `RESERVED_HEADER_EXACT` |
| `options.rs` | `BedrockToolChoice::to_wire` |
| `params.rs` | `build_additional_model_request_fields`, `default_thinking_budget` |
| `sigv4.rs` | `SIGV4_SERVICE`, `civil_from_days` |

The five `framing.rs`/`options.rs`/`blocks.rs` members in that list are the ones the earlier hand
analysis missed — a method and four fields — which is why the loop drives this rather than a table.

The seven structs and one enum all stay `pub(super)`: every one is named from at least one sibling
module. Only their *fields* narrow, and only where nothing outside the file touches them.

## Constraints

- Only the `pub(super) ` token may be added or removed. No item body, signature, doc comment, `use`
  statement or module boundary changes.
- Nothing becomes `pub` or `pub(crate)`. The tree currently has zero `pub(crate)`; keep it there.
- The public surface stays exactly `BedrockOptions`, `BedrockThinkingDisplay`, `BedrockToolChoice`
  (re-exported from `mod.rs`), `BedrockConverseStreamApi` and `factory()` — named by
  [`api/mod.rs`](../../crates/cyrup-provider/src/api/mod.rs) and
  [`stream.rs`](../../crates/cyrup-provider/src/stream.rs), neither of which may be edited.
- Do not run `cargo fmt`. The crate is not rustfmt-clean and formatting would rewrite moved code.

## Definition of done

- [ ] Every surviving `pub(super)` is justified — it was restored because the compiler demanded it
- [ ] All 30 items in the table above are private
- [ ] `grep -rc 'pub(super)' .` totals roughly 91, down from 121
- [ ] Zero `pub(crate)` and no new `pub` in the tree
- [ ] `cargo build -p cyrup-provider --all-targets` — no errors, no warnings
- [ ] No `private_interfaces` warning anywhere
- [ ] `cargo clippy -p cyrup-provider --all-targets` — 37 warnings, exactly one inside this module
      (the pre-existing `result_large_err` on `run_inner`)
- [ ] `cargo doc -p cyrup-provider --no-deps` — 76 warnings, none inside this module
- [ ] `cargo test -p cyrup-provider --lib` — 1118 pass, 57 of them `api::bedrock_converse_stream`
- [ ] Any `dead_code` finding is reported in the completion summary, not suppressed
