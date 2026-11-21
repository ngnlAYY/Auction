#!/usr/bin/env bash
# release.sh 与 web-bundle.sh 共用的函数库，在仓库根目录 source：工具选择、构建身份指纹、
# BUILD-INFO 读取及 Web payload 门禁。调用方自行决定何时冻结身份，两个入口对同一源码给出同一指纹。

# 已支持 Solid 的 surface：选择 solid 时这些 surface 换用 TypeScript 产物，其余仍是 Dioxus。
web_solid_surfaces=(public admin)

# 技术栈开关只在这里解析一次；其余函数与后续消费点一律经 web_surface_stack 取逐 surface 的实际组合。
web_stack_selection=${AUCTION_WEB_STACK-dioxus}
case "$web_stack_selection" in
dioxus | solid) ;;
*)
	printf 'AUCTION_WEB_STACK 的值无效：%s；可选值为 dioxus（缺省）或 solid。\n' "$web_stack_selection" >&2
	if [[ ${BASH_SOURCE[0]} == "$0" ]]; then exit 1; else return 1; fi
	;;
esac

# 只有 release 工作流的 tag push 属于正式发行；手动触发即使选 tag 仍是候选。
is_formal_release() {
	[[ ${GITHUB_ACTIONS-} == true && ${GITHUB_EVENT_NAME-} == push &&
		${GITHUB_REF_TYPE-} == tag ]]
}

release_candidate_suffix() {
	if is_formal_release; then
		return 0
	fi
	printf '%s' '-candidate'
}

web_surface_supports_solid() {
	local surface=$1 candidate
	for candidate in "${web_solid_surfaces[@]}"; do
		[[ "$candidate" == "$surface" ]] && return 0
	done
	return 1
}

# 输出 surface 实际使用的技术栈；surface 闭集之外失败。
web_surface_stack() {
	local surface=$1
	case "$surface" in
	public | admin) ;;
	*) return 2 ;;
	esac
	if [[ "$web_stack_selection" == solid ]] && web_surface_supports_solid "$surface"; then
		printf 'solid\n'
	else
		printf 'dioxus\n'
	fi
}

# 构建入口在开始构建时记录每个 surface 的实际技术栈。
print_web_stack_selection() {
	local surface stack
	for surface in public admin; do
		stack=$(web_surface_stack "$surface") || return 1
		printf 'Web 技术栈：%s=%s\n' "$surface" "$stack"
	done
}

web_stack_uses_solid() {
	local surface
	for surface in public admin; do
		[[ $(web_surface_stack "$surface") == solid ]] && return 0
	done
	return 1
}

web_stack_uses_dioxus() {
	local surface
	for surface in public admin; do
		[[ $(web_surface_stack "$surface") == dioxus ]] && return 0
	done
	return 1
}

# 两端的实际组合参与原生制品身份；默认全 Dioxus 时不加入，保持既有指纹逐字节不变。
web_stack_marker() {
	local surface stack
	for surface in public admin; do
		stack=$(web_surface_stack "$surface") || return 1
		printf '%s=%s\n' "$surface" "$stack"
	done
}

# 发布工具的闭集：二进制名与提供它的 mise 工具。全部由 mise 按 mise.toml 的固定版本安装，
# 下载校验和记录在 mise.lock。原生 Rust 编译始终登记 sccache；页面工具按两端实际技术栈取并集。
# 有 Dioxus 时沿用原有工具顺序，保持工具身份的输入顺序。
build_dioxus_tool_names=(dx esbuild sccache wasm-bindgen wasm-opt)
web_solid_tool_names=(bun)
build_tool_names=()
declare -gA build_dioxus_tool_providers=(
	[dx]='cargo:dioxus-cli'
	[esbuild]='http:esbuild'
	[sccache]='sccache'
	[wasm-bindgen]='github:wasm-bindgen/wasm-bindgen'
	[wasm-opt]='github:WebAssembly/binaryen'
)
declare -gA build_tool_providers=()
if web_stack_uses_dioxus; then
	build_tool_names+=("${build_dioxus_tool_names[@]}")
	for name in "${build_dioxus_tool_names[@]}"; do
		build_tool_providers[$name]=${build_dioxus_tool_providers[$name]}
	done
else
	build_tool_names+=(sccache)
	build_tool_providers[sccache]=${build_dioxus_tool_providers[sccache]}
fi
if web_stack_uses_solid; then
	build_tool_names+=("${web_solid_tool_names[@]}")
	build_tool_providers[bun]='bun'
fi

