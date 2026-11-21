#!/usr/bin/env bash
# shellcheck disable=SC2030,SC2031,SC2032
# 在临时目录验证发布清单生成前的文件筛查、Solid 门禁与栈相关身份。

set -euo pipefail
cd "$(dirname "$0")/../.."
repository_root=$PWD
unset AUCTION_WEB_STACK
source ci/release/build-identity.sh

fail() {
	printf 'Web payload 回归失败：%s\n' "$1" >&2
	exit 1
}

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
payload="$fixture/public"
mkdir -p "$payload/assets"
printf '<html></html>\n' >"$payload/index.html"
touch "$payload/assets/app.js" "$payload/assets/app.js.br"
checksums=$(web_payload_checksums "$payload")
[[ "$checksums" == *'  ./index.html'* && "$checksums" == *'  ./assets/app.js.br'* ]] ||
	fail '正常入口和压缩副本应进入清单'

for file in assets/app.js.map .env assets/nested/.hidden assets/.debug/app.js; do
	mkdir -p -- "$(dirname -- "$payload/$file")"
	touch -- "$payload/$file"
	if web_payload_checksums "$payload" >"$fixture/checksums" 2>"$fixture/diagnostic"; then
		fail "应拒绝 $file"
	fi
	[[ ! -s "$fixture/checksums" ]] || fail "检查 $file 前已输出摘要"
	[[ $(<"$fixture/diagnostic") == *"./$file"* ]] || fail "诊断缺少 $file"
	rm -- "$payload/$file"
done

# 生成内嵌标记与固定清单；清单本身不包含标记，哈希写在标记里。
write_payload_marker() {
	local root=$1 stable_root=$2 surface=$3 profile=$4
	local manifest="$stable_root/PAYLOAD-SHA256" sha
	web_payload_checksums "$root" >"$manifest" || return 1
	sha=$(sha256_file "$manifest") || return 1
	printf 'schema=auction-web-assets/1\nsurface=%s\nprofile=%s\npayload_sha256=%s\n' \
		"$surface" "$profile" "$sha" >"$root/auction-web-manifest.v1"
}

make_dioxus_payload() {
	local stable_root=$1
	local root="$stable_root/public"
	mkdir -p "$root/assets"
	printf '<!doctype html><html><head><title>Auction 公开站点</title></head><body></body></html>\n' \
		>"$root/index.html"
	printf 'js\n' >"$root/assets/auction-web-public-entry.js"
	printf 'wasm\n' >"$root/assets/auction-web-public_bg01234567.wasm"
	# 额外的普通文件与压缩副本用于验证体积求和和 .br 排除，不参与 JS/WASM 唯一组判定。
	printf 'extra payload\n' >"$root/assets/extra.txt"
	printf 'compressed payload\n' >"$root/assets/extra.txt.br"
	write_payload_marker "$root" "$stable_root" public wasm-release
}

make_solid_payload() {
	local surface=$1 stable_root=$2 title
	local root="$stable_root/public"
	case "$surface" in
	public) title='Auction 公开站点' ;;
	admin) title='Auction 管理站点' ;;
	*) return 2 ;;
	esac
	mkdir -p "$root/assets"
	printf '%s\n' \
		'<!doctype html>' \
		'<html><head>' \
		'<link rel="icon" href="/favicon.ico">' \
		"<title>$title</title>" \
		'<script data-cfasync="false" type="module" crossorigin src="/assets/index-B2UikXgQ.js"></script>' \
		'<link rel="modulepreload" crossorigin href="/assets/rolldown-runtime-DK3Fl9T5.js">' \
		'<link rel="stylesheet" crossorigin href="/assets/index-ZX7NKpK-.css">' \
		'</head><body></body></html>' >"$root/index.html"
	printf 'entry\n' >"$root/assets/index-B2UikXgQ.js"
	printf 'runtime\n' >"$root/assets/rolldown-runtime-DK3Fl9T5.js"
	printf 'extra css\n' >"$root/assets/extra-ABCDEFGH.css"
	printf 'css\n' >"$root/assets/index-ZX7NKpK-.css"
	printf '<svg></svg>\n' >"$root/assets/favicon-YZabcdef.svg"
	write_payload_marker "$root" "$stable_root" "$surface" solid-release
}

