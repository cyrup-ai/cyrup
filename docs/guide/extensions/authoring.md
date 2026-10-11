# Writing an extension

An extension is a Rust crate compiled to a WebAssembly component. This page takes you from an empty
crate to a component cyrup loads, and covers the manifest that decides what it is allowed to touch.

Read [How extensions work](overview.md) first if you have not — the capability model is the part
that shapes how you write the thing.

## Before you start

```sh
rustup target add wasm32-wasip2
```

That is the whole toolchain requirement. The wasip2 linker produces a component directly, so there
is no `wasm-tools` step and nothing else to install.

There is no scaffolding command and no project template. You start from a normal crate and the
reference example described at the end of this page.

## Crate setup

```toml
[package]
name = "my-ext"
version = "0.1.0"
edition = "2024"

[lib]
crate-type = ["cdylib", "rlib"]

[dependencies]
cyrup-ext-sdk = { path = "../cyrup/crates/cyrup-ext-sdk" }
```

`cdylib` is what produces the component; `rlib` keeps the crate usable as an ordinary library so
`cargo test` still runs your logic on the host.

Then **vendor the shared `wit/` directory**. Copy `crates/cyrup-ext-sdk/wit/` from the cyrup
repository to `wit/` at the root of your crate. The SDK's bindings resolve against that directory,
and it must match the host's copy.

Your layout ends up:

```text
my-ext/
  Cargo.toml
  wit/
    world.wit
  src/
    lib.rs
  extension.json
```

## The factory and the macro

An extension is a function returning an `ExtensionApi`, plus one macro invocation that turns it into
a component:

```rust
use cyrup_ext_sdk::prelude::*;

fn build() -> ExtensionApi {
    let mut api = ExtensionApi::new();
    api.on_tool_call(|ev, _ctx| {
        if ev.name == "bash" { Outcome::block("no") } else { Outcome::noop() }
    });
    api
}

cyrup_ext_sdk::export_extension!(build);
```

`export_extension!` emits every guest export the world requires — initialisation, all 34 `on-*`
hooks, tool execution, command execution, argument completions, call and result renderers, argument
preparation, loadout preparation, markdown transformation, autocomplete suggestions, bus delivery, shortcut execution and
the six provider exports — each routed to whatever you registered on the `ExtensionApi`. You register
what you care about and ignore the rest.

(Thirty-three of the hooks mirror pi's `pi.on(...)` event catalog one for one. The thirty-fourth,
`on-terminal-input`, is the guest half of a callback pi registers as a closure; a closure cannot
cross a component boundary, so here it is an export.)

The macro compiles to nothing on non-wasm targets, so your crate still builds and tests on the host.
That is the point of keeping `rlib` in `crate-type`.

## Tools that call tools

A tool's `execute` can run other tools through the same validation, hooks and permission checks as a
call the model made. In a guest, the `ToolCall` your executor receives carries it:

```rust
define_tool(descriptor, |call: ToolCall| {
    let outcome = call
        .execute_tool("read", json!({ "path": "notes.md" }), ExecuteToolOptions::default())
        .map_err(|e| e.to_string())?;
    Ok(ToolOutput::text(outcome.result.text()))
})
```

`call.tools()` lists what `execute_tool` can reach: the active `direct` tools and every registered
`codemode` or `deferred` one. The call gets the id `<your call id>/<n>`, never enters the transcript,
and is recorded, with its usage, as `nestedCalls` on your tool's result. A tool that fails comes
back as an outcome with `is_error` set; `Err` means the call could not be made at all (the host's
text says why).

Four things differ from pi's `ctx.executeTool`. Pi's `signal` option is the named signal
`ExecuteToolOptions::signal_id` (the id `ctx.ui().abort_signal(..)` aborts, read once when the call
starts, so a signal you aborted beforehand cancels the call before it begins) together with
`ExecuteToolOptions::timeout_ms`, a deadline the host enforces, because a guest suspended inside the
call has no timer and nothing else can fire an abort while it waits; the call is always also
cancelled with your own call. `ExecuteToolOptions::on_update` is invoked with the nested tool's
partial results after the call settles, not as they stream; the same results reach
`tool_execution_update` events live. An instance runs one call at a time, so a call to a tool of
your own extension comes back as an error outcome naming the cause instead of waiting for itself.
And the nested call's events are not given to your own handlers (`tool_call`, `tool_result` and
`tool_execution_*`) while your tool is making the call, though every other extension receives them.
The last three are the single-instance store, not omissions: lifting them would make a guest
reentrant.

A native extension reads the same context inside its tool's `execute`:
`cyrup_ext::ExtensionToolContext::current()` gives `execute_tool(name, args, options)` bound to the
running call, and `tools()`. Read it at the top of `execute` and move the clone into anything you
spawn; the binding is scoped to the call's own future, so two calls running at once never see each
other's id.

