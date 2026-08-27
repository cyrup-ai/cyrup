---
stage: todo
status: pending
updated: 2026-08-27
---

# Add The Three Capability Env Overrides (`IMAGE_PROTOCOL` / `TRUE_COLOR` / `HYPERLINKS`) On Top Of The Terminal Sniff

> Identified by the `cyrup-tui` ↔ `pi` port audit (fan-out survey, adversarially verified).
> **Priority:** low · **Kind:** missing-feature · **Area:** Markdown, latex, images, diffs and message rendering

## Objective

When the env sniff misidentifies a terminal — a kitty-protocol-capable emulator that sets no known
variable, a terminal that silently swallows OSC-8, a multiplexer that does forward hyperlinks — the
user currently has no escape hatch: inline images, truecolor and hyperlinks cannot be forced on or
off. Upstream one environment variable fixes each. The inner sniff itself is already a faithful
port; only the thin override wrapper around it is missing.

## Upstream reference

[`packages/tui/src/terminal-image.ts`](../../tmp/pi/packages/tui/src/terminal-image.ts) splits
detection in two — an inner env sniff and an outer override layer. cyrup ported the inner half only.

- `:69-137` — `detectCapabilitiesFromEnvironment(tmuxForwardsHyperlink)`, the sniff.
- `:139-141` — the shared parser:

  ```ts
  function parseBooleanCapabilityOverride(value: string | undefined): boolean | undefined {
      return value === "1" ? true : value === "0" ? false : undefined;
  }
  ```

  Exactly two accepted strings; anything else (including `"true"`, `"yes"`, `""`) means "no
  override".
- `:143-162` — the outer `detectCapabilities`:

  ```ts
  const hyperlinks = parseBooleanCapabilityOverride(process.env.PI_HYPERLINKS);
  const detected = detectCapabilitiesFromEnvironment(
      hyperlinks === undefined ? tmuxForwardsHyperlink : () => hyperlinks,
  );
  const imageProtocol = process.env.PI_IMAGE_PROTOCOL?.toLowerCase();
  const images =
      imageProtocol === "kitty" || imageProtocol === "iterm2" ? imageProtocol
      : imageProtocol === "none" || imageProtocol === "0" ? null
      : undefined;
  const trueColor = parseBooleanCapabilityOverride(process.env.PI_TRUE_COLOR);
  return {
      ...detected,
      ...(images !== undefined ? { images } : {}),
      ...(trueColor !== undefined ? { trueColor } : {}),
      ...(hyperlinks !== undefined ? { hyperlinks } : {}),
  };
  ```

  Two things worth spelling out. (a) `PI_HYPERLINKS` does double duty: it **replaces the tmux
  forwarding probe** that is fed *into* the sniff (`:145-147`), and it then overrides the resulting
  field (`:161`) — so setting it also means the probe subprocess is never spawned. (b)
  `PI_IMAGE_PROTOCOL` is lower-cased before matching and has a three-way result: a protocol name, an
  explicit `null` for `"none"`/`"0"`, or `undefined` for "leave the sniff alone" — `"none"` is not
  the same as unset.
- `:164-172` — `getCapabilities()` layers the separate, non-env `capabilityOverrides` seam on top of
  `detectCapabilities`, which cyrup has already ported (see below).

## Current state in cyrup-tui

- [`image.rs:504-506`](../../crates/cyrup-tui/src/image.rs) `detect_capabilities()` goes straight to
  `detect_capabilities_from(|k| std::env::var(k).ok(), probe_tmux_hyperlinks())` — the inner
  function, with the tmux probe **eagerly evaluated as an argument**.
- [`image.rs:623-629`](../../crates/cyrup-tui/src/image.rs) `detect_capabilities_from` →
  [`:635-703`](../../crates/cyrup-tui/src/image.rs) `detect_capabilities_on_platform`, which is a
  faithful port of pi's `detectCapabilitiesFromEnvironment` alone (its own doc says so) and ends at
  the conservative default at `:701`. **Do not touch it.**
- [`image.rs:553-571`](../../crates/cyrup-tui/src/image.rs) `cached_capabilities()` — pi's
  `getCapabilities` — also calls `detect_capabilities_from(env, false)` directly, so it bypasses both
  the probe and (today) any override.
- [`image.rs:707-720`](../../crates/cyrup-tui/src/image.rs) `probe_tmux_hyperlinks()` spawns
  `tmux display-message -p '#{client_termfeatures}'`. Under `PI_HYPERLINKS` upstream never calls its
  equivalent.
- The **non-env** override seam is already ported and is a different thing — do not conflate them:
  `set_capabilities` ([`:575-579`](../../crates/cyrup-tui/src/image.rs)),
  `reset_capabilities_cache` ([`:583-587`](../../crates/cyrup-tui/src/image.rs)),
  `seed_hyperlink_support` ([`:595-605`](../../crates/cyrup-tui/src/image.rs)) and
  `seed_capabilities` ([`:608-614`](../../crates/cyrup-tui/src/image.rs)) port pi's
  `capabilityOverrides`/cache mutators (`terminal-image.ts:164-172`), not the env layer.
- `grep -rn 'PI_IMAGE\|PI_TRUE\|PI_HYPER' crates/` is **empty**. The crate's entire env surface today
  is `HOME`, `TMUX`, `XDG_SESSION_TYPE`, `TERM_PROGRAM`, `TERMINAL_EMULATOR`, `TERM`, `COLORTERM`,
  `COLORFGBG`, `CYRUP_SHARE_VIEWER_URL` and `CYRUP_EXPERIMENTAL`/`PI_EXPERIMENTAL`.
