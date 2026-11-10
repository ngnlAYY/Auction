#!/usr/bin/env bash
# release.sh 与 web-bundle.sh 共用的函数库，在仓库根目录 source：工具选择、构建身份指纹、
# BUILD-INFO 读取及 Web payload 门禁。调用方自行决定何时冻结身份，两个入口对同一源码给出同一指纹。

# 发布与 Web 共用工具选择；按工具字节身份隔离 sccache daemon，避免新客户端连接旧进程。
configure_build_tools() {
	local tools_identity repository_identity name uses_sccache=false
	tools_identity=$(bash ci/tools/bootstrap.sh identity release) || return 1
	export PATH="$PWD/target/build-tools/bin:$PATH" NO_DOWNLOADS=1
	# 发布只接受环境变量启用的 sccache 并换成受控版本；Cargo 文件中的两类包装器
	# 未经回执校验，各自的环境入口均未指定时用空值显式覆盖。
	if [[ -z ${RUSTC_WRAPPER+set} && -z ${CARGO_BUILD_RUSTC_WRAPPER+set} ]]; then
		export RUSTC_WRAPPER=
	fi
	if [[ -z ${RUSTC_WORKSPACE_WRAPPER+set} && -z ${CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER+set} ]]; then
		export RUSTC_WORKSPACE_WRAPPER=
	fi
	for name in RUSTC_WRAPPER RUSTC_WORKSPACE_WRAPPER CARGO_BUILD_RUSTC_WRAPPER CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER; do
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
	local commit
	commit=$(git rev-parse --verify 'HEAD^{commit}') || return 1
	{
		printf 'head\0%s\0' "$commit"
		git status --porcelain=v1 -z --untracked-files=all --ignore-submodules=none || return 1
		git ls-files -z --cached --others --exclude-standard |
			LC_ALL=C sort -zu | fingerprint_file_list || return 1
	} | sha256sum | awk '{print $1}'
}

# 前后端共享的构建号：两个 Web surface 输入并集指纹的前 32 位十六进制。
# 它只随浏览器 payload 的输入变化，后端专属改动不改变构建号，因此部署后不会误报；
# 值满足 1–64 位 URL/HTTP 头安全字符闭集。原生程序与浏览器必须用同一份来源计算。
web_build_identity() {
	local identity
	identity=$(path_fingerprint \
		Cargo.toml Cargo.lock rust-toolchain.toml .cargo \
		crates/auction-web-public crates/auction-web-admin \
		crates/auction-web-shared crates/auction-contracts \
		ci/release/web-bundle.sh ci/release/build-identity.sh "${tool_recipe_inputs[@]}") || return 1
	printf '%s\n' "${identity:0:32}"
}

# 读取 `key=value` 构建信息文件的单个字段；字段缺失时输出空串，交由调用方失败关闭。
read_build_info() {
	local file=$1 key=$2
	awk -F= -v wanted="$key" '$1 == wanted { print substr($0, length($1) + 2); exit }' "$file"
}

sha256_file() {
	sha256sum "$1" | awk '{print $1}'
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
	invalid=$(find "$root" ! -type f ! -type d -print -quit) || return 1
	[[ -z "$invalid" ]]
}

# payload 文件摘要清单（不含内嵌标记），按路径排序；生成与复核共用同一算法。
web_payload_checksums() (
	cd "$1" || return 1
	find . -type f ! -name auction-web-manifest.v1 -print0 | LC_ALL=C sort -z | xargs -0 -r sha256sum
)

