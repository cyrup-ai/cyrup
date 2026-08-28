---
title: "CYRUP-DELTA capability gap at crates/cyrup-tools/src/isolation/mod.rs:12"
priority: MEDIUM
crate: cyrup-tools
source: CYRUP-DELTA classification audit (workflow wf_12c49023-adf)
stage: aug
status: done
updated: 2026-08-28 21:00
---

# Protected paths: the marker's premise is false, and the guard is **narrower** than pi's

**Marker site:** [`crates/cyrup-tools/src/isolation/mod.rs`](../../../crates/cyrup-tools/src/isolation/mod.rs),
the `ProtectedFs` / `ProtectedPaths` bullet (`:10-16` at time of writing — anchor on the literal
string `**[CYRUP-DELTA], and off by default**`, line numbers in this area have drifted twice).

The marker asserts:

> `pi has no protected-path concept — core/tools/write.ts:195-225 @v0.83.0 writes whatever path it is handed.`

**That sentence is false at `e8682309`, and the direction of the surviving divergence is the
opposite of what the marker claims.** This is not an ADD-direction gap where cyrup has a guardrail
pi lacks and "closing it" means deleting a security control. Both projects ship the same opt-in
guard over the same three names, both off by default — and cyrup's version **fails to protect
`.env.local` / `.env.production` / `.env.development`, which pi's does protect.** cyrup is the
one missing coverage.

---

## Verified facts (all re-derived this pass; anchor on symbols, the line numbers are hints)

### F1 — pi has a protected-path concept, shipped, documented, and auto-discovered

[`tmp/pi/packages/coding-agent/examples/extensions/protected-paths.ts`](../../../tmp/pi/packages/coding-agent/examples/extensions/protected-paths.ts)
@ `e8682309`, complete (30 lines), the operative part verbatim:

```ts
const protectedPaths = [".env", ".git/", "node_modules/"];

pi.on("tool_call", async (event, ctx) => {
	if (event.toolName !== "write" && event.toolName !== "edit") return undefined;
	const path = event.input.path as string;
	const isProtected = protectedPaths.some((p) => path.includes(p));
	if (isProtected) {
		if (ctx.hasUI) ctx.ui.notify(`Blocked write to protected path: ${path}`, "warning");
		return { block: true, reason: `Path "${path}" is protected` };
	}
	return undefined;
});
```

Not stray sample code — catalogued in the Examples Reference under **Events & Gates**:
[`docs/extensions.md:2944`](../../../tmp/pi/packages/coding-agent/docs/extensions.md) —
`| protected-paths.ts | Block writes to specific paths | on("tool_call") |`, beside
`permission-gate.ts`. And auto-discovered without a flag or a rebuild:
[`src/core/extensions/loader.ts`](../../../tmp/pi/packages/coding-agent/src/core/extensions/loader.ts),
`loadExtensions` (`:648`) → `// 1. Project-local extensions: cwd/${CONFIG_DIR_NAME}/extensions/`
(`:779`) and `// 2. Global extensions: agentDir/extensions/` (`:783`), documented as four discovery
shapes at `docs/extensions.md:113-119`.

`tool_call` block is fail-safe and reaches the model as a tool error (`docs/extensions.md:2904`).

**What remains true:** pi has no protected-path predicate in *core* —
`grep -rni 'protectedpath|protected_path' tmp/pi/packages/coding-agent/src/` returns **zero**;
[`core/tools/write.ts`](../../../tmp/pi/packages/coding-agent/src/core/tools/write.ts)
`createWriteToolDefinition` (`:187`) → `resolveToCwd(path, cwd)` (`:208`) → `ops.mkdir(dir)` (`:221`)
→ `ops.writeFile(absolutePath, content)` (`:225`), no predicate;
[`edit.ts`](../../../tmp/pi/packages/coding-agent/src/core/tools/edit.ts) `async execute` (`:332`)
→ `ops.writeFile` (`:371`), same. The marker's stale `write.ts:195-225` range does not exist at
`e8682309`; cite the symbol.

So the honest statement is: **the capability exists upstream as an opt-in extension; only the
*configuration surface* (`SessionConfig::protect_paths`) is cyrup-original.**

### F2 — cyrup's default is already at parity, so nothing is caller-visible by default

[`crates/cyrup-session-svc/src/builder.rs`](../../../crates/cyrup-session-svc/src/builder.rs),
`impl Default for SessionConfig` → `protect_paths: false` (`:250`). Wiring, in
`build`'s backend assembly (anchor on `let base = Backend::local();`, `:866`):

