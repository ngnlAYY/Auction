#!/usr/bin/env bash
# 构建单个 Web surface 的固定 payload。复用时核对输入、共享构建号与产物完整性。
# 用法：ci/release/web-bundle.sh public|admin

# 读取准备凭据的 lock/bun/安装树三行；文件缺失、字段多余或格式无效时失败关闭。
solid_prepared_fields() {
	local prepared="$repository_root/web/node_modules/.auction-prepared"
	[[ -f "$prepared" && ! -L "$prepared" ]] || return 1
	awk -F= '
		NF != 2 || ($1 != "lock_sha256" && $1 != "bun_version" && $1 != "tree_sha256") || seen[$1]++ { exit 1 }
		{ value[$1] = $2 }
		END {
			if (NR != 3 || !seen["lock_sha256"] || !seen["bun_version"] || !seen["tree_sha256"]) exit 1
			print value["lock_sha256"]
			print value["bun_version"]
			print value["tree_sha256"]
		}' "$prepared"
}

# Solid 复用回执绑定准备凭据里的安装树摘要：只比较 lock 与 bun 版本时，重新准备出不同依赖树
# 仍会复用旧 payload。凭据缺失或摘要无效时返回失败，由调用方给出可行动诊断。
solid_prepared_tree_sha256() {
	local fields tree
	fields=$(solid_prepared_fields) || return 1
	tree=${fields##*$'\n'}
	[[ "$tree" =~ ^[0-9a-f]{64}$ ]] || return 1
	printf '%s\n' "$tree"
}

# 写入与复用比较共用字段清单。回执须绑定实际编译进 payload 的构建号。
web_build_fields() {
	local stack_value=${stack:-dioxus}
	printf '%s\n' 'schema=auction-web-build/1' "surface=$surface"
	if [[ "$stack_value" == solid ]]; then
		printf '%s\n' 'profile=solid-release' 'stack=solid' "bun_version=$bun_version" \
			"dependencies_tree_sha256=$solid_dependencies_tree"
	else
		printf '%s\n' 'profile=wasm-release' "dx_version=$dx_version" "rustc_version=$rustc_version"
	fi
	printf '%s\n' \
		"brotli_version=$brotli_version" \
		"source_fingerprint=$source_identity" \
		"build_env_sha256=$build_environment" \
		"build_id=$AUCTION_BUILD_ID"
}

# 命中后既不重新构建，也不改写 mtime，保证内嵌资源与上一份制品逐字节相同。
web_payload_reusable() {
	[[ -d "$stable_root" && ! -L "$stable_root" && -f "$web_build_info" && ! -L "$web_build_info" ]] || return 1
	[[ $(cat "$web_build_info") == "$(web_build_fields)" ]] || return 1
	verify_web_payload "$surface" "$stable_root" >/dev/null
}

web_build_inputs_unchanged() {
	local current_source_identity current_build_environment current_build_id current_dependencies_tree=''
	current_source_identity=$(web_inputs_fingerprint "$surface") || return 1
	current_build_environment=$(build_env_fingerprint) || return 1
	current_build_id=$(web_build_identity) || return 1
	if [[ "${stack:-dioxus}" == solid ]]; then
		current_dependencies_tree=$(solid_prepared_tree_sha256) || return 1
	fi
	[[ "$current_source_identity" == "$source_identity" &&
		"$current_build_environment" == "$build_environment" &&
		"$current_build_id" == "$AUCTION_BUILD_ID" &&
		"$current_dependencies_tree" == "${solid_dependencies_tree:-}" ]]
}

# 两端都支持 Dioxus 与 Solid；未知 surface 或技术栈必须在任何清理或构建之前失败。
web_bundle_stack_supported() {
	case "$1:$2" in
	public:dioxus | public:solid | admin:dioxus | admin:solid) return 0 ;;
	*) return 1 ;;
	esac
}

# 两端组合一起检查：请求 public 时 Admin 若不受支持，也不能先把 public 换掉再失败。
web_bundle_stacks_supported() {
	local surface stack
	for surface in public admin; do
		stack=$(web_surface_stack "$surface") || return 2
		if ! web_bundle_stack_supported "$surface" "$stack"; then
			printf '尚未支持的 Web 技术栈组合：%s=%s\n' "$surface" "$stack" >&2
			return 1
		fi
	done
}

