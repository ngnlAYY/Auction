#!/usr/bin/env bash
# 用 cargo audit 审计指定的 Cargo.lock：警告即失败，已核实的例外逐条以 --ignore 传入。
# 例外登记在 ci/checks/dependency-audit.ignore（每行一个 RUSTSEC 编号，# 为注释）；格式不符即失败关闭。
# 用法：ci/checks/dependency-audit.sh <Cargo.lock>

set -euo pipefail

if (($# != 1)); then
	printf '用法：%s <Cargo.lock>\n' "$0" >&2
	exit 2
fi

# 调用方可能从任意目录传入相对路径；先固定为绝对路径，再切到仓库根，锁文件和例外清单
# 都不再依赖调用方的 cwd。
lockfile=$(realpath -- "$1") || {
	printf '无法解析 Cargo.lock 路径：%s\n' "$1" >&2
	exit 2
}
script_directory=$(dirname -- "$(realpath -- "${BASH_SOURCE[0]}")")
cd -- "$script_directory/../.." || exit 2
exceptions="$script_directory/dependency-audit.ignore"

arguments=(--file "$lockfile" --deny warnings)
while IFS= read -r line || [[ -n "$line" ]]; do
	[[ -z "$line" || "$line" == \#* ]] && continue
	if [[ ! "$line" =~ ^RUSTSEC-[0-9]{4}-[0-9]{4}$ ]]; then
		printf '依赖审计例外格式无效：%s\n' "$line" >&2
		exit 1
	fi
	arguments+=(--ignore "$line")
done <"$exceptions"

cargo audit "${arguments[@]}"