Your handlers tell a nested call from a model-issued one by `parent_tool_call_id` on the event
(`ToolCallEvent`, `ToolResultEvent` and the three tool-execution events in a guest;
`HostCtx::parent_tool_call_id()` in a native). A `tool_call` gate needs no change to cover nested
calls: they reach it as the same event.

A `tool_result` handler sees the tool's machine-readable result as `structured_content` (a tool
that declares an `outputSchema` sets it, and a codemode script receives it instead of the text) and
replaces it through `ToolResultPatch::structured_content` (`EventPatch::ToolResult` in a native).
Replacing `content` without returning `structured_content` drops it, because it may no longer
match the text; return it along with `content` to keep it. Handlers chain in load order, and a
later handler that only touches `details` keeps what an earlier one set.

## Calling models

An extension can make its own model calls — to summarise, classify, or run a review pass — through
the providers the user has already configured, with their credentials, instead of carrying an HTTP
client and an API key of its own. It needs `"modelCalls": true` in its manifest (see
[Capabilities](#capabilities)).

```rust
let reply = ctx.models().complete(
    &json!({ "provider": "anthropic", "id": "claude-sonnet-4-5" }),
    &json!({
        "systemPrompt": "Answer in one line.",
        "messages": [{ "role": "user", "content": "Summarise the diff.", "timestamp": 0 }]
    }),
    &json!({ "maxTokens": 200 }),
)?;
if reply["stopReason"] == "error" {
    ctx.ui().notify(&format!("model failed: {}", reply["errorMessage"]));
} else {
    ctx.ui().notify(&message_text(&reply));
}
```

The three arguments are pi's: the model, a context of a system prompt, messages and optional tools,
and an options bag (`maxTokens`, `temperature`, `sessionId`, `cacheRetention`, `headers`,
`metadata`, `samplingParams`, `timeoutMs`, `maxRetries`, `maxRetryDelayMs`). Pass any row from
`ctx.models().list()` or `find(provider, id)`, or just `{provider, id}`; only those two keys are
read, and the host looks the model up in its own catalog, so a `baseUrl` you put in the object is
ignored rather than handed the user's key.

A failed request does not come back as `Err`. An unknown model, a provider without credentials, or a
network error all settle as a reply whose `stopReason` is `"error"` and whose `errorMessage` says
why, exactly as pi's `complete` resolves. `Err` means the call could not be made: the capability is
missing, an argument is malformed, or the extension already has 16 streams open.

`complete` holds your extension for the length of the call; none of its other handlers run until
it returns, and the host cuts it off after ten minutes. For anything long, stream instead:

```rust
let mut stream = ctx.models().stream_simple(&model, &context, &json!({ "reasoning": "low" }))?;
while let Some(event) = stream.next_event()? {
    if event["type"] == "text_delta" {
        // event["delta"] is the next piece of text
    }
}
```

Each event is pi's `AssistantMessageEvent`: `start`, the `text_*`, `thinking_*` and `toolcall_*`
triples, then exactly one `done` or `error`, after which `next_event` returns `None`.
`stream.result()` drains to that terminal and returns the message, and `stream.poll()` returns
whatever has arrived since the last poll, possibly nothing, without waiting more than about a
second. The host owns the provider request, so a stream is a handle: dropping it, or calling
`close()`, cancels the request. One you stop polling is closed after two minutes, and every stream
an extension has open is closed when it is unloaded or traps.

`stream` and `stream_simple` differ the way pi's do. `stream_simple` takes provider-neutral options,
`reasoning` among them, and is the only one of the three that routes a
[virtual model](virtual-models.md): it asks the router with reason `direct` and caps `maxTokens` to
the model it picks. `complete` and `stream` on a virtual model end with "must be routed before
streaming". A virtual model your own extension routes cannot be called from that extension,
because its instance is busy making the call; the host refuses that call instead of waiting on
itself.

What these calls are not is the session's own requests, and neither are they in pi. No
`before_provider_request`, `before_provider_headers` or `after_provider_response` handler sees them,
no attribution headers or session id are added, and their usage is not added to the session's cost;
the reply carries its own `usage`. A call to another provider's model reaches that provider without
switching the model the conversation is using. `apiKey`, `env` and pi's callback options do not
cross: the call always runs on the user's resolved credentials, and cancelling it is `close()`.

A native extension makes the same calls through the host services it is handed in
`set_host_services`: `services.model_complete(call)` and `services.model_stream(call)`, where `call`
is a `cyrup_ext::host::ModelCall` (verb, provider, model id, context, options and a `CancelToken`
that stands in for pi's `signal`). Natives are trusted in-tree code with no manifest, so no
capability gates them.

## Editing the system prompt

A `before_agent_start` handler is handed the options the prompt is built from as well as the prompt
itself (`event.options`, pi's `systemPromptOptions`). Edit a copy and return it, and the next handler —
in your extension or another — reads the prompt those options render to, and so does the model:

```rust
api.on_before_agent_start(|ev, _ctx| {
    let mut options = ev.options.clone();
    options["sections"]["team"] = json!("Prefer small, reviewed changes.");
    Outcome::before_agent_start(BeforeAgentStartResult {
        system_prompt_options: Some(options),
        ..Default::default()
    })
});
```

`sections` adds XML-wrapped sections after `<cwd>`; a name the prompt already has (`rules`, `tools`,
…) replaces that section where it stands. A name must match `^[a-z][a-z0-9_-]*$` and may not be
`preamble`, or the prompt is refused. Editing `selectedTools` changes which tools the run has.
Returning `system_prompt` instead replaces the whole prompt the model is sent for this run; the
session file keeps the structured sections either way.

## Run boundaries

`turn_end` and `agent_before_settle` are boundaries: a handler may append entries to the session and
ask for one more provider request. `agent_before_settle` runs once nothing else — a retry, a
compaction, a queued message — would continue the run.

```rust
api.on_turn_end(|ev, _ctx| {
    if wants_another_look(&ev) {
        return Outcome::boundary(BoundaryResult {
            entries: Some(json!([{
                "type": "custom_message",
                "customType": "review",
                "content": "Check the diff once more before finishing.",
                "display": true,
            }])),
            continue_: Some(true),
        });
    }
    Outcome::noop()
});
```

An entry is one of pi's drafts: `custom` (state only), `custom_message` (model context),
`context_edit` (`{targetId, replacement: {content} | null}` — replace or omit an earlier entry's
contribution) or `compaction` (`{summary, firstKeptEntryId}`; `null` keeps nothing before it).
`ev.boundary.context` previews the model context with the drafts so far appended, rebuilt after each
handler; a continuation is honoured only when that context can be continued from — a context that
ends on the assistant's reply needs something after it. `turn_end` also names the entries the turn was
persisted as (`message_entry_id`, `tool_result_entry_ids`), which a `context_edit` can target.

## Structured results, annotations and loadout hooks

A tool can declare the shape of a machine-readable result with
`ToolDescriptor::output_schema(schema)` and return it with
`ToolOutput::with_structured_content(value)`. The model still reads `content`; a codemode script
that calls the tool receives the structured value instead of the text. A result for which
`ToolOutput::error(..)` was used is reported as a failed call (`is_error`), and its structured
content is kept for programmatic callers.

`ToolDescriptor::annotations(ToolAnnotations::new().read_only(true)..)` attaches MCP-style hints
(`readOnlyHint`, `destructiveHint`, `idempotentHint`, `openWorldHint`) that `getAllTools` reports so
a permission extension can decide which calls to confirm. They are the author's claim and are not
verified.

A tool that orchestrates other tools can adjust how the loadout is presented to the model by
implementing `ToolExec::prepare_loadout` and setting `ToolDescriptor::prepare_loadout(true)`. The hook
gets the declared, callable and registered tools and returns `ToolLoadoutChanges`: new descriptions
for declared tools, and declared tools whose declarations requests should leave out. The host calls
it whenever it applies the active tools. It cannot call tools, and while the extension is busy with
another call the host reuses the hook's previous answer.

## Building

```sh
cargo build --target wasm32-wasip2 --release
```

The result is `target/wasm32-wasip2/release/my_ext.wasm`, and it is already a component. Nothing
post-processes it.

## The manifest

`extension.json` sits beside the artifact and is how you ask for capabilities:

```json
{
  "id": "my-ext",
  "version": "1.0.0",
  "world": "cyrup:ext@0.20",
  "entry": "crates/my-ext",
  "capabilities": {
    "fs": ["read:.", "write:.cyrup/todo"],
    "exec": false,
    "net": false,
    "ui": true,
    "modelCalls": false
  }
}
```

| Field | Required | Meaning |
|---|---|---|
| `id` | yes | The extension's identity; must not collide with another loaded extension |
| `version` | yes | Your version string |
| `world` | yes | The WIT world this component was built against |
| `entry` | no | Path to the source crate, for an in-host build |
| `capabilities` | no | What the extension may touch; everything denies by default |

### World compatibility

The host world is `cyrup:ext@0.20` (`HOST_WORLD` in `crates/cyrup-ext/src/manifest.rs`, which also
carries the bump history). A manifest's `world` must declare the **same major version** as the host
and a **minor version at least** the host's. Against today's host, `cyrup:ext@0.20` is the value to
write; an older minor is a mismatch, and so is a different major.

The minor moves whenever an export is added, removed or re-signed, and whenever an import is removed
or re-signed — both of those break an already-built guest at link time. A purely additive import does
not move it. `0.19` is the current value because `events.on-tool-execution-end` now carries how long
the tool ran and `events.on-turn-end` returns a result (see [Run boundaries](#run-boundaries)), so a
`0.18` component exports the older signatures; `0.18` was the prompt-cache warming
decision an extension can override, a guest export a `0.17` component does not have; `0.17` was the route callback of a virtual model (see [Virtual models](virtual-models.md)),
which a `0.16` component does not have. That is why the rule is one-directional: a *higher* minor than the host is accepted,
a lower one is refused.

A mismatch gives you a clear version error naming the problem. It does not become a link failure
inside the WebAssembly runtime, so you find out what is wrong from the message rather than from a
stack trace.

### Capabilities

Every field in the block defaults to the denying value, and the host enforces the grant — it is
handed to your component as data at instantiation, and there is no way to widen it from inside.

`fs` is a list of grants, each `read:<path>` or `write:<path>`:

```json
{ "fs": ["read:.", "write:.cyrup/todo"] }
```

- Paths are **relative to the project cwd**. An absolute path or a `..` component is a hard error
  that names the offending string and fails the load. This is deliberate: a typo that silently
  widened the sandbox is exactly the failure this refuses to have.
- `write` implies read on the same subtree. You cannot write what you cannot address, so
  `write:.cyrup/todo` does not need a matching `read:` entry.
- An empty list denies filesystem access outright.

`exec`, `net`, `ui` and `modelCalls` are booleans, all `false` unless you say otherwise:

| Capability | Grants |
|---|---|
| `exec` | Spawning processes |
| `net` | Network access, which is never ambient under WASI p2 |
| `ui` | Drawing and interacting with the terminal interface |
| `modelCalls` | [Calling models](#calling-models) through the user's configured providers, with their credentials |

`modelCalls` is separate from `net` on purpose. `net` reaches endpoints you name with credentials you
bring; a model call reaches the user's providers with the user's credentials and spends their
money. Reading the model catalog (`ctx.models().list()`, `current()` and the rest) needs neither.

**An extension with no manifest gets nothing.** cyrup will still load a bare `.wasm` — it takes the
id from the filename and the version becomes `0.0.0` — but with zero capabilities: no filesystem, no
exec, no network, no UI, no model calls. Shipping an `extension.json` is the only way to ask for anything. If your
extension mysteriously cannot read a file, check that the manifest is present and that it parses;
a malformed one falls back to the same zero-capability path, with a warning.

### The entry field

`entry` points at a source crate rather than a built artifact. When it is set and no prebuilt
`.wasm` is present, cyrup runs `cargo build --target wasm32-wasip2` itself, content-addressed and
cached so it does not rebuild on every start.

That is convenient while you are developing — edit, restart cyrup, the change is live. If the wasm
toolchain is missing, the failure is reported as a build failure rather than crashing the host.

## Loading your extension

Five ways, all equivalent once the component is built:

```sh
cyrup -e ./target/wasm32-wasip2/release/my_ext.wasm
```

`-e` also accepts a directory holding your `extension.json` plus the artifact, or a directory of
several such extensions. It works regardless of project trust and survives `--no-extensions`, which
makes it the right form for development:

```sh
cyrup --no-extensions -e ./my-ext/
```

The other four:

- drop the `.wasm` file, or a directory containing `extension.json` and the artifact, into
  `~/.cyrup/agent/extensions/` — loads in every project;
- drop it into `<project>/.cyrup/extensions/` — loads once you have trusted the project;
- list its path in the `extensions` array of your `settings.json`;
- ship it inside a package, either as a `[resources] extensions` entry in `cyrup.toml` or in a
  conventional `extensions/` directory, and `cyrup install` that package. See
  [Installing extensions](managing.md).

Remember that discovery of the two directory roots is one level deep. The `.wasm` or the extension
directory goes directly in `extensions/`, not nested inside another folder.

## The reference implementation

`crates/cyrup-ext-sdk/src/example/` in the cyrup repository is a working extension exported through
the same `export_extension!` macro you use. It demonstrates a permission gate on `tool_call`, a
notify hook on session start, a dynamically registered streaming tool, custom renderers for tool
calls, tool results and transcript entries, a bash backend, a virtual model with a router, and
commands that make model calls through the session's providers. It is
exercised by cyrup's own end-to-end tests, so it is a live example rather than a snippet that may
have drifted.

Start there, delete what you do not need, and keep the manifest honest about what is left.
