#!/usr/bin/env bash
# 部署制品来源校验：只接受 GitHub Release 上的 CI 制品，并要求其源码提交可从发布 tag 到达。
# 校验内容：外层 SHA256SUMS、归档成员路径与类型、包内 SHA256SUMS、RELEASE-MANIFEST.v1 与
# DEPENDENCIES.md 的源码身份、发布 tag 可达性，以及（默认）GitHub 构建来源证明。
# 该脚本不安装程序、不改数据库、不联网以外的行为；--offline 只用于离线核对，不能作为部署依据。

set -euo pipefail

script_root=$(cd "$(dirname "$0")/../.." && pwd)
repository_root=$script_root
archive=
tag=
release_repo=${GITHUB_REPOSITORY:-}
offline=false

usage() {
	printf '%s\n' \
		'用法：ci/release/verify-deployment-package.sh <auction-rust-*.tar.gz> [选项]' \
		'  --tag <v版本>        要求从该发布 tag 到达构建提交（默认选择包含提交的发布 tag）' \
		'  --repo <路径>        用于核对 tag 与提交的 Git 仓库（默认本仓库）' \
		'  --release-repo <owner/name>  GitHub 仓库（默认 $GITHUB_REPOSITORY 或 origin）' \
		'  --offline            跳过 GitHub 来源证明校验；仅用于离线核对' \
		'  -h, --help           显示帮助' >&2
}

fail() {
	printf '部署制品校验失败：%s\n' "$*" >&2
	exit 1
}

