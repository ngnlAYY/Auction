#!/usr/bin/env bash
# 验证 SQL 时钟门禁的字面量边界、退出码与行号。

set -euo pipefail
cd "$(dirname "$0")/../.."
fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT

cat >"$fixture/allowed.rs" <<'RUST'
// "SELECT clock_timestamp(), clock_timestamp()"
/* 注释 /* 嵌套注释 */ "SELECT clock_timestamp(), clock_timestamp()" */
const UPDATE: &str = "WITH sql_clock AS MATERIALIZED (SELECT clock_timestamp() AS at)
    UPDATE example SET a = (SELECT at FROM sql_clock), b = (SELECT at FROM sql_clock)";
const RAW: &str = r##"SELECT transaction_timestamp(), '带 " 和 "# 的文本'"##;
const QUOTE: char = '"';
const NON_CLOCK: &str = "SELECT custom_now(), custom_clock_timestamp()";
const STABLE: &str = "SELECT now(), now(), transaction_timestamp(),
    transaction_timestamp(), statement_timestamp(), statement_timestamp()";
const MIXED: &str = r#"SELECT clock_timestamp(), now(), now(),
    transaction_timestamp(), statement_timestamp(), CURRENT_TIMESTAMP"#;
RUST
printf '%s\0' "$fixture/allowed.rs" | perl ci/checks/sql-clock.pl

cat >"$fixture/forbidden.rs" <<'RUST'
const ORDINARY: &str = "SELECT clock_timestamp(), clock_timestamp()";
const RAW: &str = r#"SELECT clock_timestamp(),
    pg_catalog.clock_timestamp ()"#;
const MULTILINE: &str = "SELECT statement_timestamp(), clock_timestamp(),
    CLOCK_TIMESTAMP (
    )";
RUST
status=0
diagnostic=$(printf '%s\0' "$fixture/forbidden.rs" | perl ci/checks/sql-clock.pl 2>&1) || status=$?
expected=$(printf '%s\n' "$fixture/forbidden.rs:"{1,2,4})
if [[ $status -ne 1 || ${diagnostic#*$'\n'} != "$expected" ]]; then
    printf '%s\n' 'SQL 时钟门禁回归失败，退出码或行号不匹配。' "$diagnostic" >&2
    exit 1
fi
printf '%s\n' 'SQL 时钟门禁回归通过'
