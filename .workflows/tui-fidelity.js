export const meta = {
  name: 'cyrup-tui-presentation-fidelity',
  description: 'Read-only audit: every spacing, colour, theming and presentation difference between cyrup-tui and pi',
  phases: [
    { title: 'Map', detail: 'component + theme-token correspondence, and what has no counterpart' },
    { title: 'Audit', detail: 'seven surfaces in parallel — concrete literals, both sides' },
    { title: 'Verify', detail: 'adversarial: re-read both sides, kill anything not real' },
    { title: 'Synthesize', detail: 'ranked, actionable presentation backlog' },
  ],
}

const WS = '/home/d0m17bw/workspace'

const COMMON = `
You are auditing the PRESENTATION FIDELITY of \`cyrup-tui\` (${WS}/cyrup/crates/cyrup-tui) against pi
(${WS}/pi). Ported baseline **v0.83.0**, target **v0.84.1**.

pi's renderable surface:
  - \`packages/tui/src/components/\`                      — 17 layout/text primitives
  - \`packages/coding-agent/src/modes/interactive/\`       — 42 application components
  - pi's theme/colour definitions (find them; \`packages/coding-agent/src/modes/interactive/theme/\`
    and \`packages/tui/src/\` are the places to start)
cyrup's: \`crates/cyrup-tui/src/*.rs\` (44 modules).

## WHAT THIS AUDIT IS FOR

cyrup's TUI looks less polished than pi's — odd spacing, colours that do not match, general
presentation drift. Nobody has ever audited this. The 147-item parity backlog CANNOT contain these
findings, for two structural reasons: it was a DIFF of v0.83.0..v0.84.1, so anything that has been
wrong since the original port is invisible to it; and its TUI survey was explicitly told to skip
"renderer internals", which likely swallowed padding and alignment too.

So you are not diffing versions. You are comparing what pi DRAWS to what cyrup DRAWS, today.

## MECHANISM VS OUTPUT — the one distinction that matters

pi hand-rolls a renderer where every component is \`render(width: number): string[]\` — a pure
function from width to lines. cyrup delegates to ratatui + crossterm. **That difference is
expected and is NOT a finding.** What IS a finding is when the OUTPUT differs:

  REPORT: a padding of 1 vs 2 · a gap of 0 vs 1 · a different border glyph · a blank line present
  on one side and absent on the other · a different truncation/ellipsis rule · left vs centre
  alignment · a different colour token for the same element · bold where pi is dim · a prefix
  string like "⏺ " vs "* " · different indentation of wrapped lines · a different separator ·
  spacing around a spinner · colour that ignores the theme · a hardcoded colour where pi uses a
  token.

  DO NOT REPORT: that cyrup uses \`Constraint::Length\` where pi computes widths by hand · that
  ratatui owns the buffer · that cyrup uses \`Style\` and pi uses ANSI escapes · any difference with
  no observable effect on the rendered characters or their colours.

## HOW TO BE CONCRETE

Every finding MUST carry, on BOTH sides, the \`file:line\` and the ACTUAL LITERAL:

  BAD:  "the footer spacing differs"
  GOOD: "footer segment separator — pi \`footer.ts:88\` joins with \`\" · \"\` (space-middot-space);
         cyrup \`status.rs:243\` joins with \`\" | \"\`. Visible on every frame."

If you cannot quote the literal from both sides, you have not verified it — say so or drop it.

## HARD RULES

- **READ BOTH SIDES. Never infer from a name.** Briefs in this project have been wrong repeatedly —
  a cited line that contained nothing of the sort, an item filed under the wrong crate, a whole
  batch planned on a false premise. If this brief disagrees with the source, FOLLOW THE SOURCE.
- **Verify citations** with \`git -C ${WS}/pi show v0.84.1:packages/<path>\` and COUNT the lines.
- \`spec/\` and \`ADR-0001\` do not exist here and are never authority for a difference.
- **This audit is READ-ONLY. Do not edit a single file.** Another workflow is editing
  \`crates/cyrup-tui/\` right now — in particular \`status.rs\` — so if something there looks
  half-written, note it and move on rather than "fixing" it.
- A missing component is a finding too: if pi draws something cyrup has no counterpart for, say so.
`

