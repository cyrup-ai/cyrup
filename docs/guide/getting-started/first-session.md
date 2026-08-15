# Your first session

Ten minutes in a repository you actually work on. You will ask a question, watch cyrup use a tool,
interrupt it, run a shell command inline, and then leave and come back to the same conversation.

## Start it

```sh
cd ~/code/my-project
cyrup
```

The screen is a transcript with an input editor at the bottom and a status line showing the current
model and thinking level.

If the project carries a `.cyrup/` directory or an `.agents/skills` folder, cyrup asks whether you
trust it before loading anything from it. Answer *Trust* for your own repositories; an untrusted
project still works, it just ignores that project's settings and extensions.

## Ask something

Type a question and press `Enter`.

```text
where does this project parse command line arguments?
```

The answer streams in as the model produces it. On a question like that cyrup will reach for a tool
first — `grep` or `find` to locate the code, then `read` to open it — and each call appears in the
transcript as a compact block: the tool name, its arguments, and a few lines of result. Press
`Ctrl+O` to expand the focused block when you want the whole thing.

If the answer is heading somewhere useless, press `Esc`. The run aborts immediately, and anything
you typed while it was streaming is put back in the editor rather than thrown away.

## Point at a specific file

Type `@` and a fuzzy file picker opens over the repository. Keep typing to narrow it, choose a
path, and carry on with the sentence.

```text
@src/cli.rs what happens if --thinking gets a value that isn't a valid level?
```

The mention gives cyrup the path directly, which is faster and more accurate than describing the
file in prose.

## Run a command without leaving the prompt

A line beginning with `!` is a shell command, not a prompt. The editor border turns green while you
are in that mode.

```text
!cargo test -p cyrup-config
```

Output streams into the transcript and joins the conversation, so your next message can be *why did
that second test fail?* with nothing pasted. Use `!!` instead of `!` to run something and keep its
output out of the model's context. `Esc` cancels a command that is still running.

## Change the depth, change the model

`Shift+Tab` cycles the thinking level in place — `off`, `minimal`, `low`, `medium`, `high`, and on
models that declare support for them `xhigh` and `max`. Only the levels the current model actually
supports are offered, and the editor rule changes colour so you can see where you are. Raise it
before a hard question, drop it back for cheap ones.

`Ctrl+P` switches to the next model in your cycling set and `Ctrl+Shift+P` goes back — mid-session,
without losing the conversation. `/model` opens a picker if you would rather choose than cycle.

## Ask a follow-up

Just keep typing. The whole exchange, including tool output, stays in context. When context gets
large cyrup compacts it automatically rather than failing.

## Leave, and come back

`Ctrl+D` on an empty prompt quits. From anywhere, `Ctrl+C` twice in quick succession does the same
— a single `Ctrl+C` only clears the editor.

Nothing needs saving; it already is. To pick the conversation back up:

```sh
cyrup --continue
```

That reopens the most recent session for this directory, with the full transcript. Use
`cyrup --resume` for a picker over earlier sessions, or `/resume` from inside a running session to
switch to a different one.

## Where to go next

[The terminal interface](../guides/tui.md) covers the rest of the keys, the slash commands, and
what each part of the screen is telling you.

[Sessions](../guides/sessions.md) covers forking, naming, branching and exporting — the things that
make a long conversation manageable.
