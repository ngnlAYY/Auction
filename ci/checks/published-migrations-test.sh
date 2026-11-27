#!/usr/bin/env bash
# 已发布迁移门禁自测：在临时 Git 仓库里用真实 tag 验证两件事——已发布文件逐字节不变，
# 新迁移只能追加在已发布最大编号之后（否则 SQLx 会在升级库上把它插到历史中间执行）。
# 全程离线，不读取本仓库的 tag，也不执行任何 SQL。

set -euo pipefail
cd "$(dirname "$0")/../.."

fail() {
	printf '已发布迁移自测失败：%s\n' "$1" >&2
	exit 1
}

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
repo=$fixture/repo
mkdir -p "$repo/ci/checks" "$repo/migrations/postgres"
cp ci/checks/published-migrations.sh "$repo/ci/checks/"

git -C "$repo" init -q
git -C "$repo" config user.name 'fixture'
git -C "$repo" config user.email 'fixture@example.invalid'
git -C "$repo" config commit.gpgsign false
printf 'published first\n' >"$repo/migrations/postgres/000001_first.up.sql"
printf 'published second\n' >"$repo/migrations/postgres/000002_second.up.sql"
git -C "$repo" add -A
git -C "$repo" commit -q -m 'v0.1.0'
git -C "$repo" tag v0.1.0

run() {
	(cd "$repo" && bash ci/checks/published-migrations.sh 2>&1)
}

# 基线：已发布文件未改动时通过。
output=$(run) || fail "未改动时应通过：$output"
[[ $output == *'检查通过'* ]] || fail "通过时缺少摘要：$output"

# 已发布文件被改写、删除：都必须失败。
printf 'rewritten\n' >>"$repo/migrations/postgres/000001_first.up.sql"
if output=$(run); then fail '改写已发布迁移应失败'; fi
[[ $output == *'已发布迁移被修改'* ]] || fail "改写拒绝缺少诊断：$output"
git -C "$repo" checkout -q -- migrations/postgres/000001_first.up.sql
rm -f "$repo/migrations/postgres/000002_second.up.sql"
if output=$(run); then fail '删除已发布迁移应失败'; fi
[[ $output == *'已发布迁移被删除'* ]] || fail "删除拒绝缺少诊断：$output"
git -C "$repo" checkout -q -- migrations/postgres/000002_second.up.sql

# 追加更大编号：通过。
printf 'appended\n' >"$repo/migrations/postgres/000003_third.up.sql"
output=$(run) || fail "追加更大编号应通过：$output"

# 新增文件插到已发布最大编号之前（含使用同一编号的不同文件名）：失败并点名新规则。
printf 'inserted\n' >"$repo/migrations/postgres/000002_other.up.sql"
if output=$(run); then fail '新增编号不大于已发布最大编号应失败'; fi
[[ $output == *'只能追加更大的编号'* ]] || fail "顺序拒绝缺少诊断：$output"
rm -f "$repo/migrations/postgres/000002_other.up.sql"

# 删除新迁移后恢复通过，确认前面的拒绝没有留下失败状态。
output=$(run) || fail "删除越序文件后应恢复通过：$output"

printf '已发布迁移门禁自测通过\n'
