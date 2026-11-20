#!/usr/bin/env bash

# 用临时工作树验证装备诊断闭集门禁同时核对 Rust 与 TypeScript 的 Admin 实现。
set -euo pipefail
cd "$(dirname "$0")/../.."

fail() {
	printf 'equipment-contract 自测失败：%s\n' "$1" >&2
	exit 1
}

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT

mkdir -p "$fixture/ci/checks" "$fixture/migrations/postgres" \
	"$fixture/crates/auction-application/src/ports/mail/equipment_diagnostics" \
	"$fixture/crates/auction-web-admin/src/api" \
	"$fixture/crates/auction-web-admin/src/surface/mail" \
	"$fixture/crates/auction-web-admin/i18n" \
	"$fixture/web/admin/src/api" "$fixture/web/admin/src/surface/mail"
cp ci/checks/equipment-contract.sh "$fixture/ci/checks/equipment-contract.sh"
cp crates/auction-application/src/ports/mail/equipment_diagnostics/reason.rs \
	"$fixture/crates/auction-application/src/ports/mail/equipment_diagnostics/reason.rs"
cp migrations/postgres/*.up.sql "$fixture/migrations/postgres/"
cp crates/auction-web-admin/src/api/mail.rs "$fixture/crates/auction-web-admin/src/api/mail.rs"
cp crates/auction-web-admin/src/surface/mail/messages.rs \
	"$fixture/crates/auction-web-admin/src/surface/mail/messages.rs"
cp crates/auction-web-admin/i18n/*.ftl "$fixture/crates/auction-web-admin/i18n/"
cp web/admin/src/api/mail-response.ts "$fixture/web/admin/src/api/mail-response.ts"
cp web/admin/src/surface/mail/messages.ts "$fixture/web/admin/src/surface/mail/messages.ts"

bash "$fixture/ci/checks/equipment-contract.sh" || fail '镜像一致的正例应通过'

check_decoder_comments() {
	local source=$1 label=$2 block_comment=$3
	local decoder="$fixture/$source" diagnostic
	sed -i -e '/"equipment_name_invalid"/s|$| // "equipment_quality_unknown"|' \
		-e "/\"equipment_kind_unknown\"/s|$| $block_comment|" "$decoder"
	bash "$fixture/ci/checks/equipment-contract.sh" ||
		fail "$label 解码器在注释中提及已有原因时应通过"

	sed -i -E '/^[[:space:]]*(\||detailCode ===) "equipment_quality_unknown"/d' "$decoder"
	if diagnostic=$(bash "$fixture/ci/checks/equipment-contract.sh" 2>&1); then
		fail "$label 解码器只在行尾注释中保留缺失原因时应失败"
	fi
	[[ "$diagnostic" == *"$label 诊断原因 与装备来源原因枚举不一致"* ]] ||
		fail "$label 解码器缺项时缺少明确诊断：$diagnostic"
	cp "$source" "$decoder"
}

check_decoder_comments crates/auction-web-admin/src/api/mail.rs 'Admin' \
	'/* /* 内层注释 */ "equipment_kind_unknown" */'
check_decoder_comments web/admin/src/api/mail-response.ts 'TypeScript Admin' \
	'/* "equipment_kind_unknown" */'

sed -i '/detailCode === "equipment_canonical_mismatch"/d' \
	"$fixture/web/admin/src/api/mail-response.ts"
if diagnostic=$(bash "$fixture/ci/checks/equipment-contract.sh" 2>&1); then
	fail 'TypeScript 解码器缺少原因时应失败'
fi
[[ "$diagnostic" == *'TypeScript Admin 诊断原因'* ]] ||
	fail "TypeScript 解码器漂移缺少明确诊断：$diagnostic"

cp web/admin/src/api/mail-response.ts "$fixture/web/admin/src/api/mail-response.ts"
sed -i '/case "equipment_canonical_mismatch":/,+1d' \
	"$fixture/web/admin/src/surface/mail/messages.ts"
if diagnostic=$(bash "$fixture/ci/checks/equipment-contract.sh" 2>&1); then
	fail 'TypeScript 文案缺少原因时应失败'
fi
[[ "$diagnostic" == *'TypeScript Admin 诊断说明'* ]] ||
	fail "TypeScript 文案漂移缺少明确诊断：$diagnostic"

cp web/admin/src/surface/mail/messages.ts "$fixture/web/admin/src/surface/mail/messages.ts"
sed -i 's/admin-mail-detail-source-attributes-invalid/admin-mail-detail-source-canonical-mismatch/; t; s/admin-mail-detail-source-canonical-mismatch/admin-mail-detail-source-attributes-invalid/' \
	"$fixture/web/admin/src/surface/mail/messages.ts"
if diagnostic=$(bash "$fixture/ci/checks/equipment-contract.sh" 2>&1); then
	fail 'TypeScript 两个原因的译文键互换时应失败'
fi
[[ "$diagnostic" == *'equipment_attributes_invalid 应为 admin-mail-detail-source-attributes-invalid，实际为 admin-mail-detail-source-canonical-mismatch'* ]] ||
	fail "TypeScript 译文键互换缺少明确诊断：$diagnostic"

cp web/admin/src/surface/mail/messages.ts "$fixture/web/admin/src/surface/mail/messages.ts"
sed -i '/^admin-mail-detail-source-canonical-mismatch =/d' \
	"$fixture/crates/auction-web-admin/i18n/en.ftl"
if diagnostic=$(bash "$fixture/ci/checks/equipment-contract.sh" 2>&1); then
	fail '译文缺失时应失败'
fi
[[ "$diagnostic" == *'en 缺少诊断译文 admin-mail-detail-source-canonical-mismatch'* ]] ||
	fail "译文缺失缺少明确诊断：$diagnostic"

printf '%s\n' 'equipment-contract 自测通过'