```rust
let mut fs = base.fs.clone();
if cfg.confine_to_cwd { fs = Arc::new(TraversalFs::new(fs, cwd.clone())); }   // :870-872
if cfg.protect_paths  { fs = Arc::new(ProtectedFs::with_defaults(fs)); }      // :873-875
let backend = Backend { fs, proc: base.proc.clone() };                        // :876
```

`proc` undecorated → `bash` bypass confirmed in source. pi's extension is equally blind to `bash`
(it filters `event.toolName !== "write" && !== "edit"`). **Tie. Not a divergence.**

### F3 — the real, live defect: cyrup's matcher is a strict subset of pi's on the `.env` family

[`crates/cyrup-tools/src/isolation/protected.rs`](../../../crates/cyrup-tools/src/isolation/protected.rs),
`ProtectedPaths::is_protected` (`:55-62`) matches when a path **component equals** one of
`.env` / `.git` / `node_modules` (`defaults()`, `:30-34`). pi matches `String.includes`.

| path | cyrup today | pi w/ extension | verdict |
|---|---|---|---|
| `.env` | block | block | tie |
| **`.env.local`** | **write** | **block** | **cyrup weaker — live hole** |
| **`.env.production`** | **write** | **block** | **cyrup weaker — live hole** |
| **`.env.development.local`** | **write** | **block** | **cyrup weaker — live hole** |
| `.environment` | write | block | pi false positive |
| `config.env` | write | block | pi false positive |
| `.gitignore` | write | write | tie |
| `.git` (dir itself) | block | write | cyrup stricter, correct |
| `.git/config`, `node_modules/a/i.js` | block | block | tie |

`.env.local` / `.env.production` are where secrets actually live in JS projects. An embedder who
sets `protect_paths: true` believing `.env*` is covered gets a guard narrower than they think.
The existing unit test even pins the near-miss exclusion (`protected.rs:172`, `.environment`) but
never tests the dotenv family.

### F4 — second live defect: the matcher sees the **absolutized** path, so cwd components count

[`crates/cyrup-tools/src/tools/write.rs:106`](../../../crates/cyrup-tools/src/tools/write.rs) —
`let abs = path::resolve_to_cwd(&input.path, &self.cwd);` then `self.fs.write_in_place(&abs, …)`
(`:114`). `ProtectedFs` therefore tests **every** component of the absolute path, including the
session cwd's. A session rooted at `/home/u/proj/node_modules/mypkg` has **every write refused**.
pi's extension matches the raw `input.path` argument and does not have this failure. This is a
seam-choice false positive unique to cyrup and it makes the flag unusable in a legitimate cwd.

### F5 — the decorator is complete today; `protected_path_rule` is *not* slated for deletion

`FsOps` ([`crates/cyrup-tools/src/ops/mod.rs`](../../../crates/cyrup-tools/src/ops/mod.rs)) has
exactly eight methods — `read` (`:438`), `read_stream` (`:456`), `write_in_place` (`:480`),
`access` (`:482`), `metadata` (`:483`), `read_dir` (`:484`), `detect_image_mime` (`:487`),
`walk` (`:501`) — and `ProtectedFs` names all eight (`protected.rs:102-156`), guarding
`write_in_place` and `access` under `Access::ReadWrite`. No hole. Keep the explicit
`read_stream` forward and its docstring (`protected.rs:106-124`); its `ops/mod.rs:329-334`
citation has drifted to `:456`.

`grep -rn protected_path_rule --include=*.rs crates/` → definition
([`isolation/policy.rs:196-206`](../../../crates/cyrup-tools/src/isolation/policy.rs)), two
re-exports (`isolation/mod.rs:46`, `lib.rs:43`), one unit test (`policy.rs:284`). **Zero production
consumers** — but **`docs/gap-analysis/PARITY-GAPS.md` §5 (`:1018-1028`) lists exactly three
deletion candidates and `protected_path_rule` is not among them.** ADR-0003 D7's claim that these
helpers "remain … in `PARITY-GAPS` §5's deletion candidates" is unsupported at HEAD. Nothing is
scheduled for deletion; leave it alone.

### F6 — cyrup already has pi's enablement path, so no new user-facing surface is needed

`ADR-0003` D5 ([`docs/adr/ADR-0003-bash-scope.md:194-201`](../../../docs/adr/ADR-0003-bash-scope.md))
refuses a CLI flag / `settings.json` key on the reason "pi has neither." The *outcome* survives F1
on better grounds: pi's enablement is an auto-discovered **extension**, and cyrup has the identical
mechanism — `<cwd>/.cyrup/extensions/` + `<agentDir>/extensions/`
([`crates/cyrup-ext/src/loader.rs:3,23`](../../../crates/cyrup-ext/src/loader.rs)) with a
`tool_call` block/mutate event ([`crates/cyrup-ext-sdk/src/events.rs:9-13`](../../../crates/cyrup-ext-sdk/src/events.rs),
proven end-to-end by `gate_blocks_a_deny_rule_through_before_tool_call`,
[`crates/cyrup-it/tests/permission/gate_integration.rs:108`](../../../crates/cyrup-it/tests/permission/gate_integration.rs)).
**Do not add a CLI flag or a settings key. Do not wire `protected_path_rule`.** Both are already
reachable the way pi reaches them.