const FINDING_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['surface', 'findings'],
  properties: {
    surface: { type: 'string' },
    findings: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['element', 'pi_behaviour', 'cyrup_behaviour', 'visible_effect', 'kind'],
        properties: {
          element: { type: 'string', description: 'The specific UI element' },
          kind: {
            type: 'string',
            enum: ['spacing', 'colour', 'glyph', 'alignment', 'truncation', 'border', 'typography', 'missing-element', 'extra-element', 'other'],
          },
          pi_behaviour: { type: 'string', description: 'file:line + the ACTUAL LITERAL' },
          cyrup_behaviour: { type: 'string', description: 'file:line + the ACTUAL LITERAL' },
          visible_effect: { type: 'string', description: 'What the user SEES differently, and how often' },
          severity: { type: 'string', enum: ['high', 'medium', 'low'], description: 'high = visible on every frame or clearly looks broken' },
          effort: { type: 'string', enum: ['trivial', 'small', 'medium', 'large'] },
        },
      },
    },
    unmapped: { type: 'string', description: 'pi components with no cyrup counterpart, and vice versa' },
    notes: { type: 'string' },
  },
}

phase('Map')

const MAP_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['component_map', 'theme_map'],
  properties: {
    component_map: { type: 'string', description: 'pi file -> cyrup file, for all 59 pi renderables' },
    theme_map: { type: 'string', description: 'pi theme token -> cyrup equivalent, and where each is defined' },
    pi_only: { type: 'array', items: { type: 'string' } },
    cyrup_only: { type: 'array', items: { type: 'string' } },
    render_entry_points: { type: 'string', description: 'Where cyrup actually draws: the widget/draw fns an auditor should read' },
    notes: { type: 'string' },
  },
}

const map = await agent(
  `${COMMON}

## Your job: build the map the auditors will work from. Read-only.

1. **Component correspondence.** For each of pi's 17 \`packages/tui/src/components/*.ts\` and 42
   \`packages/coding-agent/src/modes/interactive/components/*.ts\`, name the cyrup module that draws
   the same thing. Names are NOT reliable: cyrup's \`status.rs\` may be pi's \`footer.ts\`, cyrup may
   fold several pi components into one module or split one across several. Confirm by reading what
   each actually renders.
2. **Theme / colour tokens.** Find pi's theme definition and enumerate its tokens (names + the
   colour each resolves to, per theme). Then find cyrup's (\`theme.rs\` and wherever \`Style\`/\`Color\`
   are constructed) and map token to token. Note any cyrup colour that is HARDCODED where pi uses a
   token — that is the root cause of "colours do not match" and the auditors need to know where to
   look.
3. **Render entry points.** List the cyrup functions that actually emit cells (the \`draw\`/\`render\`
   fns and the widgets they build), so an auditor reads the drawing code and not the state code.
4. **Unmapped both ways.**

Be exhaustive and concrete; everything downstream depends on this being right.`,
  { label: 'map:components-and-theme', phase: 'Map', schema: MAP_SCHEMA, effort: 'high' }
)

const mapDigest = map
  ? `### Component map
${map.component_map}

### Theme / colour token map
${map.theme_map}

### cyrup render entry points
${map.render_entry_points || '(none given)'}

### pi-only: ${(map.pi_only || []).join(', ') || '(none)'}
### cyrup-only: ${(map.cyrup_only || []).join(', ') || '(none)'}

### Map notes
${map.notes || '(none)'}`
  : '(the map phase returned nothing — build your own correspondence before auditing, and say so)'

log('Map complete')

phase('Audit')