# Solid 用例必须在独立的 source 里切换技术栈；外层仍保持缺省 Dioxus。
solid_verify() {
	local surface=$1 stable_root=$2
	(
		export AUCTION_WEB_STACK=solid
		source "$repository_root/ci/release/build-identity.sh"
		verify_web_payload "$surface" "$stable_root"
	)
}

expect_solid_failure() {
	local stable_root=${1:-} expected=${2:-}
	if diagnostic=$(solid_verify public "$stable_root" 2>&1 >/dev/null); then
		fail "Solid payload 应被拒绝：$expected"
	fi
	[[ "$diagnostic" == *"$expected"* ]] || fail "Solid 诊断缺少「$expected」：$diagnostic"
}

dioxus_root="$fixture/dioxus"
make_dioxus_payload "$dioxus_root"
verify_web_payload public "$dioxus_root" >/dev/null || fail 'Dioxus payload 正例应通过'

expected_payload_bytes=$(find "$dioxus_root/public" -type f ! -name '*.br' -printf '%s\n' |
	awk '{ total += $1 } END { print total + 0 }')
expected_wasm_bytes=$(find "$dioxus_root/public" -type f -name 'auction-web-public_bg*.wasm' -printf '%s\n' |
	awk '{ total += $1 } END { print total + 0 }')
read -r measured_wasm measured_payload <<<"$(web_payload_sizes public "$dioxus_root")"
[[ "$measured_wasm" =~ ^[0-9]+$ ]] || fail "Dioxus WASM 体积应为数字，实际为 $measured_wasm"
[[ "$measured_wasm" == "$expected_wasm_bytes" ]] || fail 'Dioxus WASM 体积应包含唯一组的 WASM 文件'
[[ "$measured_payload" == "$expected_payload_bytes" ]] || fail 'Dioxus payload 体积应包含全部非 .br 普通文件'

fixed_digest=$(printf 'd%.0s' {1..64})
expected_tools_identity=$(
	{
		printf 'auction-build-tools/2\0'
		for name in dx esbuild sccache wasm-bindgen wasm-opt; do
			printf '%s\0%s\0' "$name" "$fixed_digest"
		done
	} | sha256sum | awk '{print $1}'
)
expected_checksums=$(cd "$dioxus_root/public" &&
	find . -type f ! -name auction-web-manifest.v1 -print0 | LC_ALL=C sort -z | xargs -0 -r env sha256sum)
tmp_sizes_default=$(
	TMPDIR=/dev/null bash -c 'source "$1/ci/release/build-identity.sh"; web_payload_sizes public "$2"' \
		_ "$repository_root" "$dioxus_root"
)
tmp_sizes_explicit=$(
	TMPDIR=/dev/null AUCTION_WEB_STACK=dioxus bash -c 'source "$1/ci/release/build-identity.sh"; web_payload_sizes public "$2"' \
		_ "$repository_root" "$dioxus_root"
)
[[ "$tmp_sizes_default" == "$expected_wasm_bytes $expected_payload_bytes" &&
	"$tmp_sizes_explicit" == "$expected_wasm_bytes $expected_payload_bytes" ]] ||
	fail 'TMPDIR=/dev/null 下 Dioxus 体积测量应成功并保持一致'

