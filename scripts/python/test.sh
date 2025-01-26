#!/usr/bin/env bash

# Runs the python bdd tests
# With no arguments the whole bdd feature set will be tested
# To test with specific arguments, simply provide them, eg:
# scripts/python/test.sh tests/bdd/features/volume/create/test_feature.py -k test_sufficient_suitable_pools
# For faster test cycle, the env variable FAST can be set to anything, which will skip building certain dependencies.
# Before using it, make sure they are already built!
# Eg: FAST=1 scripts/python/test.sh tests/bdd/features/volume/create/test_feature.py -k test_sufficient_suitable_pools
# To avoid cleanup/teardown of the cluster and resources you can use this:
# Eg: CLEAN=0 scripts/python/test.sh tests/bdd/features/volume/create/test_feature.py
# This way the cluster will remain in place, which sometimes can help figure out why it failed.

set -e

SCRIPT_DIR="$(dirname "$0")"
export ROOT_DIR="$SCRIPT_DIR/../.."
CI_REPORT="$ROOT_DIR/ci-report"
REPORT="$CI_REPORT/bdd-report.xml"
export FAILED_DOCKER_LOGS="$CI_REPORT/docker-logs.txt"

cleanup() {
  "$SCRIPT_DIR"/test-residue-cleanup.sh || true
}

cleanup_handler() {
  ERROR=$?
  trap - INT QUIT TERM HUP EXIT
  cleanup
  if [ $ERROR != 0 ]; then
    exit $ERROR
  fi
}

# FAST mode to avoid rebuilding certain dependencies
FAST=${FAST:-"0"}
# Set to 0 to avoid cleanup/teardown of the deployer cluster and its resources
CLEAN=${CLEAN:-}
# How to disable the above modes.
DISABLE=("no", "n", "false", "f", "0")

if ! [[ "${DISABLE[*]}" =~ "$FAST" ]]; then
  echo "FAST enabled - will not rebuild the csi&openapi clients nor the deployer. (Make sure they are built already)"
fi

# shellcheck source=/dev/null
. "$ROOT_DIR"/tests/bdd/setup.sh

cleanup >/dev/null
[ -f "$FAILED_DOCKER_LOGS" ] && rm "$FAILED_DOCKER_LOGS"
if ! [[ "${DISABLE[*]}" =~ "${CLEAN:-yes}" ]]; then
  trap cleanup_handler INT QUIT TERM HUP EXIT
fi

mkdir -p "$CI_REPORT"

# Extra arguments will be provided directly to pytest, otherwise the bdd folder will be tested with default arguments
if [ $# -eq 0 ]; then
  pytest "$BDD_TEST_DIR" --durations=20 --junit-xml="$REPORT"
else
  pytest "$@" --junit-xml="$REPORT"
fi
