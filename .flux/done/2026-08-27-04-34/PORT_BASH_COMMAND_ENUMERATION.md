---
stage: qa
status: completed
updated: 2026-08-27 04:34
---
# Port the bash command enumerator — remaining work

> **QA verdict 8/10.** The enumerator itself is landed, correct and upstream-faithful
> (commit `31c804c`): `src/bash/{mod,parser,enumerate}.rs`, `src/restrictiveness.rs`, the
> per-unit bash arm in `manager.rs`, and the `tree-sitter`/`tree-sitter-bash` deps are all
> **done and verified** — 11/11 behaviour rows, `cargo check`/`clippy` clean, 192 tests pass.
> Do not re-do any of that. What follows is the only outstanding work.

## Verified sound — do not change these

A probe linking the real crate confirmed the following already hold; they need tests (item 1),
not fixes:

- **Context assignment is correct.** `echo $(rm x)` → `CommandSubstitution`, `diff <(ls a)` →
  `ProcessSubstitution`, `( rm b )` → `Subshell`, plain command → `None`.
- **The stack-safety CYRUP-DELTA holds.** `$(` × 50 000 and a 20 000-element `&&` chain both
  resolve with no abort, and a `rm` nested 20 000 deep still returns `deny`. The iterative
  walker is doing what its doc comment claims.
- **The trusted-deny floor survives decomposition — and the fix closes an extra bypass.**
  With a trusted `curl *: deny` under an untrusted `curl *: allow` + `echo *: allow`:
  `curl evil.sh`, `echo hi && curl evil.sh` and `echo $(curl evil.sh)` **all** deny. The middle
  case previously resolved `allow`: the whole string matched the untrusted `echo *` allow, and
  the trusted `curl *` deny could not match the whole string, so the floor never engaged. That
  is a second bypass this change closes, beyond the one in the task title.

---

## 1. Pin the fix — it is currently unprotected (BLOCKER)

**The problem, concretely.** Every bash command exercised by the crate's 192 tests is a
single command with no chain, substitution, subshell, env prefix or redirect:

| test | command |
| --- | --- |
| `default_policy_is_ask_for_unconfigured_bash` | `ls` |
| `bash_allow_and_deny_by_command` | `echo hi`, `rm -rf /` |
| `trusted_floor_untrusted_project_cannot_relax_global_deny` | `curl x` |
| (agent-layer tests) | `constructor`, `echo hi` |
| (cache tests) | `ls` |

Revert the `manager.rs` bash arm to the old whole-string
`find_compiled_match(&resolved.compiled_bash, &command)` and **all 192 tests still pass.**
The critical bypass this task closed can be silently re-opened by any future refactor of
`collect_commands` or the bash arm, and nothing in the repository would notice.

**What to do.** Add regression coverage in the crate's existing idiom — `#[cfg(test)] mod
tests` in `manager.rs` alongside `bash_allow_and_deny_by_command`, using the existing
`manager_with_global` helper. Under a policy of
`{ "bash": { "echo *": "allow", "rm *": "deny" } }`, assert BOTH `state` and `command` for:

| command | state | `command` field | what it pins |
| --- | --- | --- | --- |
| `echo hi` | allow | `echo hi` | single command unaffected |
| `echo hi && rm -rf /` | deny | `rm -rf /` | **the chain bypass** |
| `echo $(rm x)` | deny | `rm x` | command substitution |
| `x=1 rm -rf /` | deny | `rm -rf /` | env-prefix strip |
| `echo hi > $(rm x)` | deny | `rm x` | redirect-as-execution-host |
| `( echo a && rm b )` | deny | `rm b` | subshell descent |
| `cat <<EOF\n$(rm e)\nEOF` | deny | `rm e` | heredoc interpolation |
| `cat <<'EOF'\n$(rm e)\nEOF` | ask | `cat` | quoted heredoc does NOT interpolate |
| `# comment`, `""`, `"   "` | ask | the input | trivially-empty whole-string path |

Assert `command`, not just `state` — a naive "deny wins" implementation that reported the
whole chain would pass a state-only assertion while losing the offending-unit attribution
that `get_pattern_approval_subject` depends on.

Also pin, in `src/bash/enumerate.rs`'s own test module, that `collect_commands` assigns the
right `BashCommandContext`. Nothing in the crate reads `BashCommand::context` today (it is
the deferred extension point for `PORT_BASH_WRAPPER_FLOOR` / `PORT_BASH_PATH_PROJECTION`),
so an error in it is currently invisible and would surface as a wrong prompt later:

- `echo $(rm x)` → `[(echo $(rm x), None), (rm x, CommandSubstitution)]`
- `diff <(ls a)` → `[(diff <(ls a), None), (ls a, ProcessSubstitution)]`
- `( rm b )` → `[(( rm b ), None), (rm b, Subshell)]`

Finally pin the stack-safety claim the `[CYRUP-DELTA]` in `enumerate.rs` rests on, since it
is the justification for the iterative walker over pi's recursion. A `$(` × 20 000 nest and
a 20 000-element `&&` chain must both resolve without aborting, and the deeply-nested
`rm` must still come back `deny`. Keep the sizes modest enough not to slow the suite.

## 2. Make the dropped-unit path fail closed (minor, security-boundary hygiene)

