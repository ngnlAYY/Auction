#!/usr/bin/env bash
# 为已安装 Web 依赖生成确定性内容与符号链接摘要。
set -euo pipefail

node_modules=${1:?用法：web-dependency-tree.sh <node_modules>}
[[ -d "$node_modules" && ! -L "$node_modules" ]] || {
	printf 'node_modules 缺失或不是普通目录：%s\n' "$node_modules" >&2
	exit 1
}
node_modules=$(cd -- "$node_modules" && pwd -P)

link_sha256=$(
	cd -- "$node_modules"
	find . -type l ! -path './.auction-prepared*' -print0 |
		LC_ALL=C sort -z |
		while IFS= read -r -d '' link; do
			target=$(readlink -- "$link") || exit 1
			resolved=$(realpath -e -- "$link") || exit 1
			case "$resolved" in
				"$node_modules"/*) ;;
				*) printf '依赖符号链接指向 node_modules 外部：%s\n' "$link" >&2; exit 1 ;;
			esac
			printf 'symlink\0%s\0%s\0' "$link" "$target"
		done | sha256sum | awk '{print $1}'
)
file_sha256=$(
	cd -- "$node_modules"
	find -L . -type f ! -path './.auction-prepared*' -print0 |
		LC_ALL=C sort -z | xargs -0 -r sha256sum | sha256sum | awk '{print $1}'
)
[[ "$link_sha256" =~ ^[0-9a-f]{64}$ && "$file_sha256" =~ ^[0-9a-f]{64}$ ]] || {
	printf '无法计算 web/ 依赖树摘要\n' >&2
	exit 1
}
printf 'web-dependency-tree/2\0%s\0%s\0' "$file_sha256" "$link_sha256" | sha256sum | awk '{print $1}'
