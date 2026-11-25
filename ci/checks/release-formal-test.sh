#!/usr/bin/env bash
# shellcheck disable=SC1091,SC2030,SC2031,SC2034
# 验证正式发行改为双 Solid：四类调用身份（正式 tag push、tag 手工候选、分支候选、只读查询）、
# 正式身份断言的反例，以及 docs/WEB-STACK.v1 与页面导出树的装包、核对与拒绝。
# 全部在临时目录与环境变量夹具中运行：不联网、不构建、不连接任何账号。

set -euo pipefail
cd "$(dirname "$0")/../.."
repository_root=$PWD
unset AUCTION_WEB_STACK GITHUB_ACTIONS GITHUB_EVENT_NAME GITHUB_REF_TYPE GITHUB_REF_NAME \
	GITHUB_REPOSITORY GITHUB_WORKFLOW_REF

fail() {
	printf '正式发行双 Solid 回归失败：%s\n' "$1" >&2
	exit 1
}

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT
version=$(bash -c 'source ci/release/build-identity.sh; workspace_version')
[[ -n $version ]] || fail '无法读取工作区版本'

# 正式环境：release.yml 的 tag push；各反例只改一项。
formal_env=(GITHUB_ACTIONS=true GITHUB_EVENT_NAME=push GITHUB_REF_TYPE=tag "GITHUB_REF_NAME=v$version"
	GITHUB_REPOSITORY=fixture/repo
	"GITHUB_WORKFLOW_REF=fixture/repo/.github/workflows/release.yml@refs/tags/v$version")

identity() {
	env "$@" bash -c 'source ci/release/build-identity.sh && assert_formal_release_identity' 2>&1
}

# 一、四类调用身份。
identity "${formal_env[@]}" AUCTION_WEB_STACK=solid >/dev/null || fail '正式 tag push 双 Solid 应通过身份断言'
suffix=$(env "${formal_env[@]}" bash -c 'source ci/release/build-identity.sh; release_candidate_suffix')
[[ -z $suffix ]] || fail "正式发行包不应带候选后缀：$suffix"

output=$(identity "${formal_env[@]}" AUCTION_WEB_STACK=dioxus) && fail '正式发行 dioxus 应被拒绝'
[[ $output == *'正式发行必须 Public 与 Admin 两端都使用 Solid'* ]] || fail "dioxus 拒绝缺少诊断：$output"
output=$(identity "${formal_env[@]}") && fail '正式发行缺省栈应被拒绝'
[[ $output == *'两端都使用 Solid'* ]] || fail "缺省栈拒绝缺少诊断：$output"

# 单端 Solid：把 Admin 移出已支持清单，正式发行仍被拒绝。
sed 's/^web_solid_surfaces=.*/web_solid_surfaces=(public)/' ci/release/build-identity.sh >"$fixture/single-end.sh"
if output=$(env "${formal_env[@]}" AUCTION_WEB_STACK=solid bash -c \
	'source "$1" && assert_formal_release_identity' _ "$fixture/single-end.sh" 2>&1); then
	fail '正式发行单端 Solid 应被拒绝'
fi
[[ $output == *'两端都使用 Solid'* ]] || fail "单端 Solid 拒绝缺少诊断：$output"

# 调用来源与 tag：只有 release.yml 的 tag push、且 tag 等于工作区版本才是正式发行。
output=$(identity "${formal_env[@]}" AUCTION_WEB_STACK=solid \
	"GITHUB_WORKFLOW_REF=fixture/repo/.github/workflows/other.yml@refs/tags/v$version") &&
	fail '调用来源不是 release.yml 应被拒绝'
[[ $output == *'必须由 release.yml 的 tag push 调用'* ]] || fail "来源拒绝缺少诊断：$output"
identity "${formal_env[@]}" AUCTION_WEB_STACK=solid \
	GITHUB_WORKFLOW_REF=fixture/repo/.github/workflows/release.yml@refs/heads/main >/dev/null &&
	fail '分支引用的 release.yml 应被拒绝'
output=$(identity "${formal_env[@]}" AUCTION_WEB_STACK=solid GITHUB_REF_NAME=v0.0.0-other) &&
	fail 'tag 与工作区版本不一致应被拒绝'
[[ $output == *'tag 必须等于工作区版本'* ]] || fail "tag 拒绝缺少诊断：$output"

