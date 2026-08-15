# Models

Pick a model with `--model`. The pattern may be a bare id, a `provider/id` pair, or a partial match,
with an optional `:<thinking level>` suffix.

```sh
cyrup --model openai/gpt-4o
cyrup --model sonnet:high
cyrup --provider anthropic --model claude-opus-4-8
```

List what is available to you:

```sh
cyrup --list-models
cyrup --list-models anthropic
```

Only models whose provider has credentials configured are listed. If the list is empty, see
[Providers](providers.md).

Set a cycling set with `--models`, which accepts globs, then switch between them with `Ctrl+P`
while cyrup is running:

```sh
cyrup --models "anthropic/*,openai/gpt-5.5"
```

Thinking levels are `off`, `minimal`, `low`, `medium`, `high`, `xhigh`, and `max`. Availability is
per-model; a request is clamped to the nearest level the active model supports rather than
rejected. Press `Shift+Tab` to cycle through the supported levels.

## Full documentation

- [Models and thinking](guide/guides/models.md) — the complete guide
- [Connect a provider](guide/getting-started/authenticate.md) — authenticating
- [Command line](guide/reference/cli.md) — every flag
