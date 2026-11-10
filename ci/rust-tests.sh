#!/bin/sh
# Run the Rust tests of every member of the top-level Cargo workspace with line coverage, from the repository's top
# level, and write their reports: nextest's JUnit report to target/nextest/ci/junit.xml (ci/nextest.toml's ci
# profile) and the Cobertura report to coverage-rust.xml. Arguments go on to cargo-nextest, such as a filter
# (-E '...'). Exits 0 whatever tests fail, as --ignore-run-fail has it, the gate judging them from the reports; exits
# non-zero when the tests did not build or a tool could not run.
#
#   ci/rust-tests.sh [<cargo-nextest argument>...]
#
# The Jenkinsfile's Test stage runs it, and so does gate.toml's [[test]], so a gate run outside Jenkins, such as
# octoflow's `--quality-gate local`, measures coverage the way a Jenkins build does. The same lines only as far as the
# engine's generated-input tests reach the same ones: the Test stage gives them a run seed (OCTOFLOW_TEST_SEED,
# engine/src/test_support.rs) and this script gives none. Needs cargo-llvm-cov and cargo-nextest.
set -eu

cd "$(dirname "$0")/.."
# The e2e scenarios and the docs check (engine/e2e) run the orchestrator binary this command builds, instrumented,
# so that their runs of it count to its coverage and they build no engine of their own; they pass it this run's
# LLVM_PROFILE_FILE. Without it they would build an engine of their own that nothing measures.
E2E_ENGINE="${CARGO_TARGET_DIR:-$PWD/target}/llvm-cov-target/debug/orchestrator"
export E2E_ENGINE
exec cargo llvm-cov nextest --locked --workspace --ignore-run-fail --profile ci --config-file ci/nextest.toml \
    --cobertura --output-path coverage-rust.xml "$@"
