#!/usr/bin/env bash
# 构建身份字段的唯一实现；由 release.sh 与 web-bundle.sh 在仓库根目录 source。
# 调用方负责先 cd 到仓库根，并自行决定何时冻结身份，避免两个脚本对同一份源码给出不同指纹。

# 发布与 Web 共用工具选择；按工具字节身份隔离 sccache daemon，避免新客户端连接旧进程。
configure_build_tools() {
	local tools_identity repository_identity name uses_sccache=false
	tools_identity=$(bash ci/tools/bootstrap.sh identity release) || return 1
	export PATH="$PWD/target/build-tools/bin:$PATH" NO_DOWNLOADS=1
	for name in RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER; do
		if [[ ${!name-} == sccache || ${!name-} == */sccache ]]; then
			printf -v "$name" '%s' "$PWD/target/build-tools/bin/sccache"
			export "${name?}"
			uses_sccache=true
		fi
	done
	if [[ "$uses_sccache" == true ]]; then
		repository_identity=$(printf '%s' "$PWD" | sha256sum | awk '{print $1}')
		# Linux 抽象 UDS 不受工作区绝对路径长度影响；同用户/仓库/工具身份才共享 daemon。
		export SCCACHE_SERVER_UDS="\\x00auction-$UID-${repository_identity:0:16}-${tools_identity:0:24}"
	fi
}

# 批量读取普通文件元数据与摘要，避免每个文件各启动 stat/sha256sum；路径始终用 NUL 分隔。
# 临时清单只保存路径，异常也回收；缺失、链接和特殊文件仍各自参与身份。
fingerprint_file_list() (
	local path regular_files
	regular_files=$(mktemp) || return 1
	trap 'rm -f -- "$regular_files"' EXIT
	printf 'auction-input-files/2\0'
	while IFS= read -r -d '' path; do
		if [[ -L "$path" ]]; then
			printf 'symlink\0%s\0' "$path"
			readlink -z -- "$path" || return 1
			# Cargo 配置等普通文件链接还必须绑定目标内容，不能只比较链接文本。
			if [[ -f "$path" ]]; then printf '%s\0' "$path" >&3; fi
		elif [[ -f "$path" ]]; then
			printf '%s\0' "$path" >&3
		elif [[ ! -e "$path" ]]; then
			printf 'missing\0%s\0' "$path"
		else
			stat --printf 'special\0%n\0%F:%a\0' -- "$path" || return 1
		fi
	done 3>"$regular_files"
	printf 'metadata\0'
	xargs -0 -r stat -L --printf 'file\0%n\0%a:%s\0' -- <"$regular_files" || return 1
	printf 'contents\0'
	xargs -0 -r sha256sum --zero -- <"$regular_files"
)

# 源码身份覆盖已跟踪与未忽略的新文件，以及删除和 mode 变化；敏感忽略文件不进入。
source_fingerprint() {
	{
		printf 'head\0%s\0' "$(git rev-parse --verify 'HEAD^{commit}')"
		git status --porcelain=v1 -z --untracked-files=all --ignore-submodules=none || return 1
		git ls-files -z --cached --others --exclude-standard |
			LC_ALL=C sort -zu | fingerprint_file_list || return 1
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
	local name tools_identity directory
	# 同版本工具修复后字节也会改变；工具回执参与输入身份，避免复用旧工具生成的制品。
	tools_identity=$(bash ci/tools/bootstrap.sh identity release) || return 1
	{
		printf 'auction-build-tools\0%s\0' "$tools_identity"
		# Cargo 合并当前目录、父目录和 CARGO_HOME 配置；只记录摘要，不泄露文件内容。
		directory=$PWD
		while :; do
			path_fingerprint "$directory/.cargo/config" "$directory/.cargo/config.toml" || return 1
			[[ "$directory" == / ]] && break
			directory=$(dirname "$directory")
		done
		path_fingerprint "${CARGO_HOME:-$HOME/.cargo}/config" "${CARGO_HOME:-$HOME/.cargo}/config.toml" || return 1
		while IFS= read -r name; do
			case "$name" in
			CARGO_PROFILE_* | CARGO_BUILD_RUSTFLAGS | CARGO_ENCODED_RUSTFLAGS | CARGO_BUILD_TARGET | CARGO_TARGET_*_RUSTFLAGS | CARGO_TARGET_*_LINKER | RUSTDOCFLAGS | RUSTFLAGS | RUSTC | RUSTC_WRAPPER | RUSTC_WORKSPACE_WRAPPER | SCCACHE_SERVER_UDS | CC | CC_* | CXX | CXX_* | AR | AR_* | CFLAGS | CFLAGS_* | CXXFLAGS | CXXFLAGS_* | HOST_CC | HOST_CXX | HOST_AR | HOST_CFLAGS | HOST_CXXFLAGS | TARGET_CC | TARGET_CXX | TARGET_AR | TARGET_CFLAGS | TARGET_CXXFLAGS)
				printf '%s\0%s\0' "$name" "${!name-}"
				;;
			esac
		done < <(compgen -e | LC_ALL=C sort)
	} | sha256sum | awk '{print $1}'
}

# 指定输入范围按路径排序并去重；目录内的符号链接也参与身份，不依赖 mtime。
path_fingerprint() {
	local path
	{
		for path in "$@"; do
			printf 'scope\0%s\0' "$path"
		done
		{
			for path in "$@"; do
				if [[ -L "$path" ]]; then
					printf '%s\0' "$path"
				elif [[ -d "$path" ]]; then
					find "$path" \( -type f -o -type l \) -print0 || return 1
				elif [[ -e "$path" ]]; then
					printf '%s\0' "$path"
				fi
			done
		} | LC_ALL=C sort -zu | fingerprint_file_list || return 1
	} | sha256sum | awk '{print $1}'
}

# 应用发布与封存复用同一份输入范围；工具配方也属于产物身份。
build_inputs_fingerprint() {
	path_fingerprint \
		apps crates migrations \
		ops/config/examples \
		ops/deploy/retention-policy.v1.json \
		Cargo.toml Cargo.lock rust-toolchain.toml .cargo ci/release ci/tools
}

# Web payload 与封存复用同一份输入范围，surface 严格闭集。
web_inputs_fingerprint() {
	local surface=$1
	case "$surface" in public | admin) ;; *) return 2 ;; esac
	path_fingerprint \
		Cargo.toml Cargo.lock rust-toolchain.toml .cargo \
		"crates/auction-web-$surface" crates/auction-web-shared crates/auction-contracts \
		ci/release/web-bundle.sh ci/release/build-identity.sh ci/tools
}
