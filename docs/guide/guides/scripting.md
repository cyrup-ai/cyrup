# Scripting and automation

cyrup runs without a terminal: one-shot prompts, machine-readable output, and a mode for driving it
as a subprocess. This page covers the flags that matter in a script, what talks to the network and
when, and the exit codes worth branching on.

## One-shot runs

```sh
cyrup -p "list every TODO in src/ with its file and line"
```

`--print` (`-p`) processes the prompt and exits. You do not always need the flag: if either stdin or
stdout is not a terminal, cyrup runs in print mode anyway, so a plain `cyrup "..." | tee out.txt`
behaves the same way.

There are four ways to give it a prompt, and they combine:

- A bare argument is a message.
- An `@`-prefixed argument is a file reference. Text files are inlined with a wrapper naming the
  path; images are attached. `@@text` escapes a message that genuinely starts with `@`.
- Piped stdin is prepended to the first message.
- Extra bare arguments after the first become follow-up messages, replayed one at a time.

```sh
git diff | cyrup -p @.github/review-guide.md "review this diff against the guide"
```

A `-p` run does not write a session file unless you asked for one with `--continue`, `--resume`,
`--session` or `--fork`.

## Output modes

| Flag | Alias | What you get |
|---|---|---|
| `--mode text` | `--output-format text` | Plain text. The default. |
| `--mode json` | `--json`, `--output-format json` | Structured events on stdout, for a program to parse. |
| `--mode rpc` | `--rpc` | A request/response protocol for driving cyrup as a child process. |
| `--mode acp` | `--acp` | The Agent Client Protocol, for an editor. See [Zed and other ACP editors](zed-acp.md). |

`json` is what you want when a script consumes the result. `rpc` is what you want when another
program owns the conversation and calls cyrup turn by turn. Both imply non-interactive behaviour.

## A CI recipe

```sh
cyrup --offline \
      --no-approve \
      --model anthropic/claude-sonnet-5 \
      --tools read,grep,find,ls \
      --no-session \
      --mode json \
      -p "review the changes in this branch for error-handling bugs"
```

Every flag there is doing something specific. Pinning the model with a full `provider/id` stops the
default-selection ladder from picking something else when credentials change. Restricting tools
makes the run incapable of writing or shelling out. `--no-session` keeps it in memory.
`--offline` suppresses the startup network calls. And `--no-approve` makes the trust decision
explicit instead of environment-dependent.