while [[ $# -gt 0 ]]; do
	case "$1" in
	--tag)
		[[ $# -ge 2 && -n $2 ]] || fail '--tag 需要发布 tag'
		tag=$2
		shift 2
		;;
	--repo)
		[[ $# -ge 2 && -n $2 ]] || fail '--repo 需要仓库路径'
		repository_root=$2
		shift 2
		;;
	--release-repo)
		[[ $# -ge 2 && -n $2 ]] || fail '--release-repo 需要 owner/name'
		release_repo=$2
		shift 2
		;;
	--offline) offline=true; shift ;;
	-h | --help) usage; exit 0 ;;
	-*) usage; fail "未知参数：$1" ;;
	*)
		[[ -z $archive ]] || fail "只接受一个归档参数：$1"
		archive=$1
		shift
		;;
	esac
done

[[ -n $archive ]] || { usage; fail '缺少发布归档路径'; }
[[ -f $archive && ! -L $archive ]] || fail "发布归档必须是普通文件：$archive"
archive=$(cd "$(dirname "$archive")" && pwd)/$(basename "$archive")
package_dir=$(dirname "$archive")
package_archive=$(basename "$archive")
checksums="$package_dir/SHA256SUMS"
dependencies="$package_dir/DEPENDENCIES.md"
[[ -f $checksums && ! -L $checksums ]] || fail "缺少外层校验清单：$checksums"
[[ -f $dependencies && ! -L $dependencies ]] || fail "缺少发布摘要：$dependencies"

base=${package_archive%.tar.gz}
[[ $base != "$package_archive" ]] || fail '归档必须使用 .tar.gz 后缀'
[[ $base != *-dirty ]] || fail '拒绝部署带 -dirty 后缀的本地制品'
[[ $base =~ ^auction-rust-(.+)-([0-9a-f]{12})(-single)?$ ]] ||
	fail "归档名不符合 auction-rust-<版本>-<提交前12位>[-single].tar.gz：$package_archive"
short_commit=${BASH_REMATCH[2]}

# 外层清单只应描述本次发布的两个文件；用清单本身校验，不信任文件名。
(
	cd "$package_dir"
	sha256sum --check --strict SHA256SUMS >/dev/null
) || fail '外层 SHA256SUMS 校验失败'
grep -Eq "^[0-9a-f]{64}  ${package_archive//./\\.}$" "$checksums" ||
	fail "外层 SHA256SUMS 未包含 $package_archive"
grep -Eq '^[0-9a-f]{64}  DEPENDENCIES\.md$' "$checksums" ||
	fail '外层 SHA256SUMS 未包含 DEPENDENCIES.md'

temporary_root=$(mktemp -d)
trap 'rm -rf -- "$temporary_root"' EXIT

# 先核对归档成员，避免解包路径穿越、符号链接或特殊文件。
members=$(tar --list --gzip --file "$archive")
[[ -n $members ]] || fail '归档为空'
while IFS= read -r member; do
	[[ $member == "$base" || $member == "$base"/* ]] ||
		fail "归档成员不在 $base/ 下：$member"
	case "/$member/" in
	*/../* | */./*) fail "归档成员包含相对路径：$member" ;;
	esac
done <<<"$members"
verbose=$(tar --list --verbose --gzip --file "$archive")
if awk '{ if (substr($1, 1, 1) != "-" && substr($1, 1, 1) != "d") bad = 1 } END { exit bad }' <<<"$verbose"; then
	:
else
	fail '归档包含符号链接或其他特殊文件'
fi

tar --extract --gzip --file "$archive" --directory "$temporary_root" --no-same-owner
package_root="$temporary_root/$base"
[[ -d $package_root ]] || fail "归档缺少顶层目录：$base"
(
	cd "$package_root"
	sha256sum --check --strict SHA256SUMS >/dev/null
) || fail '包内 SHA256SUMS 校验失败'

manifest="$package_root/RELEASE-MANIFEST.v1"
[[ -f $manifest && ! -L $manifest ]] || fail '归档缺少 RELEASE-MANIFEST.v1'
read_build_info() {
	awk -F= -v wanted="$1" '$1 == wanted { print substr($0, length($1) + 2); exit }' "$2"
}
[[ $(read_build_info schema "$manifest") == auction-release-manifest/1 ]] ||
	fail 'RELEASE-MANIFEST.v1 的 schema 不受支持'
[[ $(read_build_info package "$manifest") == "$base" ]] ||
	fail 'RELEASE-MANIFEST.v1 的 package 与归档名不一致'
commit=$(read_build_info source_commit "$manifest")
fingerprint=$(read_build_info source_fingerprint "$manifest")
[[ $commit =~ ^[0-9a-f]{40}$ ]] || fail 'RELEASE-MANIFEST.v1 的 source_commit 无效'
[[ $fingerprint =~ ^[0-9a-f]{64}$ ]] || fail 'RELEASE-MANIFEST.v1 的 source_fingerprint 无效'
[[ ${commit:0:12} == "$short_commit" ]] ||
	fail "归档名提交 ${short_commit} 与清单提交 $commit 不一致"

# 发布摘要装在包内 docs/（与 RELEASE-MANIFEST.v1 白名单、auction-cli 制品的
# docs/DEPENDENCIES.md 一致）；外层副本只用于比对，不是包内路径。
packaged_dependencies="$package_root/docs/DEPENDENCIES.md"
[[ -f $packaged_dependencies && ! -L $packaged_dependencies ]] ||
	fail '归档缺少 docs/DEPENDENCIES.md'
grep -Fq -- "- Source commit：\`$commit\`" "$packaged_dependencies" ||
	fail 'DEPENDENCIES.md 的 Source commit 与清单不一致'
grep -Fq -- '- Source worktree：`clean`' "$packaged_dependencies" ||
	fail '拒绝部署工作树不干净时生成的制品'
cmp -s "$dependencies" "$packaged_dependencies" ||
	fail '外层 DEPENDENCIES.md 与包内文件不一致'

# 提交必须存在于本地 Git 历史，并且可从发布 tag 到达。
git -C "$repository_root" rev-parse --git-dir >/dev/null 2>&1 ||
	fail "不是 Git 仓库：$repository_root"
git -C "$repository_root" cat-file -e "$commit^{commit}" 2>/dev/null ||
	fail "本地仓库缺少构建提交 $commit；先执行 git fetch --tags"
mapfile -t release_tags < <(
	git -C "$repository_root" tag --list 'v[0-9]*' | LC_ALL=C sort -V || true
)
[[ ${#release_tags[@]} -gt 0 ]] || fail '本地没有发布 tag；先执行 git fetch --tags'
if [[ -n $tag ]]; then
	[[ $tag =~ ^v[0-9] ]] || fail "不是发布 tag：$tag"
	printf '%s\n' "${release_tags[@]}" | grep -Fxq -- "$tag" ||
		fail "本地不存在发布 tag：$tag"
	git -C "$repository_root" merge-base --is-ancestor "$commit" "$tag" ||
		fail "构建提交 $commit 不能从发布 tag $tag 到达"
else
	exact=
	while IFS= read -r candidate; do
		[[ -n $candidate ]] || continue
		if [[ $(git -C "$repository_root" rev-parse --verify --quiet "$candidate^{commit}") == "$commit" ]]; then
			exact=$candidate
			break
		fi
	done < <(git -C "$repository_root" tag --list 'v[0-9]*' --points-at "$commit")
	if [[ -n $exact ]]; then
		tag=$exact
	else
		while IFS= read -r candidate; do
			[[ -n $candidate ]] || continue
			if git -C "$repository_root" merge-base --is-ancestor "$commit" "$candidate"; then
				tag=$candidate
			fi
		done <<<"$(printf '%s\n' "${release_tags[@]}")"
	fi
	[[ -n $tag ]] || fail "构建提交 $commit 不能从任何发布 tag 到达"
fi

# 来源证明：绑定发布仓库、发布工作流、tag 与构建提交。默认失败关闭，--offline 才跳过。
provenance=skipped
if [[ $offline != true ]]; then
	command -v gh >/dev/null 2>&1 || fail '缺少 gh；无法校验来源证明（离线核对请显式使用 --offline）'
	if [[ -z $release_repo ]]; then
		origin=$(git -C "$repository_root" remote get-url origin 2>/dev/null || true)
		release_repo=$(sed -E 's#^(https://|ssh://git@|git@)github\.com[:/]##; s#\.git$##' <<<"$origin")
		[[ -n $release_repo ]] || fail '无法从 origin 推断 GitHub 仓库；传入 --release-repo'
	fi
	[[ $release_repo =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] ||
		fail "GitHub 仓库格式无效：$release_repo"
	for attempt in 1 2 3; do
		verified=true
		for subject in "$archive" "$checksums" "$dependencies"; do
			if ! gh attestation verify "$subject" \
				--repo "$release_repo" \
				--signer-workflow "$release_repo/.github/workflows/release.yml" \
				--source-ref "refs/tags/$tag" \
				--source-digest "$commit" >/dev/null 2>"$temporary_root/attestation.err"; then
				verified=false
				break
			fi
		done
		if [[ $verified == true ]]; then break; fi
		if [[ $attempt -eq 3 ]]; then
			# gh 的错误只含 API 状态与证明不匹配原因，不含令牌；只回显前几行便于定位。
			head -n 5 "$temporary_root/attestation.err" >&2
			fail "来源证明校验失败（仓库 $release_repo，tag $tag，提交 $commit）"
		fi
		sleep 5
	done
	provenance=verified
else
	printf '%s\n' '警告：已跳过 GitHub 来源证明校验（--offline）；仅用于离线核对，不能作为部署依据。' >&2
fi

printf '部署制品校验通过\n'
printf 'archive=%s\n' "$package_archive"
printf 'sha256=%s\n' "$(sha256sum "$archive" | awk '{print $1}')"
printf 'commit=%s\n' "$commit"
printf 'tag=%s\n' "$tag"
printf 'provenance=%s\n' "$provenance"