tmp_tools_default=$(
	TMPDIR=/dev/null bash -c 'source "$1/ci/release/build-identity.sh"; build_tool_path() { printf "/bin/true\\n"; }; sha256_file() { printf "%s\\n" "$DIGEST"; }; DIGEST=$2; build_tools_identity' \
		_ "$repository_root" "$fixed_digest"
)
tmp_tools_explicit=$(
	TMPDIR=/dev/null AUCTION_WEB_STACK=dioxus bash -c 'source "$1/ci/release/build-identity.sh"; build_tool_path() { printf "/bin/true\\n"; }; sha256_file() { printf "%s\\n" "$DIGEST"; }; DIGEST=$2; build_tools_identity' \
		_ "$repository_root" "$fixed_digest"
)
[[ "$tmp_tools_default" == "$expected_tools_identity" &&
	"$tmp_tools_explicit" == "$expected_tools_identity" ]] ||
	fail 'TMPDIR=/dev/null 下 Dioxus 工具身份应成功并保持一致'

tmp_checksums_default=$(
	TMPDIR=/dev/null bash -c 'source "$1/ci/release/build-identity.sh"; web_payload_checksums "$2"' \
		_ "$repository_root" "$dioxus_root/public"
)
tmp_checksums_explicit=$(
	TMPDIR=/dev/null AUCTION_WEB_STACK=dioxus bash -c 'source "$1/ci/release/build-identity.sh"; web_payload_checksums "$2"' \
		_ "$repository_root" "$dioxus_root/public"
)
[[ "$tmp_checksums_default" == "$expected_checksums" &&
	"$tmp_checksums_explicit" == "$expected_checksums" ]] ||
	fail 'TMPDIR=/dev/null 下 Dioxus payload 清单应成功并保持一致'

[[ " ${build_tool_names[*]} " == ' dx esbuild sccache wasm-bindgen wasm-opt ' ]] ||
	fail '全 Dioxus 时工具闭集顺序应保持不变'
[[ -z ${build_tool_providers[bun]-} ]] || fail '全 Dioxus 时不应给 bun 登记提供者'
if build_tool_path bun >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail '全 Dioxus 时 bun 不应属于工具闭集'
fi

dioxus_bad_profile="$fixture/dioxus-bad-profile"
cp -a "$dioxus_root" "$dioxus_bad_profile"
write_payload_marker "$dioxus_bad_profile/public" "$dioxus_bad_profile" public solid-release
if verify_web_payload public "$dioxus_bad_profile" >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'Dioxus payload 的 profile 不对时应失败'
fi
[[ $(<"$fixture/diagnostic") == *'制品标记无效'* ]] || fail 'Dioxus 错误 profile 缺少诊断'

solid_root="$fixture/solid"
make_solid_payload public "$solid_root"
solid_verify public "$solid_root" >/dev/null || fail 'Solid payload 正例应通过'
solid_references=$(
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	web_entry_references "$solid_root/public/index.html"
)
[[ "$solid_references" == *$'modulepreload\t/assets/rolldown-runtime-DK3Fl9T5.js'* ]] ||
	fail 'Solid 解析应产出 modulepreload 引用'

commented_entry="$fixture/solid-commented-entry"
cp -a "$solid_root" "$commented_entry"
sed -i '/data-cfasync="false"/d' "$commented_entry/public/index.html"
sed -i 's#</head>#<!-- ignored > <script data-cfasync="false" type="module" crossorigin src="/assets/index-B2UikXgQ.js"></script> --></head>#' \
	"$commented_entry/public/index.html"
write_payload_marker "$commented_entry/public" "$commented_entry" public solid-release
expect_solid_failure "$commented_entry" '不允许包含注释'

single_quote_entry="$fixture/solid-single-quote-entry"
cp -a "$solid_root" "$single_quote_entry"
sed -i "s#src=\"/assets/index-B2UikXgQ.js\"#src='/assets/index-B2UikXgQ.js'#" \
	"$single_quote_entry/public/index.html"
write_payload_marker "$single_quote_entry/public" "$single_quote_entry" public solid-release
expect_solid_failure "$single_quote_entry" '不允许的资源标签'