# tag 手工候选：无正式身份，允许任一栈，带 -candidate。
for stack in dioxus solid; do
	candidate_env=(GITHUB_ACTIONS=true GITHUB_EVENT_NAME=workflow_dispatch GITHUB_REF_TYPE=tag
		"GITHUB_REF_NAME=v$version" "AUCTION_WEB_STACK=$stack")
	identity "${candidate_env[@]}" >/dev/null || fail "tag 手工候选（$stack）不应受正式断言约束"
	suffix=$(env "${candidate_env[@]}" bash -c 'source ci/release/build-identity.sh; release_candidate_suffix')
	[[ $suffix == -candidate ]] || fail "tag 手工候选（$stack）必须带 -candidate：$suffix"
done
# 分支候选：同样不是正式身份。
branch_suffix=$(GITHUB_ACTIONS=true GITHUB_EVENT_NAME=push GITHUB_REF_TYPE=branch \
	bash -c 'source ci/release/build-identity.sh; release_candidate_suffix')
[[ $branch_suffix == -candidate ]] || fail "分支候选必须带 -candidate：$branch_suffix"

# 调用方声明的 release_kind 必须与调用身份一致（release-build.yml 在安装工具前执行同一函数）。
release_kind() {
	env "$@" bash -c 'source ci/release/build-identity.sh && assert_release_kind "$RELEASE_KIND"' 2>&1
}
release_kind "${formal_env[@]}" RELEASE_KIND=formal >/dev/null || fail '正式 tag push 声明 formal 应通过'
output=$(release_kind "${formal_env[@]}" RELEASE_KIND=candidate) && fail '正式 tag push 声明 candidate 应被拒绝'
[[ $output == *'正式发行必须声明 release_kind=formal'* ]] || fail "正式声明 candidate 拒绝缺少诊断：$output"
release_kind GITHUB_ACTIONS=true GITHUB_EVENT_NAME=workflow_dispatch GITHUB_REF_TYPE=tag RELEASE_KIND=candidate \
	>/dev/null || fail 'tag 手工候选声明 candidate 应通过'
output=$(release_kind GITHUB_ACTIONS=true GITHUB_EVENT_NAME=workflow_dispatch GITHUB_REF_TYPE=tag RELEASE_KIND=formal) &&
	fail '非正式调用声明 formal 应被拒绝'
[[ $output == *'只允许 release.yml 的 tag push 使用'* ]] || fail "非正式声明 formal 拒绝缺少诊断：$output"
for invalid in '' Formal other; do
	output=$(release_kind "${formal_env[@]}" "RELEASE_KIND=$invalid") && fail "release_kind=$invalid 应被拒绝"
	[[ $output == *'release_kind 只允许 formal 或 candidate'* ]] || fail "release_kind=$invalid 拒绝缺少诊断：$output"
done

# 二、release.sh：正式身份不合格时，在任何工具与目录副作用之前拒绝；只读入口照常运行。
release_fixture=$fixture/release
mkdir -p "$release_fixture/ci/release" "$release_fixture/bin" "$release_fixture/target/dist/artifacts/embedded" \
	"$release_fixture/target/web/public"
cp ci/release/release.sh ci/release/build-identity.sh ci/release/web-stack.sh "$release_fixture/ci/release/"
printf '[workspace.package]\nversion = "9.9.9"\n' >"$release_fixture/Cargo.toml"
printf 'lock\n' >"$release_fixture/Cargo.lock"
printf 'schema=auction-web-build/1\nsurface=public\nbuild_id=0123456789abcdef0123456789abcdef\n' \
	>"$release_fixture/target/web/public/WEB-BUILD-INFO"
printf 'artifact sentinel\n' >"$release_fixture/target/dist/artifacts/embedded/SENTINEL"
for tool in cargo mktemp bun dx; do
	printf '#!/usr/bin/env bash\nprintf "%%s\\n" "${0##*/}" >>"${TOOL_SENTINEL_LOG:?}"\nexit 1\n' >"$release_fixture/bin/$tool"
	chmod +x "$release_fixture/bin/$tool"
