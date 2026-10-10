# Models and thinking

cyrup talks to thirty-five providers through one set of flags. This page covers picking a model on
the command line, switching mid-session, and setting how hard the model thinks before it answers.

If you have not connected a provider yet, do that first —
[Connect a provider](../getting-started/authenticate.md).

To run models on your own machine, see [Local models with llama.cpp](../llama-cpp.md): the
`llama.cpp` provider is built in and talks to a llama.cpp router server.

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
  last colon and the tail must be one of the seven thinking levels. A tail that is not a level stays
  part of the model id: on a known provider you end up with a custom model whose id still carries the
  suffix (type `anthropic/claude-opus-4-6:hgih` and you get the `anthropic` provider with the model
  id `claude-opus-4-6:hgih` — the provider prefix is stripped, the bad suffix is not), and otherwise
  you get `Model "…" not found. Use --list-models to see available models.`

`--provider` selects the provider separately, so `--provider openai --model gpt-4o` is equivalent to
the prefixed form. A redundant prefix (`--provider openai --model openai/gpt-4o`) is stripped rather
than rejected.

`--model` does not accept globs. Wildcards are a `--models` feature and nothing else — see
[The Ctrl+P cycling set](#the-ctrlp-cycling-set).

### When the model is not in the catalog

An unknown provider is a hard error:
`Unknown provider "acme". Use --list-models to see available providers/models.` The provider name is
matched case-insensitively against the providers that have models in your registry.

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

A following token that starts with `-` or `@` is *not* taken as the pattern. `cyrup --list-models
@notes.md` lists the whole catalog and keeps `@notes.md` as a file attachment for the run; only a
plain word (`--list-models gpt`) filters.

**`--list-models` shows only providers you have authenticated.** It is a list of what you can use
right now, not a catalog of what exists. With no configured provider at all you get
`No models available.` and the `/login` guidance instead of a table — the usual cause is missing
credentials, not a missing model.

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
`Invalid thinking level "...". Valid values: ...`, drops both the flag and its value, and the run
starts at the default level as if you had not passed it.

### Availability is per model

Each model declares which levels it supports:

- A model that does not reason at all supports only `off`.
- `off` through `high` are available unless the model explicitly marks one unsupported.
- `xhigh` and `max` exist only where a model declares them. They are not universally available, and
  a model synthesised by the unknown-id fallback never has them.

Requests are clamped rather than rejected. Ask for a level a model does not support and you get the
nearest supported level above it, or failing that the nearest below. `Shift+Tab` cycles only the
levels the active model actually supports. On a model that does not reason — and in a session with
no model at all — it changes nothing and prints `Current model does not support thinking`.

### What a level does at the provider

Providers fall into two families and cyrup translates for both.

Token-budget providers get a thinking-token budget: roughly 1k for `minimal`, 2k for `low`, 8k for
`medium`, and 16k for `high`. These providers have no separate `xhigh` or `max`, so both collapse to
`high`. Override the budgets in settings:

```json
{
  "thinkingBudgets": { "medium": 12000, "high": 32000 }
}
```

Effort-string providers get a reasoning-effort string mapped from the level by the model's own
declaration — `high` becomes `reasoning_effort: "high"`, and so on.

The starting level for a new session comes from `defaultThinkingLevel`. **With the key unset that is
`medium`** — a fresh install reasons at medium depth without you configuring anything. (It used to
be `off`, because the settings getter folded "unset" into the type's zero value; every consumer now
names `medium` explicitly.)

Set the key to move that starting point. `"off"` is how you turn reasoning off by default:

```json
{
  "defaultThinkingLevel": "off"
}
```

Two things override it. A resumed session keeps the level recorded in its own transcript, and a
session that ends up with no model is forced to `off` whatever the setting says.

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
`contextWindow`, `maxTokens`, `reasoning` flag, `thinkingLevelMap`, `samplingParams`,
`samplingParamsByThinkingLevel`, `promptCache` lifetimes, or
[`inputLimits`](#how-big-an-image-gets-sent). A provider declared here is selectable with
`--provider` and appears in `--list-models` once its key resolves.

`samplingParams` is the escape hatch for parameters cyrup does not model — `top_p`, `top_k`,
`min_p`, `repetition_penalty`, anything your server accepts:

```json
{ "id": "internal-large", "samplingParams": { "top_p": 0.95, "min_p": 0.05 } }
```

The map is written onto the request body **last**, so a key here beats the `temperature` and
`max_tokens` cyrup would otherwise send. A `modelOverrides` entry merges into it per key rather than
replacing it, so a `top_p`-only patch leaves `min_p` alone. Only the three OpenAI-compatible wire
protocols apply it (`openai-completions`, `openai-responses`, `azure-openai-responses`); every other
api — including `openai-codex-responses` — ignores it silently.

`samplingParamsByThinkingLevel` overrides those defaults per thinking level. Its keys are cyrup's
thinking levels (`off`, `minimal`, `low`, `medium`, `high`, `xhigh`, `max`), not the provider values
from `thinkingLevelMap`, and each value is a sampling map like `samplingParams`:

```json
{
  "id": "qwen-thinking-model",
  "reasoning": true,
  "samplingParams": { "temperature": 1.0, "top_p": 0.95 },
  "samplingParamsByThinkingLevel": {
    "off": { "temperature": 0.7, "top_p": 0.8 },
    "high": { "top_k": 20 }
  }
}
```

cyrup first clamps the requested level to one the model supports, then merges the model's
`samplingParams`, that level's entry, and the request's own sampling params, in that order; a later
key wins. A level with no entry gets the model defaults alone. A `modelOverrides` entry merges into
each level per key rather than replacing it. Like `samplingParams`, it applies only to the three
OpenAI-compatible protocols, and a wrong shape (a level that is not an object) makes the whole
`models.json` invalid rather than being ignored.

## How big an image gets sent

Images are not sent at whatever size you hand them over. Before a new image enters the
conversation, cyrup re-encodes it to fit a **resize profile** that belongs to the model the request
is actually going to — so the same screenshot costs the same whether it arrived as an `@file`
argument, from the `read` tool, over the RPC or SDK transport (which is also how the editor
integrations send one), or as the result of a tool that produces pictures itself, such as an MCP
screenshot bridge.

One case is deliberately left alone: an image attached to a message you **queue while the agent is
already running** — a steer, a follow-up, or a `prompt` that arrives mid-stream — is sent at its
original size. The resize profile is chosen when a prompt is turned into a request, and a queued
message skips that step by design; this matches upstream.

The default profile is 2000×2000 pixels, 4.5 MB of base64, JPEG quality 80. A model can declare its
own under `inputLimits`, and a `modelOverrides` entry can patch one:

```json
{
  "providers": {
    "my-gateway": {
      "modelOverrides": {
        "internal-large": {
          "inputLimits": { "images": { "resize": { "maxWidth": 1024, "maxHeight": 1024 } } }
        }
      }
    }
  }
}
```

| Key under `inputLimits.images.resize` | Default | Meaning |
|---|---|---|
| `maxWidth` | `2000` | Width clamp, in pixels. |
| `maxHeight` | `2000` | Height clamp, applied as a second clamp after `maxWidth`. |
| `maxBytes` | `4718592` | Ceiling on the **base64-encoded** payload, not on the raw bytes. |
| `jpegQuality` | `80` | Leading quality of the JPEG re-encode ladder, `1`–`100`. |

Each key resolves on its own, so a profile naming only `maxBytes` keeps the 2000-pixel clamps. The
two clamps are sequential rather than interchangeable: a 3000×500 image under
`{"maxWidth": 1000, "maxHeight": 2000}` lands at 1000×167, with the height clamp never binding. If
the image is already inside the profile it is sent byte-for-byte, untouched.

A clamp alone does not always get a picture under `maxBytes`, so cyrup re-encodes down a ladder:
PNG first at each size, then JPEG at `jpegQuality` and then 85, 70, 55 and 40, shrinking by a
quarter and trying again until something fits. `jpegQuality` leads that ladder rather than replacing
it, so setting it to 40 starts low instead of walking down from 80.

`inputLimits` also accepts `maxRequestBytes`, `images.maxPerMessage` and `images.maxPerRequest`.
cyrup reads them, keeps them and hands them back out unchanged, but **nothing enforces them** — and
that is not cyrup lagging behind: upstream declares, generates and validates the same three keys
without rewriting or rejecting a conversation on their basis either. Only `images.resize` changes
what is sent.

### Which model's profile you get

The profile comes from the model the request will actually use, resolved **after** extensions have
had their say — so an extension that switches the model in `before_agent_start` switches the resize
profile with it.

The prompt path and the `read` tool ask slightly different questions, and under a
[virtual model](../extensions/virtual-models.md) the two answers differ:

| Path | Whose profile |
|---|---|
| Prompt images, and images returned by a tool | The **physical** model that answers the request — under a virtual selection, the row the virtual model routed to, not the virtual row itself. |
| The `read` tool | The model **currently selected**, which under a virtual selection is the virtual row. |

That split is upstream's own (`ctx.model` for `read` against `_limitsModel()` for everything else),
not a cyrup quirk. In the ordinary case — no virtual model — both are the same model and the
distinction never shows. The `read` tool reads the profile per call, so a `/model` switch
mid-session reaches the very next `read` rather than the one after it.

An image is encoded **once**, on the way into history. Switching models afterwards does not re-encode
what is already there.

### When an image cannot be sent

A picture cyrup cannot decode, or cannot get under `maxBytes` at any size, does not fail your
request. The image block is dropped and a note is appended to your message text instead, so the
model is told what happened:

```
[Image omitted: could not be converted to a supported inline image format.]
[Image omitted: could not be resized below the inline image size limit.]
```

A successful send can add a note too. A converted format says so, and anything that was actually
downscaled carries the scale factor, so the model can map coordinates back:

```
[Image converted from image/bmp to image/png.]
[Image: original 6000x4000, displayed at 2000x1333. Multiply coordinates by 3.00 to map to original image.]
```

Setting [`images.autoResize`](../reference/settings.md#appearance-and-the-terminal-interface) to
`false` turns the clamps and the byte ladder off: an image is still converted to a format the
provider accepts, but it is sent at its original size, and an oversized one is then the provider's
problem rather than cyrup's. `images.blockImages` drops images altogether.
