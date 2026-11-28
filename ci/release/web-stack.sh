#!/usr/bin/env bash
# 发布包内页面凭据 docs/WEB-STACK.v1 与页面导出树 web-assets/ 的生成与核对，供 release.sh、
# verify-deployment-package.sh 与自测在仓库根目录 source。本文件自成一体：不依赖 build-identity.sh，
# 不读环境，不联网；函数只在失败时向 stderr 报告原因并返回非零。

web_stack_schema='auction-web-stack/1'
web_stack_keys=(schema source_commit source_fingerprint public_stack admin_stack
	public_payload_sha256 admin_payload_sha256 build_id)

web_stack_sha256_file() {
	local digest
	digest=$(sha256sum -- "$1") || return 1
	digest=${digest%% *}
	[[ $digest =~ ^[0-9a-f]{64}$ ]] || return 1
	printf '%s\n' "$digest"
}

# 读取 key=value 文件的单个字段；字段缺失输出空串。
web_stack_read() {
	awk -F= -v wanted="$2" '$1 == wanted { print substr($0, length($1) + 2); exit }' "$1"
}

# 按固定字段与顺序输出 WEB-STACK.v1；参数：提交、源码指纹、Public 栈、Admin 栈、
# Public/Admin 的 WEB-*-SHA256 摘要、构建号。字段形状在 web_stack_check_file 统一核对。
web_stack_render() {
	printf '%s\n' \
		"schema=$web_stack_schema" \
		"source_commit=$1" \
		"source_fingerprint=$2" \
		"public_stack=$3" \
		"admin_stack=$4" \
		"public_payload_sha256=$5" \
		"admin_payload_sha256=$6" \
		"build_id=$7"
}

# WEB-STACK.v1 必须恰为 8 行、字段按固定顺序、LF 结尾、无空行与多余空白；任何偏离都拒绝。
web_stack_check_file() {
	local file=$1 index line key value pattern
	local -a lines=()
	if [[ ! -f $file || -L $file ]]; then
		printf 'WEB-STACK.v1 必须是普通文件：%s\n' "$file" >&2
		return 1
	fi
	if [[ $(stat -c '%a' -- "$file") != 644 ]]; then
		printf 'WEB-STACK.v1 必须是 0644 权限：%s\n' "$file" >&2
		return 1
	fi
	mapfile -t lines <"$file"
	if [[ ${#lines[@]} -ne ${#web_stack_keys[@]} || $(tail -c 1 -- "$file" | od -An -tx1) != ' 0a' ]]; then
		printf 'WEB-STACK.v1 行数或结尾无效\n' >&2
		return 1
	fi
	for index in "${!web_stack_keys[@]}"; do
		line=${lines[$index]}
		key=${web_stack_keys[$index]}
		value=${line#"$key="}
		case "$key" in
		schema) pattern="^${web_stack_schema}$" ;;
		source_commit) pattern='^[0-9a-f]{40}$' ;;
		public_stack | admin_stack) pattern='^(solid|dioxus)$' ;;
		build_id) pattern='^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$' ;;
		*) pattern='^[0-9a-f]{64}$' ;;
		esac
		if [[ $line != "$key="* || ! $value =~ $pattern ]]; then
			printf 'WEB-STACK.v1 第 %s 行应为合法的 %s\n' "$((index + 1))" "$key" >&2
			return 1
		fi
	done
}

