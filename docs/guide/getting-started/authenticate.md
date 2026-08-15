# Connect a provider

cyrup ships with thirty-five model providers built in and credentials for none of them. This page
takes you from an installed binary to a working model, by environment variable or by an interactive
login.

## The fastest path: an environment variable

Export the provider's key and start cyrup. It is picked up at launch and cyrup selects a default
model from that provider.

```sh
export ANTHROPIC_API_KEY=sk-ant-...
cyrup
```

OpenAI is the same shape:

```sh
export OPENAI_API_KEY=sk-...
cyrup
```

Put the export in your shell profile if you want it to survive a new terminal. Nothing is written
to disk — cyrup reads the variable on each run.

## The interactive path: `/login`

**There is no `cyrup login` subcommand.** Authentication is a slash command inside the terminal
interface. Start cyrup with no arguments, then type:

```text
/login
```

cyrup asks how you want to sign in — *Sign in with an account* or *Sign in with an API key* — then
which provider, then walks the flow. The credential is saved, so the next run needs nothing.
`/login anthropic` skips straight to that provider.

Most API-key flows ask one question. A few ask more: `cloudflare-workers-ai` wants an account ID
alongside the key, `cloudflare-ai-gateway` adds a gateway ID, and `google-vertex` and
`amazon-bedrock` first ask which authentication method you are using.

## Which providers do OAuth

Six of them: `anthropic`, `kimi-coding`, `xai`, `openrouter`, `github-copilot` and `openai-codex`.
Everything else is API key only.

`anthropic`, `kimi-coding` and `xai` sign in against a subscription. `openrouter`'s OAuth is
metered rather than a subscription. `github-copilot` and `openai-codex` each run their own device
flow. `openai-codex` is **OAuth only** — it has no API key at all, so it is the one provider you
cannot reach with an environment variable.

## Where credentials are stored

Saved credentials go in `~/.cyrup/agent/auth.json`, mode `0600`, one entry per provider.

`/logout` removes stored credentials and nothing else. It does not unset environment variables or
touch `models.json`. If `/logout` reports that there is nothing to remove, your key is coming from
the environment, not from disk.

## Precedence when there is more than one key

For a given provider, cyrup takes the first of these that exists:

1. `--api-key` on the command line — a per-run credential. It requires one of `--model`,
   `--provider` or `--models` alongside it.
2. A stored credential in `auth.json`.
3. The provider's environment variable.
4. An `apiKey` entry in `models.json`.

**A stored credential suppresses the environment variable.** If you logged in once with `/login`
and later exported a different key, the export is ignored until you `/logout`. This is the single
most common source of "why is it still using the old key".

## Provider environment variables

The twelve you are most likely to want:

| Provider | Environment variable |
|---|---|
| `anthropic` | `ANTHROPIC_API_KEY` (`ANTHROPIC_OAUTH_TOKEN` is checked first) |
| `openai` | `OPENAI_API_KEY` |
| `google` | `GEMINI_API_KEY` |
| `openrouter` | `OPENROUTER_API_KEY` |
| `groq` | `GROQ_API_KEY` |
| `deepseek` | `DEEPSEEK_API_KEY` |
| `xai` | `XAI_API_KEY` |
| `mistral` | `MISTRAL_API_KEY` |
| `together` | `TOGETHER_API_KEY` |
| `fireworks` | `FIREWORKS_API_KEY` |
| `github-copilot` | `COPILOT_GITHUB_TOKEN` |
| `amazon-bedrock` | *(ambient AWS credentials)* |

Note that `google` reads `GEMINI_API_KEY`, not `GOOGLE_API_KEY`.

`amazon-bedrock` and `google-vertex` have no key variable of their own — they use whatever cloud
credentials are already ambient in your shell, an AWS profile or access keys for Bedrock and
Application Default Credentials for Vertex. The exact set of variables each one accepts is in
[environment variables](../reference/environment.md).

The remaining providers and the models each one carries are listed in
[models and thinking](../guides/models.md).

## Check that it worked

```sh
cyrup auth check --provider anthropic
```

Reports `ready`, `not_ready` or `invalid`, and exits non-zero when the provider is not usable. It
needs at least one of `--provider` or `--model`.

```sh
cyrup --list-models
```

Prints every model you can currently reach, with its context window, maximum output, and whether it
supports thinking and images. It lists **only** models whose provider has auth configured, so an
empty result means nothing is authenticated — not that there are no models.

## Next

[Your first session](first-session.md).
