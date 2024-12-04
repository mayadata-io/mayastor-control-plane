#!/usr/bin/env bash

cargo fmt -- --version
cargo fmt --all -- --config imports_granularity=Crate

cargo clippy -- --version
cargo clippy --all --all-targets $1 -- -D warnings
