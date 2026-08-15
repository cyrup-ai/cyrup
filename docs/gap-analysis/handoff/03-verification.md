# 03 — Verification

Exact commands, and the environment hazard that will otherwise cost you a day.

---

## The environment hazard — read before running anything

cyrup's three optional native extensions arm on environment flags, and **this machine's `~/.zshrc`
exports them unconditionally**:

```sh
export CYRUP_SUBAGENTS=1
export CYRUP_INTERCOM=1
export CYRUP_PERMISSION_SYSTEM=1
export CYRUP_EXPERIMENTAL=1
```

Any shell started from that profile — **including an agent's** — inherits them. That matters for two
reasons:

**The integration suite has guard tests that fail when feature flags or provider credentials leak in
from the ambient environment.** They exist so a test cannot make a real network call, and they have
caught exactly that.

**Ambient provider credentials cause hangs that look like deadlocks.** This has been misdiagnosed
here twice. What appeared to be a deadlock was the binary resolving a real provider from an ambient
`TOGETHER_API_KEY` and making a network call. Hours went into "debugging" a deadlock that did not
exist.

A `cyrup-scrub` helper is defined in `~/.zshrc` for this. Use it for any integration run:

```sh
cyrup-scrub cargo nextest run -p cyrup-it --features it
```

It is equivalent to:

```sh
env -u CYRUP_SUBAGENTS -u CYRUP_INTERCOM -u CYRUP_PERMISSION_SYSTEM -u CYRUP_EXPERIMENTAL \
    -u TOGETHER_API_KEY -u ANTHROPIC_API_KEY -u OPENAI_API_KEY \
    cargo nextest run -p cyrup-it --features it
```

> **Known unknown:** the unit gate has been passing with those three flags ambient, and the
> integration suite has **not** been run recently under any known conditions. The 473-test figure
> quoted in `README.md` was measured before this was understood. **Run the scrubbed integration
> suite once and record what you actually get** before trusting any integration number.

---

## The commands

### The everyday gate — run this centrally, after a batch

```sh
cargo check --workspace --all-targets     # type-checks everything, including tests
cargo clippy --workspace --all-targets    # REQUIRED — see below
cargo nextest run --workspace --no-fail-fast
```

Expected at HEAD `e815e08`: **7,112 passed, 8 skipped, ~18s**; check and clippy clean with 79
pre-existing warnings.

**`--no-fail-fast` is not optional.** nextest stops at the first failure by default; a run without it
once hid 1,659 unrun tests behind a single failing assertion.

**`cargo clippy` is not optional either.** The no-panic policy — `unwrap`, `expect`, `panic!`, raw
indexing — is expressed as `[workspace.lints.clippy]` denials, and **clippy tool lints do not fire
under `cargo build` or `cargo check`.** There is no CI in this repository. Nothing runs these for
you.

### Capturing output

Always redirect; never truncate something you need:

```sh
cargo nextest run --workspace --no-fail-fast > /tmp/gate.txt 2>&1; echo "exit=$?"
grep -E 'Summary|FAIL' /tmp/gate.txt
```

Piping a run through `tail -120` once discarded 3,900 results and produced a confidently wrong
conclusion.

### The integration suite — rarely, and scrubbed

```sh
cargo build -p cyrup-ext-sdk --target wasm32-wasip2      # build the WASM fixture first
cyrup-scrub cargo nextest run -p cyrup-it --features it
```

`cyrup-it` is behind `required-features = ["it"]`, so the everyday gate never builds it. Its
`build.rs` resolves fixture binaries and the WASM component once via
`cargo build --message-format=json`.

### Per-crate, for workers

```sh
cargo check -p <crate>
cargo clippy -p <crate>
```

For anything cfg-gated, check **both** arms — `crates/cyrup-ext` has `default = ["wasm-host"]`:

```sh
cargo check -p cyrup-ext
cargo check -p cyrup-ext --no-default-features
```

---

## Test architecture

**Every crate holds unit tests inline under `crates/*/src/`.** Integration tests — anything that
spawns the binary, drives a subagent, loads a WASM component or opens a broker socket — live in the
gated `crates/cyrup-it`.

Ten in-crate binaries under `crates/*/tests/` remain, and that is deliberate rather than unfinished:
`CARGO_BIN_EXE_*` is package-scoped, so a test that spawns the `cyrup` binary cannot move to
`cyrup-it`, and artifact dependencies are nightly-only. Others need their own process because they
mutate the process environment.

This layout was earned. The suite previously had **310 files** under `crates/*/tests/`, and because
Cargo makes every such file its own binary and process, a full run took **hours** to execute roughly
two minutes of assertions. One run was killed at 4h39m, incomplete. It is now ~18 seconds.

**Do not add new files under `crates/*/tests/`.** Add unit tests inline, or an integration test to
`cyrup-it`.

---

## Debugging a hang

The prescribed method, which works, and which replaced hours of re-running the suite:

1. Add file logging to the suspect path.
2. Run **once**.
3. Read the log; find where it stops.
4. Compare that exact spot to pi's equivalent symbol.
5. Align cyrup's code to pi.
6. Remove the instrumentation.

Do **not** run the suite repeatedly hoping to characterise the hang. And before concluding
"deadlock", rule out the two things that have impersonated one here: an ambient provider credential
causing a real network call, and a `MutexGuard` held across an `.await` (clippy's
`await_holding_lock` is currently at 0 — keep it there).