const SURFACES = [
  {
    key: 'theme',
    title: 'Theme, colour and styling — the highest-value surface',
    brief: `Audit COLOUR and STYLE end to end. This is most likely the biggest contributor to
"looks less pro".

  - Enumerate pi's theme tokens and what each resolves to, per built-in theme. Do the same for
    cyrup. Report every token whose colour differs, and every token pi has that cyrup lacks.
  - Find every HARDCODED colour in \`crates/cyrup-tui/src/*.rs\` (\`Color::Rgb\`, \`Color::Red\`,
    named constants, raw ANSI) and check whether pi uses a THEME TOKEN for that same element. A
    hardcoded colour is a bug even when it looks similar, because it ignores the user's theme.
  - Compare STYLE attributes for the same element: bold / dim / italic / underline / reversed.
    pi leans on dim heavily for secondary text; check whether cyrup does too, or uses a dark colour
    instead (which reads differently on light backgrounds).
  - Check the light-theme story specifically: pi picks different colours for light backgrounds.
    Does cyrup? If cyrup ships one palette that assumes dark, that is a high-severity finding.
  - Check how each side handles a terminal WITHOUT truecolor (256-colour or 16-colour fallback).`,
  },
  {
    key: 'layout',
    title: 'Layout primitives — padding, gaps, margins, borders',
    brief: `Audit pi's \`box.ts\`, \`h-stack.ts\`, \`v-stack.ts\`, \`stack.ts\`, \`spacer.ts\`,
\`scroll-view.ts\`, \`dynamic-border.ts\` against cyrup's layout code.

  - Extract every numeric constant: padding, margin, gap, indent, border width, min/max widths.
    Compare them one by one. An off-by-one padding repeated across a screen is exactly the kind of
    thing that reads as "unpolished".
  - Compare BORDER GLYPHS character by character (corners, edges, junctions) and which elements get
    a border at all.
  - Compare how each side spends leftover width when a row does not divide evenly, and how each
    handles a width too small for its content.
  - Compare vertical rhythm: blank lines between blocks. Report any place pi emits a blank line and
    cyrup does not, or vice versa.`,
  },
  {
    key: 'transcript',
    title: 'Transcript: assistant messages, tool calls, diffs, bash output',
    brief: `The screen a user stares at all day. Audit pi's \`assistant-message.ts\`,
\`bash-execution.ts\`, \`diff.ts\`, \`custom-message.ts\`, \`custom-entry.ts\`,
\`compaction-summary-message.ts\`, \`branch-summary-message.ts\` against cyrup's \`transcript.rs\`,
\`diff.rs\`, \`bash.rs\` and the message rendering in \`app.rs\`.

  - Message PREFIXES and markers: quote the exact glyph and trailing spaces on both sides.
  - INDENTATION of message bodies and of WRAPPED continuation lines.
  - Blank lines between messages, and around tool calls.
  - Tool call header format: name, args, the collapsed/expanded indicator, the expand hint text.
  - DIFF rendering: +/- markers, gutter width, line numbers, colours for added/removed/context,
    how an intra-line change is highlighted.
  - Bash output: truncation rule, the "N lines hidden" text, exit-code display.`,
  },
  {
    key: 'chrome',
    title: 'Footer, status line, loaders, spinners, hints',
    brief: `Audit pi's \`footer.ts\`, \`keybinding-hints.ts\`, \`loader.ts\`, \`cancellable-loader.ts\`,
\`bordered-loader.ts\`, \`countdown-timer.ts\` against cyrup's \`status.rs\`, \`status_indicator.rs\`,
\`footer_data.rs\`, \`chrome.rs\`, \`resume_hint.rs\`.

  - The footer is on screen every frame, so every difference here is high severity. Compare: segment
    ORDER, the SEPARATOR literal, padding at each end, alignment, what is shown vs hidden at narrow
    widths, and the exact format of each segment (model name, token counts, cost, elapsed time,
    branch).
  - SPINNER: compare the frame sequence character by character and the frame interval in ms. A
    different spinner or cadence is instantly noticeable.
  - Keybinding hints: exact text, key formatting (\`^C\` vs \`Ctrl+C\`), separators, when shown.`,
  },
  {
    key: 'lists',
    title: 'Selectors, lists and overlays',
    brief: `Audit pi's \`select-list.ts\`, \`settings-list.ts\`, \`model-selector.ts\`,
\`config-selector.ts\`, \`session-selector.ts\`, \`session-selector-search.ts\`, \`oauth-selector.ts\`,
\`scoped-models-selector.ts\`, \`extension-selector.ts\` against cyrup's \`select_list.rs\`,
\`selector.rs\`, \`*_selector.rs\`, \`settings_selector.rs\`, \`session_search.rs\`, \`overlay.rs\`.

  - SELECTION INDICATOR: the exact glyph/prefix and whether the row is also styled (reverse video,
    bold, background colour). Quote both.
  - Row padding, gap between columns, how a long label truncates and with what ellipsis character.
  - Scroll indicators: what pi shows when a list overflows, and where.
  - Overlay geometry: width (fixed? percentage?), max height, anchor position, margin, and whether
    there is a shadow/dim behind it.
  - Search/filter: prompt glyph, match highlighting style, empty-result text.`,
  },
  {
    key: 'editor',
    title: 'Editor, input, autocomplete',
    brief: `Audit pi's \`editor.ts\`, \`input.ts\`, \`custom-editor.ts\`, \`extension-editor.ts\`,
\`extension-input.ts\` against cyrup's \`editor.rs\`, \`text_input.rs\`, \`extension_editor.rs\`,
\`autocomplete.rs\`.

  - PROMPT GLYPH and its trailing spacing, quoted exactly.
  - Placeholder text and its style.
  - Cursor rendering, and how the cursor looks on an empty line.
  - Soft-wrap: continuation indent, and whether a wrapped line is marked.
  - Multi-line: how line count changes the box height, and any max height.
  - Autocomplete popup: position, width, row format, selection style, max visible rows.`,
  },
  {
    key: 'markdown',
    title: 'Markdown and syntax rendering',
    brief: `Audit pi's \`markdown.ts\` (\`packages/tui/src/components/\`), \`markdown-transform.ts\` and
\`text.ts\`/\`truncated-text.ts\` against cyrup's \`markdown.rs\` and \`ansi.rs\`.

  - HEADINGS: styling per level, and any prefix/underline.
  - LISTS: bullet glyphs per nesting level, indent per level, ordered-list formatting.
  - CODE BLOCKS: border/background, language label, padding, and the syntax colours themselves
    (cyrup uses syntect — compare its theme against pi's highlighting).
  - INLINE code, bold, italic, links: exact styling.
  - BLOCKQUOTES: prefix glyph and indent.
  - Horizontal rules, tables if supported.
  - Text wrapping: word vs character, and how each handles a very long unbroken token.`,
  },
]

