---
title: Branch Leaves Rustfmt Violations In Its Own New Lines
priority: LOW
stage: aug
status: done
updated: 2026-08-23 03:49
---

# Format the seven files this branch took from rustfmt-clean to rustfmt-dirty

**Owns files** (all seven, and nothing else):

| File | Hunks | At merge base |
| --- | --- | --- |
| [`crates/cyrup-tools/src/lock.rs`](../../crates/cyrup-tools/src/lock.rs) | 2 | 10 (branch's own `cargo fmt` pass cleared them) |
| [`crates/cyrup-config/src/lock.rs`](../../crates/cyrup-config/src/lock.rs) | 3 | 0 |
| [`crates/cyrup-config/src/settings/manager.rs`](../../crates/cyrup-config/src/settings/manager.rs) | 10 | 0 |
| [`crates/cyrup-config/src/settings/tests/merge_and_scope.rs`](../../crates/cyrup-config/src/settings/tests/merge_and_scope.rs) | 5 | 0 |
| [`crates/cyrup-config/src/settings/tests/write_refusal.rs`](../../crates/cyrup-config/src/settings/tests/write_refusal.rs) | 10 | 0 |
| [`crates/cyrup-config/src/trust.rs`](../../crates/cyrup-config/src/trust.rs) | 4 | 0 |
| [`crates/cyrup-core/src/keyed_lock.rs`](../../crates/cyrup-core/src/keyed_lock.rs) | 3 | new file |
| **Total** | **37** | |

> **Run this LAST**, after every other task in the queue has landed. All seven files are owned by
> other queued tasks (`cyrup-config/src/lock.rs` alone appears in eleven). Formatting is the one job
> that touches lines everybody else is editing, so scheduling it early guarantees rework. Same rule
> the repo already applied in [`_backlog/RESOURCES_RUSTFMT_DRIFT.md`](_backlog/RESOURCES_RUSTFMT_DRIFT.md).
>
> One hard ordering edge: [`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md)
> definition-of-done #7 requires `crates/cyrup-tools/src/lock.rs` to *still* report its two
> pre-existing diffs at `:92` and `:146`. That task must land before this one, or its DoD cannot be
> checked.

## Description

Two review findings, one root cause. `fe86c7f` ran `cargo fmt` over `cyrup-tools`, and separately
drove a compiler-span script that appended `.await` textually across `cyrup-config` and
`cyrup-core`. Neither pass reformatted what it had just written. The result is 37 rustfmt hunks
sitting entirely on lines this branch authored, in seven files.

The job is to run rustfmt over exactly those seven files. It is not to run `cargo fmt`.

## Measurement

`rustfmt 1.9.0-stable (88d9e12ae1 2026-08-18)`, the pinned `stable` toolchain
([`rust-toolchain.toml`](../../rust-toolchain.toml)). No `rustfmt.toml` anywhere in the tree, so
these are stock defaults at `edition = "2024"` ([`Cargo.toml:88`](../../Cargo.toml)).

Every `.rs` file the branch touched, checked in isolation against merge base
`4902cddf8ce7d4723e41b4a7bf652361a584f905`, with each hunk blame-attributed to the commit that
wrote the lines it wants to change:

```
79 touched .rs files
23 of them rustfmt-dirty at HEAD, 530 hunks total
   110 hunks on lines this branch wrote
   420 hunks pre-existing drift
 7 files where 100% of the remaining hunks are branch-authored  ->  37 hunks  <- THIS TASK
16 files that were already dirty at merge base                  ->  73 branch-authored hunks
                                                                    + 420 pre-existing, entangled
```

The 16-file remainder is where the "do not reach for a blanket `cargo fmt`" warning bites, and the
numbers are worth seeing before anyone is tempted:

| Package | Dirty hunks at HEAD | Of those, branch-authored |
| --- | ---: | ---: |
| `cyrup-tools` | 2 | 2 |
| `cyrup-config` | 32 | 32 |
| `cyrup-core` | 49 | 4 |
| `cyrup` | 182 | 14 |
| `cyrup-session-svc` | 1062 | 8 |
| `cyrup-tui` | 2710 | 2 |
| `cyrup-ext-subagents` | 3026 | 87 |

`cargo fmt -p cyrup-ext-subagents` would rewrite 3026 hunks to fix 87. A single file is no safer:
`discovery/management.rs` is 165 hunks, of which 120 predate the branch.

### The scope rule

> Format a touched file **iff every rustfmt hunk it still has sits on a line this branch wrote.**

That rule selects exactly the seven files above and absorbs zero pre-existing drift by construction.
It is the reason `crates/cyrup-tools/src/lock.rs` is in scope despite having been dirty at merge
base (the branch's own fmt pass cleaned it; the two survivors are both branch-written), and the
reason `crates/cyrup-core/src/lib.rs` is out of scope despite being touched (its 3 hunks are all
pre-existing).

Re-derive the set before formatting — sibling tasks will have edited these files by the time this
runs. Write the script to `./tmp/` (gitignored):

```bash
mkdir -p tmp && cat > tmp/fmt-scope.py <<'PY'
import re, subprocess, collections
ROOT="/home/user/cyrup"; MB="4902cddf8ce7d4723e41b4a7bf652361a584f905"
g=lambda *a: subprocess.run(["git","-C",ROOT,*a],capture_output=True,text=True).stdout
branch=set(g("rev-list",f"{MB}..HEAD").split())
files=[f for f in g("diff","--name-only",MB,"HEAD","--","*.rs").split() if f]
out=subprocess.run(["rustfmt","--check","--color","never","--edition","2024",
                    "--config","skip_children=true",*files],
                   capture_output=True,text=True,cwd=ROOT).stdout
hunks=[];cur=None;cursor=0;minus=[];ctx=[]
def flush():
    if cur is not None: hunks.append((cur, minus[:] or ctx[:]))
for line in out.splitlines():
    m=re.match(r"^Diff in (\S+):(\d+):$",line)
    if m:
        flush(); cur=m.group(1).replace(ROOT+"/",""); cursor=int(m.group(2)); minus=[];ctx=[]; continue
    if cur is None or line.startswith("+"): continue
    (minus if line.startswith("-") else ctx).append(cursor); cursor+=1
flush()
per=collections.defaultdict(lambda:[0,0])
blame={}
for f in {h[0] for h in hunks}:
    blame[f]={int(mm.group(2)):mm.group(1) for mm in
              (re.match(r"^\^?([0-9a-f]{40})\s+(?:\S+\s+)?\(?\s*(\d+)\)",l)
               for l in g("blame","-l","-s","HEAD","--",f).splitlines()) if mm}
for f,lines in hunks:
    per[f][0 if any(blame[f].get(i) in branch for i in lines) else 1]+=1
print("IN SCOPE (100% branch-authored):")
for f in sorted(per):
    n,p=per[f]
    if p==0: print(f"  {f}  ({n} hunks)")
print("OUT OF SCOPE (entangled with pre-existing drift):")
for f in sorted(per):
    n,p=per[f]
    if p: print(f"  {f}  ({n} branch / {p} pre-existing)")
PY
python3 tmp/fmt-scope.py
```

If the "IN SCOPE" list still prints exactly the seven files, proceed. If a sibling task has since
cleaned or dirtied one, format what the script prints and note the delta in the commit message —
the rule is authoritative, the frozen list above is only today's snapshot.

## Required path

One command. Run it from the repo root.

```bash
rustfmt --edition 2024 --config skip_children=true \
  crates/cyrup-tools/src/lock.rs \
  crates/cyrup-config/src/lock.rs \
  crates/cyrup-config/src/settings/manager.rs \
  crates/cyrup-config/src/settings/tests/merge_and_scope.rs \
  crates/cyrup-config/src/settings/tests/write_refusal.rs \
  crates/cyrup-config/src/trust.rs \
  crates/cyrup-core/src/keyed_lock.rs
```

Every part of it is load-bearing:

- **`rustfmt`, not `cargo fmt`.** `cargo fmt` has no file granularity; its smallest unit is `-p`.
  It happens to be true *today* that `cargo fmt -p cyrup-tools -p cyrup-config` would produce an
  identical result — both packages are clean apart from this branch's own damage (2 and 32 hunks,
  100% branch-authored) — but that is a coincidence of the current queue state, and eleven pending
  tasks edit those packages. It is never true for `cyrup-core`: `cargo fmt -p cyrup-core` drags in
  46 unrelated hunks to fix `keyed_lock.rs`'s 3.
- **`--edition 2024` is mandatory, and omitting it fails silently.** Invoked directly, rustfmt
  defaults to edition 2015, cannot parse `async fn`, writes `error[E0670]` to stderr and formats
  nothing. In `--check` mode that presents as **zero** `Diff in` lines — a clean bill of health for
  a file it never read. `cargo fmt` passes the edition from the manifest, which is why the failure
  mode only appears here.
- **`--config skip_children=true`** stops rustfmt descending into `mod foo;` declarations. All
  seven files are leaf modules today, so it is currently a no-op — but without it, pointing rustfmt
  at a file that gains a child module reformats the child too. Verified accepted on stable 1.9.0
  (it is not in `--help`, but it applies: on `settings/mod.rs` it takes the recursive result from
  25 hunks to 0).
- **No `--backup`.** It would litter `*.rs.bk` files.

## What changes

Expected diff: **+161 / −89** across the seven files.

```
crates/cyrup-tools/src/lock.rs                              +8   -2
crates/cyrup-config/src/lock.rs                            +11   -4
crates/cyrup-config/src/settings/manager.rs                +76  -60
crates/cyrup-config/src/settings/tests/merge_and_scope.rs  +10   -5
crates/cyrup-config/src/settings/tests/write_refusal.rs    +31  -11
crates/cyrup-config/src/trust.rs                           +17   -4
crates/cyrup-core/src/keyed_lock.rs                         +8   -3
```

**Nothing but reflow, trailing commas, and one import swap.** Six of the seven files are
byte-identical to their formatted output once whitespace and commas are stripped — the only edits
are line-splitting and the trailing commas rustfmt adds when it explodes a struct literal. The
seventh, `cyrup-config/src/lock.rs`, additionally swaps two adjacent `use` lines; no token is added
or removed.

### `crates/cyrup-tools/src/lock.rs` — 2 hunks

```
 92 |         Self { inner: KeyedLocks::new(Arc::clone(&map)), map }
146 |         self.inner.guard(key, cancel).await.map_err(|_| error::aborted())
```

The bodies of `FileMutationLocks::new` and `FileMutationLocks::guard` — precisely the two functions
this branch rewrote, and the only rustfmt violations left in the whole `cyrup-tools` package.

### `crates/cyrup-config/src/lock.rs` — 3 hunks

```
  9 | use cyrup_core::keyed_lock::{KeyedGuard, KeyedLockMap, KeyedLocks};   <- reorders below CancelToken
 67 |             .map_err(|_| ConfigError::Lock { path: target.to_path_buf() })??;
 68 |         Ok(Self { _in_process: in_process, file })
 87 |     FileExt::lock(&file).map_err(|_| ConfigError::Lock { path: target.to_path_buf() })?;
```

Line 9 is the one non-whitespace edit in this task: the branch inserted the `keyed_lock` import
above `use cyrup_core::CancelToken;`, and `reorder_imports` (on by default) moves it below.

### `crates/cyrup-core/src/keyed_lock.rs` — 3 hunks

```
 56 |         let _pending = PendingEntry { map: Arc::clone(&self.map), key: key.clone() };
 97 |         self.map.remove_if(&self.key, |_, v| Arc::strong_count(v) == 1);
124 |         self.map.remove_if(&self.key, |_, v| Arc::strong_count(v) == 1);
```

New file — it inherited these three from the `cyrup-tools` original without a formatting pass.

### `crates/cyrup-config/src/trust.rs` — 4 hunks

```
198 |     pub async fn set(&self, cwd: &Path, decision: Option<TrustDecision>) -> Result<(), ConfigError> {
502 |         store.set(&root, Some(TrustDecision::Trusted)).await.unwrap();
542 |         store.set(&cwd, Some(TrustDecision::Untrusted)).await.unwrap();
651 |         assert!(matches!(store.nearest(&cwd).await, Err(ConfigError::Trust(_))));
```

Line 198 is a 103-column signature — `async ` pushed it over.

### `crates/cyrup-config/src/settings/manager.rs` — 10 hunks

Four are `})` terminators where `.await?` was glued on, which forces the entire closure body to
re-indent (this is where 60 of the 76 added lines come from):

- `set` (`:232-260`), `set_nested` (`:292-303`), `persist_nested` (`:338-349`) — the
  `store.with_lock(scope, &mut |current| { ... }).await?;` calls
- `set_enable_analytics` (`:447`)

Six are convenience setters whose `.await` pushed the call or the signature past the width limit:
`set_mermaid_rendering_mode` (`:368`), `set_editor_padding_x` (`:374`),
`set_autocomplete_max_visible` (`:378` signature at 101 columns, `:380` body),
`set_image_width_cells` (`:390`), `set_http_idle_timeout_ms` (`:405`), `set_show_images` (`:414`).

### `settings/tests/merge_and_scope.rs` (5) and `settings/tests/write_refusal.rs` (10)

Uniformly `.await.unwrap()` and `).await` chains left mid-line by the script — e.g.
`merge_and_scope.rs:415,421,429,435,497` and `write_refusal.rs:113,129,149,150,184,217,221,234,247,264,270`.

## Do not touch

- **Any file not in the seven.** In particular not `crates/cyrup-core/src/lib.rs`,
  `crates/cyrup-ext-subagents/src/discovery/management.rs`, `crates/cyrup/src/main.rs`,
  `crates/cyrup-session-svc/src/builder.rs`, `crates/cyrup-tui/src/app/execute*.rs` — all touched by
  the branch, all already dirty at merge base. Their 73 branch-authored hunks are unreachable
  without absorbing 420 hunks of unrelated drift; that is a separate drift-absorption job, not this
  one.
- **`cargo fmt` at any scope.** Not `--all`, not `-p`. See the required path.
- **The `mod tests` bodies in `crates/cyrup-tools/src/lock.rs`.** The superseded advice on this
  task was to restore them to merge-base text so the field comment's "no test changes at all" claim
  would read true. That is now measured to be impossible: merge-base `lock.rs` had 10 rustfmt
  violations, **6 of them inside `mod tests`** (`:290`, `:316`, `:371`, `:377`, `:415-416`, `:425`).
  Restoring those bodies re-dirties the file, so "tests byte-identical to merge base" and "file is
  rustfmt-clean" cannot both hold. Fmt-clean wins.
  [`MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md`](./MEDIUM-map-field-doc-claims-a-property-the-diff-refutes.md)
  has already reached the same conclusion independently (its rejected alternative #5) and owns the
  sentence; it rewrites the comment as a property claim. Nothing about the test text is in scope here.
- **Logic, signatures, imports-as-such.** The one import that moves does so because rustfmt moves
  it, not because anyone edited it.

## Definition of done

1. The seven files report zero violations, and the command exits `0` with no output:

   ```bash
   rustfmt --check --color never --edition 2024 --config skip_children=true \
     crates/cyrup-tools/src/lock.rs crates/cyrup-config/src/lock.rs \
     crates/cyrup-config/src/settings/manager.rs \
     crates/cyrup-config/src/settings/tests/merge_and_scope.rs \
     crates/cyrup-config/src/settings/tests/write_refusal.rs \
     crates/cyrup-config/src/trust.rs crates/cyrup-core/src/keyed_lock.rs
   ```

   Exit `0` is the assertion that matters — a missing `--edition 2024` prints nothing but exits `1`,
   so "no output" alone is not proof.

2. `git status --porcelain` lists **exactly those seven paths** and nothing else.

3. `git diff --stat` matches `+161 / −89` (or the re-derived figure, if a sibling task changed the
   inputs first).

4. Both packages the branch damaged are now fully clean:

   ```bash
   cargo fmt -p cyrup-tools -- --check    # exit 0
   cargo fmt -p cyrup-config -- --check   # exit 0
   ```

5. `cyrup-core` improved by exactly the three `keyed_lock.rs` hunks and absorbed no drift:

   ```bash
   cargo fmt -p cyrup-core -- --check 2>&1 | grep -c '^Diff in'   # 46, was 49
   cargo fmt -p cyrup-core -- --check 2>&1 | grep -c 'keyed_lock.rs'   # 0
   ```

6. No token was added or removed except trailing commas and the one `use` reorder. For each of the
   six non-`lock.rs` files, whitespace-and-comma-stripped content is unchanged:

   ```bash
   for f in crates/cyrup-tools/src/lock.rs crates/cyrup-config/src/settings/manager.rs \
            crates/cyrup-config/src/settings/tests/merge_and_scope.rs \
            crates/cyrup-config/src/settings/tests/write_refusal.rs \
            crates/cyrup-config/src/trust.rs crates/cyrup-core/src/keyed_lock.rs; do
     a=$(git show HEAD:$f | tr -d '[:space:],' | md5sum)
     b=$(tr -d '[:space:],' < $f | md5sum)
     [ "$a" = "$b" ] && echo "ok   $f" || echo "FAIL $f"
   done
   ```

   All six print `ok`. `crates/cyrup-config/src/lock.rs` is excluded because its `use` line moves;
   verify that one by eye — the diff must be the two adjacent `use cyrup_core::…` lines swapping,
   plus reflow.

   Do **not** use `git diff -w` for this. `--ignore-all-space` normalises whitespace within a line
   but not line-splitting, so it still prints every hunk here and proves nothing.

7. Committed alone, as a formatting-only commit, so the churn is isolated in `git blame`. The
   message should name `crates/cyrup-config/src/lock.rs`'s import reorder explicitly so a reader
   does not mistake it for a logic edit.

## Not in this task

- No new tests, no doc changes.
- No `cargo fmt --check` gate. There is none in-tree today — no `.github/`, no fmt target in
  [`xtask`](../../xtask), no active git hooks — and adding one would fail immediately on the 420
  hunks of pre-existing drift this task deliberately leaves alone.
- The 73 branch-authored hunks in the 16 already-dirty files, and the 420 pre-existing hunks around
  them. If someone wants those, it is a per-package drift-absorption task in the shape of
  [`_backlog/RESOURCES_RUSTFMT_DRIFT.md`](_backlog/RESOURCES_RUSTFMT_DRIFT.md), scheduled per
  package and committed per package.
