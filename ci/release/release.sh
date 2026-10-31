#!/usr/bin/env bash
# 发布唯一入口：API-only 对照、双端嵌入、可选合并服务的构建与门禁，以及确定性装包。
# 子命令见文件末尾；复用、身份与 Web payload 门禁来自 build-identity.sh。

set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
cd "$repository_root"

# 工具选择与身份共用同一入口；回执校验失败时不进入任何发布操作。
source ci/release/build-identity.sh
configure_build_tools

artifact_root="$repository_root/target/dist/artifacts"
api_artifact_root="$artifact_root/api"
embedded_artifact_root="$artifact_root/embedded"
single_artifact_root="$artifact_root/single"
api_build_root="$repository_root/target/dist/build/api"
embedded_build_root="$repository_root/target/dist/build/embedded"
single_build_root="$repository_root/target/dist/build/single"
package_output_root="$repository_root/target/dist/package"
api_only_public_root="$repository_root/target/dist/api-only-assets/public"
api_only_admin_root="$repository_root/target/dist/api-only-assets/admin"
assets_missing_message="web assets are not included in this binary; run \`just build-release\`"
# 发布 feature 的唯一来源：可部署制品含 Redis；API-only 对照只留 PostgreSQL。
api_only_features='postgres'
runtime_features='postgres,cache-redis'
embedded_features="$runtime_features,auction-public-api/web-embed,auction-admin-api/web-embed"
single_features="$runtime_features,auction-server/web-embed"

binaries=(
	auction-public-api
	auction-admin-api
	auction-worker
	auction-cli
)
# API-only 是不发布的资源隔离对照，只需两个 Web surface；Worker/CLI 的无页面证明由嵌入档与依赖图门禁承担，
# 省去两次 Fat LTO 链接。
api_only_binaries=(
	auction-public-api
	auction-admin-api
)

require_command() {
	local command_name=$1
	if ! command -v "$command_name" >/dev/null 2>&1; then
		printf '发布门禁缺少命令：%s\n' "$command_name" >&2
		exit 1
	fi
}

for required in cargo date diff dx find git gzip install sha256sum sort stat tar; do
	require_command "$required"
done

worktree_state() {
	local status
	status=$(git status --porcelain=v1 --untracked-files=all --ignore-submodules=none) || return 1
	if [[ -z "$status" ]]; then
		printf 'clean\n'
	else
		printf 'dirty\n'
	fi
}

write_build_info() {
	local destination=$1 variant=$2 features=$3
	local commit state fingerprint lock_sha inputs_sha environment_sha
	commit=$(git rev-parse --verify 'HEAD^{commit}') || return 1
	state=$(worktree_state) || return 1
	fingerprint=$(source_fingerprint) || return 1
	lock_sha=$(sha256_file Cargo.lock) || return 1
	# printf 的成功不能覆盖命令替换的失败；所有身份读取成功后才写入回执。
	inputs_sha=$(build_inputs_fingerprint) || return 1
	environment_sha=$(build_env_fingerprint) || return 1
	printf '%s\n' \
		'schema=auction-release-build/1' \
		"variant=$variant" \
		"source_commit=$commit" \
		"source_worktree=$state" \
		"source_fingerprint=$fingerprint" \
		"cargo_lock_sha256=$lock_sha" \
		"features=$features" \
		"build_inputs_sha256=$inputs_sha" \
		"build_env_sha256=$environment_sha" \
		>"$destination"
	chmod 0644 "$destination"
}

replace_generated_directory() {
	local next=$1 current=$2
	local backup="$current.previous.$$"
	rm -rf "$backup"
	if [[ -e "$current" ]]; then
		mv "$current" "$backup"
	fi
	if ! mv "$next" "$current"; then
		if [[ -e "$backup" ]]; then
			mv "$backup" "$current"
		fi
		return 1
	fi
	rm -rf "$backup"
}

# 从固定白名单重算摘要，不执行缓存清单提供的路径；字节损坏必须使复用失效。
artifact_checksums() (
	local root=$1 variant=$2 path
	local files=(BUILD-INFO) selected_binaries=("${binaries[@]}")
	case "$variant" in
	api-only) selected_binaries=("${api_only_binaries[@]}") ;;
	embedded) ;;
	single) selected_binaries=(auction-server auction-cli) ;;
	*) return 2 ;;
	esac
	for path in "${selected_binaries[@]}"; do files+=("bin/$path"); done
	if [[ "$variant" != api-only ]]; then
		files+=(WEB-PUBLIC-SHA256 WEB-ADMIN-SHA256 WEB-PUBLIC-MANIFEST.v1 WEB-ADMIN-MANIFEST.v1)
	fi
	cd "$root" || return 1
	for path in "${files[@]}"; do
		[[ -f "$path" && ! -L "$path" ]] || return 1
	done
	printf '%s\0' "${files[@]}" | LC_ALL=C sort -z | xargs -0 -r sha256sum
)

verify_artifact_checksums() {
	local root=$1 variant=$2 actual expected
	[[ -f "$root/SHA256SUMS" && ! -L "$root/SHA256SUMS" ]] || return 1
	actual=$(artifact_checksums "$root" "$variant") || return 1
	expected=$(cat "$root/SHA256SUMS") || return 1
	if [[ "$actual" != "$expected" ]]; then
		printf '发布制品摘要不一致，请重新构建：%s\n' "$root" >&2
		return 1
	fi
}

