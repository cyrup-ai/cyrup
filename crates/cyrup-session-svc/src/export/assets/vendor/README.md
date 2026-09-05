# Vendored libraries — HTML session export

These two files are copied **byte-for-byte** from pi `v0.84.4`
(`packages/coding-agent/src/core/export-html/vendor/`) and are embedded verbatim into every
exported transcript by `crates/cyrup-session-svc/src/export/mod.rs`, exactly as pi's
`generateHtml` does (`export-html/index.ts:148-149`, `:173-174`). They are the two runtime
dependencies of the shipped `assets/template.js`: `marked.parse` renders assistant markdown
(`template.js:1557-1641`) and `hljs.highlight` colours code blocks (`template.js:857`, `:866`,
`:1616-1630`). An export must open with no network, so they are inlined rather than linked.

They are **not** compiled, executed or otherwise reachable from cyrup itself — they are opaque
payload bytes in the generated document, run only by the browser a user opens the export in.

| file | library | version | licence |
|---|---|---|---|
| `marked.min.js` | [marked](https://github.com/markedjs/marked) | 18.0.5 | MIT — © 2018-2026 MarkedJS; © 2011-2018 Christopher Jeffrey |
| `highlight.min.js` | [highlight.js](https://github.com/highlightjs/highlight.js) | 11.9.0 (git `f47103d4f1`) | BSD-3-Clause — © 2006-2023 Ivan Sagalaev and contributors |

Each file carries its own licence header as its first bytes; both headers survive into the
exported document, which is how the attribution requirement of both licences is met for anyone the
transcript is handed to.

**Do not edit these files.** `crates/cyrup-session-svc/src/tests/export_html.rs` pins the SHA-256
of every asset in this directory and its parent, so an edit fails the suite rather than silently
forking the copies from upstream. To take a new upstream revision, re-copy with
`git -C tmp/pi show <tag>:packages/coding-agent/src/core/export-html/vendor/<file>` and update the
pinned digests in the same commit.
