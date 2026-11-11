#!/usr/bin/env bash
# Regression tests run no compiler and never take the production build lock.
set -euo pipefail
repo=$(git rev-parse --show-toplevel)
fixture=$(mktemp -d)
trap 'rm -rf "$fixture"' EXIT
sed "s@/tmp/yuuka-codex-build.lock@$fixture/build.lock@g" \
  "$repo/scripts/run-build.sh" > "$fixture/runner.sh"
git init -q --initial-branch=develop "$fixture/repo"
cd "$fixture/repo"
git -c user.name=Test -c user.email=test@example.invalid commit -qm fixture --allow-empty
sha=$(git rev-parse HEAD)
# Never inherit the surrounding Actions environment for the local scenarios.
unset GITHUB_ACTIONS RUNNER_ENVIRONMENT GITHUB_EVENT_NAME GITHUB_REF GITHUB_SHA
count=0
expect() {
  local expected=$1 actual=0
  shift
  "$@" > "$fixture/output" 2>&1 || actual=$?
  if [[ "$actual" != "$expected" ]]; then
    echo "Expected exit $expected, got $actual: $*" >&2
    cat "$fixture/output" >&2
    exit 1
  fi
  count=$((count + 1))
}
run() { bash "$fixture/runner.sh" "$@"; }
expect 0 run true
expect 0 run bash -c 'test "$CARGO_BUILD_JOBS" = 1'
expect 7 run bash -c 'exit 7'
expect 2 run true --jobs 2
expect 2 run true -j2
expect 2 run true CARGO_BUILD_JOBS=2
expect 2 run
# flock holds an isolated lock around the attempted nested runner invocation.
expect 2 flock -n "$fixture/build.lock" bash "$fixture/runner.sh" true
git switch -qc feature/fixture
expect 2 run true
git checkout -q --detach
expect 2 run true
hosted() {
  env GITHUB_ACTIONS=true RUNNER_ENVIRONMENT=github-hosted \
    GITHUB_EVENT_NAME=pull_request GITHUB_REF=refs/pull/123/merge \
    GITHUB_SHA="$sha" "$@"
}
expect 0 hosted bash "$fixture/runner.sh" true
expect 2 hosted env RUNNER_ENVIRONMENT=self-hosted bash "$fixture/runner.sh" true
expect 2 hosted env GITHUB_ACTIONS=false bash "$fixture/runner.sh" true
expect 2 hosted env GITHUB_SHA=wrong bash "$fixture/runner.sh" true
expect 2 hosted env GITHUB_SHA= bash "$fixture/runner.sh" true
expect 2 hosted env GITHUB_REF=refs/pull/123/head bash "$fixture/runner.sh" true
expect 2 hosted env GITHUB_EVENT_NAME=pull_request_target bash "$fixture/runner.sh" true
expect 0 hosted env GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/develop bash "$fixture/runner.sh" true
expect 0 hosted env GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/main bash "$fixture/runner.sh" true
expect 2 hosted env GITHUB_EVENT_NAME=push GITHUB_REF=refs/heads/feature bash "$fixture/runner.sh" true
expect 2 hosted env GITHUB_EVENT_NAME=workflow_dispatch bash "$fixture/runner.sh" true
# Exercise the exact inline gate used by Actions, rather than a duplicated copy.
awk '/^  pr-gate:/ { gate=1 } gate && /^        run: \|/ { body=1; next } body { sub(/^          /, ""); print }' \
  "$repo/.github/workflows/ci.yml" > "$fixture/gate.sh"
test -s "$fixture/gate.sh"
expect 0 env VALIDATION_RESULT=success bash -e "$fixture/gate.sh"
for result in failure cancelled skipped '' unknown neutral; do
  expect 1 env VALIDATION_RESULT="$result" bash -e "$fixture/gate.sh"
done
printf '%s regression scenarios passed\n' "$count"
