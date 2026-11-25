#!/usr/bin/env bash
# 验证发布配方拒绝预览 feature，覆盖仓库配方及临时注入的分体、合并配方。

set -euo pipefail
cd "$(dirname "$0")/../.."

fail() {
	printf '寄售发布 feature 自测失败：%s\n' "$1" >&2
	exit 1
}

for stack in dioxus solid; do
	AUCTION_WEB_STACK=$stack bash ci/release/release.sh verify-release-features
done

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
mkdir -p "$fixture/ci/release" "$fixture/bin"
cp ci/release/build-identity.sh ci/release/web-stack.sh "$fixture/ci/release/"
cat >"$fixture/bin/git" <<'FIXTURE'
#!/usr/bin/env bash
[[ "$*" == 'rev-parse --show-toplevel' ]] || exit 1
printf '%s\n' "$RELEASE_TEST_ROOT"
FIXTURE
cat >"$fixture/bin/mise" <<'FIXTURE'
#!/usr/bin/env bash
printf '发布工具不应被调用\n' >&2
exit 99
FIXTURE
chmod +x "$fixture/bin/git" "$fixture/bin/mise"

for assignment in api_only_features embedded_features single_features; do
	case "$assignment" in
	api_only_features) value=consignment-dev-preview ;;
	embedded_features) value='auction-public-api/web-solid auction-cli/consignment-dev-preview' ;;
	single_features) value='auction-server/web-embed,auction-server/consignment-dev-preview' ;;
	esac
	sed "s|^$assignment=.*|$assignment='$value'|" ci/release/release.sh >"$fixture/ci/release/release.sh"
	for ref in branch tag; do
		case "$ref" in
		branch) stack=solid ;;
		tag) stack=dioxus ;;
		esac
		if output=$(PATH="$fixture/bin:$PATH" RELEASE_TEST_ROOT="$fixture" AUCTION_WEB_STACK="$stack" \
			GITHUB_ACTIONS=true GITHUB_REF_TYPE="$ref" bash "$fixture/ci/release/release.sh" release-check 2>&1); then
			fail "$ref 中的 $assignment 接受了预览 feature"
		fi
		[[ "$output" == '发布及候选制品禁止启用 consignment-dev-preview。' ]] ||
			fail "$ref 中的 $assignment 未在工具前给出配置错误"
	done
done

printf '寄售发布 feature 自测通过（2 个合法配方，6 个拒绝用例）\n'