done
(
	cd "$release_fixture"
	git init -q
	git config user.email fixture@example.invalid
	git config user.name fixture
	git add -A
	git commit -qm fixture
	git tag v9.9.9
)
release_env=(GITHUB_ACTIONS=true GITHUB_EVENT_NAME=push GITHUB_REF_TYPE=tag GITHUB_REF_NAME=v9.9.9
	GITHUB_REPOSITORY=fixture/repo
	GITHUB_WORKFLOW_REF=fixture/repo/.github/workflows/release.yml@refs/tags/v9.9.9
	"TOOL_SENTINEL_LOG=$release_fixture/tools.log" "PATH=$release_fixture/bin:$PATH")
run_release() { (cd "$release_fixture" && env "$@" 2>&1); }
assert_no_side_effects() {
	[[ ! -e $release_fixture/tools.log ]] || fail "$1：拒绝前已调用 cargo、mktemp、bun 或 dx"
	[[ ! -e $release_fixture/target/dist/tmp && ! -e $release_fixture/target/dist/.locks ]] ||
		fail "$1：拒绝前已创建临时或锁目录"
	[[ $(cat "$release_fixture/target/dist/artifacts/embedded/SENTINEL") == 'artifact sentinel' ]] ||
		fail "$1：拒绝前改动了制品目录"
}
for command in 'release-check --allow-dirty' 'package --allow-dirty' build-embedded build-single; do
	# shellcheck disable=SC2086
	output=$(run_release "${release_env[@]}" AUCTION_WEB_STACK=dioxus bash ci/release/release.sh $command) &&
		fail "正式发行 dioxus 下 $command 应被拒绝"
	[[ $output == *'两端都使用 Solid'* ]] || fail "$command 拒绝缺少诊断：$output"
	assert_no_side_effects "dioxus $command"
done
output=$(run_release "${release_env[@]}" AUCTION_WEB_STACK=solid \
	GITHUB_WORKFLOW_REF=fixture/repo/.github/workflows/release-build.yml@refs/heads/main \
	bash ci/release/release.sh package --allow-dirty) && fail '来源不是 release.yml 时装包应被拒绝'
[[ $output == *'必须由 release.yml 的 tag push 调用'* ]] || fail "来源拒绝缺少诊断：$output"
assert_no_side_effects '来源不是 release.yml'
# 正式 + 双 Solid 通过身份守卫，随后才在后续步骤（工具与制品核对）失败，失败原因不是身份。
output=$(run_release "${release_env[@]}" AUCTION_WEB_STACK=solid bash ci/release/release.sh package --allow-dirty) &&
	fail '夹具没有工具与构建制品，装包本应在身份守卫之后失败'
[[ $output != *'正式发行必须'* && $output != *'必须由 release.yml'* && -n $output ]] || fail "正式双 Solid 应越过身份守卫：$output"

# 只读入口对 Solid、dioxus 与正式环境照常运行，且不写制品。
# 夹具里的 mktemp 等替身会让指纹计算失败，这里换回真实工具。
rm -rf "$release_fixture/target/dist/tmp" "$release_fixture/target/dist/.locks" "$release_fixture/tools.log"
real_path=$PATH
for stack in dioxus solid; do
	run_release "${release_env[@]}" "PATH=$real_path" AUCTION_WEB_STACK=$stack bash ci/release/release.sh web-feature public >/dev/null ||
		fail "只读入口 web-feature 在 $stack 正式环境下应可运行"
	# 指纹需要仓库自己的 mise 工具选择，直接在本仓库（只读）运行。
	env "${formal_env[@]}" AUCTION_WEB_STACK=$stack bash ci/release/release.sh source-fingerprint >/dev/null ||
		fail "只读入口 source-fingerprint 在 $stack 正式环境下应可运行"
done
output=$(run_release "${release_env[@]}" "PATH=$real_path" AUCTION_WEB_STACK=solid bash ci/release/release.sh web-build-id public) ||
	fail 'web-build-id 在正式 Solid 环境下应可运行'
[[ $output == 0123456789abcdef0123456789abcdef ]] || fail "web-build-id 输出不符：$output"
[[ ! -e $release_fixture/target/dist/tmp && ! -e $release_fixture/tools.log ]] || fail '只读入口不应写制品或调用工具'

