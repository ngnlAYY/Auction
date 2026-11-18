#!/usr/bin/env bash
# 在临时工作区验证路径依赖闭包、指纹和复用回执。

set -euo pipefail
cd "$(dirname "$0")/../.."
repository_root=$PWD
source ci/release/build-identity.sh
source ci/release/web-bundle.sh

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
build_env_fingerprint() { printf '%s\n' 'fixture'; }
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

printf '%s\n' 'Web 构建输入回归检查通过'