# 解析单个发布工具的绝对路径。mise 缺失、仓库配置未受信任或工具尚未安装时失败关闭。
# `mise which` 在固定版本未安装时会回退到 Cargo 的 bin 目录等位置的同名程序，所以还要求
# 结果位于该工具自己的安装目录之下，不接受别处的任意版本。
build_tool_path() {
	local name=$1 provider root path
	provider=${build_tool_providers[$name]-}
	if [[ -z "$provider" ]] || ! root=$(mise where "$provider" 2>/dev/null) ||
		! path=$(mise which "$name" 2>/dev/null) ||
		[[ "$path" != "$root"/* || ! -f "$path" || ! -x "$path" ]]; then
		printf '发布工具 %s 不可用：请先在仓库根目录运行 mise install\n' "$name" >&2
		return 1
	fi
	printf '%s\n' "$path"
}

# 发布工具的字节身份：闭集内每个二进制的 SHA-256。同版本工具重装后字节变化也会改变身份。
# 全 Dioxus 沿用原有流式实现；Solid 需要同时读取 bun 版本并以临时清单固定失败边界。
build_tools_identity() (
	set -o pipefail
	local name path digest version file identity
	if web_stack_uses_solid; then
		file=$(mktemp) || {
			printf '无法创建发布工具身份清单\n' >&2
			return 1
		}
		trap 'rm -f -- "$file"' EXIT
		{
			printf 'auction-build-tools/2\0'
			for name in "${build_tool_names[@]}"; do
				path=$(build_tool_path "$name") || return 1
				digest=$(sha256_file "$path") || return 1
				if [[ "$name" == bun ]]; then
					if ! version=$("$path" --version 2>/dev/null); then
						printf '发布工具 bun 无法读取版本：%s\n' "$path" >&2
						return 1
					fi
					version=${version%%$'\n'*}
					if [[ -z "$version" ]]; then
						printf '发布工具 bun 无法读取版本：%s\n' "$path" >&2
						return 1
					fi
					printf '%s\0%s\0%s\0' "$name" "$version" "$digest"
				else
					printf '%s\0%s\0' "$name" "$digest"
				fi
			done
		} >"$file" || return 1
		identity=$(sha256sum "$file") || {
			printf '无法生成发布工具身份摘要\n' >&2
			return 1
		}
		identity=${identity%% *}
		[[ "$identity" =~ ^[0-9a-f]{64}$ ]] || {
			printf '发布工具身份摘要无效：%q\n' "$identity" >&2
			return 1
		}
		printf '%s\n' "$identity"
		return
	fi
	{
		printf 'auction-build-tools/2\0'
		for name in "${build_tool_names[@]}"; do
			path=$(build_tool_path "$name") || return 1
			digest=$(sha256_file "$path") || return 1
			printf '%s\0%s\0' "$name" "$digest"
		done
	} | sha256_stdin '发布工具身份'
)

# 发布与 Web 共用工具选择；按工具字节身份隔离 sccache daemon，避免新客户端连接旧进程。
configure_build_tools() {
	local tools_identity repository_identity name path tools_path='' uses_sccache=false
	tools_identity=$(build_tools_identity) || return 1
	# wasm-bindgen、wasm-opt、esbuild 与 bun 只取这里放到 PATH 前部的固定版本。
	for name in "${build_tool_names[@]}"; do
		path=$(build_tool_path "$name") || return 1
		tools_path+="${path%/*}:"
	done
	export PATH="$tools_path$PATH" NO_DOWNLOADS=1
	# dx 会优先使用自己数据目录里缓存过的 wasm-opt 与 esbuild，也会读取那里的用户级设置；把数据目录
	# 指到 target 下的专用目录，让它只能取上面的固定版本。仍有 Dioxus surface 时都要这样做。
	if web_stack_uses_dioxus; then
		export DX_HOME="$PWD/target/dx-home"
	fi
	# 发布只接受环境变量启用的 sccache 并换成 mise 安装的固定版本；Cargo 文件中的两类包装器
	# 不在发布工具闭集内，各自的环境入口均未指定时用空值显式覆盖。
	if [[ -z ${RUSTC_WRAPPER+set} && -z ${CARGO_BUILD_RUSTC_WRAPPER+set} ]]; then
		export RUSTC_WRAPPER=
	fi
	if [[ -z ${RUSTC_WORKSPACE_WRAPPER+set} && -z ${CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER+set} ]]; then
		export RUSTC_WORKSPACE_WRAPPER=
	fi
	for name in RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_RUSTC_WRAPPER CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER; do
		if [[ ${!name-} == sccache || ${!name-} == */sccache ]]; then
			path=$(build_tool_path sccache) || return 1
			printf -v "$name" '%s' "$path"
			export "${name?}"
			uses_sccache=true
		fi
	done
	if [[ "$uses_sccache" == true ]]; then
		repository_identity=$(printf '%s' "$PWD" | sha256_stdin) || return 1
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
source_fingerprint() (
	set -o pipefail
	local commit
	commit=$(git rev-parse --verify 'HEAD^{commit}') || return 1
	{
		printf 'head\0%s\0' "$commit"
		git status --porcelain=v1 -z --untracked-files=all --ignore-submodules=none || return 1
		git ls-files -z --cached --others --exclude-standard |
			LC_ALL=C sort -zu | fingerprint_file_list || return 1
	} | sha256_stdin
)


# 浏览器工作区路径依赖的闭包由 web-build-inputs 核对。依赖变化时，单端缓存与共享构建号一同失效。
web_workspace_dependency_paths=(
	crates/auction-web-shared
	crates/auction-contracts
	crates/auction-time
)

# 前后端共享的构建号：两个 Web surface 按实际技术栈取输入并集指纹的前 32 位十六进制。
# 它只随浏览器 payload 的输入变化，后端专属改动不改变构建号，因此部署后不会误报；
# 值满足 1–64 位 URL/HTTP 头安全字符闭集。原生程序与浏览器必须用同一份来源计算。
web_build_identity() {
	local identity public_identity admin_identity stack_marker
	if web_stack_uses_solid; then
		public_identity=$(web_inputs_fingerprint public) || return 1
		admin_identity=$(web_inputs_fingerprint admin) || return 1
		stack_marker=$(web_stack_marker) || return 1
		identity=$(printf 'auction-web-build-id/2\0%s\0public\0%s\0admin\0%s\0' \
			"$stack_marker" "$public_identity" "$admin_identity" | sha256_stdin 'Web 构建号') || {
			printf '无法生成 Web 构建号摘要\n' >&2
			return 1
		}
	else
		identity=$(path_fingerprint \
			Cargo.toml Cargo.lock rust-toolchain.toml .cargo \
			crates/auction-web-public crates/auction-web-admin \
			"${web_workspace_dependency_paths[@]}" \
			ci/release/web-bundle.sh ci/release/build-identity.sh "${tool_recipe_inputs[@]}") || return 1
	fi
	if [[ ! "$identity" =~ ^[0-9a-f]{64}$ ]]; then
		printf 'Web 构建号摘要无效：期望 64 位小写十六进制，实际为 %q\n' "$identity" >&2
		return 1
	fi
	printf '%s\n' "${identity:0:32}"
}

# 读取 `key=value` 构建信息文件的单个字段；字段缺失时输出空串，交由调用方失败关闭。
read_build_info() {
	local file=$1 key=$2
	awk -F= -v wanted="$key" '$1 == wanted { print substr($0, length($1) + 2); exit }' "$file"
}

sha256_file() {
	local identity
	identity=$(sha256sum "$1") || {
		printf '无法生成文件 SHA-256 摘要：%s\n' "$1" >&2
		return 1
	}
	identity=${identity%% *}
	[[ "$identity" =~ ^[0-9a-f]{64}$ ]] || {
		printf '文件 SHA-256 摘要无效：%q\n' "$identity" >&2
		return 1
	}
	printf '%s\n' "$identity"
}

# 读取摘要的管道必须同时检查生产者退出码与输出形态；失败时不能留下空摘要供调用方误用。
sha256_stdin() {
	local identity label=${1:-SHA-256}
	identity=$(sha256sum) || {
		printf '无法生成 %s摘要\n' "$label" >&2
		return 1
	}
	identity=${identity%% *}
	[[ "$identity" =~ ^[0-9a-f]{64}$ ]] || {
		printf '%s摘要无效（期望 64 位小写十六进制）：%q\n' "$label" "$identity" >&2
		return 1
	}
	printf '%s\n' "$identity"
}

# 两个 Web surface 的包名、页面标题与对端；闭集之外失败。
web_surface_package() {
	case "$1" in
	public) printf 'auction-web-public\n' ;;
	admin) printf 'auction-web-admin\n' ;;
	*) return 2 ;;
	esac
}