# 三、包格式：docs/WEB-STACK.v1、页面导出树与部署校验脚本。
source ci/release/build-identity.sh
source ci/release/web-stack.sh
make_payload() {
	local surface=$1 root=$2
	mkdir -p "$root/$surface/public/assets"
	printf '<title>%s</title>\n' "$surface" >"$root/$surface/public/index.html"
	printf 'js-%s\n' "$surface" >"$root/$surface/public/assets/app-abc.js"
	printf 'br-%s\n' "$surface" >"$root/$surface/public/assets/app-abc.js.br"
	printf 'schema=auction-web-assets/1\n' >"$root/$surface/public/auction-web-manifest.v1"
	(cd "$root/$surface/public" && find . -type f ! -name auction-web-manifest.v1 -print0 |
		LC_ALL=C sort -z | xargs -0 sha256sum) >"$root/$surface/PAYLOAD-SHA256"
	printf 'schema=auction-web-build/1\nsurface=%s\nbuild_id=%s\n' "$surface" "${3:-0123456789abcdef0123456789abcdef}" \
		>"$root/$surface/WEB-BUILD-INFO"
}
install_artifact_root() {
	local stable=$1 artifact=$2 surface upper
	mkdir -p "$artifact"
	for surface in public admin; do
		upper=${surface^^}
		cp "$stable/$surface/PAYLOAD-SHA256" "$artifact/WEB-$upper-SHA256"
		printf 'schema=auction-web-assets/1\nsurface=%s\nprofile=solid-release\npayload_sha256=%s\n' \
			"$surface" "$(web_stack_sha256_file "$artifact/WEB-$upper-SHA256")" >"$artifact/WEB-$upper-MANIFEST.v1"
	done
}
extract_function() {
	sed -n "/^$1() {/,/^}/p" ci/release/release.sh
}
setup_stable() {
	local base=$1
	mkdir -p "$base/repo/target/web"
	make_payload public "$base/repo/target/web" "${2:-}"
	make_payload admin "$base/repo/target/web" "${3:-}"
	install_artifact_root "$base/repo/target/web" "$base/artifact"
}

# 装包函数：导出树与凭据按摘要清单生成，登记为 0644；稳定目录陈旧或两端构建号不一致时失败。
run_package_web_stack() {
	local stable=$1 package=$2 scratch=$3
	(
		# 栈选择在 source 时解析一次，所以在子 shell 里以 solid 重新加载。
		export AUCTION_WEB_STACK=solid
		source ci/release/build-identity.sh
		repository_root=$stable/repo
		embedded_artifact_root=$stable/artifact
		release_sources=() release_targets=() release_modes=()
		eval "$(extract_function install_release_file)"
		eval "$(extract_function package_web_stack)"
		mkdir -p "$package/docs"
		cp "$embedded_artifact_root"/WEB-* "$package/docs/"
		package_web_stack "$package" "$scratch" "$(printf 'a%.0s' {1..40})" "$(printf 'b%.0s' {1..64})" || exit 1
		printf '%s\n' "${release_modes[@]}" | sort -u >"$scratch/modes"
		printf '%s\n' "${release_targets[@]}" >"$scratch/targets"
	)
}
good=$fixture/stack-good
setup_stable "$good"
mkdir -p "$good/scratch" "$good/package"
run_package_web_stack "$good" "$good/package" "$good/scratch" >/dev/null || fail '双 Solid 装包函数应成功'
[[ $(<"$good/scratch/modes") == 644 ]] || fail '导出树与凭据必须以 0644 登记'
for expected in docs/WEB-STACK.v1 web-assets/public/index.html web-assets/admin/assets/app-abc.js.br; do
	grep -qx "$expected" "$good/scratch/targets" || fail "装包未登记 $expected"
done
if grep -q 'auction-web-manifest.v1' "$good/scratch/targets"; then fail '导出树不得含 payload 清单文件'; fi

stale=$fixture/stack-stale
setup_stable "$stale"
printf 'tampered\n' >"$stale/repo/target/web/public/public/index.html"
mkdir -p "$stale/scratch" "$stale/package"
if run_package_web_stack "$stale" "$stale/package" "$stale/scratch" >/dev/null 2>&1; then
	fail '稳定目录与摘要清单不一致（陈旧页面）应使装包失败'
fi
mismatch=$fixture/stack-build-id
setup_stable "$mismatch" '' fedcba9876543210fedcba9876543210
mkdir -p "$mismatch/scratch" "$mismatch/package"
if run_package_web_stack "$mismatch" "$mismatch/package" "$mismatch/scratch" >/dev/null 2>&1; then
	fail '两端构建号不一致应使装包失败'
fi