single_quote_missing_entry="$fixture/solid-single-quote-missing-entry"
cp -a "$solid_root" "$single_quote_missing_entry"
sed -i "s#src=\"/assets/index-B2UikXgQ.js\"#src='/assets/missing-ABCDEFGH.js'#" \
	"$single_quote_missing_entry/public/index.html"
write_payload_marker "$single_quote_missing_entry/public" "$single_quote_missing_entry" public solid-release
expect_solid_failure "$single_quote_missing_entry" '不允许的资源标签'

unquoted_entry="$fixture/solid-unquoted-entry"
cp -a "$solid_root" "$unquoted_entry"
sed -i 's#src="/assets/index-B2UikXgQ.js"#src=/assets/index-B2UikXgQ.js#' \
	"$unquoted_entry/public/index.html"
write_payload_marker "$unquoted_entry/public" "$unquoted_entry" public solid-release
expect_solid_failure "$unquoted_entry" '不允许的资源标签'

non_js_entry="$fixture/solid-non-js-entry"
cp -a "$solid_root" "$non_js_entry"
sed -i 's#src="/assets/index-B2UikXgQ.js"#src="/assets/index-ZX7NKpK-.css"#' \
	"$non_js_entry/public/index.html"
write_payload_marker "$non_js_entry/public" "$non_js_entry" public solid-release
expect_solid_failure "$non_js_entry" '不允许的资源标签'

data_src_entry="$fixture/solid-data-src-entry"
cp -a "$solid_root" "$data_src_entry"
sed -i 's# src="/assets/index-B2UikXgQ.js"# data-src="/assets/index-B2UikXgQ.js"#' \
	"$data_src_entry/public/index.html"
write_payload_marker "$data_src_entry/public" "$data_src_entry" public solid-release
expect_solid_failure "$data_src_entry" '不允许的资源标签'

multiline_preload="$fixture/solid-multiline-preload"
cp -a "$solid_root" "$multiline_preload"
sed -i 's#<link rel="modulepreload" crossorigin href="/assets/rolldown-runtime-DK3Fl9T5.js">#<link rel="modulepreload" crossorigin\n href="/assets/rolldown-runtime-DK3Fl9T5.js">#' \
	"$multiline_preload/public/index.html"
write_payload_marker "$multiline_preload/public" "$multiline_preload" public solid-release
expect_solid_failure "$multiline_preload" '未在同一行闭合'

unquoted_preload="$fixture/solid-unquoted-preload"
cp -a "$solid_root" "$unquoted_preload"
sed -i 's#href="/assets/rolldown-runtime-DK3Fl9T5.js"#href=/assets/rolldown-runtime-DK3Fl9T5.js#' \
	"$unquoted_preload/public/index.html"
write_payload_marker "$unquoted_preload/public" "$unquoted_preload" public solid-release
expect_solid_failure "$unquoted_preload" '不允许的资源标签'

missing_entry="$fixture/solid-missing-entry"
cp -a "$solid_root" "$missing_entry"
sed -i '/data-cfasync="false"/d' "$missing_entry/public/index.html"
write_payload_marker "$missing_entry/public" "$missing_entry" public solid-release
expect_solid_failure "$missing_entry" '恰好包含一个模块入口脚本'

two_entries="$fixture/solid-two-entries"
cp -a "$solid_root" "$two_entries"
sed -i 's#</head>#<script data-cfasync="false" type="module" crossorigin src="/assets/index-B2UikXgQ.js"></script></head>#' \
	"$two_entries/public/index.html"
write_payload_marker "$two_entries/public" "$two_entries" public solid-release
expect_solid_failure "$two_entries" '恰好包含一个模块入口脚本'

two_icons="$fixture/solid-two-icons"
cp -a "$solid_root" "$two_icons"
sed -i 's#<link rel="icon" href="/favicon.ico">#<link rel="icon" href="/favicon.ico">\n<link rel="icon" href="/favicon.ico">#' \
	"$two_icons/public/index.html"
write_payload_marker "$two_icons/public" "$two_icons" public solid-release
expect_solid_failure "$two_icons" '最多只能包含一个站点图标'

