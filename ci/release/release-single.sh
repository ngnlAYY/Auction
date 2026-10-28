#!/usr/bin/env bash
# 由 release.sh 加载；必需变量使用失败关闭的展开，不能独立绕过发布门禁。

build_single() {
	verify_artifacts
	build_variant single "${single_build_root:?}" "${single_artifact_root:?}" \
		"${single_features:?}" auction-server auctionctl
}

verify_single_artifacts() {
	# 合并包不会跳过分体资源隔离证明。
	verify_artifacts
	local root=${single_artifact_root:?} embedded=${embedded_artifact_root:?}
	verify_artifact_checksums "$root" single
	verify_build_info "$root" single
	local public_sha admin_sha binary marker server_tree
	public_sha=$(verify_web_root public)
	admin_sha=$(verify_web_root admin)
	for binary in auction-server auctionctl; do
		if [[ ! -s "$root/bin/$binary" || ! -x "$root/bin/$binary" || -L "$root/bin/$binary" ]]; then
			printf '合并发布二进制无效：%s\n' "$binary" >&2
			return 1
		fi
	done
	cmp "$root/WEB-PUBLIC-MANIFEST.v1" "$embedded/WEB-PUBLIC-MANIFEST.v1"
	cmp "$root/WEB-ADMIN-MANIFEST.v1" "$embedded/WEB-ADMIN-MANIFEST.v1"
	for marker in 'Auction 公开站点' 'Auction 管理站点' "$public_sha" "$admin_sha"; do
		assert_binary_contains "$root/bin/auction-server" "$marker" 'Single server'
		assert_binary_excludes "$root/bin/auctionctl" "$marker" 'Single auctionctl'
	done
	# 先收集完整依赖图，不能让 grep -q 的提前关闭与 pipefail 掩盖违规。
	server_tree=$(cargo tree --locked -e normal,build -p auction-server --no-default-features --features "${single_features:?}")
	if grep -Fq 'auctionctl v' <<<"$server_tree"; then
		printf 'auction-server 不得依赖 auctionctl\n' >&2
		return 1
	fi
	printf '合并制品双 surface、独立 auctionctl 与分体隔离门禁通过。\n'
}

release_check() {
	local single=false arg
	# 先验证参数，不能在拼写错误时先启动大规模构建。
	for arg in "$@"; do
		case "$arg" in
		--single-binary) single=true ;;
		--allow-dirty) ;;
		*)
			printf '未知发布参数：%s\n' "$arg" >&2
			return 2
			;;
		esac
	done
	build_variant api-only "${api_build_root:?}" "${api_artifact_root:?}" "${api_only_features:?}"
	build_variant embedded "${embedded_build_root:?}" "${embedded_artifact_root:?}" \
		"${embedded_features:?}"
	if [[ "$single" == true ]]; then build_single; fi
	package_release "$@"
}
