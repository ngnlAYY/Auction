#!/usr/bin/env bash
# 构建工具的受控来源、独立锁图与安装回执；本地和 Actions 共用，生成文件只进入 target。
set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
cd "$repository_root"
profile=${2:-release}
case "$profile" in
release) tools_root="$repository_root/target/build-tools" ;;
audit) tools_root="$repository_root/target/audit-tools" ;;
*) printf '未知工具档位：%s\n' "$profile" >&2; exit 2 ;;
esac
work_root="$repository_root/target/secure-build-tools"
target_triple=x86_64-unknown-linux-gnu

fail() {
	printf '构建工具校验失败：%s\n' "$*" >&2
	exit 1
}

check_platform() {
	[[ $(uname -s) == Linux && $(uname -m) == x86_64 ]] || fail '仅支持 Linux x86_64'
}

# 只更新已审阅的独立工具锁；正常安装与发布禁止在线重新解析依赖。
# 0.26.11 是仅转发至 1.x 的兼容包装，底层根证书仍必须精确锁定到修复版。
check_tls_lock() {
	local lock=$1
	awk '
    /^name = / { name=$3; gsub(/"/, "", name) }
    /^version = / {
      version=$3; gsub(/"/, "", version)
      if (name == "rustls") { rustls++; if (version != "0.23.46") bad=1 }
      if (name == "rustls-webpki") { webpki++; if (version != "0.103.16") bad=1 }
      if (name == "webpki-roots") {
        roots++
        if (version != "1.0.10" && version != "0.26.11") bad=1
      }
      if (name == "webpki-root-certs") { roots++; if (version != "1.0.10") bad=1 }
    }
    END { exit(bad || !rustls || !webpki || !roots) }
  ' "$lock" || fail "$lock 的 TLS 依赖未达到已审阅基线"
}

recipe_fingerprint() {
	{
		sha256sum ci/tools/bootstrap.sh rust-toolchain.toml
		if [[ "$profile" == audit ]]; then
			sha256sum ci/tools/locks/cargo-audit.Cargo.lock
		else
			sha256sum ci/tools/locks/dioxus-cli.Cargo.lock ci/tools/locks/sccache.Cargo.lock
			sha256sum ci/tools/patches/dioxus-cli-controlled-tools.patch \
				ci/tools/patches/sccache-zstd.patch ci/tools/patches/sccache-cache-permissions.patch \
				ci/tools/patches/sccache-object-features.patch
		fi
		rustc --version --verbose
		printf '%s\n' "$target_triple" "$profile" 'dx:disable-telemetry,no-downloads;sccache:gha;cargo-audit:lockfile-only'
	} | sha256sum | awk '{print $1}'
}

verify_install() {
	local install_root=${1:-$tools_root}
	local receipt="$install_root/BUILD-INFO" name
	local binaries=(dx sccache esbuild wasm-bindgen wasm-opt)
	if [[ "$profile" == audit ]]; then binaries=(cargo-audit); fi
	[[ -f "$receipt" && ! -L "$receipt" && -f "$install_root/SHA256SUMS" ]] || return 1
	[[ $(sed -n 's/^schema=//p' "$receipt") == auction-build-tools/1 ]] || return 1
	[[ $(sed -n 's/^profile=//p' "$receipt") == "$profile" ]] || return 1
	[[ $(sed -n 's/^recipe_sha256=//p' "$receipt") == "$(recipe_fingerprint)" ]] || return 1
	for name in "${binaries[@]}"; do
		[[ -f "$install_root/bin/$name" && ! -L "$install_root/bin/$name" && -x "$install_root/bin/$name" ]] || return 1
	done
	# 清单精确覆盖本档的全部可执行文件，避免遗漏校验条目或夹带额外文件。
	[[ $(awk '{print $2}' "$install_root/SHA256SUMS") == "$(printf 'bin/%s\n' "${binaries[@]}" | LC_ALL=C sort)" ]] || return 1
	[[ $(find "$install_root/bin" "$install_root/lib" -mindepth 1 -printf '%P\n' | LC_ALL=C sort) == "$(printf '%s\n' "${binaries[@]}" | LC_ALL=C sort)" ]] || return 1
	# 清单本身由回执绑定，不能通过只替换二进制与清单掩盖缓存损坏。
	[[ $(sed -n 's/^manifest_sha256=//p' "$receipt") == "$(sha256sum "$install_root/SHA256SUMS" | awk '{print $1}')" ]] || return 1
	(cd "$install_root" && sha256sum --check --status SHA256SUMS)
}

# 下载文件先在同目录临时落盘，核验成功才替换缓存；缓存命中也执行固定哈希校验。
download_verified() {
	local url=$1 digest=$2 destination=$3 temporary
	if [[ -f "$destination" ]] && printf '%s  %s\n' "$digest" "$destination" | sha256sum --check --status; then
		return
	fi
	temporary=$(mktemp "$destination.next.XXXXXX")
	if ! curl -q --proto '=https' --proto-redir '=https' --tlsv1.2 --fail --silent --show-error --location --retry 3 \
		--connect-timeout 10 --max-time 300 "$url" --output "$temporary" ||
		! printf '%s  %s\n' "$digest" "$temporary" | sha256sum --check --status; then
		rm -f "$temporary"
		fail "下载或哈希不匹配：$url"
	fi
	mv "$temporary" "$destination"
}

prepare_source() {
	local package=$1 version=$2 digest=$3
	local index
	local source_patches=() patch_sha256s=()
	local archive="$work_root/$package-$version.crate"
	local source_root="$work_root/$package-$version"
	download_verified "https://static.crates.io/crates/$package/$package-$version.crate" "$digest" "$archive"
	# 精确删除本脚本拥有的解包目录；不接收任意路径或清理 Cargo 的共享缓存。
	rm -rf "$source_root"
	tar --extract --gzip --file "$archive" --directory "$work_root" --no-same-owner
	# crates.io 的发布包是独立包，在本仓库 target 下构建时隔离父工作区。
	printf '\n[workspace]\n' >>"$source_root/Cargo.toml"
	cp "ci/tools/locks/$package.Cargo.lock" "$source_root/Cargo.lock"
	case "$package" in
	dioxus-cli)
		source_patches=(ci/tools/patches/dioxus-cli-controlled-tools.patch)
		patch_sha256s=(6d27b8a443065017e3734a9f30ea63a5adaa3bd4ffa6a332e20d81e3420fc00e)
		;;
	sccache)
		source_patches=(ci/tools/patches/sccache-zstd.patch ci/tools/patches/sccache-cache-permissions.patch
			ci/tools/patches/sccache-object-features.patch)
		patch_sha256s=(6e991aab616fe71817ff9c401dcd393992c4587b55e4ec208804864abb33f2f4
			5849df23e64bb01f9980f855a33b947031552cd219f7042d53aadac85e5403a6
			8750b194884284ebf7b636ce2a2b5e6821c985e96e8d30398ecf9d6113f5c856)
		;;
	esac
	for index in "${!source_patches[@]}"; do
		# 固定原始 crate 和补丁双重身份；零模糊匹配，来源漂移即停止。
		printf '%s  %s\n' "${patch_sha256s[$index]}" "${source_patches[$index]}" |
			sha256sum --check --status || fail "$package 补丁身份不匹配"
		patch --batch --forward --fuzz=0 --strip=1 --directory "$source_root" <"${source_patches[$index]}"
	done
}

build_tool() (
	local package=$1 version=$2 binary=$3 features=$4 name
	# 工具采用独立 profile，不能继承应用的 CPU/profile/wrapper 参数或把自身套入 sccache。
	while IFS= read -r name; do
		case "$name" in
		CARGO_PROFILE_* | CARGO_BUILD_RUSTC | CARGO_BUILD_RUSTC_WRAPPER | CARGO_BUILD_RUSTC_WORKSPACE_WRAPPER | CARGO_BUILD_INCREMENTAL | CARGO_INCREMENTAL | CARGO_BUILD_RUSTFLAGS | CARGO_ENCODED_RUSTFLAGS | CARGO_BUILD_TARGET | CARGO_TARGET_*_RUSTFLAGS | CARGO_TARGET_*_LINKER | RUSTFLAGS | RUSTC | RUSTC_WRAPPER | RUSTC_WORKSPACE_WRAPPER)
			unset "$name"
			;;
		esac
	done < <(compgen -e)
	export CARGO_PROFILE_RELEASE_DEBUG=0 CARGO_PROFILE_RELEASE_STRIP=symbols
	export CARGO_PROFILE_RELEASE_LTO=false CARGO_PROFILE_RELEASE_CODEGEN_UNITS=16
	# 发布包源身份固定为上游提交，不能误读父 Auction 仓库的 .git。
	export DIOXUS_CLI_GIT_SHA=57d6794ad60b949e5bd8aa282f6f8c3dc97a365e DIOXUS_CLI_GIT_SHA_SHORT=57d6794
	cargo build --manifest-path "$work_root/$package-$version/Cargo.toml" \
		--locked --release --bin "$binary" --no-default-features --features "$features" \
		--jobs 2 --target "$target_triple" --target-dir "$work_root/build/$binary"
	install -m 0755 "$work_root/build/$binary/$target_triple/release/$binary" "$next_root/bin/$binary"
)

