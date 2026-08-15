# Models and thinking

cyrup talks to thirty-five providers through one set of flags. This page covers picking a model on
the command line, switching mid-session, and setting how hard the model thinks before it answers.

If you have not connected a provider yet, do that first —
[Connect a provider](../getting-started/authenticate.md).

## Picking a model

```sh
cyrup --model openai/gpt-4o "explain this repo"
```

`--model` takes four shapes, and you can mix them:

- **`provider/id`** — `openai/gpt-4o`. The prefix is matched case-insensitively against the known
  provider ids; if it matches, it selects the provider and the rest is the model pattern.
- **A bare id** — `gpt-4o`. Matched exactly across every provider first. If more than one provider
  carries the same id, cyrup falls through to partial matching rather than erroring.
- **A partial name** — `sonnet`, `opus-4-6`. Any substring that identifies a model.
- **A `:level` suffix** — `sonnet:high`, `anthropic/claude-opus-4-6:max`. The token is split on its
  last colon and the tail must be one of the seven thinking levels; anything else is treated as part
  of the model id, and resolution fails.

`--provider` selects the provider separately, so `--provider openai --model gpt-4o` is equivalent to
the prefixed form. A redundant prefix (`--provider openai --model openai/gpt-4o`) is stripped rather
than rejected.

