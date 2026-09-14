#!/usr/bin/env bash
# Dev-container setup for cyrup.
#
# This is now a thin delegator. The real work lives in the SessionStart hook at
# .claude/hooks/session-start.sh, which Claude Code runs at the start of every
# session (wired in .claude/settings.json).
#
# Keeping one script means the container-build path and the session-start path
# cannot drift apart — which is exactly what happened before: this file said
# "runs before Claude Code launches", but nothing referenced it, so containers
# came up with the image's stale rustc 1.94.1 against a workspace that declares
# rust-version = "1.96" and refuses to compile below it.
#
# Edit .claude/hooks/session-start.sh, not this file.
set -euo pipefail

exec env CYRUP_INIT_FORCE=1 \
  "$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)/.claude/hooks/session-start.sh" "$@"