**An untrusted project stays untrusted in a script.** `-p`, `--mode json` and `--mode rpc` cannot
show the trust prompt, so a project with no saved decision is treated as untrusted and its `.cyrup/`
directory is ignored. If the run needs the project's settings, skills or extensions, pass
`--approve`. See [Tools and permissions](tools-and-permissions.md#project-trust).

## Exit codes

A `-p` run exits `0` normally and `1` when the final turn errored or was aborted.

`--mode json` always exits `0`, whatever happened. The exit code is not the signal there — the stop
reason is in the streamed event records, and a consumer is expected to read it.

With no provider configured, a non-interactive run writes guidance to stderr and exits `1`. The same
situation interactively launches modelless instead, so a script cannot rely on the interactive
behaviour as a signal.

`cyrup auth check` is a usable precondition because its exit code is its answer:

```sh
cyrup auth check --provider anthropic --json || exit 1
```

`0` means ready, `1` means not ready, and `2` means the stored credential is invalid or the check
itself failed. It takes `--provider` or `--model` (at least one is required), `--credentials` to
print the resolved credential rather than the status word, and `--no-refresh` to skip refreshing an
expired OAuth token.

## What `--offline` actually does

`--offline` — or `CYRUP_OFFLINE=1` — disables four things: the package update check, install
telemetry, analytics, and the model-catalog refresh.

**It does not block LLM API calls.** `--offline` governs *startup* operations. Run a turn offline
and the inference request still goes out. If you need a genuinely airgapped run, that is a network
policy problem, not a flag.

The persisted model catalog is still read from disk when offline, and the catalogs compiled into the
binary are always the floor, so an offline run has a full model list.

## What talks to the network, and when

Two things happen on their own, both at startup, both optional. One more happens only when you ask
for it.

**The package update check** runs in interactive mode only. For each installed package that came
from a git source it runs `git rev-parse HEAD` locally and `git ls-remote` against the origin, four
at a time, with a ten-second budget per git invocation. Local and pinned sources are skipped, and
any failure resolves to "no update". Disable it with `CYRUP_SKIP_VERSION_CHECK=1` without touching
anything else.

**The model-catalog refresh** fetches provider catalogs from `https://pi.dev` with ETag
revalidation, and caches the result in `~/.cyrup/agent/models-store.json`. It runs only in
interactive and rpc modes, only for providers you have actually authenticated, and it is
fire-and-forget — every error is swallowed and you fall back to the embedded catalogs. `-p` and
`--mode json` issue zero catalog requests, and an unconfigured cyrup fetches nothing at all.

**`cyrup update --models`** is the third. It refreshes the catalogs for every authenticated provider
in the foreground with a 15-second budget, printing `Model catalogs refreshed` on success, or
`Error: Could not refresh model catalogs: <provider>: <reason>` and exit `1` on failure. The budget
covers the whole pass, not each request, and overrunning it is a failure of its own —
`Error: Model catalog refresh timed out.` and exit `1`, not a partial success. Because you asked for
it explicitly, `--offline` does not suppress it.

There is no self-update check and no release feed. cyrup never phones home to ask about its own
version.

## Proxies

cyrup's outbound requests — inference, the catalog fetch, OAuth refreshes — resolve their proxy
through one path, which reads `HTTPS_PROXY`, `HTTP_PROXY`, `ALL_PROXY` and `NO_PROXY` in either
case. The `httpProxy` setting stands in for `HTTP_PROXY`/`HTTPS_PROXY` when neither is exported.

**The setting is installed before any subcommand runs**, from the global `settings.json` alone. That
matters in a script: `cyrup update --models` fetches catalogs, and `cyrup auth check` /
`cyrup auth print-bearer-token` refresh an expired OAuth token unless you pass `--no-refresh` — all
before any session exists, and all through the proxy.

Two consequences worth knowing:

- `httpProxy` is read from `~/.cyrup/agent/settings.json` only. A project `.cyrup/settings.json`
  supplies nothing, trusted or not, so checking a repository out cannot redirect your egress.
- An exported `HTTP_PROXY`/`HTTPS_PROXY` wins over the setting, so the environment of the run is
  still the last word.

SOCKS and PAC proxy URLs are rejected rather than ignored: the request fails with `Unsupported proxy
protocol. SOCKS and PAC proxy URLs are not supported; use an HTTP or HTTPS proxy URL.`, followed by
`Got <scheme>`.

## Exporting a session

```sh
cyrup --export ~/.cyrup/agent/sessions/0f3a....jsonl report.html
```

Reads a session file, renders it to standalone HTML, and exits. The output path is optional — the
default is the input path with an `.html` extension. On success it prints `Exported to: <path>`; on
failure it prints the error and exits `1`. See [Sessions](sessions.md) for finding the file.

It runs before the session-flag validation, so it still works when the rest of the command line does
not: `cyrup --export s.jsonl --api-key K` and `cyrup --export s.jsonl --fork X --continue` both
export and exit `0`. An `@file` argument is left as a file reference and is never taken for the
output path.

## Environment variables

| Variable | Effect |
|---|---|
| `CYRUP_OFFLINE` | Same as `--offline`. |
| `CYRUP_SKIP_VERSION_CHECK` | Disables the package update check only. |
| `CYRUP_TELEMETRY` | Overrides the `enableInstallTelemetry` setting. |
| `CYRUP_AGENT_DIR` | Relocates the agent directory — settings, credentials, trust, packages. |
| `CYRUP_SESSION_DIR` | Relocates session storage; beats the `sessionDir` setting. |

Pointing `CYRUP_AGENT_DIR` and `CYRUP_SESSION_DIR` at a scratch directory is how you isolate a run
from your own configuration:

```sh
CYRUP_AGENT_DIR="$RUNNER_TEMP/cyrup" cyrup -p "..."
```

`CYRUP_TELEMETRY` is tri-state, unlike the others. Unset defers to the setting; set to a truthy
value turns it on; set to anything else — including empty — is an explicit off, so
`CYRUP_TELEMETRY= cyrup ...` disables it.

Truthiness here is narrow: `CYRUP_OFFLINE` and `CYRUP_SKIP_VERSION_CHECK` accept exactly `1`, `true`
and `yes`. `on` is not accepted, and values are not trimmed — `CYRUP_OFFLINE=" 1"` is false.

The full list, including the variables cyrup exports rather than reads, is in
[Environment variables](../reference/environment.md).
