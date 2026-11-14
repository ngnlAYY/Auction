#!/usr/bin/env bash
# 用 cargo audit 审计指定的 Cargo.lock：警告即失败，已核实的例外逐条以 --ignore 传入。
# 例外登记在 ci/checks/dependency-audit.ignore（每行一个 RUSTSEC 编号，# 为注释）；格式不符即失败关闭。
# 用法：ci/checks/dependency-audit.sh <Cargo.lock>

set -euo pipefail

lockfile=${1:?用法：ci/checks/dependency-audit.sh <Cargo.lock>}
exceptions="$(dirname "$0")/dependency-audit.ignore"

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