# 复用判定：构建输入、锁图、feature 和制品完整性全部一致才返回成功。
# 复用不重建 Web payload；payload 由 web-bundle 按自身输入指纹复用，构建输入一致即
# 二进制与嵌入资源一致，因此可直接作为同一来源的产物。
artifacts_reusable() {
	local variant=$1 artifact_directory=$2 features=$3
	local selected_binaries=("${@:4}")
	local info="$artifact_directory/BUILD-INFO"

	if [[ ! -d "$artifact_directory" || -L "$artifact_directory" || ! -f "$info" || -L "$info" ]]; then
		return 1
	fi
	verify_artifact_checksums "$artifact_directory" "$variant" || return 1
	verify_build_info "$artifact_directory" "$variant" >/dev/null || return 1
	if [[ $(read_build_info "$info" features) != "$features" ]]; then
		printf '%s 已记 feature 与当前配方不同，重新构建：%s\n' "$variant" "$artifact_directory" >&2
		return 1
	fi
	local binary path
	for binary in "${selected_binaries[@]}"; do
		path="$artifact_directory/bin/$binary"
		if [[ ! -f "$path" || -L "$path" || ! -s "$path" || ! -x "$path" ]]; then
			printf '%s 制品二进制不完整，重新构建：%s\n' "$variant" "$path" >&2
			return 1
		fi
	done

	# 内嵌或合并变体另需可解析的 payload 标记，避免把删过资源的目录当作可复用制品。
	local marker payload_sha
	if [[ "$variant" == embedded || "$variant" == single ]]; then
		for marker in "$artifact_directory/WEB-PUBLIC-MANIFEST.v1" "$artifact_directory/WEB-ADMIN-MANIFEST.v1"; do
			payload_sha=$(read_build_info "$marker" payload_sha256)
			if [[ ! "$payload_sha" =~ ^[0-9a-f]{64}$ ]]; then
				printf '%s Web 标记缺失或无效，重新构建：%s\n' "$variant" "$marker" >&2
				return 1
			fi
		done
	fi
	return 0
}

