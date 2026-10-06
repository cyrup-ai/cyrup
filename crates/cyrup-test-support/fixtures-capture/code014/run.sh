#!/bin/sh
# Re-captures `../../fixtures/pi/code014-prompt-sections.pi-captured.json` from pi v1.0.0's own code.
#
#   PI_REPO=/path/to/a/pi/clone ./run.sh
#
# Only files at the tag are used: they are extracted with `git archive v1.0.0`, never read from a
# working tree. Three files that are not under test are replaced by stubs (`stubs/`): the package
# paths (`config.ts`), the skills formatter (`skills.ts`, never reached: no skills are passed) and
# the default stream function (`stream-fn.ts`, never reached: the capture supplies its own). The
# `@earendil-works/pi-ai` import is redirected to the three real modules the code under test uses
# (`shim/`), because Node does not strip types for files under `node_modules`.
set -eu
here=$(cd "$(dirname "$0")" && pwd)
work=$(mktemp -d)
git -C "${PI_REPO:?set PI_REPO to a pi clone}" archive v1.0.0 \
  packages/agent/src packages/ai/src/utils packages/ai/src/types.ts \
  packages/coding-agent/src/core/system-prompt.ts packages/coding-agent/src/core/skills.ts \
  | tar -x -C "$work"
cp "$here/stubs/config.ts" "$work/packages/coding-agent/src/config.ts"
cp "$here/stubs/skills.ts" "$work/packages/coding-agent/src/core/skills.ts"
cp "$here/stubs/stream-fn.ts" "$work/packages/agent/src/stream-fn.ts"
cp -r "$here/shim" "$work/shim"
cp "$here/capture.ts" "$work/capture.ts"
cd "$work"
node --experimental-strip-types --no-warnings --import ./shim/register.mjs capture.ts \
  > "$here/../../fixtures/pi/code014-prompt-sections.pi-captured.json"
echo "wrote fixtures/pi/code014-prompt-sections.pi-captured.json (node $(node --version))"
