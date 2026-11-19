#!/usr/bin/env bash
# 在临时工作区验证路径依赖闭包、指纹和复用回执。

set -euo pipefail
cd "$(dirname "$0")/../.."
repository_root=$PWD
unset AUCTION_WEB_STACK
source ci/release/build-identity.sh
source ci/release/web-bundle.sh

# 指纹测试使用真实函数，但把工具身份与 Cargo 配置固定成同一个替身；两栈只在技术栈上不同。
fixture_build_tools_identity() { printf '%s\n' 'fixture-tools'; }
fixture_cargo_config_fingerprint() { printf '%s\n' 'fixture-cargo'; }
build_tools_identity() { fixture_build_tools_identity; }
cargo_config_fingerprint() { fixture_cargo_config_fingerprint; }

fail() {
	printf 'Web 构建输入回归失败：%s\n' "$1" >&2
	exit 1
}

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
printf '[workspace]\nmembers = ["crates/*"]\nresolver = "3"\n' >"$fixture/Cargo.toml"
for package in auction-{web-public,web-admin,web-shared,contracts,time,jobs}; do
	directory="$fixture/crates/$package"
	mkdir -p "$directory/src"
	printf '[package]\nname = "%s"\nversion = "0.0.0"\n' "$package" >"$directory/Cargo.toml"
	touch "$directory/src/lib.rs"
done
for surface in public admin; do
	printf '\n[dependencies]\nauction-web-shared = { path = "../auction-web-shared" }\n' \
		>>"$fixture/crates/auction-web-$surface/Cargo.toml"
done
printf '\n[dependencies]\nauction-contracts = { path = "../auction-contracts" }\n' \
	>>"$fixture/crates/auction-web-shared/Cargo.toml"
printf '\n[dependencies]\ntime = { package = "auction-time", path = "../auction-time" }\n' \
	>>"$fixture/crates/auction-contracts/Cargo.toml"

metadata=$(cargo metadata --manifest-path "$fixture/Cargo.toml" --format-version 1 --no-deps --locked --offline)
checker="$repository_root/ci/checks/web-build-inputs.pl"
perl "$checker" crates/auction-web-shared crates/auction-contracts crates/auction-time <<<"$metadata"
if diagnostic=$(perl "$checker" crates/auction-web-shared crates/auction-contracts crates/auction-jobs <<<"$metadata" 2>&1); then
	fail '同时漏列传递依赖和多列无关成员时应失败'
fi
[[ "$diagnostic" == *'缺少工作区依赖：crates/auction-time'* &&
	"$diagnostic" == *'多出工作区依赖：crates/auction-jobs'* ]] || fail "$diagnostic"

cd "$fixture"
public_fingerprint=$(web_inputs_fingerprint public)
admin_fingerprint=$(web_inputs_fingerprint admin)
shared_build_id=$(web_build_identity)

printf '// 后端源码变化\n' >>crates/auction-jobs/src/lib.rs
[[ $(web_inputs_fingerprint public) == "$public_fingerprint" &&
	$(web_inputs_fingerprint admin) == "$admin_fingerprint" &&
	$(web_build_identity) == "$shared_build_id" ]] || fail '无关 crate 进入了 Web 输入'

printf '// 时间库源码变化\n' >>crates/auction-time/src/lib.rs
[[ $(web_inputs_fingerprint public) != "$public_fingerprint" &&
	$(web_inputs_fingerprint admin) != "$admin_fingerprint" &&
	$(web_build_identity) != "$shared_build_id" ]] || fail '时间库变化应使两端指纹和共享构建号失效'

# 固定工具环境和 payload 完整性，让两种判定都以构建号和输入变化为准。
verify_web_payload() { [[ $1 == "$surface" && $2 == "$stable_root" ]]; }
dx_version=fixture
rustc_version=fixture
brotli_version=fixture
build_environment=$(build_env_fingerprint)
stable_root="$fixture/payload"
mkdir "$stable_root"
web_build_info="$stable_root/WEB-BUILD-INFO"
for surface in public admin; do
	AUCTION_BUILD_ID=$(web_build_identity)
	source_identity=$(web_inputs_fingerprint "$surface")
	web_build_fields >"$web_build_info"
	web_payload_reusable || fail "$surface 回执匹配时应能复用"
	web_build_inputs_unchanged || fail "$surface 输入一致时应通过构建结束复核"

	other=$(web_other_surface "$surface")
	printf '// 对端源码变化\n' >>"crates/auction-web-$other/src/lib.rs"
	current_build_id=$(web_build_identity)
	[[ $(web_inputs_fingerprint "$surface") == "$source_identity" &&
		"$current_build_id" != "$AUCTION_BUILD_ID" ]] || fail "$surface 单端指纹应保持原值，共享构建号应变化"
	if web_build_inputs_unchanged; then fail "$surface 构建号变化时复核应失败"; fi
	AUCTION_BUILD_ID=$current_build_id
	if web_payload_reusable; then fail "$surface 带旧构建号的回执应拒绝复用"; fi

	web_build_fields >"$web_build_info"
	sed '/^build_id=/d' "$web_build_info" >"$fixture/legacy-receipt"
	mv -- "$fixture/legacy-receipt" "$web_build_info"
	if web_payload_reusable; then fail "$surface 缺少构建号的旧回执应拒绝复用"; fi