`--model` does not accept globs. Wildcards are a `--models` feature and nothing else — see
[The Ctrl+P cycling set](#the-ctrlp-cycling-set).

### When the model is not in the catalog

An unknown provider is a hard error that lists every provider id cyrup knows.

An unknown *model* on a *known* provider is not. cyrup synthesises a custom model id, warns
`Model "<pattern>" not found for provider "<provider>". Using custom model id.`, and sends the
request anyway. That is the supported way to use a model your build's catalog has not heard of yet:

```sh
cyrup --model anthropic/claude-opus-4-9 "..."
```

The synthesised model carries no declared capabilities, so features that depend on a model
declaration — notably the `xhigh` and `max` thinking levels — are unavailable on it.

## Seeing what you can use

```sh
cyrup --list-models sonnet
```

Prints an aligned table sorted by provider then model id:

```text
provider   model                       context  max-out  thinking  images
anthropic  claude-sonnet-4-5           1M       64K      yes       yes
anthropic  claude-sonnet-4-6           1M       128K     yes       yes
anthropic  claude-sonnet-5             1M       128K     yes       yes
```

`thinking` is whether the model reasons at all; `images` is whether it accepts image input. With no
argument the whole list prints. With an argument you get a fuzzy filter: the query is split on
whitespace and `/` and every token must match, so `--list-models anthropic/sonnet` and
`--list-models anthropic sonnet` are the same search.

**`--list-models` shows only providers you have authenticated.** It is a list of what you can use
right now, not a catalog of what exists. An empty result usually means no credentials, not no
models.

## Switching models in a session

`/model` opens a picker with fuzzy search, an `all | scoped` toggle, and a check mark on the active
model. `/model anthropic/claude-sonnet-5` jumps straight there.

`Ctrl+P` cycles forward through your cycling set and `Ctrl+Shift+P` cycles backward, wrapping at
both ends. With no cycling set configured, they cycle every model whose provider is authenticated.

The model survives the switch: the session keeps its transcript and re-clamps the thinking level to
whatever the new model supports.

## The Ctrl+P cycling set

`--models` narrows what `Ctrl+P` walks through. It takes a comma-separated list, and each entry may
be a literal reference or a glob:

```sh
cyrup --models "anthropic/*:high,openai/gpt-5*,*sonnet*"
```

Globbing is minimatch-style and case-insensitive:

- `*`, `?` and `[...]` match within one path segment — they do not cross `/`.
- `**` is a globstar and does cross `/`.
- `{a,b}` and `{1..3}` brace expansion works.
- Each pattern is matched against both `provider/id` and the bare `id`, so `*sonnet*` catches
  `claude-sonnet-5` under any provider.
- A `:level` suffix works on a glob: `anthropic/*:high` scopes the whole provider at high thinking.

An exact reference match is tried before globbing, so a literal id containing `[` or `?` still
resolves. Results keep the order you wrote them and are de-duplicated by provider and id.

Patterns that match nothing warn (`No models match pattern "..."`) and the run continues with
whatever did match. A bad `:level` on a non-glob pattern warns too, and the model is still selected
at the default level.

`/scoped-models` opens the same set as a checkbox list over the full catalog, with a search box at
the top. `Enter` toggles the highlighted model, `Alt+Up` and `Alt+Down` reorder it within the cycle,
`Ctrl+P` toggles every model of its provider, `Ctrl+A` enables everything, `Ctrl+X` clears, and
`Ctrl+S` saves and closes. (On macOS `Alt` is the Option key.) What you save applies to the running
session; it is not written to disk. To make a cycling set stick, put it in `enabledModels` in
`~/.cyrup/agent/settings.json`:

```json
{
  "enabledModels": ["anthropic/claude-sonnet-5", "openai/gpt-5.5"]
}
```

`--models` overrides `enabledModels` for that run. An unset `enabledModels` means every model; an
explicit `[]` means none.

## Which model you get when you ask for nothing

With no `--model`, cyrup works down this ladder and takes the first rung that produces a model:

1. `--provider` and `--model` together.
2. The first entry of `--models` (skipped when you are resuming with `--continue` or `--resume` —
   a resumed session keeps its own model).
3. `defaultProvider` + `defaultModel` from `settings.json`, but only if that provider has
   credentials configured.
4. A curated default model per provider, walking the built-in provider list in a fixed order and
   taking the first whose default is available to you.
5. Nothing — cyrup launches modelless, and you pick a model with `/model` or connect a provider with
   `/login`.

The `--help` output says `--provider` defaults to `google`. It does not; there is no default
provider in cyrup, and the ladder above is what actually runs.

To pin your own default:

```json
{
  "defaultProvider": "anthropic",
  "defaultModel": "claude-sonnet-5"
}
```

## Thinking levels

The thinking level is how much reasoning the model does before it answers. There are seven, in
order: `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`.

Set one for a run with `--thinking high`, or attach it to a model with `--model sonnet:high`. Press
`Shift+Tab` in a session to cycle. The colour of the rules around the input editor tracks the
current level, so you can see your reasoning depth without looking anywhere else.

A misspelled `--thinking` value is not a usage error. cyrup warns
`Invalid thinking level "...". Valid values: ...`, drops the flag, and runs with no level set.

### Availability is per model

Each model declares which levels it supports:

- A model that does not reason at all supports only `off`.
- `off` through `high` are available unless the model explicitly marks one unsupported.
- **`xhigh` and `max` exist only where a model declares them.** They are not universally available,
  and a model synthesised by the unknown-id fallback never has them.

Requests are clamped rather than rejected. Ask for a level a model does not support and you get the
nearest supported level above it, or failing that the nearest below. `Shift+Tab` cycles only the
levels the active model actually supports; with no model installed the offered set is `off` through
`high`.

### What a level does at the provider

Providers fall into two families and cyrup translates for both.

**Token-budget providers** get a thinking-token budget: roughly 1k for `minimal`, 2k for `low`, 8k
for `medium`, and 16k for `high`. These providers have no separate `xhigh` or `max`, so both collapse
to `high`. Override the budgets in settings:

```json
{
  "thinkingBudgets": { "medium": 12000, "high": 32000 }
}
```

**Effort-string providers** get a reasoning-effort string mapped from the level by the model's own
declaration — `high` becomes `reasoning_effort: "high"`, and so on.

The starting level for every session comes from `defaultThinkingLevel`, which defaults to `off`:

```json
{
  "defaultThinkingLevel": "medium"
}
```

**`CYRUP_REASONING_LEVEL` does not set the thinking level.** cyrup *exports* it into the environment
of every shell command the agent runs, so a script can see what depth it was invoked at. Setting it
yourself changes nothing about the model. See
[Tools and permissions](tools-and-permissions.md#what-the-bash-tool-exports).

## Custom providers and models

`~/.cyrup/agent/models.json` adds providers and models that are not built in, and patches ones that
are. A provider block needs a `baseUrl`, an `api` naming the wire protocol, and a credential:

```json
{
  "providers": {
    "my-gateway": {
      "name": "My Gateway",
      "baseUrl": "https://gateway.internal/v1",
      "api": "openai-completions",
      "apiKey": "$MY_GATEWAY_KEY",
      "models": [
        { "id": "internal-large", "name": "Internal Large", "contextWindow": 200000 }
      ]
    }
  }
}
```

`apiKey` is a template, not a literal: a value starting with `!` is run as a shell command and its
trimmed stdout becomes the key, and `$VAR` / `${VAR}` interpolate from the environment. `$$` and
`$!` escape a literal `$` or `!`.

Alongside `models`, a provider block accepts `headers`, `compat` for protocol quirks, and
`modelOverrides` for patching individual models — including the built-in ones — with a different
`contextWindow`, `maxTokens`, `reasoning` flag, or `thinkingLevelMap`. A provider declared here is
selectable with `--provider` and appears in `--list-models` once its key resolves.