web_surface_title() {
	case "$1" in
	public) printf 'Auction 公开站点\n' ;;
	admin) printf 'Auction 管理站点\n' ;;
	*) return 2 ;;
	esac
}

web_other_surface() {
	case "$1" in
	public) printf 'admin\n' ;;
	admin) printf 'public\n' ;;
	*) return 2 ;;
	esac
}

# Web payload 只允许普通文件与目录；符号链接可能让后续 chmod/touch 越出 payload。
web_payload_tree_is_plain() {
	local root=$1 invalid
	if ! invalid=$(find "$root" ! -type f ! -type d -print -quit); then
		printf '无法检查 Web payload 文件类型：%s\n' "$root" >&2
		return 1
	fi
	[[ -z "$invalid" ]]
}

# 先完整生成清单并检查 find 的退出码，再交给调用方消费；返回 2 表示枚举失败而不是空清单。
web_write_find() {
	local output=$1 root=$2
	shift 2
	find "$root" "$@" >"$output" || {
		printf '无法枚举 Web payload 文件：%s\n' "$root" >&2
		return 2
	}
}

web_count_files() (
	local root=$1 file count=0 item
	shift
	file=$(mktemp) || {
		printf '无法创建 Web payload 计数清单：%s\n' "$root" >&2
		return 1
	}
	trap 'rm -f -- "$file"' EXIT
	web_write_find "$file" "$root" "$@" -print0 || return 1
	while IFS= read -r -d '' item; do count=$((count + 1)); done <"$file"
	printf '%s\n' "$count"
)

# 汇总字节数，只有枚举和求和均成功时才发布结果。
web_find_sizes_sum() (
	set -o pipefail
	local root=$1 total
	shift
	total=$(find "$root" "$@" -printf '%s\n' | awk '{ total += $1 } END { print total + 0 }') || {
		printf '无法测量 Web payload 文件大小：%s\n' "$root" >&2
		return 1
	}
	printf '%s\n' "$total"
)

# 0=找到，1=没找到，2=枚举失败；调用方必须把 2 当作校验失败，不能当成空清单。
web_find_contains() (
	local root=$1 needle=$2 file find_status grep_status
	shift 2
	file=$(mktemp) || {
		printf '无法创建 Web payload 搜索清单：%s\n' "$root" >&2
		return 2
	}
	trap 'rm -f -- "$file"' EXIT
	if find "$root" "$@" -print0 >"$file"; then
		find_status=0
	else
		find_status=$?
	fi
	if grep -zFq -- "$needle" "$file"; then
		return 0
	else
		grep_status=$?
	fi
	if ((grep_status != 1)); then
		printf '无法在 Web payload 中搜索：%s\n' "$root" >&2
		return 2
	fi
	if ((find_status != 0)); then
		printf '无法在 Web payload 中搜索：%s\n' "$root" >&2
		return 2
	fi
	return 1
)

# Web 摘要在枚举、排序和哈希均成功后才输出，保留 NUL 分隔的文件名。
web_payload_checksums() (
	set -o pipefail
	cd "$1" || {
		printf '无法进入 Web payload 目录：%s\n' "$1" >&2
		return 1
	}
	local forbidden_path checksums
	forbidden_path=$(find . -type f \( -name '*.map' -o -path '*/.*' \) -print -quit) || {
		printf '无法枚举 Web payload 文件：%s\n' "$1" >&2
		return 1
	}
	if [[ -n "$forbidden_path" ]]; then
		printf 'Web payload 含禁止发布的文件：%q\n' "$forbidden_path" >&2
		return 1
	fi
	checksums=$(
		find . -type f ! -name auction-web-manifest.v1 -print0 |
			LC_ALL=C sort -z | xargs -0 -r sha256sum
		local -a stages=("${PIPESTATUS[@]}")
		if ((stages[0] != 0)); then
			printf '无法枚举 Web payload 文件：%s\n' "$1" >&2
			return 1
		fi
		if ((stages[1] != 0 || stages[2] != 0)); then
			printf '无法排序或生成 Web payload 文件摘要：%s\n' "$1" >&2
			return 1
		fi
	) || return 1
	if [[ -n "$checksums" ]]; then
		printf '%s\n' "$checksums"
	fi
)

