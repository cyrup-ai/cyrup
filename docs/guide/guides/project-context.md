# Project context and skills

Everything cyrup knows about your project beyond what it reads at runtime comes from four things:
context files, the system prompt, skills, and prompt templates. This page covers where each lives,
how to add your own, and how to turn any of them off.

All project-scoped resources are gated on trust. In a folder you have not trusted, cyrup loads only
your global configuration — see [Tools and permissions](tools-and-permissions.md#project-trust).

## Context files

`AGENTS.md` and `CLAUDE.md` are the standing instructions for a repository: conventions, build
commands, the things you would otherwise re-type every session. cyrup loads them automatically.

Discovery walks three tiers:

1. The agent directory, `~/.cyrup/agent` — your personal instructions, applied everywhere. This tier
   loads regardless of trust.
2. Every ancestor of your working directory, top down.
3. The working directory itself.

Within one directory only the first match is used, in this order: `AGENTS.override.md`, `AGENTS.md`,
`AGENTS.MD`, `CLAUDE.md`, `CLAUDE.MD`. `AGENTS.override.md` therefore shadows a sibling `AGENTS.md`
rather than adding to it, which is how you keep a local variant out of version control.

The files are concatenated in discovery order — global, then ancestors top down, then the working
directory — so the most specific instructions land last. In a linked git worktree, the main
repository's copy of a file the worktree also has is skipped rather than loaded twice.

To run without any of it:

```sh
cyrup --no-context-files -p "what does src/main.rs do?"
```

`-nc` is the short form.

## The system prompt

cyrup ships a default system prompt. You can replace it or extend it.

```sh
cyrup --system-prompt ./prompts/reviewer.md "review the diff"
```

`--system-prompt` takes either literal text or a path to a file. If the value names a file that
exists, its contents are used; otherwise the value itself is the prompt. `--append-system-prompt`
works the same way but adds to the default rather than replacing it, and is repeatable — each entry
resolves independently and they are joined with a blank line.

The same two things can live on disk:

| File | Effect |
|---|---|
| `.cyrup/SYSTEM.md` | Replaces the system prompt for this project. |
| `.cyrup/APPEND_SYSTEM.md` | Appends to the system prompt for this project. |
| `~/.cyrup/agent/SYSTEM.md` | Replaces it globally. |
| `~/.cyrup/agent/APPEND_SYSTEM.md` | Appends globally. |

The tiers replace rather than stack. A project `SYSTEM.md` means the global one is not read; a
`--system-prompt` flag means neither file is read. The append leg behaves the same way: any
`--append-system-prompt` at all, and neither `APPEND_SYSTEM.md` is consulted. Both project files
require a trusted project.

## Skills

A skill is a folder containing a `SKILL.md` — YAML front matter with a `name` and a "use this skill
when…" `description`, followed by a body of instructions. cyrup shows the model the name and
description of every discovered skill and lets it pull in the body when the description matches what
it is doing. A skill with no description is dropped.

```markdown
---
name: release
description: Use this skill when cutting a release or writing release notes.
---

Our release process is: ...
```

Skills are discovered from:

- `~/.cyrup/agent/skills` and `~/.cyrup/agent/agents/skills`
- `.cyrup/skills` in the working directory (no ancestor walk)
- `.agents/skills` in the working directory and every ancestor up to the git repository root
- installed packages
- explicit `--skill <path>` arguments

`--skill` takes a file or a directory and is repeatable. When two skills share a name, package
skills win over `--skill` paths, and project scope wins over global.

Two front-matter keys are worth knowing. `disable-model-invocation: true` keeps a skill out of the
prompt so it can only be run deliberately, and `allowed-tools` narrows the tools available while it
runs.

`enableSkillCommands` (on by default) registers every discovered skill as a `/skill:<name>` slash
command, which is how you invoke one explicitly. Turn skills off entirely with `--no-skills` (`-ns`),
or pin a set in settings:

```json
{
  "skills": ["~/work/shared-skills", "+release", "-legacy-deploy"]
}
```

Bare entries are paths to load. Entries prefixed `+` or `-` are enable and disable overrides for a
discovered skill by name or pattern; `cyrup config` writes those for you.

When two skills share a name, exactly one wins, in this order: a path listed in project settings, an
auto-discovered project skill, a path listed in global settings, an auto-discovered global skill, a
package skill, and last a `--skill` argument. A package skill therefore beats one you named on the
command line, which surprises people the first time.

## Prompt templates

A prompt template is a markdown file that becomes a slash command named after the file. `/name args`
expands the body with shell-style substitution — `$1`, `$2`, `$@`, `$ARGUMENTS`, `${1:-default}`,
`${@:2}`. The description shown in the command list comes from front matter, or from the first
non-empty line of the body truncated to 60 characters; `argument-hint` in front matter documents the
arguments.

Templates are discovered from `~/.cyrup/agent/prompts`, `.cyrup/prompts` in a trusted project,
installed packages, and explicit `--prompt-template <path>` arguments (repeatable, file or
directory). `--no-prompt-templates` (`-np`) turns discovery off, and the `prompts` settings array
takes the same path and `+`/`-` entries as `skills`.

## Turning things on and off

```sh
cyrup config
```

Opens a picker over the loose skills, prompt templates and themes cyrup discovered — the ones found
in the agent directory and the project, not the ones a package contributed — with a checkbox on
each. Toggling one writes a `+pattern` or `-pattern` entry into the corresponding `skills`,
`prompts` or `themes` array in your global settings, replacing any earlier entry for the same
pattern.

`cyrup config -l` opens the same picker in project write scope, so toggles land in
`.cyrup/settings.json` instead. `Tab` switches between the two scopes while the picker is open, but
only in a trusted project — writing project settings in an untrusted folder is refused, and the
picker reports which changes could not be saved. `--approve` gets you past that for one run.

Packages are the other way skills and prompt templates arrive: a package can ship any of them
alongside its extensions, and they are discovered without you listing a path. See
[Installing extensions](../extensions/managing.md).

## The off switches

Each kind of project context has a flag that disables discovery for one run, and each takes a short
form:

| Flag | Short | Disables |
|---|---|---|
| `--no-context-files` | `-nc` | `AGENTS.md` and `CLAUDE.md` |
| `--no-skills` | `-ns` | Skill discovery and loading |
| `--no-prompt-templates` | `-np` | Prompt-template discovery |
| `--no-themes` | — | Theme discovery |
| `--no-extensions` | `-ne` | Extension discovery; explicit `-e` paths still load |

They stack, so a run with none of your standing instructions is:

```sh
cyrup -nc -ns -np -p "summarise what this repository does, from the code alone"
```

That is the useful shape for reproducing a report, or for checking whether a bad answer came from
the model or from something you told it three months ago.

