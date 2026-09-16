#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/.."

# 仅编译并运行静态文档检查器；输出留在被忽略的 target 目录。
rustfmt --edition 2024 --check ci/docs/main.rs
mkdir -p target/docs-check
rustc --edition=2024 -D warnings ci/docs/main.rs -o target/docs-check/verify-docs
target/docs-check/verify-docs
