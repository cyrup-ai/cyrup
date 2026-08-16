---
stage: new
status: done
updated: 2026-08-15 21:16
---

# FLUX_08 — `/flux/cheatsheet` + `/flux/about` native renderers

## OBJECTIVE

Add the two remaining static renderers to `crates/cyrup-ext-flux`, completing the trio of
`exec:`-replacement commands (spec [§3.4.3](../flux.md)). Both are single-source-of-truth
renderers: the cheatsheet parses `pipeline.md`, about renders the about body — exactly like
their Python originals.

## SUBTASKS

### SUBTASK 1: Vendor the source-of-truth content into the crate

```
crates/cyrup-ext-flux/resources/prompts/flux/_docs/
├── pipeline.md     # parsed by render_cheatsheet.rs
├── README.md       # completeness (FLUX_11 ships the whole dir)
├── cheatsheet.md
├── synopsis.md
└── about.md        # parsed by render_about.rs (crate-internal asset; _docs never registers)
```

Copy the four `_docs` files byte-identical from the package
(`/Users/davidmaple/cyrup.ai/cyrup-flux/prompts/flux/_docs/`, itself byte-identical to
[code-puppy](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/_docs/) — FLUX_01).
Copy `about.md`'s BODY (frontmatter and the AI-only preamble stripped, per the Python
transform below) from
[`../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/about.md`](../../tmp/code-puppy/flux_bootstrap/bundled/commands/flux/about.md)
into `_docs/about.md`. The `_`-prefix means the resource scanner never registers these
(spec §0.1), so co-locating is safe both now and after FLUX_11.

### SUBTASK 2: `render_cheatsheet.rs`

Port [`flux_cheatsheet.py`](../../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_cheatsheet.py):

- Embed the doc: `const PIPELINE_MD: &str = include_str!("../resources/prompts/flux/_docs/pipeline.md");`
- Reimplement the Python's two parses (nothing hardcoded but presentation — the doc is the
  single source of truth): the PIPELINE A–D section blocks (headers, their step lists) and the
  Available Commands table.
- Normalize every `//flux` → `/flux` in rendered output, as the Python does.
- Optional positional arg `A|B|C|D` renders one pipeline; no arg renders all stacked; anything
  else → Error notify + `Ok(None)` (spec §0.2 output-channel contract).
- No ANSI (spec §5.8): keep the `𝕱` header, rules, and layout.

### SUBTASK 3: `render_about.rs`

Port [`flux_about.py`](../../tmp/code-puppy/flux_bootstrap/bundled/scripts/flux_about.py):

- `const ABOUT_MD: &str = include_str!("../resources/prompts/flux/_docs/about.md");`
- The Python strips frontmatter + the AI-only preamble and normalizes `//cmd` → `/cmd` before
  handing to Rich. SUBTASK 1 already stripped frontmatter/preamble at vendor time, so the Rust
  side applies only the `//` → `/` normalization and returns the text (no Rich equivalent —
  the glyph/table layout is already terminal-friendly; the notification channel shows it
  as-is, spec §3.4.3).
- No args; always `Ok(Some(text))`.

### SUBTASK 4: Register + route

In `extension.rs` (added to the FLUX_07 `init` and `execute_command`):

```rust
api.register_command("flux/cheatsheet", cmd("Flux pipeline cheatsheet (stages A–D)"));
api.register_command("flux/about", cmd("About the Flux pipeline"));
// execute_command:
"flux/cheatsheet" => Ok(Some(crate::render_cheatsheet::render(args))),
"flux/about" => Ok(Some(crate::render_about::render())),
```

### SUBTASK 5: Build + behavioral check

```bash
cargo build -p cyrup-ext-flux && cargo build -p cyrup
```

In the TUI: `/flux/cheatsheet` prints all four pipelines with the command table and no `//flux`
strings anywhere (`rg '//' ` on the output mentally — the Python normalizes); `/flux/cheatsheet B`
prints only PIPELINE B; `/flux/cheatsheet Z` → Error notify; `/flux/about` prints the overview
with the 9-step pipeline line and both command tables.

## RESEARCH NOTES

- Why parse `pipeline.md` instead of hardcoding: the Python's docstring is explicit — "SINGLE
  SOURCE OF TRUTH: the pipeline definitions are parsed at runtime from `pipeline.md` … Editing
  that doc changes this output automatically." Preserve that property.
- `include_str!` makes the crate self-contained; FLUX_11's `ResourcesDiscover` contribution
  exposes the same files to the resource system — the two consumers (renderer via
  `include_str!`, resource registry via directory scan) read one on-disk file.
- The about body keeps its `/flux/…` references (already single-slash upstream apart from the
  `//cmd` cases the transform fixes).

## DEFINITION OF DONE

- [ ] Crate + binary build cleanly.
- [ ] `/flux/cheatsheet` (all / A / B / C / D / invalid) behaves as above with zero `//flux`
      strings in output.
- [ ] `/flux/about` prints the full overview body.
- [ ] Both commands appear in the TUI command list; `/flux/status` from FLUX_07 unaffected.

No tests to be written. No benchmarks to be written.