missing_favicon="$fixture/solid-missing-favicon"
cp -a "$solid_root" "$missing_favicon"
sed -i '/rel="icon"/d' "$missing_favicon/public/index.html"
rm -- "$missing_favicon/public/assets/favicon-YZabcdef.svg"
write_payload_marker "$missing_favicon/public" "$missing_favicon" public solid-release
expect_solid_failure "$missing_favicon" '入口必须恰好包含一个站点图标'

missing_favicon_file="$fixture/solid-missing-favicon-file"
cp -a "$solid_root" "$missing_favicon_file"
rm -- "$missing_favicon_file/public/assets/favicon-YZabcdef.svg"
write_payload_marker "$missing_favicon_file/public" "$missing_favicon_file" public solid-release
expect_solid_failure "$missing_favicon_file" '必须包含唯一的带指纹站点图标'

missing_resource="$fixture/solid-missing-resource"
cp -a "$solid_root" "$missing_resource"
rm -- "$missing_resource/public/assets/rolldown-runtime-DK3Fl9T5.js"
write_payload_marker "$missing_resource/public" "$missing_resource" public solid-release
expect_solid_failure "$missing_resource" '入口引用了不存在的资源'

missing_entry_file="$fixture/solid-missing-entry-file"
cp -a "$solid_root" "$missing_entry_file"
rm -- "$missing_entry_file/public/assets/index-B2UikXgQ.js"
write_payload_marker "$missing_entry_file/public" "$missing_entry_file" public solid-release
expect_solid_failure "$missing_entry_file" '入口引用了不存在的资源'

no_preload="$fixture/solid-no-preload"
cp -a "$solid_root" "$no_preload"
sed -i '/rel="modulepreload"/d' "$no_preload/public/index.html"
write_payload_marker "$no_preload/public" "$no_preload" public solid-release
solid_verify public "$no_preload" >/dev/null || fail '合法的零条 modulepreload 应通过'

unfingerprinted_asset="$fixture/solid-unfingerprinted"
cp -a "$solid_root" "$unfingerprinted_asset"
printf 'plain\n' >"$unfingerprinted_asset/public/assets/plain.js"
write_payload_marker "$unfingerprinted_asset/public" "$unfingerprinted_asset" public solid-release
expect_solid_failure "$unfingerprinted_asset" 'assets/ 文件未带内容指纹'

bad_profile="$fixture/solid-bad-profile"
cp -a "$solid_root" "$bad_profile"
write_payload_marker "$bad_profile/public" "$bad_profile" public wasm-release
expect_solid_failure "$bad_profile" '制品标记无效'

uppercase_script="$fixture/solid-uppercase-script"
cp -a "$solid_root" "$uppercase_script"
sed -i 's#<script data-cfasync="false" type="module" crossorigin src="/assets/index-B2UikXgQ.js"></script>#<SCRIPT TYPE="module" DATA-CFASYNC="false" CROSSORIGIN SRC="/assets/index-B2UikXgQ.js"></SCRIPT>#' \
	"$uppercase_script/public/index.html"
write_payload_marker "$uppercase_script/public" "$uppercase_script" public solid-release
expect_solid_failure "$uppercase_script" '不允许的资源标签'

template_entry="$fixture/solid-template-entry"
cp -a "$solid_root" "$template_entry"
sed -i 's#<script data-cfasync="false" type="module" crossorigin src="/assets/index-B2UikXgQ.js"></script>#<template><script data-cfasync="false" type="module" crossorigin src="/assets/index-B2UikXgQ.js"></script></template>#' \
	"$template_entry/public/index.html"
write_payload_marker "$template_entry/public" "$template_entry" public solid-release
expect_solid_failure "$template_entry" '不允许的标签'