# Web payload 完整门禁：入口与本 surface 标题、跨 surface 隔离、普通文件、内嵌标记、固定清单及唯一一组
# JS/WASM。成功时输出 payload 哈希；web-bundle 的新建/复用与 release 的制品核对共用同一判定。
verify_web_payload() (
	local surface=$1 stable_root=$2
	local root="$stable_root/public" other title other_title marker manifest marker_sha actual_sha file
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
	if ! grep -Fq "<title>$title</title>" "$root/index.html" || grep -Fq "$other_title" "$root/index.html"; then
		printf '%s Web 入口混入错误 surface 标题\n' "$surface" >&2
		return 1
	fi
	if grep -Fq "auction-web-$other" < <(find "$root" -type f -print); then
		printf '%s Web payload 混入 %s 文件\n' "$surface" "$other" >&2
		return 1
	fi

	marker_sha=$(read_build_info "$marker" payload_sha256)
	if [[ $(read_build_info "$marker" schema) != auction-web-assets/1 ||
	$(read_build_info "$marker" surface) != "$surface" ||
	$(read_build_info "$marker" profile) != wasm-release ||
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

	local wasm_count js_count
	wasm_count=$(find "$root" -type f -name "auction-web-${surface}_bg*.wasm" | wc -l)
	js_count=$(find "$root" -type f -name "auction-web-${surface}-*.js" | wc -l)
	if [[ "$wasm_count" -ne 1 || "$js_count" -ne 1 ]]; then
		printf '%s Web payload 必须且只能包含一组优化 JS/WASM：js=%s wasm=%s\n' \
			"$surface" "$js_count" "$wasm_count" >&2
		return 1
	fi
	printf '%s\n' "$marker_sha"
)

# 稳定 Web payload 目录；surface 闭集之外失败，避免调用方自行拼包名而漂移。
web_surface_root() {
	local surface=$1 repository_root=$2 package
	package=$(web_surface_package "$surface") || return 2
	printf '%s/target/dx/build/%s/web\n' "$repository_root" "$package"
}

# 单端 payload 体积：唯一优化 WASM 字节数与内嵌的全部普通文件合计。
# verify_web_payload 已保证只存在一组 JS/WASM；输出 `wasm_bytes payload_bytes`。
web_payload_sizes() (
	local surface=$1
	local stable_root=$2
	local root wasm_bytes payload_bytes
	root="$stable_root/public"
	[[ -d "$root" && ! -L "$root" ]] || return 1
	wasm_bytes=$(find "$root" -type f -name "auction-web-${surface}_bg*.wasm" -printf '%s\n' |
		awk '{ total += $1 } END { print total + 0 }') || return 1
	# 体积预算按原始表示统计；构建期 brotli 副本（*.br）只影响传输量，不计入 payload。
	payload_bytes=$(find "$root" -type f ! -name '*.br' -printf '%s\n' |
		awk '{ total += $1 } END { print total + 0 }') || return 1
	printf '%s %s\n' "$wasm_bytes" "$payload_bytes"
)

# 发布体积预算：超过 baseline + 万分比告警阈值只打印 WARN，超过硬上限失败关闭。
# baseline 必须来自刻意复核的 payload，不能随构建自动刷新，否则体积回归永远不会被发现。
verify_web_size_budget() {
	local surface=$1 stable_root=$2 budget_file=$3
	local schema warn_basis_points sizes wasm_bytes payload_bytes measured_bytes
	local metric baseline_key max_key baseline_bytes max_bytes warn_bytes failed=0
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
	wasm_bytes=${BASH_REMATCH[1]}
	payload_bytes=${BASH_REMATCH[2]}
	for metric in wasm payload; do
		baseline_key="${surface}_${metric}_baseline_bytes"
		max_key="${surface}_${metric}_max_bytes"
		baseline_bytes=$(read_build_info "$budget_file" "$baseline_key")
		max_bytes=$(read_build_info "$budget_file" "$max_key")
		if [[ ! "$baseline_bytes" =~ ^[1-9][0-9]*$ || ! "$max_bytes" =~ ^[1-9][0-9]*$ ]]; then
			printf 'Web 体积预算缺少有效的 %s/%s：%s\n' \
				"$baseline_key" "$max_key" "$budget_file" >&2
			return 1
		fi
		if ((baseline_bytes > max_bytes)); then
			printf 'Web 体积预算基线不得高于硬上限：%s\n' "$budget_file" >&2
			return 1
		fi
		if [[ "$metric" == wasm ]]; then measured_bytes=$wasm_bytes; else measured_bytes=$payload_bytes; fi
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
	} | sha256sum | awk '{print $1}'
}

# 原生制品与 Web payload 共用构建环境指纹；profile、工具包装器及目标参数变化都必须失效缓存。
# 名称和值用 NUL 分隔后只输出摘要，避免分隔符歧义，也不把环境原文写进制品元数据。
build_env_fingerprint() {
	local name tools_identity
	# 同版本工具修复后字节也会改变；工具回执参与输入身份，避免复用旧工具生成的制品。
	tools_identity=$(bash ci/tools/bootstrap.sh identity release) || return 1
	{
		printf 'auction-build-tools\0%s\0' "$tools_identity"
		cargo_config_fingerprint || return 1
		# Cargo 的配置环境别名同样能替换编译器/包装器或覆盖 profile 的增量策略。
		while IFS= read -r name; do
			case "$name" in
			CARGO_PROFILE_* | CARGO_BUILD_RUSTC | CARGO_BUILD_RUSTC_WRAPPER | CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER | CARGO_BUILD_INCREMENTAL | CARGO_INCREMENTAL | CARGO_BUILD_RUSTFLAGS | CARGO_ENCODED_RUSTFLAGS | CARGO_BUILD_TARGET | CARGO_TARGET_*_RUSTFLAGS | CARGO_TARGET_*_LINKER | RUSTDOCFLAGS | RUSTFLAGS | RUSTC | RUSTC_WRAPPER | RUSTC_WORKSPACE_WRAPPER | SCCACHE_SERVER_UDS | CC | CC_* | CXX | CXX_* | AR | AR_* | CFLAGS | CFLAGS_* | CXXFLAGS | CXXFLAGS_* | HOST_CC | HOST_CXX | HOST_AR | HOST_CFLAGS | HOST_CXXFLAGS | TARGET_CC | TARGET_CXX | TARGET_AR | TARGET_CFLAGS | TARGET_CXXFLAGS)
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

# 受控工具的配方文件；ci/tools 下的说明文档与本地试验入口不影响产物，不参与身份。
tool_recipe_inputs=(ci/tools/bootstrap.sh ci/tools/rustup.sh ci/tools/locks ci/tools/patches)

# 应用发布与封存复用同一份输入范围；工具配方也属于产物身份。
build_inputs_fingerprint() {
	path_fingerprint \
		apps crates migrations \
		ops/config/examples \
		ops/deploy/retention-policy.v2.json \
		Cargo.toml Cargo.lock rust-toolchain.toml .cargo ci/release "${tool_recipe_inputs[@]}"
}

# Web payload 与封存复用同一份输入范围，surface 严格闭集。
web_inputs_fingerprint() {
	local surface=$1
	case "$surface" in public | admin) ;; *) return 2 ;; esac
	path_fingerprint \
		Cargo.toml Cargo.lock rust-toolchain.toml .cargo \
		"crates/auction-web-$surface" crates/auction-web-shared crates/auction-contracts \
		ci/release/web-bundle.sh ci/release/build-identity.sh "${tool_recipe_inputs[@]}"
}