# 构建阶段不联网：依赖凭据由 mise run web-prepare 在安装成功后写入，这里只核对凭据。
solid_web_dependencies_ready() {
	local web_root="$repository_root/web"
	local prepared="$web_root/node_modules/.auction-prepared"
	local vite="$web_root/node_modules/.bin/vite"
	local lock_sha256 prepared_values prepared_lock prepared_bun current_bun installed_sha256 prepared_tree
	if [[ ! -d "$web_root/node_modules" || -L "$web_root/node_modules" ]]; then
		printf 'web/ 依赖未安装：先运行 mise run web-prepare\n' >&2
		return 1
	fi
	if [[ ! -f "$vite" || ! -x "$vite" ]]; then
		printf 'web/ 依赖不完整：缺少 node_modules/.bin/vite，先运行 mise run web-prepare\n' >&2
		return 1
	fi
	if [[ ! -f "$prepared" || -L "$prepared" ]]; then
		printf 'web/ 依赖缺少准备凭据：先运行 mise run web-prepare\n' >&2
		return 1
	fi
	if ! prepared_values=$(solid_prepared_fields); then
		printf 'web/ 依赖准备凭据格式无效：先运行 mise run web-prepare\n' >&2
		return 1
	fi
	prepared_lock=${prepared_values%%$'\n'*}
	prepared_values=${prepared_values#*$'\n'}
	prepared_bun=${prepared_values%%$'\n'*}
	prepared_tree=${prepared_values#*$'\n'}
	if [[ ! "$prepared_lock" =~ ^[0-9a-f]{64}$ || -z "$prepared_bun" ]]; then
		printf 'web/ 依赖准备凭据内容无效：先运行 mise run web-prepare\n' >&2
		return 1
	fi
	if [[ ! -f "$web_root/bun.lock" || -L "$web_root/bun.lock" ]]; then
		printf 'web/bun.lock 缺失或不是普通文件：先运行 mise run web-prepare\n' >&2
		return 1
	fi
	if ! lock_sha256=$(sha256_file "$web_root/bun.lock"); then
		printf 'web/ 依赖准备凭据无法校验 web/bun.lock：先运行 mise run web-prepare\n' >&2
		return 1
	fi
	if [[ "$lock_sha256" != "$prepared_lock" ]]; then
		printf 'web/ 依赖准备凭据与 web/bun.lock 不一致：先运行 mise run web-prepare\n' >&2
		return 1
	fi
	if [[ ! "$prepared_tree" =~ ^[0-9a-f]{64}$ ]]; then
		printf 'web/ 依赖准备凭据的安装树摘要无效\n' >&2
		return 1
	fi
	if ! installed_sha256=$(bash "$repository_root/ci/release/web-dependency-tree.sh" "$web_root/node_modules"); then
		printf '无法核对 web/ 已安装依赖树\n' >&2
		return 1
	fi
	[[ "$installed_sha256" == "$prepared_tree" ]] || {
		printf 'web/ 已安装依赖与准备凭据不一致：先运行 mise run web-prepare\n' >&2
		return 1
	}
	current_bun=${bun_version:-}
	if [[ -z "$current_bun" ]]; then
		if ! current_bun=$(bun --version); then
			printf '无法读取当前 bun 版本：先运行 mise run web-prepare\n' >&2
			return 1
		fi
		current_bun=${current_bun%%$'\n'*}
	fi
	if [[ "$current_bun" != "$prepared_bun" ]]; then
		printf 'web/ 依赖准备凭据的 bun 版本与当前工具不一致：先运行 mise run web-prepare\n' >&2
		return 1
	fi
}

# 入口文档的唯一模块脚本必须带 Rocket Loader 豁免；引用清单同时拒绝其他标签写法。
verify_solid_entry_opt_out() {
	local references
	references=$(web_entry_references "$payload_root/index.html") || return 1
	[[ "$references" == *$'entry\t/assets/'* ]]
}

# 先把产物写到同目录的临时位置，校验与回执都完成后再换成稳定目录；失败时保留旧 payload。
install_generated_directory() {
	local next=$1 current=$2 backup=''
	# 调用方在复用判定前已持有 surface 锁；这里不重复获取同一把锁，以免本进程自锁。
	if [[ -e "$current" || -L "$current" ]]; then
		backup=$(mktemp -d "$current.previous.XXXXXX") || return 1
		mv -T -- "$current" "$backup/payload" || return 1
	fi
	if ! mv -T -- "$next" "$current"; then
		if [[ -n "$backup" ]]; then
			mv -T -- "$backup/payload" "$current" || return 1
			rmdir -- "$backup" || return 1
		fi
		return 1
	fi
	if [[ -n "$backup" ]] && ! rm -rf -- "$backup"; then
		printf 'Web payload 已发布，但旧目录清理失败：%s\n' "$backup" >&2
	fi
}

# 恢复目录替换在两次 rename 之间被中断时留下的稳定 payload；调用方先持有 surface 锁。
recover_generated_directory() {
	local current=$1 backup
	local -a previous=()
	shopt -s nullglob
	previous=("$current".previous.*)
	shopt -u nullglob
	if [[ ! -e "$current" && ! -L "$current" ]]; then
		for backup in "${previous[@]}"; do
			# 中断在保存旧目录之前只会留下空容器。
			[[ -e "$backup/payload" || -L "$backup/payload" ]] || continue
			mv -T -- "$backup/payload" "$current" || return 1
			break
		done
	fi
	if ((${#previous[@]})); then
		rm -rf -- "${previous[@]}"
	fi
}

# 构建期生成高等级 brotli 副本；后端按 Accept-Encoding 协商，Caddy 的即时压缩只作兜底。
# 只压缩 >=1 KiB 的静态资源；压缩后不小于原文件 90% 的不保留副本。
compress_payload_assets() {
	local file size compressed compressed_size files
	local -a files_list=()
	if ! files=$(mktemp); then
		printf '无法创建 brotli 输入清单\n' >&2
		return 1
	fi
	if ! find "$payload_root/assets" -type f \
		\( -name '*.wasm' -o -name '*.js' -o -name '*.css' -o -name '*.ftl' -o -name '*.svg' \) \
		-print0 >"$files"; then
		rm -f -- "$files"
		printf '无法枚举 Web payload 的 brotli 输入\n' >&2
		return 1
	fi
	if ! mapfile -d '' -t files_list <"$files"; then
		rm -f -- "$files"
		printf '无法读取 brotli 输入清单\n' >&2
		return 1
	fi
	if ! rm -f -- "$files"; then
		printf '无法删除 brotli 输入清单\n' >&2
		return 1
	fi
	for file in "${files_list[@]}"; do
		if ! size=$(stat -c %s "$file"); then
			printf '无法读取 Web payload 文件大小：%s\n' "$file" >&2
			return 1
		fi
		if ((size < 1024)); then continue; fi
		if ! compressed=$(mktemp "$file.br.next.XXXXXX"); then
			printf '无法创建 brotli 临时文件：%s\n' "$file" >&2
			return 1
		fi
		if ! brotli -q 11 -f -o "$compressed" "$file"; then
			rm -f -- "$compressed"
			printf 'brotli 预压缩失败：%s\n' "$file" >&2
			return 1
		fi
		if ! compressed_size=$(stat -c %s "$compressed"); then
			rm -f -- "$compressed"
			printf '无法读取 brotli 预压缩结果大小：%s\n' "$file" >&2
			return 1
		fi
		if ((compressed_size * 10 >= size * 9)); then
			if ! rm -f -- "$compressed" "$file.br"; then
				printf '无法删除未采用的 brotli 副本：%s\n' "$file.br" >&2
				return 1
			fi
		else
			if ! mv -f -- "$compressed" "$file.br"; then
				rm -f -- "$compressed"
				printf '无法写入 brotli 副本：%s\n' "$file.br" >&2
				return 1
			fi
		fi
	done
}

if [[ ${BASH_SOURCE[0]} != "$0" ]]; then return; fi

set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
cd "$repository_root"

# 回执验证、工具选择、指纹与 payload 门禁和原生发布入口共用同一份实现。
source ci/release/build-identity.sh
print_web_stack_selection || exit 1
web_bundle_stacks_supported || exit 1

surface=${1:-}
if ! package=$(web_surface_package "$surface"); then
	printf '未知 Web surface：%s\n' "$surface" >&2
	exit 2
fi
stack=$(web_surface_stack "$surface") || exit 2

configure_build_tools

stable_root=$(web_surface_root "$surface" "$repository_root") || exit 2
payload_root="$stable_root/public"
payload_manifest="$stable_root/PAYLOAD-SHA256"
marker="$payload_root/auction-web-manifest.v1"
# 复用标记放在 payload 之外：它不属于内嵌资源，也不计入 payload 哈希。
web_build_info="$stable_root/WEB-BUILD-INFO"

lock_directory="$repository_root/target/web/.locks"
mkdir -p -- "$lock_directory"
exec {surface_lock_fd}>"$lock_directory/$surface.lock"
flock -x "$surface_lock_fd"

recover_generated_directory "$stable_root"

# 复用判定必须基于锁内重新读取的输入：等锁期间源码或工具变化时，不能拿锁前快照复用旧 payload。
# 浏览器与原生程序使用同一构建号，即使单端输入未变，复用也必须维持这一不变量。
AUCTION_BUILD_ID=$(web_build_identity) || exit 1
export AUCTION_BUILD_ID
source_epoch=$(git show -s --format=%ct HEAD)
if [[ ! "$source_epoch" =~ ^[1-9][0-9]*$ ]]; then
	printf '无法取得有效的 SOURCE_DATE_EPOCH：%s\n' "$source_epoch" >&2
	exit 1
fi

# 复用判定还必须区分构建工具：Dioxus 看 dx/rustc，Solid 看 bun；brotli 影响两栈的压缩副本。
if [[ "$stack" == solid ]]; then
	bun_version=$(bun --version 2>/dev/null | sed -n '1p')
else
	dx_version=$(dx --version 2>/dev/null | sed -n '1p')
	rustc_version=$(rustc --version --verbose 2>/dev/null | sed -n '1p')
fi
if ! brotli_version=$(brotli --version 2>&1); then
	printf '无法读取 brotli 版本：请确认发布工具 brotli 可用\n' >&2
	exit 1
fi
brotli_version=${brotli_version%%$'\n'*}
if [[ "$stack" == solid && -z "$bun_version" ]] ||
	[[ "$stack" == dioxus && (-z "$dx_version" || -z "$rustc_version") ]]; then
	printf '无法读取 %s 构建工具版本，Web 构建不能建立复用标记\n' "$stack" >&2
	exit 1
fi
if [[ -z "$brotli_version" ]]; then
	printf '无法读取 brotli 版本，Web 构建不能建立复用标记\n' >&2
	exit 1
fi
# payload 指纹覆盖本 surface 及浏览器的工作区路径依赖，防止交付由旧依赖编译的 WASM。
source_identity=$(web_inputs_fingerprint "$surface")
# WASM 自定义 profile 与 rustflags 同样影响页面产物，不能只使外层原生二进制缓存失效。
build_environment=$(build_env_fingerprint)

# Solid 的 Lock 与 bun 版本可能不变而依赖树重新准备；复用前先取准备凭据里的树摘要。
if [[ "$stack" == solid ]]; then
	solid_dependencies_tree=$(solid_prepared_tree_sha256) || {
		printf 'web/ 依赖准备凭据缺少安装树摘要：先运行 mise run web-prepare\n' >&2
		exit 1
	}
fi

if web_payload_reusable; then
	printf '%s Web payload 输入未变，复用：sha256=%s files=%s\n' \
		"$surface" "$(read_build_info "$marker" payload_sha256)" "$(wc -l <"$payload_manifest")"
	exit 0
fi

next_files=()
generated_build_root=''
cleanup() {
	rm -f -- "${next_files[@]}"
	if [[ -n "$generated_build_root" ]]; then
		rm -rf -- "$generated_build_root"
	fi
}
trap cleanup EXIT

mkdir -p "$(dirname "$stable_root")"
build_root=$(mktemp -d "$stable_root.next.XXXXXX") || exit 1
generated_build_root=$build_root
payload_root="$build_root/public"
payload_manifest="$build_root/PAYLOAD-SHA256"
marker="$payload_root/auction-web-manifest.v1"
web_build_info="$build_root/WEB-BUILD-INFO"

if [[ "$stack" == dioxus ]]; then
	# Dioxus 会把继承 release 的自定义 profile 归入 release 中间目录；两个精确路径都清理，避免把历史
	# 指纹文件再次复制进发布 payload，同时不触碰另一个 surface 或 Cargo 的共享依赖缓存。
	rm -rf "$repository_root/target/dx/$package/release/web" \
		"$repository_root/target/dx/$package/wasm-release/web"

	SOURCE_DATE_EPOCH=$source_epoch \
		dx bundle \
		-p "$package" \
		--platform web \
		--profile wasm-release \
		--debug-symbols false \
		--out-dir "$build_root" \
		--locked

	# 加载器要等 JS 下载并执行后才请求 WASM；入口文档提前声明 preload，让最大的资源与 JS 并行下载。
	# 加载器的 fetch() 使用 cors 模式与同源凭据，和匿名 crossorigin 的预加载匹配，浏览器复用同一响应；
	# 路径写法与加载器一致（`/./assets/`）。必须在生成 payload 清单之前改写，清单随后固定入口内容。
	add_wasm_preload() {
		local document="$payload_root/index.html" anchor='<link rel="preload" as="script"'
		local wasm_names html preload
		wasm_names=$(find "$payload_root/assets" -maxdepth 1 -type f \
			-name "auction-web-${surface}_bg*.wasm" -printf '%f\n') || return 1
		# 文件名不唯一时预加载会多下载一份 WASM；锚点缺失或重复说明入口结构已变，不能猜测插入位置。
		[[ -n "$wasm_names" && "$wasm_names" != *$'\n'* ]] || return 1
		html=$(<"$document") || return 1
		[[ "$html" == *"$anchor"* && "${html#*"$anchor"}" != *"$anchor"* ]] || return 1
		# 入口已带 WASM 预加载（重复执行或 dx 自行输出）时不再叠加第二份。
		[[ "$html" != *'type="application/wasm"'* ]] || return 1
		printf -v preload '<link rel="preload" as="fetch" type="application/wasm" href="/./assets/%s" crossorigin>' \
			"$wasm_names"
		printf '%s\n' "${html/"$anchor"/"$preload$anchor"}" >"$document"
	}
	add_wasm_preload || {
		printf '%s Web 入口无法声明 WASM 预加载\n' "$surface" >&2
		exit 1
	}

	# Cloudflare Rocket Loader 在 zone 级开启时会把入口模块脚本改成自定义类型、等它自己的加载器执行后
	# 才启动应用：冷启动多一次请求与一段串行等待，注入的内联脚本还会被本站 CSP 拦截。
	# `data-cfasync="false"` 让 Rocket Loader 跳过该脚本，不依赖 CDN 侧另建规则。
	add_rocket_loader_opt_out() {
		local document="$payload_root/index.html" anchor='<script type="module"'
		local html
		html=$(<"$document") || return 1
		# 入口应恰好一个模块脚本；缺失或重复说明 dx 输出结构已变，不能猜测改写位置。
		[[ "$html" == *"$anchor"* && "${html#*"$anchor"}" != *"$anchor"* ]] || return 1
		[[ "$html" != *'data-cfasync='* ]] || return 1
		printf '%s\n' "${html/"$anchor"/<script data-cfasync=\"false\" type=\"module\"}" >"$document"
	}
	add_rocket_loader_opt_out || {
		printf '%s Web 入口无法声明 Rocket Loader 豁免\n' "$surface" >&2
		exit 1
	}

	# 样式表只由 dx 写入静态 head（运行时不再插入第二份），dx 按源码路径排列；同一 @layer 内同特异度的
	# 规则靠加载先后覆盖：base 末尾的通用手机默认值须先于 surface，console 外壳须在 surface 之后。
	# 改排为 base → surface → desktop → mobile → console；数量、名称不符或不连成一段时构建失败。
	order_stylesheets() {
		local document="$payload_root/index.html" marker='rel="stylesheet"'
		local pattern='<link rel="stylesheet" href="/\./assets/([a-z]+)-dxh[0-9a-f]+\.css" type="text/css">'
		local html rest tag stem block='' ordered='' total
		local -A tags=()
		html=$(<"$document") || return 1
		rest=$html
		while [[ "$rest" =~ $pattern ]]; do
			tag=${BASH_REMATCH[0]}
			stem=${BASH_REMATCH[1]}
			# 同名样式表出现两次说明产物结构已变，不能猜测保留哪一份。
			[[ -z "${tags[$stem]+set}" ]] || return 1
			tags[$stem]=$tag
			block+=$tag
			rest=${rest#*"$tag"}
		done
		# 入口里的每个样式表都必须被上面的模式识别，未知样式表不能留在原位。
		rest=${html//"$marker"/}
		total=$(((${#html} - ${#rest}) / ${#marker}))
		((total == 5 && ${#tags[@]} == 5)) || return 1
		for stem in base "$surface" desktop mobile console; do
			[[ -n "${tags[$stem]+set}" ]] || return 1
			ordered+=${tags[$stem]}
		done
		# 只在样式表首尾相连时整体替换，不移动夹在其间的其他节点。
		[[ "$html" == *"$block"* ]] || return 1
		printf '%s\n' "${html/"$block"/"$ordered"}" >"$document"
	}
	order_stylesheets || {
		printf '%s Web 入口的样式表与预期不符，无法固定加载顺序\n' "$surface" >&2
		exit 1
	}
	payload_profile=wasm-release
else
	solid_web_dependencies_ready || exit 1
	(
		cd "$repository_root/web" || exit 1
		NODE_ENV=production SOURCE_DATE_EPOCH=$source_epoch AUCTION_BUILD_ID=$AUCTION_BUILD_ID \
			bun ./node_modules/.bin/vite build \
			--config "$surface/vite.config.ts" \
			--outDir "$payload_root" \
			--emptyOutDir
	) || {
		printf 'Solid Web 构建失败；确认 web/node_modules 与 web/bun.lock 一致，必要时运行 mise run web-prepare\n' >&2
		exit 1
	}
	verify_solid_entry_opt_out || {
		printf '%s Web 入口的唯一模块脚本缺少 data-cfasync="false" 或结构不符\n' "$surface" >&2
		exit 1
	}
	payload_profile=solid-release
fi

compress_payload_assets || {
	printf '%s Web payload 的 brotli 预压缩失败\n' "$surface" >&2
	exit 1
}

# 先拒绝符号链接和特殊文件，再执行会跟随路径的 chmod/touch。
if [[ ! -f "$payload_root/index.html" ]] || ! web_payload_tree_is_plain "$payload_root"; then
	printf '%s Web payload 缺少 index.html 或含符号链接/特殊文件\n' "$surface" >&2
	exit 1
fi

payload_next=$(mktemp "$build_root/PAYLOAD-SHA256.next.XXXXXX")
next_files+=("$payload_next")
web_payload_checksums "$payload_root" >"$payload_next"
if ! grep -Fq '  ./index.html' "$payload_next"; then
	printf '%s Web payload 清单没有绑定 index.html\n' "$surface" >&2
	exit 1
fi
payload_sha256=$(sha256_file "$payload_next")
printf '%s\n' \
	'schema=auction-web-assets/1' \
	"surface=$surface" \
	"profile=$payload_profile" \
	"payload_sha256=$payload_sha256" \
	>"$marker"
install -m 0644 "$payload_next" "$payload_manifest"

# rust-embed 会保留资源 mtime；统一时间和权限使相同 payload 的二进制元数据可复核。
find "$payload_root" -type f -exec chmod 0644 {} +
find "$payload_root" -type d -exec chmod 0755 {} +
find "$payload_root" -exec touch -d "@$source_epoch" {} +
touch -d "@$source_epoch" "$payload_manifest"

# 新 payload 与复用走同一完整门禁。回执只能绑定构建前冻结的输入和构建号。
verify_web_payload "$surface" "$build_root" >/dev/null
if ! web_build_inputs_unchanged; then
	printf '%s Web 构建期间输入发生变化，请重新构建\n' "$surface" >&2
	exit 1
fi

# 复用标记最后在同目录写好再原子发布：中断留下的半成品 payload 没有匹配标记，下次仍会重建。
web_build_info_next=$(mktemp "$build_root/WEB-BUILD-INFO.next.XXXXXX")
next_files+=("$web_build_info_next")
web_build_fields >"$web_build_info_next"
chmod 0644 "$web_build_info_next"
touch -d "@$source_epoch" "$web_build_info_next"
mv -f -- "$web_build_info_next" "$web_build_info"

rm -f -- "${next_files[@]}"
next_files=()
chmod 0755 "$build_root"
touch -d "@$source_epoch" "$build_root"
install_generated_directory "$build_root" "$stable_root" || {
	printf '%s Web payload 无法换入稳定目录：%s\n' "$surface" "$stable_root" >&2
	exit 1
}
generated_build_root=''
payload_root="$stable_root/public"
payload_manifest="$stable_root/PAYLOAD-SHA256"
marker="$payload_root/auction-web-manifest.v1"
web_build_info="$stable_root/WEB-BUILD-INFO"

printf '%s Web 发布 payload 已生成：sha256=%s files=%s\n' \
	"$surface" "$payload_sha256" "$(wc -l <"$payload_manifest")"
