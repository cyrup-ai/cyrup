# Install

cyrup builds from source with cargo. This page gets you a working binary, and explains what
installation touches and what it leaves alone.

## Prerequisites

A stable Rust toolchain, 1.96 or newer. If you do not have one, install
[rustup](https://rustup.rs) and then:

```sh
rustup toolchain install stable
```

That is the whole list. cyrup is a single static binary — no Node, no Python, no runtime to install
alongside it. You do not need the `wasm32-wasip2` target either; that is only for authoring your
own WASM extensions.

## Install

```sh
cargo install --git https://github.com/cyrup-ai/cyrup cyrup
```

This builds and installs exactly one binary, `cyrup`, into `~/.cargo/bin/cyrup`.

**The first build takes several minutes.** cyrup compiles a WebAssembly runtime, a git
implementation and a TLS stack from source, and cargo goes quiet for a long stretch in the middle.
It has not hung. Let it finish; subsequent builds reuse the cache and are much faster.

Confirm it landed:

```sh
cyrup --version
```

It prints `0.0.0`. If the shell reports command not found, `~/.cargo/bin` is not on your `PATH`.

## What installation writes

The binary, and nothing else. cyrup creates no configuration until you use it — the agent
directory `~/.cyrup/agent` and the files in it (`settings.json`, `auth.json`, `trust.json`) appear
the first time you log in, change a setting, or answer a trust prompt. A fresh install leaves your
home directory untouched.

To remove it, `cargo uninstall cyrup`. Add `rm -rf ~/.cyrup` if you also want the configuration and
saved sessions gone.

## Upgrading

Re-run the install command. cargo rebuilds from the current tip and replaces the binary in place.

```sh
cargo install --git https://github.com/cyrup-ai/cyrup cyrup
```

`cyrup update` does not update cyrup itself. The self-update path writes three lines to stderr and
exits 1:

```text
error: cyrup cannot self-update this installation.
Update it with: cargo install --git https://github.com/cyrup-ai/cyrup cyrup

Location of cyrup executable: /Users/you/.cargo/bin/cyrup
```

That covers bare `cyrup update`, `--self`, `--all`, `--force`, and the three positional aliases
`cyrup update cyrup` / `self` / `pi`.

Two `update` targets do work, and neither touches the binary: `cyrup update --extensions` updates
installed [packages](../extensions/overview.md), and `cyrup update --models` refreshes the remote
model catalogs for the providers you have authenticated.

## Building from a clone

If you want to read or change the source:

```sh
git clone https://github.com/cyrup-ai/cyrup
cd cyrup
cargo build --release
```

The binary lands at `target/release/cyrup`. Run it in place or copy it onto your `PATH`. The
repository pins the stable toolchain along with rustfmt and clippy, so rustup selects the right one
when you enter the directory. The source is MIT licensed.

## Distribution channels

There is no crates.io release and no Homebrew tap. Installing from git is the only supported path
today, and this page will change when that changes.

## Next

[Connect a provider](authenticate.md). cyrup needs credentials for at least one model provider
before it can answer anything.