# 入口文档只接受构建实际产出的四种资源标签；任何其他标签或换行写法都失败关闭。
web_solid_asset_filename() {
	local filename=$1 extension=$2
	[[ -n "$filename" && "$filename" != */* && "$filename" == *".$extension" ]] || return 1
	[[ "$filename" =~ $web_asset_fingerprint_pattern ]]
}

web_solid_reference_tag() {
	local tag=$1 filename
	local script_prefix='<script data-cfasync="false" type="module" crossorigin src="/assets/'
	local script_suffix='"></script>'
	local link_prefix link_suffix
	if [[ "$tag" == '<link rel="icon" href="/favicon.ico">' ]]; then
		printf 'icon\t/favicon.ico\n'
		return 0
	fi
	if [[ "$tag" == "$script_prefix"*"$script_suffix" ]]; then
		filename=${tag#"$script_prefix"}
		filename=${filename%"$script_suffix"}
		web_solid_asset_filename "$filename" js || return 1
		printf 'entry\t/assets/%s\n' "$filename"
		return 0
	fi
	link_prefix='<link rel="modulepreload" crossorigin href="/assets/'
	link_suffix='">'
	if [[ "$tag" == "$link_prefix"*"$link_suffix" ]]; then
		filename=${tag#"$link_prefix"}
		filename=${filename%"$link_suffix"}
		web_solid_asset_filename "$filename" js || return 1
		printf 'modulepreload\t/assets/%s\n' "$filename"
		return 0
	fi
	link_prefix='<link rel="stylesheet" crossorigin href="/assets/'
	link_suffix='">'
	if [[ "$tag" == "$link_prefix"*"$link_suffix" ]]; then
		filename=${tag#"$link_prefix"}
		filename=${filename%"$link_suffix"}
		web_solid_asset_filename "$filename" css || return 1
		printf 'stylesheet\t/assets/%s\n' "$filename"
		return 0
	fi
	return 1
}

# 从同一份闭集解析结果取入口、样式表、预加载与图标；校验和预算共用它。
web_entry_references() (
	local document=$1 line line_number=0 remaining rest lower_line lower_rest tag reference kind ref
	local entry_count=0 icon_count=0 forbidden comment_status
	local -a records=()
	[[ -f "$document" && ! -L "$document" ]] || {
		printf 'Web 入口文档缺失或不是普通文件：%s\n' "$document" >&2
		return 1
	}
	if grep -Fq -- '<!--' "$document"; then
		printf 'Web 入口文档不允许包含注释：%s\n' "$document" >&2
		return 1
	else
		comment_status=$?
		if ((comment_status == 2)); then
			printf '无法读取 Web 入口文档：%s\n' "$document" >&2
			return 1
		fi
	fi
	while IFS= read -r line || [[ -n "$line" ]]; do
		line_number=$((line_number + 1))
		lower_line=${line,,}
		for forbidden in template base iframe object embed; do
			if [[ "$lower_line" == *"<$forbidden"* ]]; then
				printf 'Web 入口第 %s 行出现不允许的标签：%s\n' "$line_number" "$line" >&2
				return 1
			fi
		done
		remaining=$line
		while [[ "$remaining" == *"<"* ]]; do
			rest=${remaining#*"<"}
			lower_rest=${rest,,}
			if [[ "$lower_rest" == script* ]]; then
				if [[ "$rest" != *"</script>"* ]]; then
					printf 'Web 入口第 %s 行出现不允许的资源标签：%s\n' "$line_number" "$line" >&2
					return 1
				fi
				tag="<${rest%%"</script>"*}</script>"
			elif [[ "$lower_rest" == link* ]]; then
				if [[ "$rest" != *">"* ]]; then
					printf 'Web 入口第 %s 行资源标签未在同一行闭合：%s\n' "$line_number" "$line" >&2
					return 1
				fi
				tag="<${rest%%>*}>"
			else
				remaining=$rest
				continue
			fi
			if ! reference=$(web_solid_reference_tag "$tag"); then
				printf 'Web 入口第 %s 行出现不允许的资源标签：%s\n' "$line_number" "$line" >&2
				return 1
			fi
			kind=${reference%%$'\t'*}
			ref=${reference#*$'\t'}
			case "$kind" in
			entry) entry_count=$((entry_count + 1)) ;;
			icon) icon_count=$((icon_count + 1)) ;;
			esac
			records+=("$reference")
			if [[ "$lower_rest" == script* ]]; then
				remaining=${rest#*"</script>"}
			else
				remaining=${rest#*>}
			fi
		done
	done <"$document"
	if ((entry_count != 1)); then
		printf 'Web 入口必须恰好包含一个模块入口脚本，实际为 %s 个\n' "$entry_count" >&2
		return 1
	fi
	if ((icon_count > 1)); then
		printf 'Web 入口最多只能包含一个站点图标，实际为 %s 个\n' "$icon_count" >&2
		return 1
	fi
	printf '%s\n' "${records[@]}"
)

# 把入口资源引用映射为 payload 内路径；拒绝目录穿越。查询串与片段由调用方先剥离。
web_payload_reference_path() {
	local root=$1 ref=$2
	[[ -n "$ref" && "$ref" != *$'\n'* ]] || return 2
	[[ "/$ref/" != *"/../"* ]] || return 2
	if [[ "$ref" == /* ]]; then
		printf '%s%s\n' "$root" "$ref"
	else
		printf '%s/%s\n' "$root" "$ref"
	fi
}

# Vite 的默认资源名是 <name>-<8 位 base64url 内容哈希>.<扩展名>；.br 副本沿用原文件名的指纹。
web_asset_fingerprint_pattern='^[A-Za-z0-9][A-Za-z0-9._-]*-[A-Za-z0-9_-]{8}\.[A-Za-z0-9]+$'

# Solid 特有门禁：唯一入口模块脚本、引用资源闭包完整、assets/ 文件名都带内容指纹。
verify_solid_web_payload() (
	local surface=$1 root=$2
	local document="$root/index.html"
	local references kind ref lookup path favicon_count favicon_link_count=0 file base assets_file
	[[ -f "$document" && ! -L "$document" ]] || {
		printf '%s Web payload 缺少普通入口文档：%s\n' "$surface" "$document" >&2
		return 1
	}
	if [[ ! -d "$root/assets" || -L "$root/assets" ]]; then
		printf '%s Web payload 缺少 assets/ 目录\n' "$surface" >&2
		return 1
	fi
	references=$(web_entry_references "$document") || {
		printf '%s Web 入口解析失败\n' "$surface" >&2
		return 1
	}
	while IFS=$'\t' read -r kind ref; do
		[[ -n "$kind" ]] || continue
		if [[ "$kind" == icon ]]; then
			favicon_link_count=$((favicon_link_count + 1))
		fi
		lookup=${ref%%[?#]*}
		if [[ -z "$lookup" || "$ref" == \#* ]]; then continue; fi
		case "$lookup" in
		data:* | http:* | https:* | //* | mailto:* | javascript:*)
			printf '%s Web 入口引用了 payload 之外的资源：%s\n' "$surface" "$ref" >&2
			return 1
			;;
		/favicon.ico)
			# /favicon.ico 由后端映射到 assets/ 里唯一的带指纹 SVG，不是 payload 中的文件。
			continue
			;;
		esac
		path=$(web_payload_reference_path "$root" "$lookup") || {
			printf '%s Web 入口引用了非法资源路径：%s\n' "$surface" "$ref" >&2
			return 1
		}
		if [[ ! -f "$path" || -L "$path" ]]; then
			printf '%s Web 入口引用了不存在的资源：%s\n' "$surface" "$ref" >&2
			return 1
		fi
	done <<<"$references"
	if ((favicon_link_count != 1)); then
		printf '%s Web 入口必须恰好包含一个站点图标\n' "$surface" >&2
		return 1
	fi
	favicon_count=$(web_count_files "$root/assets" -maxdepth 1 -type f -name 'favicon-*.svg') || {
		printf '%s Web payload 无法枚举站点图标\n' "$surface" >&2
		return 1
	}
	if [[ "$favicon_count" -ne 1 ]]; then
		printf '%s Web payload 必须包含唯一的带指纹站点图标\n' "$surface" >&2
		return 1
	fi
	assets_file=$(mktemp) || {
		printf '%s Web payload 无法创建 assets 清单\n' "$surface" >&2
		return 1
	}
	trap 'rm -f -- "$assets_file"' EXIT
	web_write_find "$assets_file" "$root/assets" -type f -print0 || return 1
	while IFS= read -r -d '' file; do
		base=${file##*/}
		base=${base%.br}
		if [[ ! "$base" =~ $web_asset_fingerprint_pattern ]]; then
			printf '%s Web payload 的 assets/ 文件未带内容指纹：%s\n' "$surface" "$file" >&2
			return 1
		fi
	done <"$assets_file"
)

# Web payload 完整门禁：入口与本 surface 标题、跨 surface 隔离、普通文件、内嵌标记、固定清单及按技术栈
# 选择的一组资源。成功时输出 payload 哈希；web-bundle 的新建/复用与 release 的制品核对共用同一判定。
verify_web_payload() (
	local surface=$1 stable_root=$2
	local root="$stable_root/public" other title other_title marker manifest marker_sha actual_sha file
	local stack profile
	stack=$(web_surface_stack "$surface") || return 2
	if [[ "$stack" == solid ]]; then profile=solid-release; else profile=wasm-release; fi
	other=$(web_other_surface "$surface") || return 2
	title=$(web_surface_title "$surface")
	other_title=$(web_surface_title "$other")
	marker="$root/auction-web-manifest.v1"
	manifest="$stable_root/PAYLOAD-SHA256"

	for file in "$root/index.html" "$marker" "$manifest"; do
		if [[ ! -f "$file" || -L "$file" ]]; then
			printf '%s Web payload 缺少普通文件：%s\n' "$surface" "$file" >&2
			return 1
		fi
	done
	if ! web_payload_tree_is_plain "$root"; then
		printf '%s Web payload 含符号链接或特殊文件\n' "$surface" >&2
		return 1
	fi
	local title_status other_title_status
	if grep -Fq "<title>$title</title>" "$root/index.html"; then
		title_status=0
	else
		title_status=$?
	fi
	if ((title_status == 2)); then
		printf '%s Web 入口标题无法读取\n' "$surface" >&2
		return 1
	fi
	if grep -Fq "$other_title" "$root/index.html"; then
		other_title_status=0
	else
		other_title_status=$?
	fi
	if ((other_title_status == 2)); then
		printf '%s Web 入口标题无法读取\n' "$surface" >&2
		return 1
	fi
	if ((title_status != 0 || other_title_status == 0)); then
		printf '%s Web 入口混入错误 surface 标题\n' "$surface" >&2
		return 1
	fi
	if web_find_contains "$root" "auction-web-$other" -type f; then
		printf '%s Web payload 混入 %s 文件\n' "$surface" "$other" >&2
		return 1
	else
		local match_status=$?
		if ((match_status != 1)); then
			printf '%s Web payload 无法枚举 %s 文件\n' "$surface" "$other" >&2
			return 1
		fi
	fi

	marker_sha=$(read_build_info "$marker" payload_sha256)
	if [[ $(read_build_info "$marker" schema) != auction-web-assets/1 ||
	$(read_build_info "$marker" surface) != "$surface" ||
	$(read_build_info "$marker" profile) != "$profile" ||
	! "$marker_sha" =~ ^[0-9a-f]{64}$ ]]; then
		printf '%s Web 制品标记无效\n' "$surface" >&2
		return 1
	fi
	# 显式检查清单生成的退出码；进程替换失败可能留下仍与缓存相等的不完整输出。
	local checksums
	checksums=$(mktemp) || return 1
	trap 'rm -f -- "$checksums"' EXIT
	if ! web_payload_checksums "$root" >"$checksums" || ! cmp -s "$manifest" "$checksums"; then
		printf '%s Web payload 与固定清单不一致\n' "$surface" >&2
		return 1
	fi
	actual_sha=$(sha256_file "$manifest") || return 1
	if [[ "$actual_sha" != "$marker_sha" ]]; then
		printf '%s Web payload 哈希与内嵌标记不一致\n' "$surface" >&2
		return 1
	fi

	if [[ "$stack" == dioxus ]]; then
		local wasm_count js_count
		wasm_count=$(web_count_files "$root" -type f -name "auction-web-${surface}_bg*.wasm") || return 1
		js_count=$(web_count_files "$root" -type f -name "auction-web-${surface}-*.js") || return 1
		if [[ "$wasm_count" -ne 1 || "$js_count" -ne 1 ]]; then
			printf '%s Web payload 必须且只能包含一组优化 JS/WASM：js=%s wasm=%s\n' \
				"$surface" "$js_count" "$wasm_count" >&2
			return 1
		fi
	else
		verify_solid_web_payload "$surface" "$root" || return 1
	fi
	printf '%s\n' "$marker_sha"
)

# 稳定 Web payload 目录；按 surface 的实际技术栈选择，surface 闭集之外失败。
web_surface_root() {
	local surface=$1 repository_root=$2 stack package
	stack=$(web_surface_stack "$surface") || return 2
	if [[ "$stack" == solid ]]; then
		printf '%s/target/web/%s\n' "$repository_root" "$surface"
	else
		package=$(web_surface_package "$surface") || return 2
		printf '%s/target/dx/build/%s/web\n' "$repository_root" "$package"
	fi
}

# 发布构建使用的 cargo feature：分体 API 直接选择自己的内嵌实现；合并服务先内嵌，
# 再按两端各自的实际技术栈追加 selector。缺省组合与改动前的常量逐字符相同。
web_cargo_features() {
	local target=$1 surface stack features
	case "$target" in
	public-api) surface='public' ;;
	admin-api) surface='admin' ;;
	server) ;;
	*) return 2 ;;
	esac
	if [[ "$target" == server ]]; then
		features=auction-server/web-embed
		for surface in public admin; do
			stack=$(web_surface_stack "$surface") || return 2
			if [[ "$stack" == solid ]]; then
				features+=",auction-server/web-solid-$surface"
			fi
		done
		printf '%s\n' "$features"
		return 0
	fi
	stack=$(web_surface_stack "$surface") || return 2
	if [[ "$stack" == solid ]]; then
		printf 'auction-%s-api/web-solid\n' "$surface"
	else
		printf 'auction-%s-api/web-embed\n' "$surface"
	fi
}