done

# 补齐 Solid 指纹需要的路径；前面的 Dioxus 用例已经跑完，不会受这些文件影响。
mkdir -p "$fixture/web/src" "$fixture/web/node_modules/pkg" "$fixture/web/.tmp"
mkdir -p "$fixture/web/public/node_modules/.vite/cache" "$fixture/web/tooling/.tmp" "$fixture/web/public/.tmp"
mkdir -p "$fixture/web/public/deep/node_modules/pkg" "$fixture/web/public/deep/.tmp"
mkdir -p "$fixture/crates/auction-web-shared/style" "$fixture/crates/auction-web-shared/assets"
for surface in public admin; do
	mkdir -p "$fixture/crates/auction-web-$surface/assets" "$fixture/crates/auction-web-$surface/i18n"
	printf 'surface\n' >"$fixture/crates/auction-web-$surface/assets/asset.txt"
	printf 'surface\n' >"$fixture/crates/auction-web-$surface/i18n/en.ftl"
done
printf 'package\n' >"$fixture/web/package.json"
printf 'app\n' >"$fixture/web/src/app.ts"
printf 'module\n' >"$fixture/web/node_modules/pkg/index.js"
printf 'scratch\n' >"$fixture/web/.tmp/scratch.js"
printf 'nested module\n' >"$fixture/web/public/node_modules/.vite/cache/index.js"
printf 'nested scratch\n' >"$fixture/web/tooling/.tmp/scratch.js"
printf 'surface scratch\n' >"$fixture/web/public/.tmp/scratch.js"
printf 'deep nested module\n' >"$fixture/web/public/deep/node_modules/pkg/index.js"
printf 'deep nested scratch\n' >"$fixture/web/public/deep/.tmp/scratch.js"
printf 'style\n' >"$fixture/crates/auction-web-shared/style/base.css"
printf 'asset\n' >"$fixture/crates/auction-web-shared/assets/shared.txt"
printf 'lock\n' >"$fixture/mise.lock"
mkdir -p "$fixture/ci/release"
printf 'bundle\n' >"$fixture/ci/release/web-bundle.sh"
printf 'identity\n' >"$fixture/ci/release/build-identity.sh"

# path_fingerprint 的缺省口径不能变化；排除项必须同时从 scope 标记和目录遍历中消失。
printf 'one\n' >>"$fixture/web/node_modules/pkg/index.js"
default_node=$(cd "$fixture" && path_fingerprint web)
printf 'two\n' >>"$fixture/web/node_modules/pkg/index.js"
[[ $(cd "$fixture" && path_fingerprint web) != "$default_node" ]] ||
	fail '缺省 path_fingerprint 应包含 node_modules'
excluded_node=$(cd "$fixture" && path_fingerprint --exclude web/node_modules web)
printf 'three\n' >>"$fixture/web/node_modules/pkg/index.js"
[[ $(cd "$fixture" && path_fingerprint --exclude web/node_modules web) == "$excluded_node" ]] ||
	fail '--exclude 应跳过 node_modules 子树'
default_tmp=$(cd "$fixture" && path_fingerprint --exclude web/node_modules web)
printf 'scratch\n' >>"$fixture/web/.tmp/scratch.js"
[[ $(cd "$fixture" && path_fingerprint --exclude web/node_modules web) != "$default_tmp" ]] ||
	fail '缺省 path_fingerprint 应包含临时目录'
excluded_tmp=$(cd "$fixture" && path_fingerprint --exclude web/node_modules --exclude web/.tmp web)
printf 'scratch again\n' >>"$fixture/web/.tmp/scratch.js"
[[ $(cd "$fixture" && path_fingerprint --exclude web/node_modules --exclude web/.tmp web) == "$excluded_tmp" ]] ||
	fail '--exclude 应跳过临时目录'

# 目录名级排除必须覆盖任意层级；默认调用仍要把这些文件算进去。
default_nested=$(cd "$fixture" && path_fingerprint web)
printf 'nested module changed\n' >>"$fixture/web/public/node_modules/.vite/cache/index.js"
[[ $(cd "$fixture" && path_fingerprint web) != "$default_nested" ]] ||
	fail '缺省 path_fingerprint 应包含嵌套 node_modules'
