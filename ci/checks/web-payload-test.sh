#!/usr/bin/env bash
# 在临时目录验证发布清单生成前的文件筛查。

set -euo pipefail
cd "$(dirname "$0")/../.."
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

printf '%s\n' 'Web payload 回归检查通过'
