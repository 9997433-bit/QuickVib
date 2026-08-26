#!/usr/bin/env bash
set -euo pipefail

cd "$(git rev-parse --show-toplevel)"

export CARGO_TERM_COLOR=never
TIMEFORMAT='wall=%R s user=%U s sys=%S s'

run() {
    printf '\n$'
    printf ' %q' "$@"
    printf '\n'
    time "$@"
}

run cargo test --workspace
run cargo test --workspace --features gui
run cargo test -p quickvib-ui
run cargo clippy --workspace --all-targets -- -D warnings
run cargo clippy --workspace --all-targets --features gui -- -D warnings

if [[ -x ci/check-deps.sh ]]; then
    run ci/check-deps.sh
fi
