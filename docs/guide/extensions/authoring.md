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
preparation, markdown transformation, autocomplete suggestions, bus delivery, shortcut execution and
the six provider exports — each routed to whatever you registered on the `ExtensionApi`. You register
what you care about and ignore the rest.

(Thirty-three of the hooks mirror pi's `pi.on(...)` event catalog one for one. The thirty-fourth,
`on-terminal-input`, is the guest half of a callback pi registers as a closure; a closure cannot
cross a component boundary, so here it is an export.)

The macro compiles to nothing on non-wasm targets, so your crate still builds and tests on the host.
That is the point of keeping `rlib` in `crate-type`.

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
  "world": "cyrup:ext@0.10",
  "entry": "crates/my-ext",
  "capabilities": {
    "fs": ["read:.", "write:.cyrup/todo"],
    "exec": false,
    "net": false,
    "ui": true
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

The host world is `cyrup:ext@0.10` (`HOST_WORLD` in `crates/cyrup-ext/src/manifest.rs`, which also
carries the bump history). A manifest's `world` must declare the **same major version** as the host
and a **minor version at least** the host's. Against today's host, `cyrup:ext@0.10` is the value to
write; an older minor is a mismatch, and so is a different major.

The minor moves whenever an export is added, removed or re-signed, and whenever an import is removed
or re-signed — both of those break an already-built guest at link time. A purely additive import does
not move it. That is why the rule is one-directional: a *higher* minor than the host is accepted,
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

`exec`, `net` and `ui` are booleans, all `false` unless you say otherwise:

| Capability | Grants |
|---|---|
| `exec` | Spawning processes |
| `net` | Network access, which is never ambient under WASI p2 |
| `ui` | Drawing and interacting with the terminal interface |

**An extension with no manifest gets nothing.** cyrup will still load a bare `.wasm` — it takes the
id from the filename and the version becomes `0.0.0` — but with zero capabilities: no filesystem, no
exec, no network, no UI. Shipping an `extension.json` is the only way to ask for anything. If your
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

`crates/cyrup-ext-sdk/src/example.rs` in the cyrup repository is a working extension exported through
the same `export_extension!` macro you use. It demonstrates a permission gate on `tool_call`, a
notify hook on session start, a dynamically registered streaming tool, and custom renderers for tool
calls, tool results and transcript entries. It is exercised by cyrup's own end-to-end tests, so it
is a live example rather than a snippet that may have drifted.

Start there, delete what you do not need, and keep the manifest honest about what is left.
