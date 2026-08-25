#!/usr/bin/env bash
# Enforce the dependency policy from docs/PLAN.md 6.3 / D18: the only third-party runtime
# dependencies anywhere in the default workspace members are `serde` and `serde_json`, and
# the only dev-dependency is `tempfile`. `libloading` is allowed, but only inside the
# Windows-only `quickvib-m300` crate (which is outside `default-members`).
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

if [ "$fail" -ne 0 ]; then
    echo "dependency policy violation (see docs/PLAN.md 6.3)" >&2
    exit 1
fi

echo "dependency policy OK"
