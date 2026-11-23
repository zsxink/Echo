#!/usr/bin/env bash
# Pin cache identity to the toolchain this job installs, not the hosted
# image's unrelated preinstalled Rust. Keep CARGO_HOME and its cache intact.
set -euo pipefail
printf 'RUSTUP_HOME=%s\n' "${HOME:?}/.rustup-echo-ci" >> "${GITHUB_ENV:?}"
