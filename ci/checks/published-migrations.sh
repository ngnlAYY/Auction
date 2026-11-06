#!/usr/bin/env bash
# 已发布迁移不可变：任何发布 tag（v<版本>）中存在的 migrations/postgres/*.up.sql
# 都必须在当前工作树中逐字节保持不变。迁移只允许追加新文件，不允许修改或删除
# 已经随发布 tag 交付的文件。检查不联网、不执行 SQL，只比较 Git 对象与工作树。

set -euo pipefail
cd "$(dirname "$0")/../.."

if [[ $(git rev-parse --is-shallow-repository 2>/dev/null || printf 'unknown') != false ]]; then
	printf '%s\n' '已发布迁移检查需要完整 Git 历史与 tag；请先 git fetch --tags --unshallow。' >&2
	exit 1
fi

# 只把 v<数字> 开头的 tag 视为发布版本；其他 tag（如临时标记）不定义已发布迁移集合。
mapfile -t release_tags < <(
	git for-each-ref --format='%(refname:short)' refs/tags |
		grep -E '^v[0-9]' | LC_ALL=C sort || true
)
if [[ ${#release_tags[@]} -eq 0 ]]; then
	printf '%s\n' '没有发布 tag（v<版本>）；仅有未发布的迁移，检查通过。'
	exit 0
fi

failures=0
checked=0
for tag in "${release_tags[@]}"; do
	if ! git rev-parse --verify --quiet "$tag^{commit}" >/dev/null; then
		printf '发布 tag 无法解析为提交：%s\n' "$tag" >&2
		failures=1
		continue
	fi
	while IFS=$'\t' read -r metadata path; do
		[[ -z ${path-} || $path != *.up.sql ]] && continue
		read -r mode type object <<<"$metadata"
		if [[ $type != blob || ($mode != 100644 && $mode != 100755) ]]; then
			printf '%s: 已发布迁移不是普通文件：%s（%s %s）\n' \
				"$tag" "$path" "$mode" "$type" >&2
			failures=1
			continue
		fi
		if [[ ! -e $path ]]; then
			printf '%s: 已发布迁移被删除：%s\n' "$tag" "$path" >&2
			failures=1
			continue
		fi
		if [[ ! -f $path || -L $path ]]; then
			printf '%s: 已发布迁移不再是普通文件：%s\n' "$tag" "$path" >&2
			failures=1
			continue
		fi
		current=$(git hash-object -- "$path")
		if [[ $current != "$object" ]]; then
			printf '%s: 已发布迁移被修改：%s\n' "$tag" "$path" >&2
			failures=1
			continue
		fi
		checked=$((checked + 1))
	done < <(git ls-tree -r "$tag" -- migrations/postgres)
done

if [[ $failures -ne 0 ]]; then
	printf '%s\n' '已发布迁移文件必须保持不变；新增迁移请追加新的六位编号文件。' >&2
	exit 1
fi

printf '已发布迁移检查通过：%d 个发布 tag，%d 个 tag 内迁移文件比对未变。\n' \
	"${#release_tags[@]}" "$checked"
