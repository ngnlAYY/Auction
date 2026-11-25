#!/usr/bin/env bash

# 核对 ops/deploy/RECOVERY.md 的“跨 breaking 迁移回退汇总核对”一节覆盖迁移 000134–000137 新增的
# 每个 consignment_* 表；并在临时副本中删去该节、删去一个表名，确认核对会失败。
set -euo pipefail
cd "$(dirname "$0")/../.."

fail() {
	printf 'recovery-tables 自测失败：%s\n' "$1" >&2
	exit 1
}

heading='## 寄售跨 breaking 迁移回退汇总核对'

# 打印汇总节中缺失的表名；root 是含 migrations/ 与 ops/ 的目录。
missing_tables() {
	local root=$1 tables section table
	tables=$(cat "$root"/migrations/postgres/00013[4-7]_*.up.sql |
		grep -oE 'CREATE TABLE (IF NOT EXISTS )?(public\.)?consignment_[a-z0-9_]+' |
		grep -oE 'consignment_[a-z0-9_]+' | sort -u)
	[[ -n "$tables" ]] || fail '迁移文件里没有取到任何 consignment_* 表'
	section=$(awk -v heading="$heading" '
		$0 == heading { inside = 1; next }
		inside && /^## / { exit }
		inside { print }
	' "$root/ops/deploy/RECOVERY.md")
	while IFS= read -r table; do
		grep -qE "public\.$table\b" <<<"$section" || printf '%s\n' "$table"
	done <<<"$tables"
}

gaps=$(missing_tables .)
[[ -z "$gaps" ]] || fail "汇总节缺少表：$(tr '\n' ' ' <<<"$gaps")"

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/migrations/postgres" "$fixture/ops/deploy"
cp migrations/postgres/00013[4-7]_*.up.sql "$fixture/migrations/postgres/"

# 反例一：整节删除，所有表都应报缺。
awk -v heading="$heading" '
	$0 == heading { skip = 1; next }
	skip && /^## / { skip = 0 }
	!skip { print }
' ops/deploy/RECOVERY.md >"$fixture/ops/deploy/RECOVERY.md"
[[ -n "$(missing_tables "$fixture")" ]] || fail '删去汇总节后核对仍通过'

# 反例二：只去掉一个表的汇总行，应恰好报这一个表。
sed '/public\.consignment_sale_closures\b/d' ops/deploy/RECOVERY.md >"$fixture/ops/deploy/RECOVERY.md"
[[ "$(missing_tables "$fixture")" == consignment_sale_closures ]] || fail '去掉单个表后没有恰好报出该表'

printf 'recovery-tables 自测通过\n'
