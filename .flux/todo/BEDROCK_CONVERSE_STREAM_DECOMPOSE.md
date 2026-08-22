---
stage: qa
status: needs-rework
updated: 2026-08-22 18:10
---

# Decompose Bedrock Converse Stream Into Submodules — Rework

## QA verdict: 9/10

The decomposition is done and verified. `crates/cyrup-provider/src/api/bedrock_converse_stream.rs`
is now a 16-module directory plus a 10-file `tests/` tree, and QA independently confirmed it is a
pure code move: normalizing indentation, visibility prefixes, comments and `use` blocks, the only
differing lines between the old file and the new tree are three wrapped signatures. Item-name
multisets match exactly (194 in, 194 out). All gates hold against a measured pre-split baseline —
build clean, clippy 37 = 37, `cargo doc` 76 = 76 with none in this module, 57 bedrock tests pass,
workspace builds.

**Do not redo any of that.** One thing remains.

## Outstanding: tighten the 25 over-exposed items

Every moved item was widened to `pub(super)` uniformly. That is what the plan said, and it grants
exactly the scope these items had inside the old single file — so nothing is *wrong*. But the point
of splitting a 4,721-line file into concern-sized modules is the boundaries it buys, and 26% of the
widened items never cross one. Each is called only from within its own file, so each can go back to
private, and the module boundary then means something a reader can rely on.

Delete the `pub(super) ` prefix from exactly these 25 declarations:

| File | Items |
|---|---|
| `blocks.rs` | `blocks_to_content`, `now_millis` |
| `capabilities.rs` | `model_match_candidates`, `supports_native_xhigh_effort` |
| `config.rs` | `SKIP_AUTH_SECRET_KEY`, `arn_region`, `parse_ini_profile`, `should_use_explicit_bedrock_endpoint` |
| `convert.rs` | `cache_point`, `convert_tool_result_content`, `create_image_block`, `non_blank_text_block`, `required_text_block` |
| `driver.rs` | `EVENT_STREAM_MEDIA_TYPE` |
| `events.rs` | `handle_content_block_start`, `handle_content_block_delta`, `handle_content_block_stop`, `handle_metadata` |
| `framing.rs` | `MAX_EVENT_FRAME_BYTES`, `be_u32` |
| `headers.rs` | `RESERVED_HEADER_EXACT` |
| `params.rs` | `build_additional_model_request_fields`, `default_thinking_budget` |
| `sigv4.rs` | `SIGV4_SERVICE`, `civil_from_days` |

Nothing else changes: not the item bodies, not the struct fields (those genuinely cross module
lines), not the inherent methods, not the `use` blocks.

Re-derive the list rather than trusting this table if the tree has moved on since:

```bash
cd crates/cyrup-provider/src/api/bedrock_converse_stream
python3 - <<'PY'
import re, glob, collections
files = sorted(glob.glob('*.rs')) + sorted(glob.glob('tests/*.rs'))
texts = {f: open(f).read() for f in files}
decl = re.compile(r'^\s*pub\(super\)\s+(?:async\s+)?(?:fn|struct|enum|const|type)\s+([A-Za-z_]\w*)')
owners = {}
for f, t in texts.items():
    for ln in t.split('\n'):
        m = decl.match(ln)
        if m: owners.setdefault(m.group(1), f)
for name, own in sorted(owners.items()):
    if not any(re.search(r'\b'+re.escape(name)+r'\b', t) for f, t in texts.items() if f != own):
        print(f"{own:<18} {name}")
PY
```

**Watch for a genuine discovery.** A `pub(super)` item that is never used raises no lint; a *private*
one raises `dead_code`. If privatizing any of these 25 produces a `dead_code` warning, that item is
actually unreachable — report it rather than re-widening it to silence the warning.

## Definition of done

- [ ] The 25 declarations above are private; no other line in the tree changes
- [ ] `grep -c 'pub(super)'` over the tree drops by exactly 25
- [ ] `cargo build -p cyrup-provider --all-targets` clean
- [ ] `cargo clippy -p cyrup-provider --all-targets` — still 37 warnings, still exactly one inside
      this module (the pre-existing `result_large_err` on `run_inner`)
- [ ] `cargo doc -p cyrup-provider --no-deps` — still 76 warnings, still none in this module
- [ ] `cargo test -p cyrup-provider --lib` — 1118 pass, 57 of them `api::bedrock_converse_stream`
- [ ] Any `dead_code` warning surfaced by the tightening is reported, not suppressed
