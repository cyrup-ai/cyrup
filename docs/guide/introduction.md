# cyrup

cyrup is a coding agent that runs in your terminal. It reads and edits files, runs commands, and
works through a task with you — as a single static binary with no runtime to install alongside it.

**cyrup is pre-release.** There is no crates.io release and no Homebrew tap yet; you install it
from source. The rest of this guide documents the software as it behaves today, and calls out the
places where a feature is deliberately unfinished. Where you see a limit described plainly, that is
the current behaviour, not a warning about the future.

## What you get

A terminal interface that streams the model's response as it arrives, keeps every conversation as a
resumable session tree, and lets you switch models mid-turn without losing your place.

Thirty-five model providers are built in — Anthropic, OpenAI, Google, Bedrock, OpenRouter,
Copilot and more — behind one set of flags. You authenticate once and switch between them with a
keystroke.

Seven built-in tools (`read`, `write`, `edit`, `bash`, `grep`, `find`, `ls`) that you can narrow to
an allowlist, or turn off entirely for a read-only review session. Four of the seven — `read`,
`bash`, `edit`, `write` — are active in a default session; `grep`, `find` and `ls` are registered but
off until you name them.

Extensions as WebAssembly components, sandboxed by a capability manifest the host enforces. An
extension gets filesystem, process, network or UI access only if it declared that it needs it.

Three larger subsystems ship built in and default off: **subagents** for delegating work to child
agent processes, **the permission system** for allow/ask/deny policy over every tool call, and
**intercom** for coordination between concurrent sessions. A fourth, **Flux**, ships built in too
but is on by default: a structured, file-persisted development pipeline (`new → ask → split → aug →
exec → qa → tests → commit → create-pr`) driven by `/flux/*` commands — see
[Flux](extensions/flux.md).

## Where to start

If you have not installed it yet, start at [Install](getting-started/install.md), then
[Connect a provider](getting-started/authenticate.md), then
[Your first session](getting-started/first-session.md). Those three pages take about ten minutes
end to end.

If cyrup is already running and you want to get productive, read
[The terminal interface](guides/tui.md) — it covers the keys, the slash commands, and what each
part of the screen is telling you.

If you are looking for a specific flag, setting, environment variable, or keybinding, go straight
to the [reference section](reference/cli.md).

## A note on names

**cyrup** · /ˈsɪr.əp/ · *SIR-up* — rhymes with syrup, as in maple syrup.