# 部署校验脚本：用夹具包与本地 origin，离线核对（不读取任何凭据，不联网）。
origin=$fixture/origin.git
work=$fixture/work
git init -q --bare "$origin"
git init -q "$work"
(
	cd "$work"
	git config user.email fixture@example.invalid
	git config user.name fixture
	printf 'x\n' >file
	git add file
	git commit -qm fixture
	git tag v0.1.9
	git tag v0.1.8
	git remote add origin "$origin"
	git push -q origin --tags HEAD:refs/heads/main
)
commit=$(git -C "$work" rev-parse HEAD)
fingerprint=$(printf 'c%.0s' {1..64})
packages=$fixture/packages
mkdir -p "$packages"
backtick=$'\x60'

# make_package <目录名> <版本> [变异函数]：在写校验清单之前调用变异函数，使篡改后的包仍自洽。
make_package() {
	local name=$1 package_version=$2 mutate=${3:-:} directory root surface manifest_extra=
	directory=$packages/$name
	root=$directory/auction-rust-$package_version-${commit:0:12}
	mkdir -p "$root/docs"
	setup_stable "$directory/stable"
	printf -- '- Source commit：%s%s%s\n- Source worktree：%sclean%s\n' \
		"$backtick" "$commit" "$backtick" "$backtick" "$backtick" >"$root/docs/DEPENDENCIES.md"
	cp "$directory/stable/artifact"/WEB-* "$root/docs/"
	for surface in public admin; do
		mkdir -p "$root/web-assets/$surface"
		(cd "$directory/stable/repo/target/web/$surface/public" && find . -type f ! -name auction-web-manifest.v1 -print0 |
			xargs -0 -I{} install -D -m 0644 {} "$root/web-assets/$surface/{}")
	done
	web_stack_render "$commit" "$fingerprint" solid solid \
		"$(web_stack_sha256_file "$root/docs/WEB-PUBLIC-SHA256")" \
		"$(web_stack_sha256_file "$root/docs/WEB-ADMIN-SHA256")" 0123456789abcdef0123456789abcdef >"$root/docs/WEB-STACK.v1"
	"$mutate" "$root"
	printf 'schema=auction-release-manifest/1\npackage=%s\nsource_commit=%s\nsource_fingerprint=%s\n%sfiles=\n' \
		"${root##*/}" "$commit" "$fingerprint" "$manifest_extra" >"$root/RELEASE-MANIFEST.v1"
	(cd "$root" && find . -type f ! -name SHA256SUMS -print0 | LC_ALL=C sort -z | xargs -0 sha256sum >SHA256SUMS)
	tar --create --gzip --file "$directory/${root##*/}.tar.gz" --directory "$directory" "${root##*/}"
	cp "$root/docs/DEPENDENCIES.md" "$directory/DEPENDENCIES.md"
	(cd "$directory" && sha256sum DEPENDENCIES.md "${root##*/}.tar.gz" >SHA256SUMS)
	printf '%s\n' "$directory/${root##*/}.tar.gz"
}
verify_package() {
	bash ci/release/verify-deployment-package.sh "$1" --offline --repo "$work" --tag "$2" "${@:3}" 2>&1
}
expect_reject() {
	local label=$1 archive output
	shift
	archive=$(make_package "$@")
	if output=$(verify_package "$archive" v0.1.9); then fail "$label 应被部署校验拒绝"; fi
	[[ $output == *'部署制品校验失败'* ]] || fail "$label 拒绝缺少诊断：$output"
}
mutate_digest() { sed -i "s/^public_payload_sha256=.*/public_payload_sha256=$(printf '0%.0s' {1..64})/" "$1/docs/WEB-STACK.v1"; }
mutate_missing() { rm -f "$1/docs/WEB-STACK.v1"; }
mutate_extra() { printf 'extra\n' >"$1/web-assets/public/unlisted.js"; }
mutate_changed() { printf 'tampered\n' >"$1/web-assets/public/index.html"; }
mutate_foreign() { cp "$1/web-assets/admin/index.html" "$1/web-assets/public/admin-index.html"; }
mutate_stray_tree() { mkdir -p "$1/web-assets/other" && printf 'x\n' >"$1/web-assets/other/x.js"; }
mutate_dioxus() { sed -i 's/^admin_stack=solid/admin_stack=dioxus/' "$1/docs/WEB-STACK.v1"; }
mutate_missing_file() { rm -f "$1/web-assets/admin/assets/app-abc.js.br"; }
mutate_header() { manifest_extra=$'web_stack=solid\n'; }
mutate_legacy() { rm -rf "$1/docs/WEB-STACK.v1" "$1/web-assets"; }
# 栈凭据的来源必须与清单头一致：把别的提交或指纹的凭据拼进正式包必须被拒。
mutate_source_commit() { sed -i "s/^source_commit=.*/source_commit=$(printf 'f%.0s' {1..40})/" "$1/docs/WEB-STACK.v1"; }
mutate_source_fingerprint() { sed -i "s/^source_fingerprint=.*/source_fingerprint=$(printf 'e%.0s' {1..64})/" "$1/docs/WEB-STACK.v1"; }