build_variant() (
	local variant=$1 build_root=$2 artifact_directory=$3 features=$4
	local selected_binaries=("${@:5}")
	if [[ ${#selected_binaries[@]} -eq 0 ]]; then selected_binaries=("${binaries[@]}"); fi
	local next="$artifact_root/.${variant}.next.$$"

	if [[ "$variant" == api-only ]] && { [[ -e "$api_only_public_root" ]] || [[ -e "$api_only_admin_root" ]]; }; then
		printf 'API-only 固定空资源目录不得存在：%s 或 %s\n' \
			"$api_only_public_root" "$api_only_admin_root" >&2
		exit 1
	fi
	# 构建输入、锁图与 feature 全部一致时复用既有制品，避免重复 WASM 构建和 Fat LTO 链接。
	if artifacts_reusable "$variant" "$artifact_directory" "$features" "${selected_binaries[@]}"; then
		printf '%s 构建输入未变，复用：%s\n' "$variant" "$artifact_directory"
		return 0
	fi
	# 每次构建只拥有自己的候选目录；正常或错误退出时回收，保留旧制品和 Cargo 缓存。
	trap 'rm -rf -- "$next"' EXIT
	mkdir -p "$artifact_root"
	rm -rf "$next"
	mkdir -p "$next/bin"

	# 在任何 Web/native 构建前冻结源码身份；构建期间的修改不得被事后标记成产物来源。
	write_build_info "$next/BUILD-INFO" "$variant" "$features"
	if [[ "$variant" == embedded ]]; then
		bash ci/release/web-bundle.sh public
		verify_build_info "$next" "$variant"
		bash ci/release/web-bundle.sh admin
		web_payload_sha public >/dev/null
		web_payload_sha admin >/dev/null
	fi
	verify_build_info "$next" "$variant"

	# 强制重新展开两个 RustEmbed 类型，避免 Cargo 复用前一份 Web payload 的过程宏结果。
	if [[ -f "$build_root/CACHEDIR.TAG" ]]; then
		cargo clean --target-dir "$build_root" -p auction-public-api -p auction-admin-api
	elif [[ -e "$build_root" ]]; then
		# 该路径由本脚本独占；没有 Cargo 标记时只可能是中断后留下的未初始化生成目录。
		rm -rf "$build_root"
	fi
	local packages=() package
	for package in "${selected_binaries[@]}"; do packages+=(-p "$package"); done
	cargo build \
		--locked \
		--release \
		--target-dir "$build_root" \
		--no-default-features \
		"${packages[@]}" \
		--features "$features"
	verify_build_info "$next" "$variant"

	local binary
	for binary in "${selected_binaries[@]}"; do
		install -m 0755 "$build_root/release/$binary" "$next/bin/$binary"
	done
	if [[ "$variant" == embedded || "$variant" == single ]]; then
		install -m 0644 \
			target/dx/build/auction-web-public/web/PAYLOAD-SHA256 \
			"$next/WEB-PUBLIC-SHA256"
		install -m 0644 \
			target/dx/build/auction-web-admin/web/PAYLOAD-SHA256 \
			"$next/WEB-ADMIN-SHA256"
		install -m 0644 \
			target/dx/build/auction-web-public/web/public/auction-web-manifest.v1 \
			"$next/WEB-PUBLIC-MANIFEST.v1"
		install -m 0644 \
			target/dx/build/auction-web-admin/web/public/auction-web-manifest.v1 \
			"$next/WEB-ADMIN-MANIFEST.v1"
	fi

	verify_build_info "$next" "$variant"
	artifact_checksums "$next" "$variant" >"$next/SHA256SUMS"
	chmod 0644 "$next/SHA256SUMS"
	replace_generated_directory "$next" "$artifact_directory"
	printf '%s 发布二进制已生成：%s\n' "$variant" "$artifact_directory"
)

# 已构建的稳定 Web payload 通过完整门禁后输出其哈希。
web_payload_sha() {
	local package
	package=$(web_surface_package "$1") || return 2
	verify_web_payload "$1" "$repository_root/target/dx/build/$package/web"
}

# contains 要求每个标记都出现，excludes 要求每个标记都缺席；用于证明 Web 资源只进入对应二进制。
assert_binary_markers() {
	local mode=$1 binary=$2 label=$3 marker found
	shift 3
	for marker in "$@"; do
		found=false
		if LC_ALL=C grep -aFq -- "$marker" "$binary"; then found=true; fi
		if [[ "$mode" == contains && "$found" == false ]]; then
			printf '%s 缺少内嵌标记：%s\n' "$label" "$marker" >&2
			return 1
		fi
		if [[ "$mode" == excludes && "$found" == true ]]; then
			printf '%s 意外包含禁用标记：%s\n' "$label" "$marker" >&2
			return 1
		fi
	done
}

verify_build_info() {
	local root=$1 expected_variant=$2
	local info="$root/BUILD-INFO"
	if [[ ! -f "$info" || -L "$info" ]]; then
		printf '缺少发布构建信息：%s\n' "$info" >&2
		return 1
	fi
	local current_lock current_inputs current_environment
	current_lock=$(sha256_file Cargo.lock) || return 1
	current_inputs=$(build_inputs_fingerprint) || return 1
	current_environment=$(build_env_fingerprint) || return 1
	# source_commit/source_fingerprint 只记录构建当时的来源；复用判定以构建输入为准，
	# 因此“旧提交构建、文档已更新”的制品仍可发布，但代码或迁移变动必然触发重建。
	if [[ $(read_build_info "$info" schema) != auction-release-build/1 ||
	$(read_build_info "$info" variant) != "$expected_variant" ||
	$(read_build_info "$info" build_inputs_sha256) != "$current_inputs" ||
	$(read_build_info "$info" cargo_lock_sha256) != "$current_lock" ||
	$(read_build_info "$info" build_env_sha256) != "$current_environment" ]]; then
		printf '发布构建信息与当前构建输入不一致，请重新构建：%s\n' "$root" >&2
		return 1
	fi
}

verify_dependency_boundaries() {
	local worker_tree public_tree admin_tree
	# 依赖边界必须覆盖实际发布 feature，默认无数据库图会漏查条件依赖。
	worker_tree=$(cargo tree --locked -e normal,build -p auction-worker --no-default-features --features "$runtime_features")
	public_tree=$(cargo tree --locked -e normal,build -p auction-public-api --no-default-features --features "$runtime_features,web-embed")
	admin_tree=$(cargo tree --locked -e normal,build -p auction-admin-api --no-default-features --features "$runtime_features,web-embed")
	if grep -Eq 'auction-web-(public|admin|shared)' <<<"$worker_tree"; then
		printf 'Worker 依赖图不得包含浏览器 crate\n' >&2
		return 1
	fi
	if grep -Fq 'auction-web-admin' <<<"$public_tree"; then
		printf 'Public 依赖图不得包含 Admin Web crate\n' >&2
		return 1
	fi
	if grep -Fq 'auction-web-public' <<<"$admin_tree"; then
		printf 'Admin 依赖图不得包含 Public Web crate\n' >&2
		return 1
	fi
}

verify_artifacts() {
	if [[ -e "$api_only_public_root" || -e "$api_only_admin_root" ]]; then
		printf 'API-only 固定空资源目录不得存在\n' >&2
		return 1
	fi
	verify_artifact_checksums "$api_artifact_root" api-only
	verify_artifact_checksums "$embedded_artifact_root" embedded
	verify_build_info "$api_artifact_root" api-only
	verify_build_info "$embedded_artifact_root" embedded

	local public_web_sha admin_web_sha
	public_web_sha=$(web_payload_sha public)
	admin_web_sha=$(web_payload_sha admin)
	if [[ "$public_web_sha" == "$admin_web_sha" ]]; then
		printf 'Public/Admin Web payload 哈希不得相同\n' >&2
		return 1
	fi
	if [[ $(read_build_info "$embedded_artifact_root/WEB-PUBLIC-MANIFEST.v1" payload_sha256) != "$public_web_sha" ||
	$(read_build_info "$embedded_artifact_root/WEB-ADMIN-MANIFEST.v1" payload_sha256) != "$admin_web_sha" ]]; then
		printf '嵌入构建保存的 Web 标记与当前 payload 不一致\n' >&2
		return 1
	fi

	local binary path
	for path in "${api_only_binaries[@]/#/$api_artifact_root/bin/}" "${binaries[@]/#/$embedded_artifact_root/bin/}"; do
		if [[ ! -f "$path" || -L "$path" || ! -s "$path" || ! -x "$path" ]]; then
			printf '发布二进制无效：%s\n' "$path" >&2
			return 1
		fi
	done

	# 每个 surface 的标记是页面标题加 payload 哈希；资源只能出现在对应的嵌入二进制里。
	local public_markers=('Auction 公开站点' "$public_web_sha")
	local admin_markers=('Auction 管理站点' "$admin_web_sha")
	local api="$api_artifact_root/bin" embedded="$embedded_artifact_root/bin"
	for binary in "${api_only_binaries[@]}"; do
		assert_binary_markers excludes "$api/$binary" "API-only $binary" "${public_markers[@]}" "${admin_markers[@]}"
	done
	assert_binary_markers contains "$api/auction-public-api" 'API-only Public' "$assets_missing_message"
	assert_binary_markers contains "$api/auction-admin-api" 'API-only Admin' "$assets_missing_message"
	assert_binary_markers contains "$embedded/auction-public-api" 'Embedded Public' "${public_markers[@]}"
	assert_binary_markers excludes "$embedded/auction-public-api" 'Embedded Public' "${admin_markers[@]}"
	assert_binary_markers contains "$embedded/auction-admin-api" 'Embedded Admin' "${admin_markers[@]}"
	assert_binary_markers excludes "$embedded/auction-admin-api" 'Embedded Admin' "${public_markers[@]}"
	for binary in auction-worker auction-cli; do
		assert_binary_markers excludes "$embedded/$binary" "Embedded $binary" "${public_markers[@]}" "${admin_markers[@]}"
	done

	if cmp -s \
		"$api_artifact_root/bin/auction-public-api" \
		"$embedded_artifact_root/bin/auction-public-api" ||
		cmp -s \
			"$api_artifact_root/bin/auction-admin-api" \
			"$embedded_artifact_root/bin/auction-admin-api"; then
		printf 'API-only 与嵌入 Web 的 surface 二进制不得相同\n' >&2
		return 1
	fi

	verify_dependency_boundaries
	printf 'API-only、双 surface 嵌入资源、缺失资源 503 标记与 Worker 依赖边界门禁通过。\n'
}

# 可选合并服务：在分体制品与资源隔离证明通过后，构建 auction-server 与独立 auction-cli。
build_single() {
	verify_artifacts
	build_variant single "$single_build_root" "$single_artifact_root" "$single_features" \
		auction-server auction-cli
}

verify_single_artifacts() {
	# 合并包不会跳过分体资源隔离证明。
	verify_artifacts
	verify_artifact_checksums "$single_artifact_root" single
	verify_build_info "$single_artifact_root" single
	local public_sha admin_sha binary server_tree
	public_sha=$(web_payload_sha public)
	admin_sha=$(web_payload_sha admin)
	for binary in auction-server auction-cli; do
		if [[ ! -s "$single_artifact_root/bin/$binary" || ! -x "$single_artifact_root/bin/$binary" ||
		-L "$single_artifact_root/bin/$binary" ]]; then
			printf '合并发布二进制无效：%s\n' "$binary" >&2
			return 1
		fi
	done
	cmp "$single_artifact_root/WEB-PUBLIC-MANIFEST.v1" "$embedded_artifact_root/WEB-PUBLIC-MANIFEST.v1"
	cmp "$single_artifact_root/WEB-ADMIN-MANIFEST.v1" "$embedded_artifact_root/WEB-ADMIN-MANIFEST.v1"
	local markers=('Auction 公开站点' 'Auction 管理站点' "$public_sha" "$admin_sha")
	assert_binary_markers contains "$single_artifact_root/bin/auction-server" 'Single server' "${markers[@]}"
	assert_binary_markers excludes "$single_artifact_root/bin/auction-cli" 'Single auction-cli' "${markers[@]}"
	# 先收集完整依赖图，不能让 grep -q 的提前关闭与 pipefail 掩盖违规。
	server_tree=$(cargo tree --locked -e normal,build -p auction-server --no-default-features --features "$single_features")
	if grep -Fq 'auction-cli v' <<<"$server_tree"; then
		printf 'auction-server 不得依赖 auction-cli\n' >&2
		return 1
	fi
	printf '合并制品双 surface、独立 auction-cli 与分体隔离门禁通过。\n'
}

workspace_version() {
	awk '
    /^\[workspace\.package\]$/ { active = 1; next }
    active && /^\[/ { active = 0 }
    active && /^version[[:space:]]*=/ {
      value = $0
      sub(/^[^=]+=[[:space:]]*"/, "", value)
      sub(/"[[:space:]]*$/, "", value)
      print value
      exit
    }
  ' Cargo.toml
}

first_version_line() {
	"$@" 2>&1 | sed -n '1p'
}

write_dependencies() {
	local destination=$1 commit=$2 epoch=$3 state=$4 fingerprint=$5 version=$6
	local public_web_sha admin_web_sha dependency_list
	public_web_sha=$(read_build_info "$embedded_artifact_root/WEB-PUBLIC-MANIFEST.v1" payload_sha256)
	admin_web_sha=$(read_build_info "$embedded_artifact_root/WEB-ADMIN-MANIFEST.v1" payload_sha256)
	dependency_list=$(mktemp)
	cargo tree --locked --workspace --all-features --prefix none --format '{p}' |
		sed "s# (${repository_root}[^)]*)# (workspace)#" |
		LC_ALL=C sort -u \
			>"$dependency_list"

	{
		printf '# 发布与依赖摘要\n\n'
		printf '## 发布证据\n\n'
		printf -- "- Schema：\`auction-release/1\`\n"
		printf -- "- Source commit：\`%s\`\n" "$commit"
		printf -- "- Application version：\`%s\`\n" "$version"
		printf -- "- Single binary：\`%s\`\n" "${single_binary:-false}"
		printf -- "- Source worktree：\`%s\`\n" "$state"
		printf -- "- Source fingerprint：\`%s\`\n" "$fingerprint"
		printf -- "- SOURCE_DATE_EPOCH：\`%s\`\n" "$epoch"
		printf -- "- Cargo.lock SHA-256：\`%s\`\n" "$(sha256_file Cargo.lock)"
		printf -- "- Public Web payload SHA-256：\`%s\`\n" "$public_web_sha"
		printf -- "- Admin Web payload SHA-256：\`%s\`\n" "$admin_web_sha"
		printf -- "- 数据库发布档位：\`PostgreSQL full\`\n"
		printf -- "- 浏览器交付：\`Public/Admin 独立 RustEmbed\`\n\n"
		printf '## 工具链\n\n'
		printf -- "- \`%s\`\n" "$(first_version_line git --version)"
		printf -- "- \`%s\`\n" "$(first_version_line rustc --version --verbose)"
		printf -- "- \`%s\`\n" "$(first_version_line cargo --version --verbose)"
		printf -- "- \`%s\`\n" "$(first_version_line dx --version)"
		printf -- "- \`%s\`\n" "$(first_version_line tar --version)"
		printf -- "- \`%s\`\n\n" "$(first_version_line gzip --version)"
		printf '## Cargo 解析依赖\n\n'
		while IFS= read -r dependency; do
			printf -- "- \`%s\`\n" "$dependency"
		done <"$dependency_list"
	} >"$destination"
	rm -f "$dependency_list"
	chmod 0644 "$destination"
}

release_sources=()
release_targets=()
release_modes=()

install_release_file() {
	local source=$1 target=$2 mode=$3 package_root=$4
	if [[ ! -f "$source" || -L "$source" ]]; then
		printf 'release source 必须是普通文件：%s\n' "$source" >&2
		return 1
	fi
	mkdir -p "$package_root/$(dirname "$target")"
	install -m "$mode" "$source" "$package_root/$target"
	release_sources+=("$source")
	release_targets+=("$target")
	release_modes+=("$mode")
}

# 批量安装“源路径|归档路径|权限”三元组；用竖线分隔，仓库绝对路径含空白也不会被拆开。
install_release_files() {
	local package_root=$1 entry source target mode
	shift
	for entry in "$@"; do
		IFS='|' read -r source target mode <<<"$entry"
		install_release_file "$source" "$target" "$mode" "$package_root"
	done
}

# 按同一装包清单重定位文档链接；只改目标地址，源码引用文字、文件权限与制品白名单保持不变。
rewrite_packaged_doc_links() {
	local package_root=$1 commit=$2 index
	local mapping=()
	for index in "${!release_sources[@]}"; do
		mapping+=("${release_sources[$index]}" "${release_targets[$index]}")
	done
	perl - "$repository_root" "$package_root" "$commit" "${mapping[@]}" <<'PERL'
use strict;
use warnings;
use Cwd qw(abs_path);
use File::Basename qw(dirname);
use File::Spec::Unix;

my ($repository, $package, $commit, @mapping) = @ARGV;
$repository = abs_path($repository);
$package = abs_path($package);
my (%destinations, @documents);
while (@mapping) {
    my ($source, $target) = splice(@mapping, 0, 2);
    # 生成文件已有包内链接；绝对路径的构建产物不是仓库文档。
    next if $source eq '' || $source =~ m{^/};
    push @documents, [$source, $target] if $source =~ /\.md$/;
    my $resolved = abs_path("$repository/$source") // die "装包源文件不存在：$source\n";
    $destinations{$resolved}{$target} = 1;
    # 目录只有唯一落点才可映射；例如不同变体未打包的模板目录仍转向源码。
    while (($source = dirname($source)) ne '.') {
        $target = dirname($target);
        $destinations{abs_path("$repository/$source")}{$target} = 1;
    }
}

sub rewrite_link {
    my ($link, $source, $target) = @_;
    return $link if $link =~ m{^(?:[A-Za-z][A-Za-z0-9+.-]*:|//|#)};
    my ($path, $suffix) = split /(?=[?#])/, $link, 2;
    return $link if !defined($path) || $path eq '';
    $suffix //= '';
    $path =~ s/%([0-9a-fA-F]{2})/chr(hex($1))/ge;
    my $resolved = abs_path("$repository/" . dirname($source) . "/$path");
    # abs_path 可返回尚不存在的末级路径，仍须单独确认目标存在。
    die "文档链接目标不存在：$source -> $link\n" unless defined($resolved) && -e $resolved;
    die "文档链接越出仓库：$source -> $link\n"
        unless $resolved eq $repository || index($resolved, "$repository/") == 0;
    my @targets = keys %{$destinations{$resolved} // {}};
    my ($destination, $prefix);
    if (@targets == 1) {
        $destination = File::Spec::Unix->abs2rel(
            "$package/$targets[0]", dirname("$package/$target"));
        $prefix = '';
    } else {
        $destination = File::Spec::Unix->abs2rel($resolved, $repository);
        my $kind = -d $resolved ? 'tree' : 'blob';
        $prefix = "https://github.com/ngnlAYY/Auction/$kind/$commit/";
    }
    $destination =~ s{([^A-Za-z0-9._~/\-])}{sprintf('%%%02X', ord($1))}ge;
    return "$prefix$destination$suffix";
}

for my $document (@documents) {
    my ($source, $target) = @$document;
    open my $input, '<:raw', "$package/$target" or die "$target: $!\n";
    my @lines = <$input>;
    close $input or die "$target: $!\n";
    my $fence = '';
    for my $line (@lines) {
        if ($line =~ /^\s{0,3}(`{3,}|~{3,})/) {
            my $marker = substr($1, 0, 1);
            $fence = $fence eq '' ? $marker : ($fence eq $marker ? '' : $fence);
            next;
        }
        next if $fence ne '';
        # 跳过行内代码；反引号位于链接显示文字内时仍由整个链接分支处理。
        $line =~ s{`+[^`\n]*`+(*SKIP)(*F)|(\[[^\]\n]*\]\()([^\s()]+)(\))}
            {$1 . rewrite_link($2, $source, $target) . $3}ge;
        $line =~ s{^(\s{0,3}\[[^\]\n]+\]:\s*<?)([^\s>]+)(>?)}
            {$1 . rewrite_link($2, $source, $target) . $3}e;
    }
    open my $output, '>:raw', "$package/$target" or die "$target: $!\n";
    print {$output} @lines or die "$target: $!\n";
    close $output or die "$target: $!\n";
}
PERL
}

register_generated_file() {
	release_sources+=("")
	release_targets+=("$1")
	release_modes+=("$2")
}

verify_staged_package() {
	local package_root=$1 expected_list actual_list index target mode actual_mode
	expected_list=$(mktemp)
	actual_list=$(mktemp)
	printf '%s\n' "${release_targets[@]}" | LC_ALL=C sort >"$expected_list"
	(
		cd "$package_root"
		find . -type f -printf '%P\n' | LC_ALL=C sort
	) >"$actual_list"
	if ! diff -u "$expected_list" "$actual_list"; then
		rm -f "$expected_list" "$actual_list"
		printf 'release package 文件白名单不一致\n' >&2
		return 1
	fi
	rm -f "$expected_list" "$actual_list"

	if [[ -n $(find "$package_root" -type l -print -quit) ]] ||
		[[ -n $(find "$package_root" ! -type f ! -type d -print -quit) ]]; then
		printf 'release package 禁止符号链接或特殊文件\n' >&2
		return 1
	fi
	for index in "${!release_targets[@]}"; do
		target=${release_targets[$index]}
		mode=${release_modes[$index]}
		actual_mode=$(stat -c '%a' "$package_root/$target")
		if [[ "$actual_mode" != "$mode" ]]; then
			printf 'release package mode 错误：%s=%s want=%s\n' \
				"$target" "$actual_mode" "$mode" >&2
			return 1
		fi
	done
	if [[ -n $(find "$package_root" -type f \
		\( -name '*.log' -o -name '*.log.gz' -o -name '.keyenv' -o -name '*.secret.toml' \) \
		-print -quit) ]]; then
		printf 'release package 含运行日志或真实敏感文件名\n' >&2
		return 1
	fi
}

create_archive() {
	local parent=$1 package_name=$2 destination=$3 epoch=$4
	rm -f "$destination"
	# tar 直接交给 gzip，避免为两次确定性核对各落盘一份未压缩归档。
	LANG=C LC_ALL=C TZ=UTC tar \
		--sort=name \
		--mtime="@$epoch" \
		--owner=0 \
		--group=0 \
		--numeric-owner \
		--format=ustar \
		--create \
		--file - \
		--directory "$parent" \
		"$package_name" | gzip -n -9 >"$destination"
}

verify_archive() {
	local archive=$1 package_name=$2 epoch=$3 extraction_root=$4
	local members duplicates expected_timestamp verbose
	members=$(tar --list --gzip --file "$archive")
	if [[ -z "$members" ]]; then
		printf 'release archive 为空\n' >&2
		return 1
	fi
	while IFS= read -r member; do
		if [[ "$member" == /* || "$member" == *'/../'* || "$member" != "$package_name"/* ]]; then
			printf 'release archive member 路径非法：%s\n' "$member" >&2
			return 1
		fi
	done <<<"$members"
	duplicates=$(printf '%s\n' "$members" | LC_ALL=C sort | uniq -d)
	if [[ -n "$duplicates" ]]; then
		printf 'release archive member 重复：%s\n' "$duplicates" >&2
		return 1
	fi

	verbose=$(LANG=C LC_ALL=C TZ=UTC tar \
		--list --verbose --numeric-owner --full-time --gzip --file "$archive")
	local invalid_verbose
	invalid_verbose=$(grep -Ev '^[dl-][rwx-]{9} 0/0 ' <<<"$verbose" || true)
	if [[ -n "$invalid_verbose" ]]; then
		printf 'release archive owner 或 mode 输出异常\n' >&2
		return 1
	fi
	expected_timestamp=$(date -u -d "@$epoch" '+%Y-%m-%d %H:%M:%S')
	invalid_verbose=$(grep -Fv "$expected_timestamp" <<<"$verbose" || true)
	if [[ -n "$invalid_verbose" ]]; then
		printf 'release archive mtime 未统一为 SOURCE_DATE_EPOCH\n' >&2
		return 1
	fi

	mkdir -p "$extraction_root"
	tar --extract --gzip --file "$archive" --directory "$extraction_root"
	(
		cd "$extraction_root/$package_name"
		sha256sum --check SHA256SUMS >/dev/null
	)
}

package_release() (
	local allow_dirty=false single_binary=false argument
	for argument in "$@"; do
		case "$argument" in
		--allow-dirty) allow_dirty=true ;;
		--single-binary) single_binary=true ;;
		*)
			printf '用法：ci/release/release.sh package [--single-binary] [--allow-dirty]\n' >&2
			return 2
			;;
		esac
	done
	require_command perl
	local selected_root="$embedded_artifact_root" selected_binaries=("${binaries[@]}")
	local package_output_root="$package_output_root" variant_suffix=
	if [[ "$single_binary" == true ]]; then
		verify_single_artifacts
		selected_root="$single_artifact_root"
		selected_binaries=(auction-server auction-cli)
		package_output_root="$repository_root/target/dist/package-single"
		variant_suffix=-single
	else
		verify_artifacts
	fi
	local commit state fingerprint epoch version suffix package_name temporary_root package_root
	commit=$(git rev-parse --verify 'HEAD^{commit}')
	state=$(worktree_state)
	fingerprint=$(source_fingerprint)
	epoch=$(git show -s --format=%ct "$commit")
	version=$(workspace_version)
	if [[ ! "$commit" =~ ^[0-9a-f]{40}$ || ! "$epoch" =~ ^[1-9][0-9]*$ || -z "$version" ]]; then
		printf 'release source identity 无效\n' >&2
		return 1
	fi
	if [[ "$state" == dirty && "$allow_dirty" != true ]]; then
		printf '正式发布要求干净工作树；本地证据可显式传入 --allow-dirty\n' >&2
		return 1
	fi
	suffix=
	if [[ "$state" == dirty ]]; then
		suffix=-dirty
	fi
	package_name="auction-rust-${version}-${commit:0:12}${variant_suffix}${suffix}"
	mkdir -p "$repository_root/target/dist/tmp"
	temporary_root=$(mktemp -d "$repository_root/target/dist/tmp/auction-release.XXXXXX")
	# 子 shell 限定 EXIT trap 的生命周期；任一步骤失败也回收本次装包目录，不触碰旧发布包。
	trap 'rm -rf -- "$temporary_root"; if [[ -n ${publish_next:-} ]]; then rm -rf -- "$publish_next"; fi' EXIT
	package_root="$temporary_root/$package_name"
	mkdir -p "$package_root"

	release_sources=()
	release_targets=()
	release_modes=()
	local binary
	for binary in "${selected_binaries[@]}"; do
		install_release_file \
			"$selected_root/bin/$binary" \
			"bin/$binary" \
			755 \
			"$package_root"
	done

	install_release_file ops/README.md docs/DEPLOYMENT.md 644 "$package_root"
	# 仓库合并安装说明，但保持归档内既有文档路径、权限及白名单。
	printf '# systemd 服务与顺序控制\n\n见[部署手册](../docs/DEPLOYMENT.md#systemd-服务与顺序控制)。\n' \
		>"$temporary_root/SYSTEMD.md"
	install_release_files "$package_root" \
		"ops/deploy/RECOVERY.md|docs/RECOVERY.md|644" \
		"apps/auction-cli/README.md|docs/CLI.md|644" \
		"ops/deploy/retention-policy.v1.json|retention-policy.v1.json|644" \
		"ops/deploy/caddy/Caddyfile|caddy/Caddyfile|644" \
		"$temporary_root/SYSTEMD.md|systemd/README.md|644" \
		"ops/config/README.md|config/README.md|644" \
		"ops/config/examples/common.toml|config/examples/common.toml|640" \
		"ops/config/examples/public.toml|config/examples/public.toml|640" \
		"ops/config/examples/admin.toml|config/examples/admin.toml|640" \
		"ops/config/examples/worker.toml|config/examples/worker.toml|640" \
		"ops/config/examples/runtime/admin.toml|config/examples/runtime/admin.toml|640" \
		"ops/config/examples/secrets.toml|config/examples/secrets.toml.example|600" \
		"ops/config/examples/secrets/postgres-rebuild.toml|config/examples/secrets/postgres-rebuild.toml.example|600" \
		"$embedded_artifact_root/WEB-PUBLIC-SHA256|docs/WEB-PUBLIC-SHA256|644" \
		"$embedded_artifact_root/WEB-ADMIN-SHA256|docs/WEB-ADMIN-SHA256|644" \
		"$embedded_artifact_root/WEB-PUBLIC-MANIFEST.v1|docs/WEB-PUBLIC-MANIFEST.v1|644" \
		"$embedded_artifact_root/WEB-ADMIN-MANIFEST.v1|docs/WEB-ADMIN-MANIFEST.v1|644"
	if [[ "$single_binary" == true ]]; then
		install_release_files "$package_root" \
			"ops/deploy/systemd/auction-server.service.example|systemd/auction-server.service.example|644" \
			"apps/auction-server/README.md|docs/SINGLE-SERVER.md|644" \
			"ops/deploy/caddy/setup.Caddyfile.example|caddy/setup.Caddyfile.example|644"
	else
		install_release_files "$package_root" \
			"ops/deploy/systemd/auction-public-api.service.example|systemd/auction-public-api.service.example|644" \
			"ops/deploy/systemd/auction-admin-api.service.example|systemd/auction-admin-api.service.example|644" \
			"ops/deploy/systemd/auction-worker.service.example|systemd/auction-worker.service.example|644" \
			"ops/deploy/systemd/auction-stack.service.example|systemd/auction-stack.service.example|644" \
			"ops/deploy/tmpfiles/hv-auction.conf.example|tmpfiles/hv-auction.conf.example|644"
	fi
	# 全部仓库文档（含 config/README.md）按安装映射重写；敏感示例的 .example 后缀也来自清单。
	rewrite_packaged_doc_links "$package_root" "$commit"

	write_dependencies \
		"$temporary_root/DEPENDENCIES.md" \
		"$commit" "$epoch" "$state" "$fingerprint" "$version"
	install_release_file "$temporary_root/DEPENDENCIES.md" docs/DEPENDENCIES.md 644 "$package_root"

	{
		printf '%s\n' \
			'schema=auction-release-manifest/1' \
			"package=$package_name" \
			"source_commit=$commit" \
			"source_fingerprint=$fingerprint" \
			'files='
		local index
		for index in "${!release_targets[@]}"; do
			printf '%s %s\n' "${release_modes[$index]}" "${release_targets[$index]}"
		done | LC_ALL=C sort
		printf '644 RELEASE-MANIFEST.v1\n'
		printf '644 SHA256SUMS\n'
	} >"$package_root/RELEASE-MANIFEST.v1"
	chmod 0644 "$package_root/RELEASE-MANIFEST.v1"
	register_generated_file RELEASE-MANIFEST.v1 644

	(
		cd "$package_root"
		printf '%s\0' "${release_targets[@]}" |
			LC_ALL=C sort -z |
			xargs -0 -r sha256sum
	) >"$package_root/SHA256SUMS"
	chmod 0644 "$package_root/SHA256SUMS"
	register_generated_file SHA256SUMS 644

	find "$package_root" -type d -exec chmod 0755 {} +
	find "$package_root" -exec touch -d "@$epoch" {} +
	verify_staged_package "$package_root"

	local first_archive="$temporary_root/first.tar.gz"
	local archive="$temporary_root/$package_name.tar.gz"
	create_archive "$temporary_root" "$package_name" "$first_archive" "$epoch"
	create_archive "$temporary_root" "$package_name" "$archive" "$epoch"
	local first_archive_sha archive_sha
	first_archive_sha=$(sha256_file "$first_archive") || return 1
	archive_sha=$(sha256_file "$archive") || return 1
	if [[ "$first_archive_sha" != "$archive_sha" ]]; then
		printf '同一白名单的两次 release archive 不一致\n' >&2
		return 1
	fi
	verify_archive "$archive" "$package_name" "$epoch" "$temporary_root/extracted"

	local publish_next="$repository_root/target/dist/tmp/.package.next.$$"
	rm -rf "$publish_next"
	mkdir -p "$publish_next"
	install -m 0644 "$archive" "$publish_next/$package_name.tar.gz"
	install -m 0644 "$temporary_root/DEPENDENCIES.md" "$publish_next/DEPENDENCIES.md"
	(
		cd "$publish_next"
		sha256sum DEPENDENCIES.md "$package_name.tar.gz"
	) >"$publish_next/SHA256SUMS"
	chmod 0644 "$publish_next/SHA256SUMS"
	# 文档与配置也属于源码身份；装包期间发生变化时保留上一份发布目录，拒绝发布混合快照。
	local final_commit final_fingerprint
	final_commit=$(git rev-parse --verify 'HEAD^{commit}') || return 1
	final_fingerprint=$(source_fingerprint) || return 1
	if [[ "$final_commit" != "$commit" || "$final_fingerprint" != "$fingerprint" ]]; then
		printf '装包期间源码发生变化，请重新构建并装包\n' >&2
		return 1
	fi
	replace_generated_directory "$publish_next" "$package_output_root"

	printf 'release package 检查通过：commit=%s state=%s fingerprint=%s\n' \
		"$commit" "$state" "$fingerprint"
	printf 'archive=%s sha256=%s\n' \
		"$package_output_root/$package_name.tar.gz" \
		"$(sha256_file "$package_output_root/$package_name.tar.gz")"
	printf 'publication_sha256=%s\n' "$(sha256_file "$package_output_root/SHA256SUMS")"
)

# 完整发布：先校验参数，不能在拼写错误时先启动大规模构建。
release_check() {
	local single=false argument
	for argument in "$@"; do
		case "$argument" in
		--single-binary) single=true ;;
		--allow-dirty) ;;
		*)
			printf '未知发布参数：%s\n' "$argument" >&2
			return 2
			;;
		esac
	done
	build_variant api-only "$api_build_root" "$api_artifact_root" "$api_only_features" "${api_only_binaries[@]}"
	build_variant embedded "$embedded_build_root" "$embedded_artifact_root" "$embedded_features"
	if [[ "$single" == true ]]; then build_single; fi
	package_release "$@"
}

usage() {
	printf '%s\n' \
		'用法：ci/release/release.sh release-check [--single-binary] [--allow-dirty]' \
		'      ci/release/release.sh package [--single-binary] [--allow-dirty]' \
		'      ci/release/release.sh build-api | build-embedded | verify-artifacts' \
		'      ci/release/release.sh build-single | verify-single' \
		'      ci/release/release.sh source-fingerprint' >&2
}

command_name=${1:-}
if [[ $# -gt 0 ]]; then shift; fi
case "$command_name" in
release-check) release_check "$@" ;;
package) package_release "$@" ;;
build-api | build-embedded | build-single | verify-single | verify-artifacts | source-fingerprint)
	if [[ $# -ne 0 ]]; then
		printf '%s 不接受额外参数\n' "$command_name" >&2
		exit 2
	fi
	case "$command_name" in
	build-api) build_variant api-only "$api_build_root" "$api_artifact_root" "$api_only_features" "${api_only_binaries[@]}" ;;
	# 在同一源码身份约束内重建双端 Web 和 native 制品。
	build-embedded) build_variant embedded "$embedded_build_root" "$embedded_artifact_root" "$embedded_features" ;;
	build-single) build_single ;;
	verify-single) verify_single_artifacts ;;
	verify-artifacts) verify_artifacts ;;
	# 只输出源码身份指纹，供 CI 用作制品缓存键或人工核对复用判定；不读写产物目录。
	source-fingerprint) source_fingerprint ;;
	esac
	;;
*)
	usage
	exit 2
	;;
esac
