#!/usr/bin/env bash
# 构建单个 Web surface 的固定 payload；输入未变且产物通过完整门禁时直接复用。
# 用法：ci/release/web-bundle.sh public|admin

set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
cd "$repository_root"

# 回执验证、工具选择、指纹与 payload 门禁和原生发布入口共用同一份实现。
source ci/release/build-identity.sh
configure_build_tools

# 浏览器构建号与原生程序共用来源指纹；部署后旧页面据此发现偏差。复用判定不因它改变。
AUCTION_BUILD_ID=$(web_build_identity) || exit 1
export AUCTION_BUILD_ID

surface=${1:-}
if ! package=$(web_surface_package "$surface"); then
	printf '未知 Web surface：%s\n' "$surface" >&2
	exit 2
fi

stable_root="$repository_root/target/dx/build/$package/web"
payload_root="$stable_root/public"
payload_manifest="$stable_root/PAYLOAD-SHA256"
marker="$payload_root/auction-web-manifest.v1"
# 复用标记放在 payload 之外：它不属于内嵌资源，也不计入 payload 哈希。
web_build_info="$stable_root/WEB-BUILD-INFO"

source_epoch=$(git show -s --format=%ct HEAD)
if [[ ! "$source_epoch" =~ ^[1-9][0-9]*$ ]]; then
	printf '无法取得有效的 SOURCE_DATE_EPOCH：%s\n' "$source_epoch" >&2
	exit 1
fi

# 复用判定还必须区分构建工具：dx 与 rustc 版本变化会改变生成的 JS/WASM。
dx_version=$(dx --version 2>/dev/null | sed -n '1p')
rustc_version=$(rustc --version --verbose 2>/dev/null | sed -n '1p')
if [[ -z "$dx_version" || -z "$rustc_version" ]]; then
	printf '无法读取 dx/rustc 版本，Web 构建不能建立复用标记\n' >&2
	exit 1
fi
# payload 指纹覆盖本 surface、共享层与契约源码；DTO 变化必须重建两个浏览器，避免内嵌旧协议。
source_identity=$(web_inputs_fingerprint "$surface")
# WASM 自定义 profile 与 rustflags 同样影响页面产物，不能只使外层原生二进制缓存失效。
build_environment=$(build_env_fingerprint)

# WEB-BUILD-INFO 的全部字段；写入与复用比较共用一份清单。
web_build_fields() {
	printf '%s\n' \
		'schema=auction-web-build/1' \
		"surface=$surface" \
		'profile=wasm-release' \
		"dx_version=$dx_version" \
		"rustc_version=$rustc_version" \
		"source_fingerprint=$source_identity" \
		"build_env_sha256=$build_environment"
}

# 复用条件：构建标记逐字段一致，且已有 payload 重新通过完整门禁。
# 命中后既不重新运行 dx，也不改写 mtime，保证内嵌资源与上一份制品逐字节相同。
web_payload_reusable() {
	[[ -d "$stable_root" && ! -L "$stable_root" && -f "$web_build_info" && ! -L "$web_build_info" ]] || return 1
	[[ $(cat "$web_build_info") == "$(web_build_fields)" ]] || return 1
	verify_web_payload "$surface" "$stable_root" >/dev/null
}

if web_payload_reusable; then
	printf '%s Web payload 输入未变，复用：sha256=%s files=%s\n' \
		"$surface" "$(read_build_info "$marker" payload_sha256)" "$(wc -l <"$payload_manifest")"
	exit 0
fi

# Dioxus 会把继承 release 的自定义 profile 归入 release 中间目录；两个精确路径都清理，避免把历史
# 指纹文件再次复制进发布 payload，同时不触碰另一个 surface 或 Cargo 的共享依赖缓存。
rm -rf "$stable_root" "$repository_root/target/dx/$package/release/web" \
	"$repository_root/target/dx/$package/wasm-release/web"

SOURCE_DATE_EPOCH=$source_epoch \
	dx bundle \
	-p "$package" \
	--platform web \
	--profile wasm-release \
	--debug-symbols false \
	--out-dir "$stable_root" \
	--locked

# 先拒绝符号链接和特殊文件，再执行会跟随路径的 chmod/touch。
if [[ ! -f "$payload_root/index.html" ]] || ! web_payload_tree_is_plain "$payload_root"; then
	printf '%s Web payload 缺少 index.html 或含符号链接/特殊文件\n' "$surface" >&2
	exit 1
fi

next_files=()
cleanup() {
	rm -f -- "${next_files[@]}"
}
trap cleanup EXIT

payload_next=$(mktemp "$stable_root/PAYLOAD-SHA256.next.XXXXXX")
next_files+=("$payload_next")
web_payload_checksums "$payload_root" >"$payload_next"
if ! grep -Fq '  ./index.html' "$payload_next"; then
	printf '%s Web payload 清单没有绑定 index.html\n' "$surface" >&2
	exit 1
fi
payload_sha256=$(sha256_file "$payload_next")
printf '%s\n' \
	'schema=auction-web-assets/1' \
	"surface=$surface" \
	'profile=wasm-release' \
	"payload_sha256=$payload_sha256" \
	>"$marker"
install -m 0644 "$payload_next" "$payload_manifest"

# rust-embed 会保留资源 mtime；统一时间和权限使相同 payload 的二进制元数据可复核。
find "$payload_root" -type f -exec chmod 0644 {} +
find "$payload_root" -type d -exec chmod 0755 {} +
find "$payload_root" -exec touch -d "@$source_epoch" {} +
touch -d "@$source_epoch" "$payload_manifest"

# 新 payload 与复用走同一完整门禁，并复核构建期间输入未变，不能把中途变化标记成旧快照。
verify_web_payload "$surface" "$stable_root" >/dev/null
current_source_identity=$(web_inputs_fingerprint "$surface")
current_build_environment=$(build_env_fingerprint)
if [[ "$current_source_identity" != "$source_identity" ||
"$current_build_environment" != "$build_environment" ]]; then
	printf '%s Web 构建期间输入发生变化，请重新构建\n' "$surface" >&2
	exit 1
fi

# 复用标记最后在同目录写好再原子发布：中断留下的半成品 payload 没有匹配标记，下次仍会重建。
web_build_info_next=$(mktemp "$stable_root/WEB-BUILD-INFO.next.XXXXXX")
next_files+=("$web_build_info_next")
web_build_fields >"$web_build_info_next"
chmod 0644 "$web_build_info_next"
touch -d "@$source_epoch" "$web_build_info_next"
mv -f -- "$web_build_info_next" "$web_build_info"
touch -d "@$source_epoch" "$stable_root"

printf '%s Web 发布 payload 已生成：sha256=%s files=%s\n' \
	"$surface" "$payload_sha256" "$(wc -l <"$payload_manifest")"
