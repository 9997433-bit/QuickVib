#!/usr/bin/env bash
set -uo pipefail

repo="${1:-$(git rev-parse --show-toplevel)}"
output_dir="${2:-$repo/.agent_workspace/round3/probes/final-output}"

mkdir -p "$output_dir"
cd "$repo"

export CARGO_TERM_COLOR=never
export CARGO_INCREMENTAL=0
TIMEFORMAT='wall=%R s user=%U s sys=%S s'

summary="$output_dir/summary.tsv"
run_info="$output_dir/run-info.txt"
printf 'check\tstatus\texit_code\n' >"$summary"
{
    printf 'started_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf 'start_head=%s\n' "$(git rev-parse HEAD)"
} >"$run_info"

run() {
    local name="$1"
    shift
    local log="$output_dir/$name.log"
    local status
    local rc

    printf '$'
    printf ' %q' "$@"
    printf '\n'

    set +e
    { time "$@"; } >"$log" 2>&1
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
run fmt-all cargo fmt --all --check
run test-workspace cargo test --workspace
run test-workspace-gui cargo test --workspace --features gui
run clippy-workspace cargo clippy --workspace --all-targets -- -D warnings
run clippy-workspace-gui cargo clippy --workspace --all-targets --features gui -- -D warnings
run check-deps ./ci/check-deps.sh

{
    printf 'finished_utc=%s\n' "$(date -u +%Y-%m-%dT%H:%M:%SZ)"
    printf 'end_head=%s\n' "$(git rev-parse HEAD)"
} >>"$run_info"

if awk -F '\t' 'NR > 1 && $2 != "PASS" { failed = 1 } END { exit failed }' "$summary"; then
    exit 0
fi
exit 1
