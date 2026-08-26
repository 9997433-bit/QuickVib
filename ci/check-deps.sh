#!/usr/bin/env bash
# Enforce the dependency policy from docs/PLAN.md 6.3 / D18: the only third-party runtime
# dependencies anywhere in the default workspace members are `serde` and `serde_json`, and
# the only dev-dependency is `tempfile`. `libloading` is allowed, but only inside the
# Windows-only `quickvib-m300` crate (which is outside `default-members`), and `eframe` only
# inside `quickvib-ui` behind its off-by-default `gui` feature.
#
# Both exceptions work the same way: the crate that needs the dependency is kept out of every
# build the UTS product, the test suite and the Windows cross-build actually run, so the
# check below — which resolves with default features — sees the unchanged two-crate graph.
set -euo pipefail

cd "$(dirname "$0")/.."

ALLOWED_RUNTIME='^(serde|serde_json|quickvib(-[a-z0-9]+)?)$'
ALLOWED_DEV='^(tempfile|serde|serde_json|quickvib(-[a-z0-9]+)?)$'

fail=0

# Direct (depth 1) normal edges of the whole default workspace.
runtime=$(cargo tree --workspace --edges normal --depth 1 --prefix none 2>/dev/null \
    | awk '{print $1}' | sort -u | grep -v '^$' || true)

while read -r dep; do
    [ -z "$dep" ] && continue
    if ! [[ "$dep" =~ $ALLOWED_RUNTIME ]]; then
        echo "unexpected direct runtime dependency: $dep"
        fail=1
    fi
done <<<"$runtime"

dev=$(cargo tree --workspace --edges dev --depth 1 --prefix none 2>/dev/null \
    | awk '{print $1}' | sort -u | grep -v '^$' || true)

while read -r dep; do
    [ -z "$dep" ] && continue
    if ! [[ "$dep" =~ $ALLOWED_DEV ]]; then
        echo "unexpected direct dev dependency: $dep"
        fail=1
    fi
done <<<"$dev"

# The windowing stack is allowed, but only as `quickvib-ui`'s own optional dependency: if it
# ever reaches another crate, or reaches this one unconditionally, the check above stops
# seeing it and the exception has quietly become the rule.
gui=$(cargo tree -p quickvib-ui --features gui --edges normal --depth 1 --prefix none 2>/dev/null \
    | awk '{print $1}' | sort -u | grep -v '^$' || true)

while read -r dep; do
    [ -z "$dep" ] && continue
    if ! [[ "$dep" =~ $ALLOWED_RUNTIME ]] && [ "$dep" != "eframe" ]; then
        echo "unexpected direct dependency of quickvib-ui/gui: $dep"
        fail=1
    fi
done <<<"$gui"

if [ "$fail" -ne 0 ]; then
    echo "dependency policy violation (see docs/PLAN.md 6.3)" >&2
    exit 1
fi

echo "dependency policy OK"
