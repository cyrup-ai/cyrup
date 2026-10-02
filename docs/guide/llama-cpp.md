# Local models with llama.cpp

cyrup can drive a [llama.cpp](https://github.com/ggml-org/llama.cpp) **router** server: one
`llama-server` process that discovers many GGUF models and loads or unloads them on demand. The
support is built into the binary. There is nothing to install and nothing to enable: it is a native
extension that is attached to every session and hidden from the startup extension list.

cyrup never starts `llama-server` for you. It is a client of one that is already running, plus a
Hugging Face search client for finding models to download into it.

If you have not connected a provider before, read [Connect a provider](getting-started/authenticate.md)
first; llama.cpp uses the same `/login` and `auth.json`.

## Start the router

Use a current llama.cpp build with router support (see the
[build instructions](https://github.com/ggml-org/llama.cpp/blob/master/docs/build.md) or a
[prebuilt release](https://github.com/ggml-org/llama.cpp/releases)). Start `llama-server` **without**
`--model`, `-m` or `-hf`: passing a model starts single-model mode, which has no router to talk to.

```sh
llama-server \
  --models-dir ~/models \
  --no-models-autoload \
  --jinja \
  --host 127.0.0.1 \
  --port 8080 \
  -ngl 999 \
  -c 32768
```

- `--models-dir ~/models` discovers local GGUF files.
- `--no-models-autoload` keeps loading explicit, through `/llama`.
- `--jinja` enables compatible chat templates and tool calling.
- `-ngl 999` offloads as many layers as possible to the GPU.
- `-c 32768` sets the context window of each loaded model. Leave it out to use each model's native
  context, which can need far more memory.

A single-file model can sit directly in the model directory. Put multimodal and multi-shard models in
a subdirectory each:

```text
~/models/
├── llama-3.2-1b-Q4_K_M.gguf
├── gemma-3-4b-it-Q4_K_M/
│   ├── gemma-3-4b-it-Q4_K_M.gguf
│   └── mmproj-F16.gguf
└── large-model-Q4_K_M/
    ├── large-model-Q4_K_M-00001-of-00003.gguf
    ├── large-model-Q4_K_M-00002-of-00003.gguf
    └── large-model-Q4_K_M-00003-of-00003.gguf
```

Restart the router after adding files by hand. For per-model context sizes and other options use
[llama.cpp model presets](https://github.com/ggml-org/llama.cpp/blob/master/tools/server/README.md#model-presets).

## Connect cyrup to it

Start cyrup and run:

```text
/login llama.cpp
```

It asks for the router URL (the default is `http://127.0.0.1:8080`) and an optional API key. The URL
and key are stored in `<agent dir>/auth.json` (by default `~/.cyrup/agent/auth.json`) like any other
credential; the URL is kept in the credential's `env.LLAMA_BASE_URL`. The URL must be `http` or
`https` and must not carry a user name or password. A trailing slash, query, fragment or `/v1` is
dropped.

If the router runs with `--no-models-autoload`, `/login llama.cpp` only stores the connection. Run
`/llama` to load a model, then `/model` to select it for the session.

Environment variables configure the same two values without `/login`:

```sh
export LLAMA_BASE_URL=http://127.0.0.1:8080
export LLAMA_API_KEY=optional-secret
cyrup
```

`LLAMA_BASE_URL` alone is enough for the provider to count as configured and for its models to be
listed. With no key set, cyrup sends the placeholder key `local`, which a router started without
`--api-key` accepts. If the router does use an API key, start it with the matching `--api-key` value
and keep `--host 127.0.0.1` for local-only access.

A stored login wins over the environment variables. `/logout llama.cpp` removes the stored login, and
with it the provider's models from the lists.

On the command line the provider is `llama.cpp`:

```sh
cyrup --list-models llama
cyrup --model llama.cpp/qwen3 "explain this repo"
cyrup --provider llama.cpp --model qwen3 "explain this repo"
```

`--provider llama.cpp` on its own is refused as an unknown provider: pass `--model` with it, or use
the `llama.cpp/<id>` form. cyrup picks its first provider before extensions load, and `llama.cpp` is
registered by one. See [Models and thinking](guides/models.md) for the model flags in general.

### What the model list shows

At startup (interactive and RPC modes, unless you pass `--offline`) cyrup asks the router which models
it has and caches the answer in `<agent dir>/models-store.json`, so the list is there on the next
launch even before the router is reachable. `/login llama.cpp` and `/llama` refresh it again.
`--offline`, print mode and JSON mode make no request to the router.

Loaded and sleeping models appear in `/model`; a sleeping model wakes when you select it. With router
autoload on, unloaded preset models appear too and load when selected. With `--no-models-autoload`,
load a model through `/llama` before selecting it.

## Manage models with `/llama`

```text
/llama
```

`/llama` is interactive only. In RPC mode it answers with a warning ("/llama is available in
interactive mode") and does nothing else. Do not send it as the whole prompt of a print-mode run
(`cyrup -p "/llama"`): print mode currently waits forever on any prompt that an extension command
handles (ledger row `SEAM-137`).

- Select an unloaded model to **load** it.
- Select a loaded model to **unload** it, after a confirmation.
- Select **Download model…**, search Hugging Face, then pick a repository and a quantization. An exact
  `owner/repository[:quant]` value works too.
- Press Escape during a load or download to confirm cancelling it.

If other models are loaded, `/llama` asks whether to unload them first or keep them loaded. It never
unloads a model on its own and never deletes model files. The router can be shared with other clients,
so `/llama` always shows the router's current state, not what cyrup last did.

If the router goes away while `/llama` is open it shows **Retry** and **Close**. Retry reconnects and
refreshes the model state; it does not replay the interrupted operation.

### Hugging Face token lookup

Search uses `HF_TOKEN` when it is set and non-empty. Otherwise it reads the first usable token file
of, in order:

1. `$HF_TOKEN_PATH`
2. `$HF_HOME/token`
3. `$XDG_CACHE_HOME/huggingface/token`
4. `~/.cache/huggingface/token`

Search also works with no token, at lower rate limits. `/llama` warns before you download a gated
repository and links to its access page. **The llama.cpp server performs the download**, not cyrup, so
the `llama-server` process needs `HF_TOKEN` in its own environment when the repository requires
access.

## Classifier models

Every model listed for chat is also listed as a **classifier model** with the same id, on the
`llama-cpp-classify` api. A classifier model answers typed questions about a piece of JSON state, of
three kinds: `choice`, `bool` and `score`. It does not generate an answer. Each question becomes one
chat prompt, and cyrup reads the probabilities the server gives the label tokens as the next token,
then normalises them. A choice returns every option's probability and a confidence of
`(n * peak - 1) / (n - 1)`; a score returns the expected level.

**In cyrup these are reachable from Rust, not from a command.** There is no flag, slash command or
`cyrup-sdk` method that asks a classifier question, and cyrup has no `codemode`. The entry point is
`cyrup_provider::Models::classify`, which a program embedding cyrup's crates, or a native extension,
can call. Classifier models are persisted next to the chat models in `models-store.json`.

Things worth knowing if you use them:

- Raw label probabilities are usually overconfident. The per-request `temperature` option divides
  the label logits before normalising; values above 1 soften the distribution and change no answer.
- Questions run one after another. Everything before the final question is shared by every question of
  a request, so the server's prompt cache evaluates it once. The state appears **twice** in the
  prompt, so it needs twice its size in context.
- A small model may follow instructions written inside the state. The prompt tells the model to judge
  the state as data, which is not a guarantee.
- Hybrid models such as Qwen3.5 cannot rewind a partly cached prompt without context checkpoints. If
  each question reprocesses the whole state, start the router with
  `--ctx-checkpoints 32 --checkpoint-min-step 0`.

## Troubleshooting

Check that the router answers:

```sh
curl http://127.0.0.1:8080/health
curl http://127.0.0.1:8080/models
```

- **No models in `/llama`:** check `--models-dir`, the directory layout, and restart the router.
- **A model is missing from `/model` with `--no-models-autoload`:** load it with `/llama` first.
- **A load fails or uses too much memory:** lower `-c`, or unload another model.
- **"Server is not in router mode":** start it without `--model`, `-m` or `-hf`.
- **The list is empty right after your first `/login llama.cpp`:** the list is read from the router,
  so a router with nothing loaded and autoload off has nothing to list yet. Load a model in `/llama`.
- **`/model` lacks a model you just loaded elsewhere:** `/model` does not refresh the router's
  catalog today; run `/llama` or restart cyrup.

## Turning it off

`--no-extensions` (`-ne`) removes the llama.cpp provider and `/llama` along with every other
extension, and a plain launch with the flag leaves `/llama` to the model as ordinary text:

```sh
cyrup --no-extensions
```

That is all-or-nothing today. pi's `-builtin:llama.cpp` entry in the `extensions` setting, its
`builtin:llama.cpp` extension path and its built-in listing in `config` have no cyrup counterpart yet,
so you cannot switch off llama.cpp alone, or keep only llama.cpp under `--no-extensions`. The
llama.cpp extension is never listed in the startup extension list in either case, because it is a
built-in.

See [How extensions work](extensions/overview.md) and the
[command line reference](reference/cli.md#resources).