---

## Required implementation path (single; no options, no ruling to defer)

Three limbs, one commit, all inside `cyrup-tools` plus one line in `cyrup-session-svc`.

### 1. Fix the matcher — dotenv family, in `ProtectedPaths::is_protected`

[`crates/cyrup-tools/src/isolation/protected.rs`](../../../crates/cyrup-tools/src/isolation/protected.rs),
`is_protected` (`:55-62`). Replace the component-equality predicate with **equality *or*
name-plus-dot prefix**, applied uniformly to every configured name:

```rust
/// True when `path` is protected: any of its components equals a protected name, or begins
/// with that name followed by `.` — so `.env` covers the dotenv family (`.env.local`,
/// `.env.production`, `.env.development.local`) exactly as pi's example extension does,
/// without inheriting its substring false positives (`.environment`, `config.env`).
/// `.gitignore` stays writable: it is not `.git` and does not begin with `.git.`.
pub fn is_protected(&self, path: &Path) -> bool {
    path.components().any(|c| match c {
        Component::Normal(os) => os.to_str().is_some_and(|s| {
            self.names.iter().any(|n| {
                s == n.as_str() || (s.len() > n.len() + 1 && s.starts_with(n.as_str()) && s.as_bytes()[n.len()] == b'.')
            })
        }),
        _ => false,
    })
}
```

Generalizing the rule to all names (not special-casing `.env`) is deliberate: a custom
`ProtectedPaths::new([".secret"])` then also covers `.secret.local`, which is the same intent, and
it keeps the existing `custom_set_and_builder` test (`:176-181`) green.

`.envrc` is **out of scope on purpose** — direnv, not dotenv; pi only catches it via the same
`includes` bug that catches `.environment`. Say so in the docstring so nobody "fixes" it later.

### 2. Root the matcher at the session cwd — kill the F4 false positive