# 逐行校验 WEB-<SURFACE>-SHA256（"<sha256>  ./<相对路径>"）并输出排序后的相对路径；
# 路径限定为不含 . 与 .. 分量的安全字符集，重复路径拒绝。
web_stack_payload_paths() {
	local sha_file=$1 paths
	if ! awk '
		length($0) < 69 || substr($0, 65, 2) != "  " || substr($0, 1, 64) ~ /[^0-9a-f]/ || substr($0, 67, 2) != "./" { bad = 1; next }
		{
			path = substr($0, 69)
			if (path !~ /^[A-Za-z0-9._@+\/-]+$/ || ("/" path "/") ~ /\/\.\.?\// || path ~ /\/\// || path ~ /\/$/) bad = 1
		}
		END { exit bad || NR == 0 }
	' "$sha_file"; then
		printf '页面摘要清单格式无效：%s\n' "$sha_file" >&2
		return 1
	fi
	paths=$(awk '{ print substr($0, 69) }' "$sha_file" | LC_ALL=C sort) || return 1
	if [[ -n $(printf '%s\n' "$paths" | uniq -d) ]]; then
		printf '页面摘要清单含重复路径：%s\n' "$sha_file" >&2
		return 1
	fi
	printf '%s\n' "$paths"
}

# 核对导出树 <root> 的路径集合与逐文件摘要必须与 <sha_file> 完全一致：缺文件、多文件、
# 同名异字节、符号链接、特殊文件与非 0644 文件都拒绝。
web_stack_verify_tree() {
	local root=$1 sha_file=$2 expected actual
	expected=$(web_stack_payload_paths "$sha_file") || return 1
	if [[ ! -d $root || -L $root ]]; then
		printf '页面导出目录缺失：%s\n' "$root" >&2
		return 1
	fi
	if [[ -n $(find "$root" ! -type f ! -type d -print -quit) ]]; then
		printf '页面导出树含符号链接或特殊文件：%s\n' "$root" >&2
		return 1
	fi
	if [[ -n $(find "$root" -type f ! -perm 0644 -print -quit) ]]; then
		printf '页面导出树含非 0644 文件：%s\n' "$root" >&2
		return 1
	fi
	actual=$(cd "$root" && find . -type f -printf '%P\n' | LC_ALL=C sort) || return 1
	if [[ $actual != "$expected" ]]; then
		printf '页面导出树与摘要清单的文件集合不一致：%s\n' "$root" >&2
		return 1
	fi
	(cd "$root" && sha256sum --check --strict --quiet "$sha_file") >/dev/null 2>&1 || {
		printf '页面导出树与摘要清单的字节不一致：%s\n' "$root" >&2
		return 1
	}
}

# 按摘要清单把 <payload_root> 中的页面文件复制进 <destination>（0644），随后用同一清单核对；
# 清单与稳定目录不一致（陈旧页面）时失败。
web_stack_export_tree() {
	local payload_root=$1 sha_file=$2 destination=$3 path
	local -a paths=()
	mapfile -t paths < <(web_stack_payload_paths "$sha_file") || return 1
	[[ ${#paths[@]} -gt 0 ]] || return 1
	for path in "${paths[@]}"; do
		if [[ ! -f $payload_root/$path || -L $payload_root/$path ]]; then
			printf '页面文件缺失或不是普通文件：%s/%s\n' "$payload_root" "$path" >&2
			return 1
		fi
		install -D -m 0644 -- "$payload_root/$path" "$destination/$path" || return 1
	done
	web_stack_verify_tree "$destination" "$sha_file"
}

# 核对已解包的发布包：WEB-STACK.v1 字段闭集、与清单头同一来源、两端均为 Solid、
# 摘要等于 docs/WEB-*-SHA256 的原字节摘要且等于 docs/WEB-*-MANIFEST.v1 的 payload_sha256，
# 并且 web-assets 下恰有 public 与 admin 两棵导出树且与摘要清单完全一致。
web_stack_verify_package() {
	local package_root=$1 commit=$2 fingerprint=$3 file surface listing upper digest
	file=$package_root/docs/WEB-STACK.v1
	web_stack_check_file "$file" || return 1
	if [[ $(web_stack_read "$file" source_commit) != "$commit" ||
		$(web_stack_read "$file" source_fingerprint) != "$fingerprint" ]]; then
		printf 'WEB-STACK.v1 的来源与 RELEASE-MANIFEST.v1 不一致\n' >&2
		return 1
	fi
	if [[ $(web_stack_read "$file" public_stack) != solid || $(web_stack_read "$file" admin_stack) != solid ]]; then
		printf 'WEB-STACK.v1 必须声明 Public 与 Admin 均为 solid\n' >&2
		return 1
	fi
	for surface in public admin; do
		upper=${surface^^}
		digest=$(web_stack_sha256_file "$package_root/docs/WEB-$upper-SHA256") || return 1
		if [[ $(web_stack_read "$file" "${surface}_payload_sha256") != "$digest" ||
			$(web_stack_read "$package_root/docs/WEB-$upper-MANIFEST.v1" payload_sha256) != "$digest" ]]; then
			printf 'WEB-STACK.v1 的 %s 摘要与 WEB-%s-SHA256 或清单不一致\n' "$surface" "$upper" >&2
			return 1
		fi
		web_stack_verify_tree "$package_root/web-assets/$surface" "$package_root/docs/WEB-$upper-SHA256" || return 1
	done
	listing=$(cd "$package_root/web-assets" && find . -mindepth 1 -maxdepth 1 -printf '%P\n' | LC_ALL=C sort) || return 1
	if [[ $listing != $'admin\npublic' ]]; then
		printf 'web-assets 只允许 public 与 admin 两棵导出树\n' >&2
		return 1
	fi
}
