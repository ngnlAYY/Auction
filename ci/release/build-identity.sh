#!/usr/bin/env bash
# 构建身份字段的唯一实现；由 release.sh 与 web-bundle.sh 在仓库根目录 source。
# 调用方负责先 cd 到仓库根，并自行决定何时冻结身份，避免两个脚本对同一份源码给出不同指纹。

# 源码身份指纹覆盖全部已跟踪及未忽略的未跟踪文件，同时记录删除和 mode 变化；target 与敏感忽略文件不进入。
source_fingerprint() {
	{
		printf 'head\0%s\0' "$(git rev-parse --verify 'HEAD^{commit}')"
		git status --porcelain=v1 -z --untracked-files=all --ignore-submodules=none
		while IFS= read -r -d '' path; do
			printf 'path\0%s\0' "$path"
			if [[ -L "$path" ]]; then
				printf 'symlink\0%s\0' "$(readlink "$path")"
			elif [[ -f "$path" ]]; then
				printf 'file\0%s\0' "$(stat -c '%a:%s' "$path")"
				sha256sum "$path"
			elif [[ ! -e "$path" ]]; then
				printf 'missing\0'
			else
				printf 'special\0%s\0' "$(stat -c '%F:%a' "$path")"
			fi
		done < <(git ls-files -z --cached --others --exclude-standard | LC_ALL=C sort -z)
	} | sha256sum | awk '{print $1}'
}

# 读取 `key=value` 构建信息文件的单个字段；字段缺失时输出空串，交由调用方失败关闭。
read_build_info() {
	local file=$1 key=$2
	awk -F= -v wanted="$key" '$1 == wanted { print substr($0, length($1) + 2); exit }' "$file"
}

# 原生制品与 Web payload 共用构建环境指纹；profile、工具包装器及目标参数变化都必须失效缓存。
# 名称和值用 NUL 分隔后只输出摘要，避免分隔符歧义，也不把环境原文写进制品元数据。
build_env_fingerprint() {
	local name
	{
		while IFS= read -r name; do
			case "$name" in
			CARGO_PROFILE_* | CARGO_BUILD_RUSTFLAGS | CARGO_ENCODED_RUSTFLAGS | CARGO_BUILD_TARGET | CARGO_TARGET_*_RUSTFLAGS | CARGO_TARGET_*_LINKER | RUSTDOCFLAGS | RUSTFLAGS | RUSTC | RUSTC_WRAPPER | RUSTC_WORKSPACE_WRAPPER)
				printf '%s\0%s\0' "$name" "${!name-}"
				;;
			esac
		done < <(compgen -e | LC_ALL=C sort)
	} | sha256sum | awk '{print $1}'
}

# 指定路径的输入指纹：只覆盖影响该产物的文件，用于跳过与产物无关的改动。
# 路径为仓库根相对；缺失路径按内容为空记录，仍参与身份。
path_fingerprint() {
	local path
	{
		for path in "$@"; do
			printf 'scope\0%s\0' "$path"
		done
		while IFS= read -r -d '' file; do
			printf 'path\0%s\0' "$file"
			if [[ -L "$file" ]]; then
				printf 'symlink\0%s\0' "$(readlink "$file")"
			elif [[ -f "$file" ]]; then
				printf 'file\0%s\0' "$(stat -c '%a:%s' "$file")"
				sha256sum "$file"
			fi
		done < <(
			for path in "$@"; do
				if [[ -d "$path" ]]; then
					find "$path" -type f -print0
				elif [[ -e "$path" ]]; then
					printf '%s\0' "$path"
				fi
			done | LC_ALL=C sort -z
		)
	} | sha256sum | awk '{print $1}'
}
