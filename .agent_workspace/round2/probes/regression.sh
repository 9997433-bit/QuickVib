#!/usr/bin/env bash
set -uo pipefail

repo="${1:-$(git rev-parse --show-toplevel)}"
output_dir="${2:-$repo/.agent_workspace/round2/probes/regression-output}"

mkdir -p "$output_dir"
cd "$repo"

export CARGO_TERM_COLOR=never
export CARGO_INCREMENTAL=0
TIMEFORMAT='wall=%R s user=%U s sys=%S s'

summary="$output_dir/summary.tsv"
printf 'check\tstatus\texit_code\n' >"$summary"

run() {
    local name="$1"
    shift
    local log="$output_dir/$name.log"
    local status
    local rc

    printf '\n$'
    printf ' %q' "$@"
    printf '\n'

    set +e
    { time "$@"; } > >(tee "$log") 2>&1
    rc=$?
    set -e

    if ((rc == 0)); then
        status=PASS
    else
        status=FAIL
    fi
    printf '%s\t%s\t%d\n' "$name" "$status" "$rc" | tee -a "$summary"
}

set -e
run test-workspace cargo test --workspace
run test-workspace-gui cargo test --workspace --features gui
run clippy-workspace cargo clippy --workspace --all-targets -- -D warnings
run clippy-workspace-gui cargo clippy --workspace --all-targets --features gui -- -D warnings
run check-deps ./ci/check-deps.sh

if awk -F '\t' 'NR > 1 && $2 != "PASS" { failed = 1 } END { exit failed }' "$summary"; then
    exit 0
fi
exit 1
