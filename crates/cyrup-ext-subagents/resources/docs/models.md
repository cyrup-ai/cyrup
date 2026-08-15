# Models

## How a child's model is chosen

Five tiers, highest first. The first tier that supplies a value wins.

1. **Inline per-call override** — `model=` on `/run`, or the `model` tool parameter.
2. **`subagents.overrides.<agent>.model` / `subagents.defaultModel`** from the layered
   `settings.json` pair (project beats user).
3. **`config.json`** — the extension's own per-installation knobs.
4. **The agent's own frontmatter** `model:` key.
5. **The hardcoded extension default.**

`/subagents-models` shows what each builtin agent actually resolves to, and
`/subagents-models <agent>` narrows it to one. Both views report the active model-scope policy
alongside the resolved model, so a model that was accepted under a warning is visible as such.

## Fallback ladders

`fallbackModels` in an agent's frontmatter lists models to try when the primary is unavailable. The
ladder is walked in order and the run records which rung it landed on, so a run that quietly
degraded to a cheaper model is visible in its result rather than only in the provider bill.

## Model scope

`subagents.modelScope` in `settings.json` is an allowlist policy:

```json
{
  "subagents": {
    "modelScope": {
      "enforce": true,
      "strict": true,
      "allow": ["anthropic/*"]
    }
  }
}
```

- `allow` is a list of glob patterns a model id must match.
- `enforce` turns the policy on.
- `strict` decides what happens to a model that came from somewhere other than an explicit call
  override. Without it, an **explicit** out-of-scope model is a hard error while an **inherited** or
  **fallback** one only warns. With `strict: true`, all three are hard errors.

A malformed `modelScope` block aborts discovery rather than being silently ignored — an
unenforceable policy that looks enforced is worse than no policy.

## Thinking

`thinking` selects the child's extended-thinking level: `off`, `minimal`, `low`, `medium`, `high`,
`xhigh`, `max`, or `inherit`. It comes from the same five-tier chain, and
`subagents.disableThinking: true` forces it off for every agent unless a same-scope override
explicitly re-sets it.

## Provider catalogs

`/subagents-refresh-provider-models <provider> [--force]` refreshes the cached model list for a
provider. `/subagents-check-profile <name>` verifies that a saved profile's models are still
resolvable against those catalogs, which is what catches a profile that points at a retired model
before a run does.