duplicate_type="$fixture/solid-duplicate-type"
cp -a "$solid_root" "$duplicate_type"
sed -i 's#type="module"#type="module" type="module"#' "$duplicate_type/public/index.html"
write_payload_marker "$duplicate_type/public" "$duplicate_type" public solid-release
expect_solid_failure "$duplicate_type" '不允许的资源标签'

relative_entry="$fixture/solid-relative-entry"
cp -a "$solid_root" "$relative_entry"
sed -i 's#src="/assets/index-B2UikXgQ.js"#src="assets/index-B2UikXgQ.js"#' \
	"$relative_entry/public/index.html"
write_payload_marker "$relative_entry/public" "$relative_entry" public solid-release
expect_solid_failure "$relative_entry" '不允许的资源标签'

compressed_reference="$fixture/solid-compressed-reference"
cp -a "$solid_root" "$compressed_reference"
sed -i 's#href="/assets/rolldown-runtime-DK3Fl9T5.js"#href="/assets/rolldown-runtime-DK3Fl9T5.js.br"#' \
	"$compressed_reference/public/index.html"
write_payload_marker "$compressed_reference/public" "$compressed_reference" public solid-release
expect_solid_failure "$compressed_reference" '不允许的资源标签'

inline_script="$fixture/solid-inline-script"
cp -a "$solid_root" "$inline_script"
sed -i 's#</head>#<script type="module"></script></head>#' "$inline_script/public/index.html"
write_payload_marker "$inline_script/public" "$inline_script" public solid-release
expect_solid_failure "$inline_script" '不允许的资源标签'

attribute_order="$fixture/solid-attribute-order"
cp -a "$solid_root" "$attribute_order"
sed -i 's#crossorigin src="/assets/index-B2UikXgQ.js"#src="/assets/index-B2UikXgQ.js" crossorigin#' \
	"$attribute_order/public/index.html"
write_payload_marker "$attribute_order/public" "$attribute_order" public solid-release
expect_solid_failure "$attribute_order" '不允许的资源标签'

for invalid_stack in x ''; do
	if (export AUCTION_WEB_STACK="$invalid_stack"; source "$repository_root/ci/release/build-identity.sh") \
		>"$fixture/out" 2>"$fixture/diagnostic"; then
		fail "非法 AUCTION_WEB_STACK 应在 source 时失败：$invalid_stack"
	fi
	[[ $(<"$fixture/diagnostic") == *'AUCTION_WEB_STACK'* &&
		$(<"$fixture/diagnostic") == *'dioxus'* &&
		$(<"$fixture/diagnostic") == *'solid'* ]] || fail '非法技术栈的值缺少可行动诊断'
done

# 生成器失败必须显式返回失败，不能把空清单当成合法结果。
if (find() { return 1; }; web_write_find "$fixture/files" "$solid_root/public" -type f -print0) \
	>"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'find 失败时 web_write_find 不应成功'
fi
if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	grep() { return 2; }
	web_payload_initial_js_bytes "$solid_root"
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail '入口解析失败时首屏 JS 不应输出空结果'
fi
if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	allocation_marker="$fixture/mktemp-failed"
	rm -f -- "$allocation_marker"
	mktemp() {
		if [[ ! -e "$allocation_marker" ]]; then
			: >"$allocation_marker"
			printf '注入的临时清单分配失败\n' >&2
			return 1
		fi
		command mktemp "$@"
	}
	verify_web_payload public "$solid_root"
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail '清单分配失败时 verify_web_payload 不应成功'
fi
[[ $(<"$fixture/diagnostic") == *'无法创建 Web payload 搜索清单'* ]] ||
	fail '清单分配失败时 verify_web_payload 缺少诊断'
if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	find() { return 1; }
	verify_solid_web_payload public "$solid_root/public"
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'assets 枚举失败时 Solid payload 校验不应成功'
fi
[[ $(<"$fixture/diagnostic") == *'无法枚举站点图标'* ]] ||
	fail 'assets 枚举失败时 Solid payload 校验缺少诊断'