excluded_nested=$(cd "$fixture" && path_fingerprint "${web_input_excludes[@]}" web)
printf 'nested module changed again\n' >>"$fixture/web/public/node_modules/.vite/cache/index.js"
printf 'nested scratch changed\n' >>"$fixture/web/tooling/.tmp/scratch.js"
printf 'surface scratch changed\n' >>"$fixture/web/public/.tmp/scratch.js"
printf 'deep nested module changed\n' >>"$fixture/web/public/deep/node_modules/pkg/index.js"
printf 'deep nested scratch changed\n' >>"$fixture/web/public/deep/.tmp/scratch.js"
[[ $(cd "$fixture" && path_fingerprint "${web_input_excludes[@]}" web) == "$excluded_nested" ]] ||
	fail '--exclude-dir 应跳过任意层级的 node_modules 与 .tmp'

solid_inputs_fingerprint() (
	export AUCTION_WEB_STACK=solid
	cd "$fixture"
	source "$repository_root/ci/release/build-identity.sh"
	web_inputs_fingerprint public
)
solid_build_inputs_fingerprint() (
	export AUCTION_WEB_STACK=solid
	cd "$fixture"
	source "$repository_root/ci/release/build-identity.sh"
	build_inputs_fingerprint
)
solid_build_id() (
	export AUCTION_WEB_STACK=solid
	cd "$fixture"
	source "$repository_root/ci/release/build-identity.sh"
	web_build_identity
)
env_fingerprint_for_stack() (
	local stack=$1
	export AUCTION_WEB_STACK=$stack
	cd "$repository_root"
	source "$repository_root/ci/release/build-identity.sh"
	build_tools_identity() { fixture_build_tools_identity; }
	cargo_config_fingerprint() { fixture_cargo_config_fingerprint; }
	build_env_fingerprint
)

solid_inputs=$(solid_inputs_fingerprint)
solid_build_inputs=$(solid_build_inputs_fingerprint)
solid_build_id_value=$(solid_build_id)

printf 'web source\n' >>"$fixture/web/src/app.ts"
[[ "$(solid_inputs_fingerprint)" != "$solid_inputs" &&
	"$(solid_build_inputs_fingerprint)" != "$solid_build_inputs" &&
	"$(solid_build_id)" != "$solid_build_id_value" ]] ||
	fail 'web/** 变化应进入 Solid 指纹'
solid_inputs=$(solid_inputs_fingerprint)
solid_build_inputs=$(solid_build_inputs_fingerprint)
solid_build_id_value=$(solid_build_id)

printf 'shared style\n' >>"$fixture/crates/auction-web-shared/style/base.css"
[[ "$(solid_inputs_fingerprint)" != "$solid_inputs" &&
	"$(solid_build_inputs_fingerprint)" != "$solid_build_inputs" &&
	"$(solid_build_id)" != "$solid_build_id_value" ]] || fail '共享样式变化应进入 Solid 指纹'
solid_inputs=$(solid_inputs_fingerprint)
solid_build_inputs=$(solid_build_inputs_fingerprint)
solid_build_id_value=$(solid_build_id)

printf 'shared asset\n' >>"$fixture/crates/auction-web-shared/assets/shared.txt"
[[ "$(solid_inputs_fingerprint)" != "$solid_inputs" &&
	"$(solid_build_inputs_fingerprint)" != "$solid_build_inputs" &&
	"$(solid_build_id)" != "$solid_build_id_value" ]] || fail '共享资源变化应进入 Solid 指纹'
solid_inputs=$(solid_inputs_fingerprint)
solid_build_inputs=$(solid_build_inputs_fingerprint)
solid_build_id_value=$(solid_build_id)

printf 'public asset\n' >>"$fixture/crates/auction-web-public/assets/asset.txt"
[[ "$(solid_inputs_fingerprint)" != "$solid_inputs" &&
	"$(solid_build_inputs_fingerprint)" != "$solid_build_inputs" &&
	"$(solid_build_id)" != "$solid_build_id_value" ]] || fail 'public 资源变化应进入 Solid 指纹'
solid_inputs=$(solid_inputs_fingerprint)
solid_build_inputs=$(solid_build_inputs_fingerprint)
solid_build_id_value=$(solid_build_id)

printf 'public i18n\n' >>"$fixture/crates/auction-web-public/i18n/en.ftl"
[[ "$(solid_inputs_fingerprint)" != "$solid_inputs" &&
	"$(solid_build_inputs_fingerprint)" != "$solid_build_inputs" &&
	"$(solid_build_id)" != "$solid_build_id_value" ]] || fail 'public 译文变化应进入 Solid 指纹'