const audits = await parallel(
  SURFACES.map((s) => () =>
    agent(
      `${COMMON}

## The map, built for this audit — use it, and correct it if it is wrong

${mapDigest}

## Your surface: ${s.title}

${s.brief}

Read BOTH sides for every claim. Quote the literal from each. Prefer 15 well-evidenced findings over
50 vague ones — but do not silently cap: if you drop findings, say how many and why in \`notes\`.`,
      { label: `audit:${s.key}`, phase: 'Audit', schema: FINDING_SCHEMA, effort: 'high' }
    )
  )
)

const okAudits = audits.filter(Boolean)
const allFindings = okAudits.flatMap((r) => (r.findings || []).map((f) => ({ ...f, surface: r.surface })))
log(`Audit: ${allFindings.length} raw findings across ${okAudits.length} surfaces`)

phase('Verify')

const VERIFY_SCHEMA = {
  type: 'object',
  additionalProperties: false,
  required: ['verdicts'],
  properties: {
    verdicts: {
      type: 'array',
      items: {
        type: 'object',
        additionalProperties: false,
        required: ['id', 'real', 'reason'],
        properties: {
          id: { type: 'string' },
          real: { type: 'boolean' },
          reason: { type: 'string', description: 'What you found when you re-read BOTH sides' },
          corrected: { type: 'string', description: 'If the finding is right but its detail is wrong, the correction' },
        },
      },
    },
    missed: { type: 'string', description: 'Anything obviously wrong in your half that no finding mentions' },
  },
}

const numbered = allFindings.map((f, i) => `F${i + 1}. [${f.surface}/${f.kind}] ${f.element}
    PI:    ${f.pi_behaviour}
    CYRUP: ${f.cyrup_behaviour}
    EFFECT: ${f.visible_effect}`)

const half = Math.ceil(numbered.length / 2)
const chunks = [numbered.slice(0, half), numbered.slice(half)]

const verdicts = allFindings.length === 0 ? [] : await parallel(
  chunks.map((chunk, idx) => () =>
    agent(
      `${COMMON}

## Your job: verify findings. Assume each is WRONG until you have re-read both sides yourself.

For each finding below, open pi's file at v0.84.1 AND cyrup's file, find the cited lines, and check
the quoted literals actually say what is claimed. Set \`real: false\` when:
  - a quoted literal is not there, or does not read as claimed;
  - the "difference" is mechanism-only with no effect on rendered characters or colours;
  - cyrup actually already matches pi and the finding misread one side;
  - the cited cyrup code is dead (nothing reaches it), so it cannot affect what a user sees.
When a finding is REAL but a detail is wrong (line number, literal, severity), set \`real: true\` and
put the correction in \`corrected\`.

Then, in \`missed\`: having read this code closely, name anything clearly wrong that NO finding
mentions.

## Findings to verify (batch ${idx + 1} of 2)

${chunk.join('\n\n')}`,
      { label: `verify:${idx + 1}`, phase: 'Verify', schema: VERIFY_SCHEMA, effort: 'high' }
    )
  )
)