# Solid 首屏 JS：唯一入口模块脚本与入口文档声明的 modulepreload JS 原始字节合计，重复路径只算一次。
web_payload_initial_js_bytes() (
	local stable_root=$1
	local root="$stable_root/public"
	local document="$root/index.html"
	local references kind ref lookup path size total=0
	local -a sources=()
	local -A seen=()
	[[ -f "$document" && ! -L "$document" ]] || {
		printf 'Solid Web payload 缺少普通入口文档：%s\n' "$document" >&2
		return 1
	}
	references=$(web_entry_references "$document") || return 1
	while IFS=$'\t' read -r kind ref; do
		[[ -n "$kind" ]] || continue
		case "$kind" in
		entry)
			sources+=("$ref")
			;;
		modulepreload)
			lookup=${ref%%[?#]*}
			case "$lookup" in
			*.js | *.mjs) sources+=("$ref") ;;
			esac
			;;
		esac
	done <<<"$references"
	[[ "${#sources[@]}" -gt 0 ]] || return 1
	for ref in "${sources[@]}"; do
		path=$(web_payload_reference_path "$root" "${ref%%[?#]*}") || {
			printf 'Solid Web payload 入口引用路径非法：%s\n' "$ref" >&2
			return 1
		}
		[[ -f "$path" && ! -L "$path" ]] || {
			printf 'Solid Web payload 缺少首屏 JS：%s\n' "$path" >&2
			return 1
		}
		[[ -n ${seen[$path]+set} ]] && continue
		seen[$path]=1
		size=$(stat -c %s "$path") || {
			printf '无法读取 Web payload 文件大小：%s\n' "$path" >&2
			return 1
		}
		total=$((total + size))
	done
	printf '%s\n' "$total"
)

