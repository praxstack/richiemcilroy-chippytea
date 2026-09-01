#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR cargo test --locked
./scripts/build.sh
app_output="${CHIPPYTEA_APP_OUTPUT:-build/Chippytea.app}"
env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR "$app_output/Contents/MacOS/Chippytea" --self-test
env -u CARGO_TARGET_DIR -u CARGO_BUILD_TARGET_DIR "$app_output/Contents/MacOS/Chippytea" --access-flow-test
