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

# 打印迁移里发现的 consignment_* 表名（排序去重）；root 是含 migrations/ 的目录。
# CREATE TABLE 可能跨行或大小写不同，先拼接成单行再统一小写匹配；语句数与唯一表数必须
# 一致，避免提取规则把一条语句拆错或同名表重复创建时静默漏检。
discover_tables() {
	local root=$1 normalized matches statements tables unique
	normalized=$(cat "$root"/migrations/postgres/00013[4-7]_*.up.sql |
		tr -s '[:space:]' ' ' | tr '[:upper:]' '[:lower:]') || fail '无法读取迁移文件'
	matches=$(grep -oE 'create table (if not exists )?(public\.)?consignment_[a-z0-9_]+' <<<"$normalized") ||
		matches=''
	[[ -n "$matches" ]] || fail '迁移文件里没有取到任何 consignment_* 表'
	statements=$(wc -l <<<"$matches")
	tables=$(grep -oE 'consignment_[a-z0-9_]+' <<<"$matches" | sort -u) || tables=''
	unique=$(wc -l <<<"$tables")
	[[ "$statements" -eq "$unique" ]] ||
		fail "CREATE TABLE 语句数 $statements 与取到的唯一表数 $unique 不一致"
	printf '%s\n' "$tables"
}

# 打印汇总节中缺失的表名；root 是含 migrations/ 与 ops/ 的目录。
missing_tables() {
	local root=$1 tables section table
	tables=$(discover_tables "$root") || exit 1
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

# 大小写与换行不应影响表发现：探针文件加入后再删除，不影响后面的反例。
probe="$fixture/migrations/postgres/000137_probe.up.sql"
printf 'create table\n\tpublic.consignment_probe_extra (\n\tid uuid\n);\n' >"$probe"
grep -qx 'consignment_probe_extra' <<<"$(discover_tables "$fixture")" ||
	fail '跨行或小写的 CREATE TABLE 未被发现'
rm -f -- "$probe"

# 反例一：整节删除，所有表都应报缺。
awk -v heading="$heading" '
	$0 == heading { skip = 1; next }
	skip && /^## / { skip = 0 }
	!skip { print }
' ops/deploy/RECOVERY.md >"$fixture/ops/deploy/RECOVERY.md"
[[ -n "$(missing_tables "$fixture")" ]] || fail '删去汇总节后核对仍通过'

# 反例二：从发现结果取一个受害表，只在汇总节内删去它的行，应恰好报这一个表。
victim_tables=$(discover_tables .) || exit 1
victim=${victim_tables%%$'\n'*}
awk -v heading="$heading" -v victim="$victim" '
	$0 == heading { inside = 1; print; next }
	inside && /^## / { inside = 0 }
	inside && $0 ~ ("public\\." victim "([^a-z0-9_]|$)") { next }
	{ print }
' ops/deploy/RECOVERY.md >"$fixture/ops/deploy/RECOVERY.md"
actual=$(missing_tables "$fixture")
[[ "$actual" == "$victim" ]] ||
	fail "只在汇总节删去 $victim 后没有恰好报出该表：$(tr '\n' ' ' <<<"$actual")"

printf 'recovery-tables 自测通过\n'