- The record being overridden is `TerminalCapabilities { images: Option<ImageProtocol>, true_color:
  bool, hyperlinks: bool }` ([`image.rs:483-490`](../../crates/cyrup-tui/src/image.rs)) with
  `ImageProtocol { Kitty, Iterm2 }` ([`:471-476`](../../crates/cyrup-tui/src/image.rs)) — so pi's
  `"kitty" | "iterm2" | null | undefined` maps onto `Option<Option<ImageProtocol>>` at the override
  layer.
- **House convention for env names:** accept `CYRUP_*` with a `PI_*` fallback, as
  [`status.rs:480-483`](../../crates/cyrup-tui/src/status.rs)
  `experimental_features_enabled_from` does for `CYRUP_EXPERIMENTAL` / `PI_EXPERIMENTAL`. Follow it
  for all three keys.

## Subtasks

1. **Add the boolean parser** in [`image.rs`](../../crates/cyrup-tui/src/image.rs) — pi's
   `parseBooleanCapabilityOverride` (`terminal-image.ts:139-141`): `Some(true)` for `"1"`,
   `Some(false)` for `"0"`, `None` for everything else. Keep it strict; do not accept `true`/`yes`.
2. **Add the image-protocol parser**, returning a three-state result
   (`Option<Option<ImageProtocol>>`): lower-cased `"kitty"` → `Kitty`, `"iterm2"` → `Iterm2`,
   `"none"` or `"0"` → an explicit "no protocol", anything else → no override
   (`terminal-image.ts:148-154`).
3. **Add the outer wrapper**, an env-injected `detect_capabilities_with_overrides(env, probe)` beside
   [`image.rs:504`](../../crates/cyrup-tui/src/image.rs), taking the env lookup as a closure (the
   crate's existing testability seam, matching `detect_capabilities_from` at `:623` and
   `experimental_features_enabled_from` at `status.rs:480`) and the tmux probe **lazily** — a
   closure or a pre-resolved value chosen after reading the hyperlinks override, so a set
   `CYRUP_HYPERLINKS`/`PI_HYPERLINKS` short-circuits `probe_tmux_hyperlinks`
   ([`:707-720`](../../crates/cyrup-tui/src/image.rs)) instead of spawning tmux. Feed the override
   into the sniff as the forwarding flag AND apply it to the resulting field, both, per
   `terminal-image.ts:145-147` + `:161`.
4. **Route both entry points through it**: `detect_capabilities`
   ([`:504-506`](../../crates/cyrup-tui/src/image.rs)) and the detection inside
   `cached_capabilities` ([`:553-571`](../../crates/cyrup-tui/src/image.rs)), so an override holds
   however the record is first obtained.
5. **Leave the sniff and the `capabilityOverrides` seam alone** —
   `detect_capabilities_on_platform` ([`:635-703`](../../crates/cyrup-tui/src/image.rs)),
   `set_capabilities`, `reset_capabilities_cache`, `seed_hyperlink_support` and `seed_capabilities`
   are all correct as they stand. The env layer sits between them.
6. **Document the three variables** in the module doc of
   [`image.rs`](../../crates/cyrup-tui/src/image.rs) with their accepted values, since they are a
   user-facing escape hatch and nothing else in the crate advertises them.

## Acceptance criteria

- [ ] `grep -rn 'IMAGE_PROTOCOL\|TRUE_COLOR\|HYPERLINKS' crates/cyrup-tui/src/image.rs` shows all
      three keys read, each with a `CYRUP_*` primary and a `PI_*` fallback (house convention,
      [`status.rs:480-483`](../../crates/cyrup-tui/src/status.rs))
- [ ] `…_TRUE_COLOR=1` forces `true_color == true` and `=0` forces `false`, on a terminal whose sniff
      says the opposite; any other value (`"true"`, `"yes"`, `""`) changes nothing
- [ ] `…_HYPERLINKS=1` / `=0` likewise forces `hyperlinks`
- [ ] With `…_HYPERLINKS` set to `1` or `0`, `probe_tmux_hyperlinks`
      ([`image.rs:707`](../../crates/cyrup-tui/src/image.rs)) is **not** called — no `tmux`
      subprocess is spawned
- [ ] With `…_HYPERLINKS` set, the same value is fed into the sniff as the tmux-forwarding flag AND
      applied to the returned field (`terminal-image.ts:145-147` and `:161`)
- [ ] `…_IMAGE_PROTOCOL=kitty` / `=iterm2` force that protocol, case-insensitively; `=none` and `=0`
      force "no protocol"; any other value leaves the sniffed result untouched — four distinct
      outcomes, not three
- [ ] `detect_capabilities_on_platform`
      ([`image.rs:635-703`](../../crates/cyrup-tui/src/image.rs)) is byte-identical to before this
      change
- [ ] The override layer is reachable from both `detect_capabilities` and `cached_capabilities`
- [ ] The wrapper takes its environment through an injected lookup, so it can be exercised without
      mutating the process env
- [ ] `cargo build -p cyrup-tui --all-targets` — 0 errors, 0 warnings
- [ ] `cargo clippy -p cyrup-tui --all-targets` — warning count not increased
- [ ] `cargo test -p cyrup-tui` — no pre-existing test in `src/tests/image_capabilities.rs` or
      `src/tests/image.rs` regresses

## Constraints

- No tests are to be written for this task; another team owns the test suite.
- No benchmarks are to be written for this task.
- Workspace lints deny unwrap_used, expect_used, panic and indexing_slicing; cyrup-tui also has
  forbid(unsafe_code) and deny(clippy::string_slice).
- Mechanical fidelity to pi where pi's behaviour is the spec; Rust idiom where it is not.