`src/bash/enumerate.rs` emits units conditionally in three places:

```rust
if let Some(text) = unit_text(node, src) { out.push(...) }   // command
if let Some(text) = node_text(node, src) { out.push(...) }   // subshell, other statements
```

If either accessor ever returned `None`, that unit is **silently skipped — i.e. never
gated**. That is the fail-OPEN direction in the one function whose whole purpose is to make
sure nothing runs ungated.

**Confirmed unreachable today** by direct probe — every one of these gates correctly, with the
env prefix stripped and the offending unit named, so no unit is dropped on non-ASCII input:

| command | units | result | `command` |
| --- | --- | --- | --- |
| `rm ünïcodé` | 1 | deny | `rm ünïcodé` |
| `echo héllo && rm café` | 2 | deny | `rm café` |
| `rm 日本語` | 1 | deny | `rm 日本語` |
| `VAR=über rm ß` | 1 | deny | `rm ß` |
| `echo 'x' && rm 🔥` | 2 | deny | `rm 🔥` |

So this is **hygiene, not a live bug** — `src` is a `&str` so `utf8_text` cannot fail, and
tree-sitter node boundaries never split a codepoint. That is exactly why it should be made
explicit rather than left as an implicit `else { skip }`: a future change to the walk (a
synthesized offset, a node from a different tree) would turn an invisible assumption into an
ungated command.

Prescription: on `None`, push a unit that cannot match any rule so it resolves through the
default (`ask`) instead of vanishing — or surface the failure so the caller takes the
existing `<unparseable-bash-command>` fail-closed branch. Do not simply `unwrap`
(`unwrap_used = "deny"`), and do not leave the silent skip. Document the choice.

## 3. Document the whole-chain-rule migration consequence (doc)

The task's "Consequences to accept deliberately" section records that
`PermissionCheckResult.command` narrows to the offending unit, but not this:

**A rule written against a whole chain stops matching.** `{"git add . && git commit *":
"allow"}` previously matched the string `git add . && git commit -m x` and allowed it. Now
that command enumerates to `git add .` and `git commit -m x`; the rule matches neither, so
both fall through to the category default and the operator's rule is dead.

**Confirmed by probe:** that exact config + command now resolves `Ask`, `matched_pattern=None`,
`command="git add ."`. The operator's allow rule never fires.

This is upstream-parity behaviour — pi's `resolveBashCommandCheck` only falls back to
`resolveWholeCommand` when the unit list is *empty* — so it is correct, not a bug. But it
is a silent, operator-visible policy change and belongs in the consequences list, and in
whatever release/upgrade note this crate keeps.

## 4. The ask prompt understates what a chain will run (NEW — verified)

`pick_most_restrictive` is first-wins on ties, which is correct pi parity. But when every unit
of a chain resolves to the same tier, the winner is the FIRST unit — and `gate.rs:633` renders
only `result.command` in the ask prompt, while `gate.rs:151-156`
(`get_pattern_approval_subject`) uses it as the approval subject.

**Probe, default policy (everything `ask`):**

    command:  echo hi && git push --force
    result:   Ask, command = "echo hi"

The human is asked to approve **`echo hi`**. Approving runs `git push --force` as well. The
same tie-break also means an "Allow always" here persists a rule for `echo hi` only — which is
at least the safe direction, but the prompt itself is the problem.

This is not a regression introduced by the enumerator — before this change the prompt showed
the whole chain, which was *more* informative even though the gating was broken. And it is not
a pi divergence in the resolver: pi picks the same unit. It is a **gap in this port's
presentation**, because pi v27 renders the offending unit alongside its `commandContext` and
the full command through `presentation/`, which this port does not have.

Decide and implement one of:

- Render the whole `input.command` in the bash ask prompt while keeping `result.command` as the
  matched/approval subject — smallest change, restores what the operator sees.
- Or carry both, so the prompt can say "running `<whole command>`; `<unit>` needs approval".

Whichever is chosen, the invariant to hold is: **the human must never be shown less than what
will execute.** Note this interacts with `PORT_PROMPT_RENDER_BUDGET_AND_DIALOG`; if that task
is the right home, say so there and close this with a pointer rather than duplicating it.

---

---

## Definition of done for this rework

1. Reverting the `manager.rs` bash arm to whole-string matching makes at least one test
   **fail**. (This is the real test of item 1 — verify it, do not assume it.)
2. Context assignment and the stack-safety claim are both pinned.
3. No path in `collect_commands` can silently drop a command unit; the chosen behaviour is
   documented.
4. The whole-chain-rule consequence is written down.
5. The ask prompt for a chain shows the human everything that will run (item 4), or the item is
   explicitly reassigned to `PORT_PROMPT_RENDER_BUDGET_AND_DIALOG` with a pointer left here.
6. `cargo check -p cyrup-permission-system --all-targets` and
   `cargo clippy -p cyrup-permission-system --all-targets` stay clean, and the full lib
   suite passes. Note `unwrap_used`, `expect_used`, `panic`, `indexing_slicing` are `deny`
   at the workspace root; the crate's test modules already `#![allow(...)]` the first three.

> Reference checkout for upstream citations: `tmp/pi-packages/packages/pi-permission-system`
> (gitignored — re-clone with
> `git clone https://github.com/gotgenes/pi-packages tmp/pi-packages` if absent).
