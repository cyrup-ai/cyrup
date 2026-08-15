# How extensions work

An extension is a WebAssembly component that cyrup loads into your session and runs inside a
capability sandbox the host enforces. This page covers what an extension can and cannot reach,
where cyrup looks for them, and the three native extensions that ship inside the binary.

## What an extension is

Extensions are compiled WebAssembly components, not scripts. There is no JavaScript runtime, no
plugin interpreter, and nothing to install alongside cyrup to make one work. A component gets
loaded, gets its declared capabilities handed to it as data at instantiation time, and then runs
against the same hooks cyrup uses internally — tool calls, session lifecycle, rendering, slash
commands.

An extension can register tools and slash commands, gate or rewrite tool calls, draw custom rows in
the transcript, and take part in the session's lifecycle. What it *cannot* do is reach anything its
manifest did not ask for.

## The capability sandbox

Every extension ships an `extension.json` manifest that declares what it needs:

```json
{
  "id": "todo",
  "version": "1.0.0",
  "world": "cyrup:ext@0.8",
  "capabilities": {
    "fs": ["read:.", "write:.cyrup/todo"],
    "exec": false,
    "net": false,
    "ui": true
  }
}
```

Four capabilities exist: filesystem (`fs`, as a list of read or write grants scoped to relative
subtrees of your project), process execution (`exec`), network access (`net`), and user-interface
access (`ui`). Every one of them defaults to the denying value.

**An extension gets nothing it did not declare.** A component with no `capabilities` block has no
filesystem, no exec, no network and no UI. The grant is enforced host-side, so an extension cannot
widen it at runtime — it cannot opt itself back in, and there is no runtime permission prompt that
grants it more.

That is also why a manifest is the only way to ask for anything. See
[Writing an extension](authoring.md) for how to write one.

## Where cyrup looks

Three roots, scanned in this order:

| Root | Loaded |
|---|---|
| `<cwd>/.cyrup/extensions/` | only after you trust the project |
| `~/.cyrup/agent/extensions/` | always, before any trust decision |
| every `-e <path>` you pass | always, before any trust decision |

The middle one is the [agent directory](../reference/settings.md), `~/.cyrup/agent` by default. The
project root is gated on [project trust](../guides/project-context.md): until you approve a
repository, extensions sitting in its `.cyrup/extensions/` are skipped, and skipping them is not an
error.

Two more sources fold into the same tier as `-e`: the `extensions` array in `settings.json`, and the
extension directories contributed by an installed package. Both are covered in
[Installing extensions](managing.md).

If the same extension is reachable through more than one root, the first one wins — project, then
global, then the explicit paths — matched on the resolved path.

## What counts as an extension on disk

Inside a discovery root, cyrup accepts two shapes:

- a bare `*.wasm` file sitting directly in the root;
- a subdirectory containing an `extension.json`, or containing a `*.wasm` file.

**Discovery is one level deep and does not recurse.** `~/.cyrup/agent/extensions/todo/todo.wasm`
is found. `~/.cyrup/agent/extensions/team/todo/todo.wasm` is not — the intermediate directory holds
neither a manifest nor a `.wasm`, and cyrup does not descend past it. Entries are scanned in sorted
order so a given set of files always loads the same way.

A directory with no `extension.json` still loads: cyrup takes the first `*.wasm` it finds, names the
extension after the file, and grants it **zero capabilities**. A directory whose `extension.json`
exists but is malformed behaves the same way and additionally warns you, naming the file and telling
you the extension now has no declared capabilities. If an extension you wrote suddenly cannot touch
the filesystem, that warning is the first thing to look for.

A genuine load fault — a component that will not instantiate, a built-in whose initialisation fails —
is reported as `Error: Failed to load extension "<path>": <error>` and stops startup with exit 1,
along with the line `Hint: Start without extensions using "cyrup -ne".` A name collision is fatal
the same way: two
extensions registering the same tool or the same command-line flag produces
`Tool "<name>" conflicts with <owner>` / `Flag "--<name>" conflicts with <owner>`, and the first
registration is the one that runs — not a silent override.

Three things are reported but *not* fatal: the project-trust skip above, a malformed
`extension.json` (which falls back to the zero-capability path), and a `-e` argument that names
nothing loadable. Those are warnings; startup continues.

## Loading one extension explicitly

```sh
cyrup -e ./target/wasm32-wasip2/debug/my_ext.wasm
```

`-e` (long form `--extension`) is repeatable and accepts a `.wasm` file, a single extension
directory, or a directory holding several extensions. Relative paths resolve against the directory
you ran cyrup from. A path that is none of those three things produces a diagnostic naming the
reason and startup continues.

## Turning extensions off

```sh
cyrup --no-extensions
```

`-ne` is accepted as a short form. This is broader than it sounds. It stops cyrup scanning the
project root and the global root, drops everything the `extensions` array of `settings.json` and
installed packages contribute, and disables all three native extensions described below.

What it does *not* disable is anything you passed with `-e`. Explicit paths always load. That
combination — everything off except the one component you name — is the usual way to test an
extension you are working on:

```sh
cyrup --no-extensions -e ./target/wasm32-wasip2/debug/my_ext.wasm
```

One further carve-out applies only inside a [subagent](subagents.md) child process: a child run with
`--no-extensions` keeps the permission system, the subagents runtime and its prompt runtime, because
its parent re-injects those deliberately. Turning the permission gate off in a parent therefore does
not turn it off in that parent's children. Intercom is not on that list and does go away.

## The three native extensions

Three larger subsystems are compiled into the binary rather than loaded from disk. They are native
extensions, and they behave like extensions in every way that matters: they register tools, slash
commands and UI, and `--no-extensions` turns them off along with everything else.

**All three are off by default.** Each one arms on its own environment variable *or* on the mere
presence of its config file. The config-file half is the one that surprises people — you can turn a
native extension on without ever setting a variable, by creating a file and nothing else.

[Subagents](subagents.md) delegates work to child `cyrup` processes, each with its own persona,
model, tool set and depth budget. Runs go in the foreground or the background, in chains, in
parallel fan-out, and optionally inside an isolated git worktree. Arm it with `CYRUP_SUBAGENTS=1` or
by creating `~/.cyrup/agent/subagents/config.json`.

[The permission system](permissions.md) is an allow / ask / deny gate in front of every tool call,
driven by a layered policy file. Arm it with `CYRUP_PERMISSION_SYSTEM=1` — or by dropping a
`cyrup-permissions.jsonc` policy file anywhere it looks, which is enough on its own. That page leads
with the consequences.

[Intercom](intercom.md) is a Unix-socket broker that lets concurrent cyrup sessions and subagent
children find each other and exchange messages, asks and replies. Arm it with `CYRUP_INTERCOM=1` or
by creating `~/.cyrup/agent/intercom/config.json`. Unix only in this milestone.

## Where to go next

To install someone else's package, read [Installing extensions](managing.md). To build your own,
read [Writing an extension](authoring.md). For the tool allowlist and the built-in tools that
extensions sit alongside, read [Tools and permissions](../guides/tools-and-permissions.md).
