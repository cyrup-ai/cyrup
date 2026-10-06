# CODE-014 capture — pi v1.0.0's own prompt-sections and system-row code

`run.sh` runs pi's real functions at the tag `v1.0.0` (commit
`a13d35a742c6ef8462812a28fbe1d8c8b7431c32`) and writes
`../../fixtures/pi/code014-prompt-sections.pi-captured.json`:

| key | produced by | pi source |
|---|---|---|
| `sections`, `sectionsCustom`, `sectionsNone` | `buildSystemPromptSections` | `core/system-prompt.ts:121-179` |
| `diffs.*` | `diffSystemPromptSections` | `core/system-prompt.ts:204-217` |
| `loopPromptAndTools`, `loopToolsOnly`, `loopSwap` | `runAgentLoop` → `declareToolChanges` → `withToolChanges`; the `message_end` of each system message, `JSON.stringify`'d | `packages/agent/src/agent-loop.ts:327-376` |
| `compactionSnapshot` | `getCurrentSystemMessage`, spread with `timestamp` overwritten, as `appendCompaction` writes it | `packages/ai/src/utils/transcript.ts:77-101`, `core/session-manager.ts:1270-1283` |

Nothing is hand-derived: the strings in the fixture are what pi's code printed. `Date.now` is frozen so
that a row the loop stamps itself is reproducible.

Three files that are not under test are stubbed (`stubs/`), and the `@earendil-works/pi-ai` import is
redirected to the three real modules the code under test uses (`shim/`); see `run.sh` for why.