Add a root-aware constructor beside `new` / `with_defaults` (`protected.rs:78-98`) and store
`root: Option<PathBuf>`; in `deny_if_protected` (`:89-97`), test
`path.strip_prefix(root).unwrap_or(path)` when a root is set, otherwise the whole path
(preserving today's semantics for embedders who call `new`). Then in
[`crates/cyrup-session-svc/src/builder.rs:874`](../../../crates/cyrup-session-svc/src/builder.rs)
change

```rust
fs = Arc::new(ProtectedFs::with_defaults(fs));
```
to
```rust
fs = Arc::new(ProtectedFs::rooted(fs, cwd.clone(), ProtectedPaths::defaults()));
```

`cwd` is already in scope at that point (it is used by the `TraversalFs` arm two lines above).

### 3. Correct the four sites that assert pi has no protected-path concept

Each must stop stating a falsehood and must cite the extension instead of the stale
`write.ts:195-225` @v0.83.0 range. Anchor by the quoted string, not the line:

| file | anchor |
|---|---|
| [`crates/cyrup-tools/src/isolation/mod.rs`](../../../crates/cyrup-tools/src/isolation/mod.rs) `:12-16` | `pi has no protected-path concept —` |
| [`crates/cyrup-session-svc/src/builder.rs`](../../../crates/cyrup-session-svc/src/builder.rs) `:180-184` | `because pi has no protected-path` |
| same file `:247-249` | `// ADR-0003 D5: pi has no protected-path concept` |
| [`crates/cyrup-tools/src/tests/isolation.rs`](../../../crates/cyrup-tools/src/tests/isolation.rs) `:161-165` | `pi has no protected-path concept at all` |

Replacement claim, true at `e8682309` and the one to write at all four:

> pi's **core** has no protected-path predicate (`write.ts::createWriteToolDefinition` →
> `ops.writeFile`; `edit.ts::execute` → `ops.writeFile`), so cyrup's default `protect_paths: false`
> matches default pi exactly. pi ships the guard as an opt-in, auto-discovered extension —
> `examples/extensions/protected-paths.ts`, catalogued at `docs/extensions.md:2944`, over the same
> three names, blocking `write`/`edit` at the `tool_call` gate and equally blind to `bash`.
> `[CYRUP-DELTA]` therefore marks the **configuration surface only** (`SessionConfig::protect_paths`
> is a Rust builder field; pi's `src/` has no `protectedPaths` option — grep returns zero), not the
> capability. cyrup's user-reachable equivalent of pi's enablement is a `.cyrup/extensions/`
> extension on the `tool_call` event, which is why there is still no CLI flag and no settings key.

Keep the `[CYRUP-DELTA]` stamp; narrow what it claims. Keep the fs-only / `bash`-uncovered scope
paragraph in `builder.rs:186-190` — it is accurate and pi is in the same position.

### Explicitly out of scope (evidence, not deferral)

- **Do not delete `ProtectedFs`/`ProtectedPaths`.** It is the only wired implementation; the gate
  rule has zero consumers, so deleting the decorator leaves cyrup strictly below pi (F5).
- **Do not wire `protected_path_rule`, and do not delete it.** ADR-0003 D7 stands and PARITY-GAPS §5
  does not list it (F5).
- **Do not decorate `ProcOps`.** ADR-0003 `:312-320`; pi's guard is equally `bash`-blind (F2).
- **Do not add a CLI flag or `settings.json` key.** F6 — the extension path already exists.
- **Do not add a UI notification** to match pi's `ctx.ui.notify`. `ProtectedFs` is a backend
  decorator with no `ctx`; the `Err` already surfaces as an `isError` tool result the model reads
  (`protected.rs:5-7`).

---

## Guard (one; fails today)

Extend `defaults_match_env_git_node_modules`
([`crates/cyrup-tools/src/isolation/protected.rs:164-173`](../../../crates/cyrup-tools/src/isolation/protected.rs))
into a table over the F3 matrix plus the F4 root case. Rows marked ✗ **fail before the change**:

```rust
let p = ProtectedPaths::defaults();
// positives — the three marked ✗ fail today
assert!(p.is_protected(Path::new("/w/.env")));
assert!(p.is_protected(Path::new("/w/.env.local")));              // ✗ today
assert!(p.is_protected(Path::new("/w/.env.production")));         // ✗ today
assert!(p.is_protected(Path::new("/w/.env.development.local")));  // ✗ today
assert!(p.is_protected(Path::new("/w/.git")));
assert!(p.is_protected(Path::new("/w/.git/config")));
assert!(p.is_protected(Path::new("/w/node_modules/a/i.js")));
// negatives — the over-correction guard: must NOT become pi's substring bug
assert!(!p.is_protected(Path::new("/w/.environment")));
assert!(!p.is_protected(Path::new("/w/config.env")));
assert!(!p.is_protected(Path::new("/w/.gitignore")));
assert!(!p.is_protected(Path::new("/w/.envrc")));
assert!(!p.is_protected(Path::new("/w/src/main.rs")));
```

Plus the F4 row, which is the whole point of limb 2 and fails today:

```rust
// A session rooted under node_modules must not have every write refused.
let root = Path::new("/home/u/proj/node_modules/mypkg");
let fs = ProtectedFs::rooted(base, root.to_path_buf(), ProtectedPaths::defaults());
// ordinary file inside that root: allowed
assert!(fs.write_in_place(&root.join("src/lib.rs"), b"x").await.is_ok());   // ✗ today
// its own .env.local: still refused
assert!(fs.write_in_place(&root.join(".env.local"), b"x").await.is_err());  // ✗ today
```

`protected_fs_is_fs_only_and_bash_is_never_covered`
([`crates/cyrup-tools/src/tests/isolation.rs:169-231`](../../../crates/cyrup-tools/src/tests/isolation.rs))
stays as-is behaviourally — only its docstring (`:161-165`) changes under limb 3. It must keep
asserting (a) undecorated `write` to `.env` succeeds, (b) decorated `write` is refused, (c) `bash`
reaches the file regardless.

---

## Definition of done

1. `ProtectedPaths::is_protected` protects `.env`, `.env.<anything>` and `<name>.<anything>` for
   every configured name, and still refuses to match `.environment`, `config.env`, `.gitignore`,
   `.envrc`.
2. `ProtectedFs` has a root-aware constructor; `builder.rs`'s `protect_paths` arm uses it with the
   session `cwd`, so components of the cwd no longer trigger the guard.
3. All four sites in limb 3 no longer assert "pi has no protected-path concept"; each cites
   `examples/extensions/protected-paths.ts` and `docs/extensions.md:2944`, and no stale
   `write.ts:195-225` @v0.83.0 range remains under `crates/`
   (`grep -rn 'no protected-path concept' crates/` → 0 hits; `grep -rn '195-225' crates/` → 0 hits).
4. The guard above is in the tree and its ✗ rows are green.
5. `protect_paths` still defaults to `false`; no CLI flag, no settings key, no `ProcOps` decoration,
   no change to `protected_path_rule`'s wiring.
6. `ProtectedFs` still names all eight `FsOps` methods (F5) — including the explicit `read_stream`
   forward.