archive=$(make_package good 0.1.9)
receipt=$fixture/receipt.v1
output=$(verify_package "$archive" v0.1.9 --receipt "$receipt") || fail "双 Solid 夹具包应通过部署校验：$output"
[[ $(stat -c %a "$receipt") == 600 ]] || fail '验证回执必须是 0600'
package_name=${archive##*/}
[[ $(sed -n '1,3p' "$receipt") == "schema=auction-package-verification/1
archive_sha256=$(sha256sum "$archive" | cut -d' ' -f1)
package=${package_name%.tar.gz}" ]] || fail "验证回执前三行不符：$(<"$receipt")"
grep -qx "source_commit=$commit" "$receipt" && grep -qx 'tag=v0.1.9' "$receipt" &&
	grep -qx 'provenance=skipped' "$receipt" && grep -qx 'web_stack_id=public-solid_admin-solid' "$receipt" &&
	grep -Eq '^verified_at=[0-9]{4}-[0-9]{2}-[0-9]{2}T[0-9:]{8}Z$' "$receipt" || fail "验证回执字段不符：$(<"$receipt")"
before=$(<"$receipt")
if verify_package "$archive" v0.1.9 --receipt "$receipt" >/dev/null; then fail '验证回执不得覆盖既有文件'; fi
[[ $(<"$receipt") == "$before" ]] || fail '覆盖被拒绝后回执内容被改动'

# 旧版本（小于硬边界）的 Rust 包按旧格式通过：无 WEB-STACK.v1、无导出树。
legacy=$(make_package legacy 0.1.8 mutate_legacy)
output=$(verify_package "$legacy" v0.1.8 --receipt "$fixture/legacy-receipt.v1") || fail "旧版本旧格式包应通过：$output"
grep -qx 'web_stack_id=none' "$fixture/legacy-receipt.v1" || fail '旧格式包回执应为 web_stack_id=none'

expect_reject '摘要被改一字节' bad-digest 0.1.9 mutate_digest
expect_reject '新版本缺 WEB-STACK.v1' no-stack 0.1.9 mutate_missing
expect_reject '多一个未登记导出文件' extra 0.1.9 mutate_extra
expect_reject '导出文件字节被改' changed 0.1.9 mutate_changed
expect_reject 'Public 混入 Admin 文件' foreign 0.1.9 mutate_foreign
expect_reject 'web-assets 多出第三棵树' stray-tree 0.1.9 mutate_stray_tree
expect_reject '凭据声明 dioxus' dioxus 0.1.9 mutate_dioxus
expect_reject '导出树缺文件' missing-file 0.1.9 mutate_missing_file
expect_reject '清单头白名单被放宽' header 0.1.9 mutate_header
expect_reject '凭据来源提交与清单头不符' source-commit 0.1.9 mutate_source_commit
expect_reject '凭据来源指纹与清单头不符' source-fingerprint 0.1.9 mutate_source_fingerprint

# 候选制品不可部署。
candidate_dir=$packages/candidate
mkdir -p "$candidate_dir"
candidate_archive=$candidate_dir/auction-rust-0.1.9-${commit:0:12}-candidate.tar.gz
cp "$archive" "$candidate_archive"
cp "$(dirname "$archive")/DEPENDENCIES.md" "$(dirname "$archive")/SHA256SUMS" "$candidate_dir/"
if output=$(verify_package "$candidate_archive" v0.1.9); then fail '候选制品应不可部署'; fi
[[ $output == *'拒绝部署候选制品'* ]] || fail "候选拒绝缺少诊断：$output"

printf '正式发行双 Solid 自测通过\n'