install_tools() {
	local required archive backup expected_recipe package version
	expected_recipe=$(recipe_fingerprint)
	if verify_install; then
		printf '构建工具已校验：%s\n' "$tools_root"
		return
	fi
	for required in cargo rustc curl tar sha256sum install cmake pkg-config cc patch; do
		command -v "$required" >/dev/null || fail "缺少命令 $required"
	done
	mkdir -p "$work_root" "$repository_root/target"
	next_root=$(mktemp -d "$tools_root.next.XXXXXX")
	trap 'rm -rf "$next_root"' EXIT
	mkdir -p "$next_root/bin" "$next_root/lib"

	if [[ "$profile" == audit ]]; then
		prepare_source cargo-audit 0.22.2 700c2b240f7fd330c24b675fe429f73a5b676531fcc6300400b2b67f155ba12a
		# 只审计 Cargo.lock，不编译未使用的 ELF/PE/WASM 二进制扫描器。
		build_tool cargo-audit 0.22.2 cargo-audit ''
	else
		prepare_source dioxus-cli 0.7.10 1fcb6578719380a2094f115bbbde340c59b65d2bf1987b8ddbbb9834db6716e6
		build_tool dioxus-cli 0.7.10 dx disable-telemetry,no-downloads
		prepare_source sccache 0.18.0 ea97b1561fe9f059860e1631e41feecfa81117bcc7bf91c895b19f24a3187538
		build_tool sccache 0.18.0 sccache gha

		# dx 禁止自行下载；配套工具由同一入口固定来源和哈希后提供。
		archive="$work_root/esbuild-0.27.3.tgz"
		download_verified 'https://registry.npmjs.org/@esbuild/linux-x64/-/linux-x64-0.27.3.tgz' \
			066e20cdb882994160e18524a552b97e03648eb9aa0c7cdf5680a6493be65ab2 "$archive"
		tar --extract --gzip --file "$archive" --to-stdout package/bin/esbuild >"$next_root/bin/esbuild"
		archive="$work_root/wasm-bindgen-0.2.129.tar.gz"
		download_verified 'https://github.com/wasm-bindgen/wasm-bindgen/releases/download/0.2.129/wasm-bindgen-0.2.129-x86_64-unknown-linux-musl.tar.gz' \
			82d12bb940e2d4e72e0d5605387fc1b8ca179044e012b620f0ce4e7440e8320e "$archive"
		tar --extract --gzip --file "$archive" --to-stdout \
			wasm-bindgen-0.2.129-x86_64-unknown-linux-musl/wasm-bindgen >"$next_root/bin/wasm-bindgen"
		archive="$work_root/binaryen-129.tar.gz"
		download_verified 'https://github.com/WebAssembly/binaryen/releases/download/version_129/binaryen-version_129-x86_64-linux.tar.gz' \
			50b9fa62b9abea752da92ec57e0c555fee578760cd237c40107957715d2976ba "$archive"
		tar --extract --gzip --file "$archive" --directory "$next_root" --no-same-owner --strip-components=1 \
			binaryen-version_129/bin/wasm-opt
	fi
	chmod 0755 "$next_root/bin/"*
	# 编译期间配方变化不能把旧产物标成新锁图；保留增量缓存，拒绝签发混合身份回执。
	[[ $(recipe_fingerprint) == "$expected_recipe" ]] || fail '编译期间工具配方已变化，请重新安装'
	if [[ "$profile" == audit ]]; then
		cmp ci/tools/locks/cargo-audit.Cargo.lock "$work_root/cargo-audit-0.22.2/Cargo.lock" || fail 'cargo-audit 构建锁发生变化'
	else
		for package in dioxus-cli sccache; do
			case "$package" in dioxus-cli) version=0.7.10 ;; sccache) version=0.18.0 ;; esac
			cmp "ci/tools/locks/$package.Cargo.lock" "$work_root/$package-$version/Cargo.lock" || fail "$package 构建锁发生变化"
		done
	fi
	(cd "$next_root" && find bin lib -type f -print0 | LC_ALL=C sort -z | xargs -0 sha256sum >SHA256SUMS)
	printf '%s\n' 'schema=auction-build-tools/1' "recipe_sha256=$expected_recipe" \
		"manifest_sha256=$(sha256sum "$next_root/SHA256SUMS" | awk '{print $1}')" \
		"profile=$profile" 'repair=auction-tls-20261022' >"$next_root/BUILD-INFO"
	verify_install "$next_root" || fail '候选工具回执验证失败'
	backup="$tools_root.previous.$$"
	if [[ -e "$tools_root" ]]; then mv "$tools_root" "$backup"; fi
	if ! mv "$next_root" "$tools_root"; then
		if [[ -e "$backup" ]]; then mv "$backup" "$tools_root"; fi
		fail '无法原子替换构建工具目录'
	fi
	if ! verify_install; then
		rm -rf "$tools_root"
		if [[ -e "$backup" ]]; then mv "$backup" "$tools_root"; fi
		fail '安装后回执验证失败，已恢复原目录'
	fi
	rm -rf "$backup"
	trap - EXIT
	printf '构建工具安装完成：%s\n' "$tools_root"
}

check_platform
if [[ "$profile" == audit ]]; then
	check_tls_lock ci/tools/locks/cargo-audit.Cargo.lock
else
	check_tls_lock ci/tools/locks/dioxus-cli.Cargo.lock
	check_tls_lock ci/tools/locks/sccache.Cargo.lock
fi
case "${1:-install}" in
install) install_tools ;;
verify) verify_install || fail "请先运行 bash ci/tools/bootstrap.sh install $profile" ;;
fingerprint) recipe_fingerprint ;;
identity)
	verify_install || fail '工具回执缺失或不匹配，请重新安装'
	cat "$tools_root/BUILD-INFO" "$tools_root/SHA256SUMS" | sha256sum | awk '{print $1}'
	;;
*) fail '用法：bootstrap.sh [install|verify|fingerprint|identity] [release|audit]' ;;
esac
