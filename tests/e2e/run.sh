#!/usr/bin/env bash
#
# Entry point for the e2e stack. See README.md.
#
#   ./run.sh up [--worker]   build and start the stack, wait until healthy
#   ./run.sh provision       install/configure REDCap + Girder (idempotent)
#   ./run.sh fixtures        generate the synthetic DICOM files
#   ./run.sh test [args]     run Playwright (extra args passed through)
#   ./run.sh upgrade [tag]   upload with a release, update to this version, check the data
#   ./run.sh all             up + provision + fixtures + test
#   ./run.sh rebuild-girder  rebuild Girder, restart, re-provision
#   ./run.sh ps | logs [svc] status / logs
#   ./run.sh down            stop and remove containers (volumes kept)
#   ./run.sh reset           down + remove volumes and .state (fresh install next time)
set -euo pipefail

E2E_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
REPO_DIR="$(cd "$E2E_DIR/../.." && pwd)"
export MODULE_VERSION="$(tr -d '[:space:]' < "$REPO_DIR/VERSION")"

compose() { docker compose -p rgc-e2e -f "$E2E_DIR/docker-compose.yml" "$@"; }

# Services of the optional profile are started only if asked for, or already running.
worker_profile_args() {
    if [ "${WITH_WORKER:-0}" = "1" ] || compose --profile worker ps -q worker 2>/dev/null | grep -q .; then
        echo "--profile worker"
    fi
}

cmd_up() {
    for a in "$@"; do [ "$a" = "--worker" ] && WITH_WORKER=1; done
    # shellcheck disable=SC2046
    compose $(worker_profile_args) up -d --build --wait
    compose $(worker_profile_args) ps
}

cmd_fixtures() {
    mkdir -p "$E2E_DIR/fixtures/out"
    compose --profile tools build fixtures
    compose --profile tools run --rm fixtures
}

cmd_test() {
    [ -f "$E2E_DIR/.state/state.json" ] || { echo "no .state/state.json: run ./run.sh provision" >&2; exit 1; }
    [ -f "$REPO_DIR/src/wasm/dicom_deid/pkg/dicom_deid_bg.wasm" ] \
        || { echo "src/wasm/dicom_deid/pkg missing: run scripts/build.sh" >&2; exit 1; }
    [ -f "$E2E_DIR/fixtures/out/manifest.json" ] || cmd_fixtures
    cd "$E2E_DIR"
    if [ ! -d node_modules/@playwright/test ]; then
        PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npm ci --no-audit --no-fund
    fi
    PLAYWRIGHT_SKIP_BROWSER_DOWNLOAD=1 npx playwright test "$@"
}

# Build a released version once, under .state/ (its WASM is not in git).
cmd_upgrade() {
    local tag="${1:-v1.5.0}"
    local legacy="$E2E_DIR/.state/legacy/$tag"
    if [ ! -f "$legacy/src/wasm/dicom_deid/pkg/dicom_deid_bg.wasm" ]; then
        mkdir -p "$legacy"
        git -C "$REPO_DIR" archive "$tag" | tar -x -C "$legacy"
        "$legacy/scripts/build.sh"
    fi
    UPGRADE_FROM="$tag" UPGRADE_SRC="$legacy/src" cmd_test specs/upgrade.spec.js
}

cmd="${1:-help}"; shift || true
case "$cmd" in
    up) cmd_up "$@" ;;
    provision) "$E2E_DIR/provision.sh" ;;
    fixtures) cmd_fixtures ;;
    test) cmd_test "$@" ;;
    upgrade) cmd_upgrade "$@" ;;
    all) cmd_up "$@"; "$E2E_DIR/provision.sh"; cmd_fixtures; cmd_test ;;
    rebuild-girder)
            compose build girder
        # shellcheck disable=SC2046
        compose $(worker_profile_args) up -d --wait girder $( [ -n "$(worker_profile_args)" ] && echo worker )
        "$E2E_DIR/provision.sh" ;;
    ps) compose --profile worker ps ;;
    logs) compose --profile worker logs --tail=200 "$@" ;;
    down) compose --profile worker --profile tools down ;;
    reset)
        compose --profile worker --profile tools down -v
        rm -rf "$E2E_DIR/.state" ;;
    *) sed -n '3,16p' "$0"; exit 1 ;;
esac
