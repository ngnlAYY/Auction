#!/usr/bin/env bash
# 用最新 RustSec 公告审计最近一个发布 tag（v<semver>）锁定的依赖。
# 部署只使用发布 tag 产出的 Release 资产；默认分支 Cargo.lock 已修复的公告不代表线上程序
# 安全。检查读取 tag 中的 Cargo.lock 到临时目录，不修改 tag、依赖或工作树，也不自动发布。

set -euo pipefail
cd "$(dirname "$0")/../.."

# 与 auction-release 的 tag 守卫同一口径；其他 tag（如临时标记）不代表已发布版本。
# versionsort.suffix=- 让预发布（v1.2.0-rc1）排在同版本正式发布之前，取最高版本。
release_pattern='^v[0-9]+\.[0-9]+\.[0-9]+([-+][0-9A-Za-z.+-]+)?$'
mapfile -t release_tags < <(
	git -c versionsort.suffix=- tag --list 'v[0-9]*' --sort=-v:refname |
		grep -E "$release_pattern" || true
)
if [[ ${#release_tags[@]} -eq 0 ]]; then
	printf '%s\n' '没有发布 tag（v<版本>）；跳过发布版本依赖审计。'
	exit 0
fi

tag=${release_tags[0]}
if ! commit=$(git rev-parse --verify --quiet "$tag^{commit}"); then
	printf '发布 tag 无法解析为提交：%s\n' "$tag" >&2
	exit 1
fi
if ! git cat-file -e "$commit:Cargo.lock" 2>/dev/null; then
	printf '发布 tag %s 中没有可读取的 Cargo.lock；CI 须抓取该 tag 的提交对象。\n' "$tag" >&2
	exit 1
fi

workdir=$(mktemp -d)
trap 'rm -rf "$workdir"' EXIT
git show "$commit:Cargo.lock" >"$workdir/Cargo.lock"
printf '审计发布 tag %s（%s）的 Cargo.lock\n' "$tag" "${commit:0:12}"
cargo audit --file "$workdir/Cargo.lock" --deny warnings