const verdictById = {}
for (const v of verdicts.filter(Boolean)) {
  for (const x of v.verdicts || []) verdictById[x.id] = x
}
const confirmed = allFindings
  .map((f, i) => ({ ...f, id: `F${i + 1}`, verdict: verdictById[`F${i + 1}`] }))
  .filter((f) => !f.verdict || f.verdict.real !== false)
const killed = allFindings
  .map((f, i) => ({ ...f, id: `F${i + 1}`, verdict: verdictById[`F${i + 1}`] }))
  .filter((f) => f.verdict && f.verdict.real === false)

log(`Verify: ${confirmed.length} confirmed, ${killed.length} killed`)

phase('Synthesize')

const fmt = (f) => `${f.id} [${f.kind}/${f.severity || '?'}/${f.effort || '?'}] ${f.element}
    PI:     ${f.pi_behaviour}
    CYRUP:  ${f.cyrup_behaviour}
    EFFECT: ${f.visible_effect}${f.verdict?.corrected ? `\n    CORRECTION: ${f.verdict.corrected}` : ''}`

const report = await agent(
  `${COMMON}

## Your job: write the TUI presentation-fidelity backlog. Markdown, for a maintainer to work from.

Structure:
1. **Headline** — how far cyrup's presentation is from pi's, in concrete terms. Lead with the single
   change that would most improve perceived polish, and say why.
2. **Theme and colour** — its own section. If cyrup hardcodes colours where pi uses tokens, or ships
   one palette where pi adapts to light/dark, say so first; that is systemic, not cosmetic.
3. **Per-surface tables** — layout, transcript, chrome, lists, editor, markdown. Columns:
   element | pi | cyrup | visible effect | severity | effort. Order by severity then effort.
4. **Quick wins** — every \`trivial\`/\`small\` item, as a single ordered checklist. These are constants
   and literals; landing them together is one focused change.
5. **Systemic issues** — anything that is one root cause behind many symptoms (a wrong shared
   constant, a missing theme token, a layout helper that pads differently). Fixing one of these
   fixes a column of the tables at once; name which findings each subsumes.
6. **Missing elements** — pi components with no cyrup counterpart, sized.
7. **Recommended sequence** — batches that can each land and be verified independently.
8. **Killed claims** — findings the verifier disproved, one line each, so nobody re-files them.

Rules: every row needs both-sides evidence; never describe a difference as intentional; do not cite
\`spec/\` or \`ADR-0001\`. Merge duplicates across surfaces into one row naming both.

## CONFIRMED (${confirmed.length})

${confirmed.map(fmt).join('\n\n')}

## KILLED (${killed.length})

${killed.map((f) => `${f.id} ${f.element} — ${f.verdict?.reason || ''}`).join('\n') || '(none)'}

## WHAT THE VERIFIERS SAID WAS MISSED

${verdicts.filter(Boolean).map((v) => v.missed || '').filter(Boolean).join('\n\n') || '(nothing)'}

## UNMAPPED COMPONENTS

${okAudits.map((r) => `[${r.surface}] ${r.unmapped || '(none)'}`).join('\n')}

## SURFACE NOTES

${okAudits.map((r) => `[${r.surface}] ${r.notes || '(none)'}`).join('\n')}`,
  { label: 'synthesize', phase: 'Synthesize', effort: 'high' }
)

return {
  raw_findings: allFindings.length,
  confirmed: confirmed.length,
  killed: killed.length,
  by_severity: {
    high: confirmed.filter((f) => f.severity === 'high').length,
    medium: confirmed.filter((f) => f.severity === 'medium').length,
    low: confirmed.filter((f) => f.severity === 'low').length,
  },
  quick_wins: confirmed.filter((f) => f.effort === 'trivial' || f.effort === 'small').length,
  report,
}