# Dioxus 仍分别统计 WASM 与未压缩 payload；枚举失败不得用部分结果完成预算核对。
web_payload_sizes_dioxus() (
	local surface=$1
	local stable_root=$2
	local root wasm_bytes payload_bytes
	root="$stable_root/public"
	[[ -d "$root" && ! -L "$root" ]] || return 1
	wasm_bytes=$(web_find_sizes_sum "$root" -type f -name "auction-web-${surface}_bg*.wasm") || return 1
	# 体积预算按原始表示统计；构建期 brotli 副本（*.br）只影响传输量，不计入 payload。
	payload_bytes=$(web_find_sizes_sum "$root" -type f ! -name '*.br') || return 1
	printf '%s %s\n' "$wasm_bytes" "$payload_bytes"
)

# Solid 体积：内嵌全部普通文件，首屏 JS 取入口模块与 modulepreload 的 JS。
web_payload_sizes_solid() (
	local surface=$1
	local stable_root=$2
	local root initial_js_bytes payload_bytes
	root="$stable_root/public"
	[[ -d "$root" && ! -L "$root" ]] || {
		printf 'Web payload 目录缺失或不是普通目录：%s\n' "$root" >&2
		return 1
	}
	payload_bytes=$(web_find_sizes_sum "$root" -type f ! -name '*.br') || return 1
	initial_js_bytes=$(web_payload_initial_js_bytes "$stable_root") || return 1
	printf '%s %s\n' "$initial_js_bytes" "$payload_bytes"
)

# 单端 payload 体积：按实际技术栈测量可比字节数与内嵌的全部普通文件合计。
# Dioxus 输出 `wasm_bytes payload_bytes`；Solid 输出 `initial_js_bytes payload_bytes`。
web_payload_sizes() (
	local stack
	stack=$(web_surface_stack "$1") || return 2
	if [[ "$stack" == dioxus ]]; then
		web_payload_sizes_dioxus "$@"
	else
		web_payload_sizes_solid "$@"
	fi
)