if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	find() {
		local arg
		for arg in "$@"; do
			[[ "$arg" == -printf ]] && return 1
		done
		command find "$@"
	}
	web_payload_sizes public "$solid_root"
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail '体积枚举失败时 web_payload_sizes 不应成功'
fi
[[ $(<"$fixture/diagnostic") == *'无法测量 Web payload 文件大小'* ]] ||
	fail '体积枚举失败时缺少定点诊断'
if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	find() {
		local arg
		for arg in "$@"; do
			[[ "$arg" == auction-web-manifest.v1 ]] && return 1
		done
		command find "$@"
	}
	web_payload_checksums "$solid_root/public"
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail '清单枚举失败时 web_payload_checksums 不应成功'
fi
[[ $(<"$fixture/diagnostic") == *'无法枚举 Web payload 文件'* ]] ||
	fail '清单枚举失败时缺少定点诊断'
if (find() { return 1; }; web_find_contains "$solid_root/public" needle -type f) \
	>"$fixture/out" 2>"$fixture/diagnostic"; then
	fail '路径搜索枚举失败时 web_find_contains 不应返回成功'
else
	[[ $? == 2 ]] || fail '路径搜索枚举失败应返回 2'
fi
if (find() { return 1; }; web_payload_tree_is_plain "$solid_root/public") >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail '文件类型枚举失败时 web_payload_tree_is_plain 不应成功'
fi
if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	stat() { return 1; }
	web_payload_initial_js_bytes "$solid_root"
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail '文件大小读取失败时首屏 JS 不应成功'
fi

fake_bun="$fixture/fake-bun"
printf '#!/bin/sh\nexit 7\n' >"$fake_bun"
chmod +x "$fake_bun"
if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	build_tool_path() { printf '%s\n' "$fake_bun"; }
	sha256_file() { printf '%s\n' deadbeef; }
	build_tools_identity
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'bun --version 失败时工具身份不应成功'
fi
[[ $(<"$fixture/diagnostic") == *'bun 无法读取版本'* ]] || fail 'bun 版本失败缺少诊断'

if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	web_inputs_fingerprint() { printf 'a%.0s' {1..64}; }
	web_stack_marker() { printf 'public=solid\nadmin=solid\n'; }
	sha256sum() { return 1; }
	web_build_identity
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail '最终构建号摘要失败时不应输出空构建号'
fi
[[ ! -s "$fixture/out" ]] || fail '最终构建号摘要失败时不应输出内容'

if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	web_inputs_fingerprint() { printf 'a%.0s' {1..64}; }
	web_stack_marker() { printf 'public=solid\nadmin=solid\n'; }
	sha256sum() { printf '%s\n' not-a-sha256; }
	web_build_identity
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail '最终构建号不是 64 位摘要时不应成功'
fi
[[ $(<"$fixture/diagnostic") == *'构建号摘要无效'* ]] || fail '最终构建号格式失败缺少诊断'

(
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	[[ $(web_surface_stack public) == solid && $(web_surface_stack admin) == solid ]] ||
		fail 'solid 应同时切换 Public 与 Admin'
	[[ $(web_surface_root public "$repository_root") == "$repository_root/target/web/public" ]] ||
		fail 'Solid public 目录应为 target/web/public'
	[[ $(web_surface_root admin "$repository_root") == "$repository_root/target/web/admin" ]] ||
		fail 'Solid admin 目录应为 target/web/admin'
	[[ " ${build_tool_names[*]} " == *' bun '* ]] || fail 'Solid 工具闭集应包含 bun'
	[[ ${build_tool_providers[bun]-} == bun ]] || fail 'Solid 工具提供者应登记 bun'
	selection=$(print_web_stack_selection)
	[[ "$selection" == *'public=solid'* && "$selection" == *'admin=solid'* ]] ||
		fail '构建入口应打印两端实际技术栈'
)

admin_solid_root="$fixture/solid-admin"
make_solid_payload admin "$admin_solid_root"
solid_verify admin "$admin_solid_root" >/dev/null || fail 'Admin Solid payload 正例应通过'