solid_inputs=$(solid_inputs_fingerprint)
solid_build_inputs=$(solid_build_inputs_fingerprint)
solid_build_id_value=$(solid_build_id)

printf 'unrelated\n' >>"$fixture/crates/auction-jobs/src/lib.rs"
[[ "$(solid_inputs_fingerprint)" == "$solid_inputs" ]] || fail '无关 crate 不应进入 Solid public 指纹'
[[ "$(solid_build_id)" == "$solid_build_id_value" ]] || fail '无关 crate 不应改变 Solid 构建号'
[[ "$(solid_build_inputs_fingerprint)" != "$solid_build_inputs" ]] ||
	fail '无关 crate 仍应进入原生构建输入'
solid_inputs=$(solid_inputs_fingerprint)
solid_build_inputs=$(solid_build_inputs_fingerprint)
solid_build_id_value=$(solid_build_id)

printf 'node_modules\n' >>"$fixture/web/node_modules/pkg/index.js"
printf 'temp\n' >>"$fixture/web/.tmp/scratch.js"
printf 'nested node_modules\n' >>"$fixture/web/public/node_modules/.vite/cache/index.js"
printf 'nested temp\n' >>"$fixture/web/tooling/.tmp/scratch.js"
printf 'nested surface temp\n' >>"$fixture/web/public/.tmp/scratch.js"
printf 'deep nested module\n' >>"$fixture/web/public/deep/node_modules/pkg/index.js"
printf 'deep nested temp\n' >>"$fixture/web/public/deep/.tmp/scratch.js"
[[ "$(solid_inputs_fingerprint)" == "$solid_inputs" ]] || fail 'Solid 指纹应排除 node_modules 与临时目录'
[[ "$(solid_build_inputs_fingerprint)" == "$solid_build_inputs" ]] ||
	fail 'Solid 原生输入指纹应排除 node_modules 与临时目录'
[[ "$(solid_build_id)" == "$solid_build_id_value" ]] || fail 'Solid 构建号应排除 node_modules 与临时目录'

printf 'lock\n' >>"$fixture/mise.lock"
[[ "$(solid_inputs_fingerprint)" != "$solid_inputs" &&
	"$(solid_build_inputs_fingerprint)" != "$solid_build_inputs" &&
	"$(solid_build_id)" != "$solid_build_id_value" ]] || fail 'mise.lock 变化应进入 Solid 指纹'
solid_inputs=$(solid_inputs_fingerprint)
solid_build_inputs=$(solid_build_inputs_fingerprint)
solid_build_id_value=$(solid_build_id)

printf 'cargo\n' >>"$fixture/Cargo.toml"
[[ "$(solid_inputs_fingerprint)" != "$solid_inputs" &&
	"$(solid_build_inputs_fingerprint)" != "$solid_build_inputs" &&
	"$(solid_build_id)" != "$solid_build_id_value" ]] || fail 'Cargo.toml 变化应进入 Solid 指纹'
solid_inputs=$(solid_inputs_fingerprint)
solid_build_inputs=$(solid_build_inputs_fingerprint)
solid_build_id_value=$(solid_build_id)

printf 'bundle\n' >>"$fixture/ci/release/web-bundle.sh"
[[ "$(solid_inputs_fingerprint)" != "$solid_inputs" &&
	"$(solid_build_inputs_fingerprint)" != "$solid_build_inputs" &&
	"$(solid_build_id)" != "$solid_build_id_value" ]] || fail '发布脚本变化应进入 Solid 指纹'
solid_inputs=$(solid_inputs_fingerprint)
solid_build_inputs=$(solid_build_inputs_fingerprint)
solid_build_id_value=$(solid_build_id)

printf 'identity\n' >>"$fixture/ci/release/build-identity.sh"
[[ "$(solid_inputs_fingerprint)" != "$solid_inputs" &&
	"$(solid_build_inputs_fingerprint)" != "$solid_build_inputs" &&
	"$(solid_build_id)" != "$solid_build_id_value" ]] || fail '身份脚本变化应进入 Solid 指纹'

default_env=$(build_env_fingerprint)
explicit_dioxus_env=$(env_fingerprint_for_stack dioxus)
solid_env=$(env_fingerprint_for_stack solid)
[[ -n "$default_env" && "$explicit_dioxus_env" == "$default_env" ]] ||
	fail '缺省与显式 dioxus 的环境身份应相同'
[[ -n "$solid_env" && "$solid_env" != "$default_env" ]] || fail 'Solid 技术栈应进入环境指纹'

printf '%s\n' 'Web 构建输入回归检查通过'