# 发布体积预算：超过 baseline + 万分比告警阈值只打印 WARN，超过硬上限失败关闭。
# baseline 必须来自刻意复核的 payload，不能随构建自动刷新，否则体积回归永远不会被发现。
verify_web_size_budget() {
	local surface=$1 stable_root=$2 budget_file=$3
	local schema warn_basis_points sizes first_bytes payload_bytes measured_bytes stack key_prefix
	local metric baseline_key max_key baseline_bytes max_bytes warn_bytes failed=0
	local -a metrics=()
	local -A measured=()
	if [[ ! -f "$budget_file" || -L "$budget_file" ]]; then
		printf 'Web 体积预算文件缺失或不是普通文件：%s\n' "$budget_file" >&2
		return 1
	fi
	schema=$(read_build_info "$budget_file" schema)
	warn_basis_points=$(read_build_info "$budget_file" warn_growth_basis_points)
	if [[ "$schema" != auction-web-size-budget/1 || ! "$warn_basis_points" =~ ^(0|[1-9][0-9]{0,4})$ ]]; then
		printf 'Web 体积预算文件 schema 或告警阈值无效：%s\n' "$budget_file" >&2
		return 1
	fi
	sizes=$(web_payload_sizes "$surface" "$stable_root") || return 1
	if [[ ! "$sizes" =~ ^([0-9]+)\ ([0-9]+)$ ]]; then
		printf '无法测量 %s Web payload 体积：%s\n' "$surface" "$stable_root" >&2
		return 1
	fi
	first_bytes=${BASH_REMATCH[1]}
	payload_bytes=${BASH_REMATCH[2]}
	stack=$(web_surface_stack "$surface") || return 2
	if [[ "$stack" == dioxus ]]; then
		key_prefix=$surface
		metrics=(wasm payload)
		measured[wasm]=$first_bytes
		measured[payload]=$payload_bytes
	else
		key_prefix="solid_${surface}"
		metrics=(payload initial_js)
		measured[payload]=$payload_bytes
		measured[initial_js]=$first_bytes
	fi
	for metric in "${metrics[@]}"; do
		baseline_key="${key_prefix}_${metric}_baseline_bytes"
		max_key="${key_prefix}_${metric}_max_bytes"
		baseline_bytes=$(read_build_info "$budget_file" "$baseline_key")
		max_bytes=$(read_build_info "$budget_file" "$max_key")
		if [[ ! "$baseline_bytes" =~ ^[1-9][0-9]*$ || ! "$max_bytes" =~ ^[1-9][0-9]*$ ]]; then
			if [[ "$stack" == solid ]]; then
				printf 'Web 体积预算缺少有效的 %s/%s（Solid 键由实测加入）：%s\n' \
					"$baseline_key" "$max_key" "$budget_file" >&2
			else
				printf 'Web 体积预算缺少有效的 %s/%s：%s\n' \
					"$baseline_key" "$max_key" "$budget_file" >&2
			fi
			return 1
		fi
		if ((baseline_bytes > max_bytes)); then
			printf 'Web 体积预算基线不得高于硬上限：%s\n' "$budget_file" >&2
			return 1
		fi
		measured_bytes=${measured[$metric]}
		warn_bytes=$((baseline_bytes + baseline_bytes * warn_basis_points / 10000))
		if ((measured_bytes > max_bytes)); then
			printf '%s %s 体积超出预算：%s > 上限 %s 字节（基线 %s）\n' \
				"$surface" "$metric" "$measured_bytes" "$max_bytes" "$baseline_bytes" >&2
			failed=1
		elif ((measured_bytes > warn_bytes)); then
			printf 'WARN %s %s 体积超过基线告警阈值：%s > %s 字节（基线 %s，容许 %s bp）\n' \
				"$surface" "$metric" "$measured_bytes" "$warn_bytes" "$baseline_bytes" "$warn_basis_points" >&2
		fi
	done
	return "$failed"
}

# Cargo 按调用目录、父目录与 CARGO_HOME 合并配置；应用与工具复用同一文件身份算法。
# 只输出摘要，保留链接目标和缺失路径身份，不把用户配置原文写入回执。
cargo_config_fingerprint() {
	local directory=$PWD
	{
		while :; do
			path_fingerprint "$directory/.cargo/config" "$directory/.cargo/config.toml" || return 1
			[[ "$directory" == / ]] && break
			directory=$(dirname "$directory") || return 1
		done
		path_fingerprint "${CARGO_HOME:-$HOME/.cargo}/config" "${CARGO_HOME:-$HOME/.cargo}/config.toml" || return 1
	} | sha256_stdin
}

# 原生制品与 Web payload 共用构建环境指纹；profile、工具包装器及目标参数变化都必须失效缓存。
# 名称和值用 NUL 分隔后只输出摘要，避免分隔符歧义，也不把环境原文写进制品元数据。
build_env_fingerprint() {
	local name tools_identity stack_marker='' env_names
	# 同版本工具重装后字节也会改变；工具字节身份参与输入身份，避免复用旧工具生成的制品。
	tools_identity=$(build_tools_identity) || return 1
	if web_stack_uses_solid; then
		stack_marker=$(web_stack_marker) || return 1
	fi
	env_names=$(compgen -e | LC_ALL=C sort) || {
		printf '无法枚举构建环境变量\n' >&2
		return 1
	}
	{
		if [[ -n "$stack_marker" ]]; then
			printf 'auction-web-stacks/1\0%s\0' "$stack_marker"
		fi
		printf 'auction-build-tools\0%s\0' "$tools_identity"
		cargo_config_fingerprint || return 1
		# Cargo 的配置环境别名同样能替换编译器/包装器或覆盖 profile 的增量策略。
		while IFS= read -r name; do
			case "$name" in
			CARGO_PROFILE_* | CARGO_BUILD_RUSTC | CARGO_BUILD_RUSTC_WRAPPER | CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER | CARGO_BUILD_INCREMENTAL | CARGO_INCREMENTAL | CARGO_BUILD_RUSTFLAGS | CARGO_ENCODED_RUSTFLAGS | CARGO_BUILD_TARGET | CARGO_TARGET_*_RUSTFLAGS | CARGO_TARGET_*_LINKER | RUSTDOCFLAGS | RUSTFLAGS | RUSTC | RUSTC_WRAPPER | RUSTC_WORKSPACE_WRAPPER | SCCACHE_SERVER_UDS | CC | CC_* | CXX | CXX_* | AR | AR_* | CFLAGS | CFLAGS_* | CXXFLAGS | CXXFLAGS_* | HOST_CC | HOST_CXX | HOST_AR | HOST_CFLAGS | HOST_CXXFLAGS | TARGET_CC | TARGET_CXX | TARGET_AR | TARGET_CFLAGS | TARGET_CXXFLAGS)
				printf '%s\0%s\0' "$name" "${!name-}"
				;;
			esac
		done <<<"$env_names"
	} | sha256_stdin
}

