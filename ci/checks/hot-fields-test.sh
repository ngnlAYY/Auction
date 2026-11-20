#!/usr/bin/env bash

# 用临时工作树验证 hot-fields 同时核对 Rust 与 TypeScript 的 Admin 镜像表。
set -euo pipefail
cd "$(dirname "$0")/../.."

fail() {
	printf 'hot-fields 自测失败：%s\n' "$1" >&2
	exit 1
}

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT

mkdir -p "$fixture/ci/checks" \
	"$fixture/crates/auction-application/src/ports/admin/runtime_config" \
	"$fixture/crates/auction-web-admin/src/surface/config" \
	"$fixture/web/admin/src/surface/config"
cp ci/checks/{check.sh,hot-fields.pl} "$fixture/ci/checks/"
cp crates/auction-application/src/ports/admin/runtime_config/hot.rs \
	"$fixture/crates/auction-application/src/ports/admin/runtime_config/hot.rs"
cp crates/auction-web-admin/src/surface/config/hot_fields.rs \
	"$fixture/crates/auction-web-admin/src/surface/config/hot_fields.rs"
cp web/admin/src/surface/config/hot-fields.ts \
	"$fixture/web/admin/src/surface/config/hot-fields.ts"

bash "$fixture/ci/checks/check.sh" hot-fields || fail '同序镜像的正例应通过'

typescript=web/admin/src/surface/config/hot-fields.ts
rust=crates/auction-web-admin/src/surface/config/hot_fields.rs
first='operations.mail.send_intervals.outbid_seconds'
second='operations.mail.send_intervals.account_seconds'
literal_error='热字段表项必须是字段路径字符串字面量'
separator_error='热字段表项后应为逗号或数组结束符'

# 正例也覆盖数组内的块注释与行尾注释，以及 Rust 的嵌套块注释。
sed -i -e "/\"$first\",/i\\  /* \"operations.test.ignored\", */" \
	-e "/\"$first\",/s|$| // \"operations.test.ignored\",|" "$fixture/$typescript"
sed -i "/\"$first\",/i\\    /* 注释 /* 嵌套 */ \"operations.test.ignored\", */" "$fixture/$rust"
bash "$fixture/ci/checks/check.sh" hot-fields || fail '有注释的同序镜像应通过'

reject_mirror() {
	local mirror=$1 case_name=$2 expected=$3 edit=$4 diagnostic
	cp "$mirror" "$fixture/$mirror"
	sed -i "$edit" "$fixture/$mirror"
	if diagnostic=$(bash "$fixture/ci/checks/check.sh" hot-fields 2>&1); then
		fail "$case_name 应失败"
	fi
	[[ "$diagnostic" == *"$mirror"* && "$diagnostic" == *"$expected"* ]] ||
		fail "$case_name 缺少明确诊断：$diagnostic"
	if [[ "$expected" == "$literal_error" || "$expected" == "$separator_error" ]]; then
		[[ "$diagnostic" =~ :[1-9][0-9]*:[1-9][0-9]*: ]] ||
			fail "$case_name 缺少行列号：$diagnostic"
	fi
	cp "$mirror" "$fixture/$mirror"
}

reject_mirror "$typescript" 'TypeScript 删项' '必须逐项同序' "/\"$first\",/d"
reject_mirror "$typescript" 'TypeScript 增项' '必须逐项同序' "/\"$first\",/a\\  \"operations.test.added\","
reject_mirror "$typescript" 'TypeScript 换序' '必须逐项同序' "s/$first/$second/; t; s/$second/$first/"
reject_mirror "$typescript" 'TypeScript 改名' '必须逐项同序' "s/$first/operations.test.renamed/"
reject_mirror "$typescript" 'TypeScript 注释掉一项' '必须逐项同序' "/\"$first\",/s|^|// |"
reject_mirror "$typescript" 'TypeScript 使用展开项' "$literal_error" "/\"$first\",/a\\  ...EXTRA_HOT_FIELDS,"
reject_mirror "$typescript" 'TypeScript 使用非字面量表达式' "$literal_error" "/\"$first\",/s/\"$first\"/EXTRA_HOT_FIELD/"
reject_mirror "$typescript" 'TypeScript 使用字符串拼接' "$separator_error" "/\"$first\",/s/\",/\" + SUFFIX,/"
reject_mirror "$rust" 'Rust 注释掉一项' '必须逐项同序' "/\"$first\",/s|^|// |"
reject_mirror "$rust" 'Rust 使用非字面量表达式' "$literal_error" "/\"$first\",/s/\"$first\"/EXTRA_HOT_FIELD/"

sed -i '/"operations.worker.delivery.finalize_timeout_seconds",/a\    "operations.test.added",' \
	"$fixture/crates/auction-application/src/ports/admin/runtime_config/hot.rs"
if diagnostic=$(bash "$fixture/ci/checks/check.sh" hot-fields 2>&1); then
	fail '应用层新增字段但镜像未同步时应失败'
fi
[[ "$diagnostic" == *"$rust"* &&
	"$diagnostic" == *'必须逐项同序'* ]] || fail "应用层漂移缺少明确诊断：$diagnostic"

printf '%s\n' 'hot-fields 自测通过'