solid_sizes=$(
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	web_payload_sizes public "$solid_root"
)
read -r initial_js_bytes payload_bytes <<<"$solid_sizes"
expected_initial_js=$((
	$(stat -c %s "$solid_root/public/assets/index-B2UikXgQ.js") +
		$(stat -c %s "$solid_root/public/assets/rolldown-runtime-DK3Fl9T5.js")
))
[[ "$initial_js_bytes" == "$expected_initial_js" ]] || fail 'Solid 首屏 JS 应包含入口与 modulepreload'

admin_solid_sizes=$(
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	web_payload_sizes admin "$admin_solid_root"
)
read -r admin_initial_js_bytes admin_payload_bytes <<<"$admin_solid_sizes"
[[ "$admin_initial_js_bytes" =~ ^[0-9]+$ && "$admin_payload_bytes" =~ ^[0-9]+$ ]] ||
	fail 'Admin Solid 体积测量应返回两个数字'

printf '%s\n' \
	'schema=auction-web-size-budget/1' \
	'warn_growth_basis_points=500' \
	"solid_public_payload_baseline_bytes=$payload_bytes" \
	"solid_public_payload_max_bytes=$payload_bytes" \
	"solid_public_initial_js_baseline_bytes=$initial_js_bytes" \
	"solid_public_initial_js_max_bytes=$initial_js_bytes" \
	"solid_admin_payload_baseline_bytes=$admin_payload_bytes" \
	"solid_admin_payload_max_bytes=$admin_payload_bytes" \
	"solid_admin_initial_js_baseline_bytes=$admin_initial_js_bytes" \
	"solid_admin_initial_js_max_bytes=$admin_initial_js_bytes" >"$fixture/solid-budget"
(
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	verify_web_size_budget public "$solid_root" "$fixture/solid-budget"
) || fail 'Solid 体积预算正例应通过'

(
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	verify_web_size_budget admin "$admin_solid_root" "$fixture/solid-budget"
) || fail 'Admin Solid 体积预算正例应通过'

sed '/^solid_public_initial_js_baseline_bytes=/d' "$fixture/solid-budget" >"$fixture/solid-budget-missing"
if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	verify_web_size_budget public "$solid_root" "$fixture/solid-budget-missing"
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'Solid 预算缺少键时应失败'
fi
[[ $(<"$fixture/diagnostic") == *'solid_public_initial_js_baseline_bytes'* &&
	$(<"$fixture/diagnostic") == *'Solid 键'* ]] || fail 'Solid 预算缺键诊断不明确'

sed 's/^warn_growth_basis_points=.*/warn_growth_basis_points=-1/' \
	"$fixture/solid-budget" >"$fixture/solid-budget-invalid"
if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	verify_web_size_budget public "$solid_root" "$fixture/solid-budget-invalid"
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'Solid 预算负告警阈值应失败'
fi
[[ $(<"$fixture/diagnostic") == *'schema 或告警阈值无效'* ]] ||
	fail 'Solid 预算负告警阈值诊断不明确'

over_budget=$((payload_bytes - 1))
sed -e "s/^solid_public_payload_baseline_bytes=.*/solid_public_payload_baseline_bytes=$over_budget/" \
	-e "s/^solid_public_payload_max_bytes=.*/solid_public_payload_max_bytes=$over_budget/" \
	"$fixture/solid-budget" >"$fixture/solid-budget-over"
if (
	export AUCTION_WEB_STACK=solid
	source "$repository_root/ci/release/build-identity.sh"
	verify_web_size_budget public "$solid_root" "$fixture/solid-budget-over"
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'Solid payload 超出硬上限时应失败'
fi
[[ $(<"$fixture/diagnostic") == *'payload 体积超出预算'* ]] ||
	fail 'Solid payload 超预算诊断不明确'

printf '%s\n' 'Web payload 回归检查通过'