path_is_excluded() {
	local candidate=$1 exclude
	shift
	for exclude in "$@"; do
		[[ "$candidate" == "$exclude" || "$candidate" == "$exclude"/* ]] && return 0
	done
	return 1
}

# 目录名级排除：排除 web/node_modules 与 web/.tmp 在任意深度的同名目录，而不是只排 web/ 根下两个。
path_is_excluded_dir() {
	local candidate=$1 exclude prefix name
	shift
	for exclude in "$@"; do
		prefix=${exclude%/*}
		name=${exclude##*/}
		[[ "$candidate" == "$prefix/$name" || "$candidate" == "$prefix"/*/"$name" ]] && return 0
	done
	return 1
}

# 指定输入范围按路径排序并去重；目录内的符号链接也参与身份，不依赖 mtime。
# `--exclude PATH` 可重复，排除该路径及其子树；`--exclude-dir PATH` 排除该目录在任意深度的同名目录。
# 默认不排除任何东西。排除路径与 scope 一样相对于当前目录书写，目录遍历用 -prune，避免进入大目录。
path_fingerprint() (
	local path exclude dir_exclude dir_prefix dir_name list sorted fingerprint identity
	local -a scopes=() excludes=() dir_excludes=() find_args=()
	while (($#)); do
		case "$1" in
		--exclude)
			if (($# < 2)) || [[ -z ${2%/} ]]; then
				printf 'path_fingerprint 的 --exclude 缺少有效路径\n' >&2
				return 2
			fi
			exclude=${2%/}
			excludes+=("$exclude")
			shift 2
			;;
		--exclude-dir)
			if (($# < 2)) || [[ -z ${2%/} ]]; then
				printf 'path_fingerprint 的 --exclude-dir 缺少有效路径\n' >&2
				return 2
			fi
			dir_exclude=${2%/}
			if [[ "$dir_exclude" != */* || "$dir_exclude" == . || "$dir_exclude" == .. || "$dir_exclude" == *..* ]]; then
				printf 'path_fingerprint 的 --exclude-dir 必须是不含点段的相对路径：%s\n' "$dir_exclude" >&2
				return 2
			fi
			dir_excludes+=("$dir_exclude")
			shift 2
			;;
		--)
			shift
			break
			;;
		-*)
			printf 'path_fingerprint 未知参数：%s\n' "$1" >&2
			return 2
			;;
		*)
			break
			;;
		esac
	done
	scopes=("$@")
	list=$(mktemp) || {
		printf '无法创建路径指纹清单\n' >&2
		return 1
	}
	sorted=$(mktemp) || {
		rm -f -- "$list"
		printf '无法创建路径指纹排序结果\n' >&2
		return 1
	}
	fingerprint=$(mktemp) || {
		rm -f -- "$list" "$sorted"
		printf '无法创建路径指纹结果\n' >&2
		return 1
	}
	trap 'rm -f -- "$list" "$sorted" "$fingerprint"' EXIT
	{
		{
			for path in "${scopes[@]}"; do
				path_is_excluded "$path" "${excludes[@]}" && continue
				path_is_excluded_dir "$path" "${dir_excludes[@]}" && continue
				if [[ -L "$path" ]]; then
					printf '%s\0' "$path"
				elif [[ -d "$path" ]]; then
					find_args=()
					for dir_exclude in "${dir_excludes[@]}"; do
						dir_prefix=${dir_exclude%/*}
						dir_name=${dir_exclude##*/}
						find_args+=(-type d -path "$dir_prefix/*" -name "$dir_name" -prune -o)
					done
					for exclude in "${excludes[@]}"; do
						find_args+=(-path "$exclude" -prune -o)
					done
					find "$path" "${find_args[@]}" \( -type f -o -type l \) -print0 || {
						printf '无法枚举路径指纹输入：%s\n' "$path" >&2
						return 1
					}
				elif [[ -e "$path" ]]; then
					printf '%s\0' "$path"
				fi
			done
		}
	} >"$list" || return 1
	LC_ALL=C sort -zu "$list" >"$sorted" || return 1
	{
		for path in "${scopes[@]}"; do
			path_is_excluded "$path" "${excludes[@]}" && continue
			path_is_excluded_dir "$path" "${dir_excludes[@]}" && continue
			printf 'scope\0%s\0' "$path"
		done
		fingerprint_file_list <"$sorted"
	} >"$fingerprint" || return 1
	identity=$(sha256sum "$fingerprint") || return 1
	identity=${identity%% *}
	[[ "$identity" =~ ^[0-9a-f]{64}$ ]] || {
		printf '路径指纹摘要无效：%q\n' "$identity" >&2
		return 1
	}
	printf '%s\n' "$identity"
)

# 工具配方：mise 锁文件记录每个工具的版本、构建选项、下载地址与校验和。mise.toml 还承载任务入口，
# 改任务不影响产物，因此不参与身份；ci/tools 下的说明文档与本地试验入口同理。
tool_recipe_inputs=(ci/tools/rustup.sh mise.lock)

# Solid 的 Web 与原生指纹共用这一处排除：任意层级的 node_modules 与 .tmp 都不属于发布输入。
web_input_excludes=(--exclude-dir web/node_modules --exclude-dir web/.tmp)

# 应用发布与封存复用同一份输入范围；工具配方也属于产物身份。
build_inputs_fingerprint() {
	local identity stack_marker
	if web_stack_uses_solid; then
		stack_marker=$(web_stack_marker) || return 1
		identity=$(path_fingerprint \
			"${web_input_excludes[@]}" \
			apps crates migrations web \
			ops/config/examples \
			ops/deploy/retention-policy.v2.json \
			Cargo.toml Cargo.lock rust-toolchain.toml .cargo ci/release "${tool_recipe_inputs[@]}") || return 1
		printf 'auction-web-stacks/1\0%s\0inputs\0%s\0' "$stack_marker" "$identity" |
			sha256_stdin
	else
		path_fingerprint \
			apps crates migrations \
			ops/config/examples \
			ops/deploy/retention-policy.v2.json \
			Cargo.toml Cargo.lock rust-toolchain.toml .cargo ci/release "${tool_recipe_inputs[@]}"
	fi
}

# Web payload 与封存复用同一份输入范围，surface 严格闭集，范围按实际技术栈选择。
web_inputs_fingerprint() {
	local surface=$1 stack identity
	case "$surface" in public | admin) ;; *) return 2 ;; esac
	stack=$(web_surface_stack "$surface") || return 2
	if [[ "$stack" == dioxus ]]; then
		path_fingerprint \
			Cargo.toml Cargo.lock rust-toolchain.toml .cargo \
			"crates/auction-web-$surface" "${web_workspace_dependency_paths[@]}" \
			ci/release/web-bundle.sh ci/release/build-identity.sh "${tool_recipe_inputs[@]}"
		return
	fi
	identity=$(path_fingerprint \
		"${web_input_excludes[@]}" \
		Cargo.toml mise.lock web \
		crates/auction-web-shared/style crates/auction-web-shared/assets \
		"crates/auction-web-$surface/assets" "crates/auction-web-$surface/i18n" \
		ci/release/web-bundle.sh ci/release/web-dependency-tree.sh ci/release/build-identity.sh) || return 1
	printf 'auction-web-inputs/3\0surface=%s\0stack=%s\0fingerprint=%s\0' \
		"$surface" "$stack" "$identity" | sha256_stdin
}
