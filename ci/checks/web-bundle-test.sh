#!/usr/bin/env bash
# shellcheck disable=SC1091,SC2030,SC2031,SC2032,SC2034,SC2329
# 验证双栈构建入口：未支持的组合在清理前失败、Solid 依赖预检、Dioxus 原路径，
# 以及发布入口的 feature 选择、回执复用与正式发行拒绝。

set -euo pipefail
cd "$(dirname "$0")/../.."
repository_root=$PWD
unset AUCTION_WEB_STACK
source ci/release/build-identity.sh
source ci/release/web-bundle.sh

fail() {
	printf 'Web 双栈构建入口回归失败：%s\n' "$1" >&2
	exit 1
}

fixture=$(mktemp -d)
trap 'rm -rf -- "$fixture"' EXIT

# 函数级闭集：当前只有 Public 可以切到 Solid，Admin 仍只接受 Dioxus。
web_bundle_stack_supported public dioxus || fail 'public/dioxus 应受支持'
web_bundle_stack_supported public solid || fail 'public/solid 应受支持'
web_bundle_stack_supported admin dioxus || fail 'admin/dioxus 应受支持'
if web_bundle_stack_supported admin solid; then fail 'admin/solid 在当前批次应被拒绝'; fi
if web_bundle_stack_supported public other; then fail '未知技术栈应被拒绝'; fi

# 发布 feature 由逐 surface 的实际组合得出；缺省必须与原有常量逐字符相同。
default_features=$(bash -c 'source "$1/ci/release/build-identity.sh"
	web_cargo_features public-api
	web_cargo_features admin-api
	web_cargo_features server' _ "$repository_root")
[[ "$default_features" == $'auction-public-api/web-embed\nauction-admin-api/web-embed\nauction-server/web-embed' ]] ||
	fail "缺省 cargo feature 与原有常量不一致：$default_features"
solid_features=$(AUCTION_WEB_STACK=solid bash -c 'source "$1/ci/release/build-identity.sh"
	web_cargo_features public-api
	web_cargo_features admin-api
	web_cargo_features server' _ "$repository_root")
[[ "$solid_features" == $'auction-public-api/web-solid\nauction-admin-api/web-embed\nauction-server/web-embed,auction-server/web-solid-public' ]] ||
	fail "solid 组合的 cargo feature 不正确：$solid_features"
[[ $(bash ci/release/release.sh web-feature public) == web-embed &&
	$(bash ci/release/release.sh web-feature admin) == web-embed ]] ||
	fail '缺省运行任务应选择 web-embed'
[[ $(AUCTION_WEB_STACK=solid bash ci/release/release.sh web-feature public) == web-solid &&
	$(AUCTION_WEB_STACK=solid bash ci/release/release.sh web-feature admin) == web-embed ]] ||
	fail 'solid 运行任务应让 Public 用 web-solid、Admin 用 web-embed'

# 原生制品的输入指纹必须随实际技术栈变化；缺省不扫描 web/，Solid 加入 web/ 与栈标记。
default_inputs=$(bash -c 'source "$1/ci/release/build-identity.sh"; build_inputs_fingerprint' _ "$repository_root") ||
	fail '无法计算缺省原生输入指纹'
solid_inputs=$(AUCTION_WEB_STACK=solid bash -c 'source "$1/ci/release/build-identity.sh"; build_inputs_fingerprint' _ "$repository_root") ||
	fail '无法计算 Solid 原生输入指纹'
[[ "$default_inputs" =~ ^[0-9a-f]{64}$ && "$solid_inputs" =~ ^[0-9a-f]{64}$ && "$default_inputs" != "$solid_inputs" ]] ||
	fail '实际技术栈未进入原生输入指纹'

# 未设置 stack 的既有单元测试仍须得到原来的 Dioxus 回执。
surface=public
dx_version='dx fixture'
rustc_version='rustc fixture'
brotli_version='brotli fixture'
source_identity='source fixture'
build_environment='environment fixture'
AUCTION_BUILD_ID=0123456789abcdef0123456789abcdef
default_fields=$(web_build_fields)
stack=dioxus
explicit_fields=$(web_build_fields)
[[ "$default_fields" == "$explicit_fields" ]] || fail '缺省与显式 dioxus 的回执应相同'
[[ "$default_fields" == *$'profile=wasm-release'* && "$default_fields" != *'stack='* ]] ||
	fail 'Dioxus 回执应保持原字段'
stack=solid
bun_version='bun fixture'
solid_fields=$(web_build_fields)
[[ "$solid_fields" == *$'profile=solid-release'* && "$solid_fields" == *$'stack=solid'* &&
	"$solid_fields" == *'bun_version=bun fixture'* && "$solid_fields" != *'dx_version='* ]] ||
	fail 'Solid 回执应记录实际技术栈与 bun 版本'

# 依赖凭据由 mise run web-prepare 生成；构建阶段只接受与当前锁文件和 bun 版本相符的凭据。
make_prepared_web() {
	local root=$1
	mkdir -p "$root/web/node_modules/.bin"
	printf 'lock\n' >"$root/web/bun.lock"
	printf '#!/usr/bin/env bash\nexit 0\n' >"$root/web/node_modules/.bin/vite"
	chmod +x "$root/web/node_modules/.bin/vite"
	printf 'lock_sha256=%s\nbun_version=%s\n' \
		"$(sha256_file "$root/web/bun.lock")" 'bun fixture' \
		>"$root/web/node_modules/.auction-prepared"
}

run_prepared_check() {
	local root=$1 diagnostic
	if diagnostic=$(
		(
			repository_root=$root
			bun_version='bun fixture'
			solid_web_dependencies_ready
		) 2>&1
	); then
		return 0
	fi
	printf '%s\n' "$diagnostic" >&2
	return 1
}

missing=$fixture/missing
mkdir -p "$missing/web"
if diagnostic=$(run_prepared_check "$missing" 2>&1); then
	fail 'node_modules 缺失时应失败'
fi
[[ "$diagnostic" == *'mise run web-prepare'* ]] || fail 'node_modules 缺失时缺少准备命令提示'

prepared_ok=$fixture/prepared-ok
make_prepared_web "$prepared_ok"
run_prepared_check "$prepared_ok" || fail '凭据相符的依赖准备应通过'

prepared_missing=$fixture/prepared-missing
cp -a "$prepared_ok" "$prepared_missing"
rm -- "$prepared_missing/web/node_modules/.auction-prepared"
if diagnostic=$(run_prepared_check "$prepared_missing" 2>&1); then
	fail '缺少准备凭据时应失败'
fi
[[ "$diagnostic" == *'缺少准备凭据'* && "$diagnostic" == *'mise run web-prepare'* ]] ||
	fail '缺少准备凭据时缺少可行动诊断'

prepared_digest=$fixture/prepared-digest
cp -a "$prepared_ok" "$prepared_digest"
printf 'lock_sha256=%064d\nbun_version=bun fixture\n' 0 \
	>"$prepared_digest/web/node_modules/.auction-prepared"
if diagnostic=$(run_prepared_check "$prepared_digest" 2>&1); then
	fail '准备凭据摘要不符时应失败'
fi
[[ "$diagnostic" == *'与 web/bun.lock 不一致'* ]] || fail '摘要不符时缺少明确诊断'

prepared_version=$fixture/prepared-version
cp -a "$prepared_ok" "$prepared_version"
printf 'lock_sha256=%s\nbun_version=other bun\n' \
	"$(sha256_file "$prepared_version/web/bun.lock")" \
	>"$prepared_version/web/node_modules/.auction-prepared"
if diagnostic=$(run_prepared_check "$prepared_version" 2>&1); then
	fail '准备凭据的 bun 版本不符时应失败'
fi
[[ "$diagnostic" == *'bun 版本与当前工具不一致'* ]] || fail '版本不符时缺少明确诊断'

prepared_vite=$fixture/prepared-vite
cp -a "$prepared_ok" "$prepared_vite"
rm -- "$prepared_vite/web/node_modules/.bin/vite"
if diagnostic=$(run_prepared_check "$prepared_vite" 2>&1); then
	fail 'node_modules/.bin/vite 缺失时应失败'
fi
[[ "$diagnostic" == *'node_modules/.bin/vite'* && "$diagnostic" == *'mise run web-prepare'* ]] ||
	fail 'Vite 缺失时缺少准备命令提示'

# 替身 dx 记录调用并生成满足现有 Dioxus 改写步骤的最小目录；身份与门禁函数用固定值隔离。
make_bundle_fixture() {
	local directory=$1
	mkdir -p "$directory/ci/release" "$directory/bin"
	cp ci/release/build-identity.sh ci/release/web-bundle.sh "$directory/ci/release/"
	cat >>"$directory/ci/release/build-identity.sh" <<'FIXTURE'
configure_build_tools() { :; }
web_inputs_fingerprint() { printf '%064d\n' 0; }
build_env_fingerprint() { printf '%064d\n' 1; }
web_build_identity() { printf '%032d\n' 2; }
web_payload_reusable() { return 1; }
web_build_inputs_unchanged() { return 0; }
verify_web_payload() { :; }
FIXTURE
	cat >"$directory/bin/git" <<'FIXTURE'
#!/usr/bin/env bash
set -euo pipefail
case "${1:-}" in
rev-parse) printf '%s\n' "${FIXTURE_ROOT:?}" ;;
show) printf '1700000000\n' ;;
*) exit 2 ;;
esac
FIXTURE
	cat >"$directory/bin/dx" <<'FIXTURE'
#!/usr/bin/env bash
set -euo pipefail
if [[ ${1:-} == --version ]]; then
	printf 'dx 0.7.0\n'
	exit 0
fi
printf '%s\n' "$*" >>"${DIOXUS_CALL_LOG:?}"
out=''
while (($#)); do
	case "$1" in
	--out-dir)
		out=$2
		shift 2
		;;
	*) shift ;;
	esac
done
[[ -n "$out" ]]
root="$out/public"
mkdir -p "$root/assets"
printf '%s\n' \
	'<!doctype html>' \
	'<html><head>' \
	'<link rel="preload" as="script" href="/./assets/entry.js">' \
	'<script type="module" src="/./assets/entry.js"></script>' \
	'<link rel="stylesheet" href="/./assets/base-dxh123.css" type="text/css"><link rel="stylesheet" href="/./assets/public-dxh123.css" type="text/css"><link rel="stylesheet" href="/./assets/desktop-dxh123.css" type="text/css"><link rel="stylesheet" href="/./assets/mobile-dxh123.css" type="text/css"><link rel="stylesheet" href="/./assets/console-dxh123.css" type="text/css">' \
	'</head><body></body></html>' >"$root/index.html"
	printf 'entry\n' >"$root/assets/entry.js"
	printf 'wasm\n' >"$root/assets/auction-web-public_bg01234567.wasm"
	for name in base public desktop mobile console; do
		printf 'body { color: black; }\n' >"$root/assets/$name-dxh123.css"
	done
FIXTURE
	chmod +x "$directory/bin/git" "$directory/bin/dx"
}

run_dioxus_fixture() {
	local directory=$1 mode=$2 log=$1/calls.log
	if [[ "$mode" == default ]]; then
		(
			unset AUCTION_WEB_STACK
			FIXTURE_ROOT=$directory DIOXUS_CALL_LOG=$log PATH="$directory/bin:$PATH" \
				bash "$directory/ci/release/web-bundle.sh" public
		) >"$directory/stdout" 2>"$directory/stderr"
	else
		(
			export AUCTION_WEB_STACK=dioxus
			FIXTURE_ROOT=$directory DIOXUS_CALL_LOG=$log PATH="$directory/bin:$PATH" \
				bash "$directory/ci/release/web-bundle.sh" public
		) >"$directory/stdout" 2>"$directory/stderr"
	fi
}

default_fixture=$fixture/default
explicit_fixture=$fixture/explicit
make_bundle_fixture "$default_fixture"
make_bundle_fixture "$explicit_fixture"
run_dioxus_fixture "$default_fixture" default
run_dioxus_fixture "$explicit_fixture" explicit
for directory in "$default_fixture" "$explicit_fixture"; do
	grep -Fq -- 'bundle -p auction-web-public --platform web --profile wasm-release' "$directory/calls.log" ||
		fail '缺省或显式 dioxus 应走原 dx bundle 路径'
	[[ -f "$directory/target/dx/build/auction-web-public/web/WEB-BUILD-INFO" ]] ||
		fail 'Dioxus 构建应写入稳定目录回执'
	grep -Eq '^profile=wasm-release$' "$directory/target/dx/build/auction-web-public/web/WEB-BUILD-INFO" ||
		fail 'Dioxus 回执 profile 不应改变'
done
sed "s|$default_fixture|<fixture>|g" "$default_fixture/calls.log" >"$fixture/default-calls"
sed "s|$explicit_fixture|<fixture>|g" "$explicit_fixture/calls.log" >"$fixture/explicit-calls"
cmp -s "$fixture/default-calls" "$fixture/explicit-calls" ||
	fail '缺省与显式 dioxus 的 dx 调用应完全一致'

# 让身份层把 Admin 也标成 Solid，验证 web-bundle 自己仍在清理前拒绝。
unsupported=$fixture/unsupported
make_bundle_fixture "$unsupported"
printf '%s\n' 'web_solid_surfaces+=(admin)' >>"$unsupported/ci/release/build-identity.sh"
for surface in public admin; do
	mkdir -p "$unsupported/target/web/$surface" \
		"$unsupported/target/dx/build/auction-web-$surface/web"
	printf '%s solid sentinel\n' "$surface" >"$unsupported/target/web/$surface/SENTINEL"
	printf '%s dioxus sentinel\n' "$surface" \
		>"$unsupported/target/dx/build/auction-web-$surface/web/SENTINEL"
done
if diagnostic=$(
	(
		export AUCTION_WEB_STACK=solid
		FIXTURE_ROOT=$unsupported PATH="$unsupported/bin:$PATH" \
				bash "$unsupported/ci/release/web-bundle.sh" public
	) 2>&1
); then
	fail '从 public 开始遇到尚未支持的 admin=solid 应失败'
fi
[[ "$diagnostic" == *'尚未支持的 Web 技术栈组合：admin=solid'* ]] ||
	fail "未支持组合的诊断不明确：$diagnostic"
for surface in public admin; do
	[[ $(<"$unsupported/target/web/$surface/SENTINEL") == "$surface solid sentinel" &&
		$(<"$unsupported/target/dx/build/auction-web-$surface/web/SENTINEL") == \
			"$surface dioxus sentinel" ]] ||
		fail "从 public 开始拒绝时清理了 $surface 稳定目录"
done

# 稳定目录换入必须整体替换已有目标，而不是把候选目录塞进旧目录。
install_fixture=$fixture/install
mkdir -p "$install_fixture/current" "$install_fixture/next"
printf 'old\n' >"$install_fixture/current/old"
printf 'new\n' >"$install_fixture/next/new"
install_generated_directory "$install_fixture/next" "$install_fixture/current" ||
	fail '精确换入目录应成功'
[[ -f "$install_fixture/current/new" && ! -e "$install_fixture/current/next" &&
	! -e "$install_fixture/current/old" ]] || fail '换入应整体替换目标目录'

# brotli 封存的枚举与各步失败必须传出来；函数不能被进程替换或 || 左侧吞掉。
compress_fixture=$fixture/compress
mkdir -p "$compress_fixture/assets"
dd if=/dev/zero of="$compress_fixture/assets/app.js" bs=2048 count=1 status=none
if (
	payload_root=$compress_fixture
	find() { return 42; }
	compress_payload_assets
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'brotli 输入枚举失败时不应成功'
fi
[[ $(<"$fixture/diagnostic") == *'无法枚举 Web payload 的 brotli 输入'* ]] ||
	fail 'brotli 枚举失败缺少诊断'

if (
	payload_root=$compress_fixture
	mktemp() { return 1; }
	compress_payload_assets
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'brotli 清单临时文件创建失败时不应成功'
fi
[[ $(<"$fixture/diagnostic") == *'无法创建 brotli 输入清单'* ]] ||
	fail 'brotli 清单创建失败缺少诊断'

if (
	payload_root=$compress_fixture
	stat() { return 1; }
	compress_payload_assets
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'brotli 输入 stat 失败时不应成功'
fi
[[ $(<"$fixture/diagnostic") == *'无法读取 Web payload 文件大小'* ]] ||
	fail 'brotli 输入 stat 失败缺少诊断'

if (
	payload_root=$compress_fixture
	brotli() { return 1; }
	compress_payload_assets
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'brotli 压缩失败时不应成功'
fi
[[ $(<"$fixture/diagnostic") == *'brotli 预压缩失败'* ]] || fail 'brotli 压缩失败缺少诊断'

if (
	payload_root=$compress_fixture
	brotli() {
		local output='' file=''
		while (($#)); do
			case "$1" in
			-o) output=$2; shift 2 ;;
			*) file=$1; shift ;;
			esac
		done
		printf 'x' >"$output"
	}
	mv() { return 1; }
	compress_payload_assets
) >"$fixture/out" 2>"$fixture/diagnostic"; then
	fail 'brotli 副本写入失败时不应成功'
fi
[[ $(<"$fixture/diagnostic") == *'无法写入 brotli 副本'* ]] ||
	fail 'brotli 副本写入失败缺少诊断'

default_fingerprint=$(bash ci/release/release.sh source-fingerprint 2>/dev/null)
[[ "$default_fingerprint" =~ ^[0-9a-f]{64}$ ]] || fail '缺省发布入口不应被 Solid 拒绝'

# 正式发行 = tag 触发的 CI 运行；Solid 必须在任何工具或目录副作用之前拒绝。
make_tagged_release_fixture() {
	local directory=$1
	mkdir -p "$directory/ci/release" "$directory/bin" \
		"$directory/target/dist/artifacts/embedded" "$directory/target/dist/build/embedded" \
		"$directory/target/web/public"
	cp ci/release/release.sh ci/release/build-identity.sh "$directory/ci/release/"
	printf '[workspace.package]\nversion = "9.9.9"\n' >"$directory/Cargo.toml"
	printf 'lock\n' >"$directory/Cargo.lock"
	cat >"$directory/bin/cargo" <<'FIXTURE'
#!/usr/bin/env bash
set -euo pipefail
if [[ " $* " != *' --target-dir '* ]]; then
	printf 'run AUCTION_BUILD_ID=%s args=%s\n' "${AUCTION_BUILD_ID-}" "$*" >>"${CARGO_CALL_LOG:?}"
	exit 0
fi
printf '%s\n' "$*" >>"${CARGO_CALL_LOG:?}"
exit 0
FIXTURE
	for tool in mktemp bun dx; do
		cat >"$directory/bin/$tool" <<'FIXTURE'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "${0##*/}" >>"${TOOL_SENTINEL_LOG:?}"
exit 1
FIXTURE
	done
	chmod +x "$directory/bin/cargo" "$directory/bin/mktemp" "$directory/bin/bun" "$directory/bin/dx"
	printf 'artifact sentinel\n' >"$directory/target/dist/artifacts/embedded/SENTINEL"
	printf 'build sentinel\n' >"$directory/target/dist/build/embedded/SENTINEL"
	printf 'schema=auction-web-build/1\nsurface=public\nbuild_id=0123456789abcdef0123456789abcdef\n' \
		>"$directory/target/web/public/WEB-BUILD-INFO"
	(
		cd "$directory"
		git init -q
		git config user.email fixture@example.invalid
		git config user.name fixture
		git add -A
		git commit -qm fixture
		git tag v9.9.9
		printf 'dirty\n' >dirty-worktree
	)
}

tagged=$fixture/tagged
make_tagged_release_fixture "$tagged"
if diagnostic=$(
	(
		cd "$tagged"
		GITHUB_ACTIONS=true GITHUB_REF_TYPE=tag AUCTION_WEB_STACK=solid \
			CARGO_CALL_LOG="$tagged/calls.log" TOOL_SENTINEL_LOG="$tagged/tools.log" \
			PATH="$tagged/bin:$PATH" bash ci/release/release.sh release-check --allow-dirty
	) 2>&1
); then
	fail '正式发行在 Solid 下应被拒绝'
fi
[[ "$diagnostic" == *'正式发行只产出 Rust 版页面'* ]] || fail "正式发行拒绝缺少明确诊断：$diagnostic"
[[ ! -e "$tagged/calls.log" ]] || fail '正式发行拒绝前已执行 cargo'
[[ ! -e "$tagged/tools.log" ]] || fail '正式发行拒绝前已调用 mktemp、bun 或 dx'
for sentinel in \
	"$tagged/target/dist/artifacts/embedded/SENTINEL" \
	"$tagged/target/dist/build/embedded/SENTINEL"; do
	[[ -f "$sentinel" ]] || fail "正式发行拒绝前改动了制品目录：$sentinel"
done
if find "$tagged/target/dist" -name '*.next.*' -print -quit | grep -q .; then
	fail '正式发行拒绝前创建了候选目录'
fi

if diagnostic=$(
	(
		cd "$tagged"
		GITHUB_ACTIONS=true GITHUB_REF_TYPE=tag AUCTION_WEB_STACK=solid \
			CARGO_CALL_LOG="$tagged/calls.log" TOOL_SENTINEL_LOG="$tagged/tools.log" \
			PATH="$tagged/bin:$PATH" bash ci/release/release.sh package --allow-dirty
	) 2>&1
); then
	fail '正式发行装包在 Solid 下应被拒绝'
fi
[[ "$diagnostic" == *'正式发行只产出 Rust 版页面'* ]] ||
	fail "正式发行装包拒绝缺少明确诊断：$diagnostic"
[[ ! -e "$tagged/calls.log" ]] || fail '正式发行装包拒绝前已执行 cargo'
[[ ! -e "$tagged/tools.log" ]] || fail '正式发行装包拒绝前已调用 mktemp、bun 或 dx'

if (
	cd "$tagged"
	GITHUB_ACTIONS=true GITHUB_REF_TYPE=branch AUCTION_WEB_STACK=solid \
		CARGO_CALL_LOG="$tagged/branch-calls.log" TOOL_SENTINEL_LOG="$tagged/branch-tools.log" \
		PATH="$tagged/bin:$PATH" bash ci/release/release.sh run-web public
) >"$tagged/branch.out" 2>&1; then
	:
else
	fail "非 tag 触发不应被正式发行守卫拒绝：$(<"$tagged/branch.out")"
fi
grep -q '^run AUCTION_BUILD_ID=0123456789abcdef0123456789abcdef' "$tagged/branch-calls.log" ||
	fail 'Solid 本地运行未注入页面构建号'
[[ ! -e "$tagged/branch-tools.log" ]] || fail '本地运行不应调用发布工具'
[[ ! -e "$tagged/target/dist/tmp" ]] || fail '正式发行装包拒绝前创建了临时目录'

# 原生复用身份必须包含两端实际技术栈：缺省回执在 solid 下判为不可复用并按 solid 重建。
make_reuse_release_fixture() {
	local directory=$1
	mkdir -p "$directory/ci/release" "$directory/bin"
	awk '/^usage\(\) \{/ && !done {
		print "verify_surface_size_budget() { :; }"
		done = 1
	}
	{ print }' ci/release/release.sh >"$directory/ci/release/release.sh"
	cp ci/release/build-identity.sh "$directory/ci/release/"
	cat >>"$directory/ci/release/build-identity.sh" <<'FIXTURE'
configure_build_tools() { :; }
source_fingerprint() { printf '%064d\n' 1; }
build_env_fingerprint() { printf '%064d\n' 2; }
web_build_identity() { printf '%032d\n' 3; }
verify_web_payload() {
	local root
	root=$(web_surface_root "$1" "${FIXTURE_ROOT:?}") || return 2
	[[ -f "$root/PAYLOAD-SHA256" ]] || return 1
	cat "$root/PAYLOAD-SHA256"
}
FIXTURE
	cat >"$directory/ci/release/web-bundle.sh" <<'FIXTURE'
#!/usr/bin/env bash
set -euo pipefail
source "$FIXTURE_ROOT/ci/release/build-identity.sh"
root=$(web_surface_root "$1" "${FIXTURE_ROOT:?}")
mkdir -p "$root/public"
payload_sha=$(printf '%064d' 5)
if [[ ${AUCTION_WEB_STACK:-dioxus} == solid && "$1" == public ]]; then
	payload_sha=$(printf '%064d' 6)
elif [[ ${AUCTION_WEB_STACK:-dioxus} == solid && "$1" == admin ]]; then
	payload_sha=$(printf '%064d' 7)
fi
printf '%s\n' "$payload_sha" >"$root/PAYLOAD-SHA256"
printf 'schema=auction-web-assets/1\nsurface=%s\npayload_sha256=%s\n' \
	"$1" "$payload_sha" >"$root/public/auction-web-manifest.v1"
printf 'schema=auction-web-build/1\nsurface=%s\nbuild_id=0123456789abcdef0123456789abcdef\n' \
	"$1" >"$root/WEB-BUILD-INFO"
printf '%s\n' "$1" >>"${WEB_BUNDLE_CALL_LOG:?}"
FIXTURE
	cat >"$directory/bin/cargo" <<'FIXTURE'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >>"${CARGO_CALL_LOG:?}"
target=''
packages=()
while (($#)); do
	case "$1" in
	--target-dir)
		target=$2
		shift 2
		;;
	-p)
		packages+=("$2")
		shift 2
		;;
	*) shift ;;
	esac
done
if [[ -z "$target" ]]; then
	printf 'run AUCTION_BUILD_ID=%s args=%s\n' "${AUCTION_BUILD_ID-}" "$*" >>"${CARGO_CALL_LOG:?}"
	exit 0
fi
mkdir -p "$target/release"
for package in "${packages[@]}"; do
	printf '#!/usr/bin/env bash\nexit 0\n' >"$target/release/$package"
	chmod +x "$target/release/$package"
done
FIXTURE
	cat >"$directory/bin/dx" <<'FIXTURE'
#!/usr/bin/env bash
exit 0
FIXTURE
	chmod +x "$directory/bin/cargo" "$directory/bin/dx"
	printf '[workspace.package]\nversion = "9.9.9"\n' >"$directory/Cargo.toml"
	printf 'lock\n' >"$directory/Cargo.lock"
	(
		cd "$directory"
		git init -q
		git config user.email fixture@example.invalid
		git config user.name fixture
		git add -A
		git commit -qm fixture
	)
}

run_release_fixture() {
	local directory=$1 stack=$2 output=$3
	if [[ "$stack" == default ]]; then
		(
			cd "$directory"
			unset AUCTION_WEB_STACK
			FIXTURE_ROOT=$directory CARGO_CALL_LOG="$directory/calls.log" \
				WEB_BUNDLE_CALL_LOG="$directory/web-bundle.log" \
				PATH="$directory/bin:$PATH" bash ci/release/release.sh build-embedded
		) >"$output" 2>&1
	else
		(
			cd "$directory"
			export AUCTION_WEB_STACK=$stack
			FIXTURE_ROOT=$directory CARGO_CALL_LOG="$directory/calls.log" \
				WEB_BUNDLE_CALL_LOG="$directory/web-bundle.log" \
				PATH="$directory/bin:$PATH" bash ci/release/release.sh build-embedded
		) >"$output" 2>&1
	fi
}

reuse=$fixture/reuse
make_reuse_release_fixture "$reuse"
run_release_fixture "$reuse" default "$reuse/default.out" || fail '缺省发布构建替身应成功'
cp "$reuse/target/dist/artifacts/embedded/BUILD-INFO" "$reuse/default-info"
if grep -q '^web_stack_' "$reuse/default-info"; then
	fail '缺省 BUILD-INFO 不应包含技术栈字段'
fi
expected_keys='schema variant source_commit source_worktree source_fingerprint cargo_lock_sha256 features build_inputs_sha256 build_env_sha256'
actual_keys=$(sed -n 's/^\([^=]*\)=.*/\1/p' "$reuse/default-info" | tr '\n' ' ')
[[ "$actual_keys" == "$expected_keys " ]] ||
	fail "缺省 BUILD-INFO 字段顺序发生变化：$actual_keys"
[[ $(wc -l <"$reuse/calls.log") -eq 1 ]] || fail '缺省发布构建应只调用一次 cargo'
run_release_fixture "$reuse" default "$reuse/reuse.out" || fail '缺省发布构建再次运行应成功'
[[ $(<"$reuse/reuse.out") == *'复用'* ]] || fail '缺省发布构建输入未变时应复用'
[[ $(wc -l <"$reuse/calls.log") -eq 1 ]] || fail '缺省复用不应重新调用 cargo'
run_release_fixture "$reuse" solid "$reuse/solid.out" || fail 'Solid 候选构建应成功'
[[ $(<"$reuse/solid.out") != *'复用'* ]] || fail '技术栈变化后不得复用缺省制品'
[[ $(wc -l <"$reuse/calls.log") -eq 2 ]] || fail '技术栈变化后应重新编译'
info="$reuse/target/dist/artifacts/embedded/BUILD-INFO"
grep -qx 'web_stack_public=solid' "$info" || fail 'Solid 回执未记录 Public 技术栈'
grep -qx 'web_stack_admin=dioxus' "$info" || fail 'Solid 回执未记录 Admin 技术栈'
grep -qx 'features=auction-public-api/web-solid,auction-admin-api/web-embed' "$info" ||
	fail 'Solid 回执未记录实际 feature'

# 混合组合重建稳定页面后，原生回执仍匹配也不能复用；两端当前 payload 必须与制品一致。
mixed=$fixture/reuse-mixed
make_reuse_release_fixture "$mixed"
run_release_fixture "$mixed" default "$mixed/default.out" || fail '混合场景的缺省构建应成功'
[[ $(wc -l <"$mixed/web-bundle.log") -eq 2 ]] || fail '缺省构建应生成两端页面'
(
	cd "$mixed"
	export AUCTION_WEB_STACK=solid
	FIXTURE_ROOT=$mixed WEB_BUNDLE_CALL_LOG="$mixed/web-bundle.log" \
		PATH="$mixed/bin:$PATH" bash ci/release/web-bundle.sh public
	FIXTURE_ROOT=$mixed WEB_BUNDLE_CALL_LOG="$mixed/web-bundle.log" \
		PATH="$mixed/bin:$PATH" bash ci/release/web-bundle.sh admin
)
[[ $(wc -l <"$mixed/web-bundle.log") -eq 4 ]] || fail 'Solid web-build 应重写两端稳定页面'
run_release_fixture "$mixed" default "$mixed/mixed-after.out" || fail '混合页面后的缺省构建应成功'
[[ $(<"$mixed/mixed-after.out") != *'复用'* ]] || fail '混合页面摘要与制品不一致时不得复用'
[[ $(wc -l <"$mixed/calls.log") -eq 2 ]] || fail '混合页面不一致时应重新编译'
[[ $(wc -l <"$mixed/web-bundle.log") -eq 6 ]] || fail '混合页面不一致时应重新生成两端页面'

# 只恢复原生制品、没有稳定页面目录时同样不能复用。
missing=$fixture/reuse-missing
make_reuse_release_fixture "$missing"
run_release_fixture "$missing" default "$missing/default.out" || fail '缺失页面场景的缺省构建应成功'
rm -rf "$missing/target/dx/build/auction-web-public/web"
run_release_fixture "$missing" default "$missing/missing-after.out" || fail '缺失页面后的缺省构建应成功'
[[ $(<"$missing/missing-after.out") != *'复用'* ]] || fail '稳定 Public 页面缺失时不得复用'
[[ $(<"$missing/missing-after.out") == *'当前 Public 页面 payload 缺失或无效'* ]] ||
	fail '稳定 Public 页面缺失时缺少重建诊断'
[[ $(wc -l <"$missing/calls.log") -eq 2 ]] || fail '稳定页面缺失时应重新编译'
[[ $(wc -l <"$missing/web-bundle.log") -eq 4 ]] || fail '稳定页面缺失时应重新生成两端页面'

# Solid 的稳定 Public 目录（target/web/public）缺失时同样不复用。
missing_solid=$fixture/reuse-missing-solid
make_reuse_release_fixture "$missing_solid"
run_release_fixture "$missing_solid" solid "$missing_solid/default.out" || fail 'Solid 缺失页面场景的首次构建应成功'
rm -rf "$missing_solid/target/web/public"
run_release_fixture "$missing_solid" solid "$missing_solid/missing-after.out" || fail 'Solid 缺失页面后的构建应成功'
[[ $(<"$missing_solid/missing-after.out") != *'复用'* ]] || fail 'Solid 稳定 Public 页面缺失时不得复用'
[[ $(<"$missing_solid/missing-after.out") == *'当前 Public 页面 payload 缺失或无效'* ]] ||
	fail 'Solid 稳定 Public 页面缺失时缺少重建诊断'
[[ $(wc -l <"$missing_solid/calls.log") -eq 2 ]] || fail 'Solid 稳定页面缺失时应重新编译'
[[ $(wc -l <"$missing_solid/web-bundle.log") -eq 4 ]] || fail 'Solid 稳定页面缺失时应重新生成两端页面'

# 运行任务先取 feature，非法开关不能把缺值传给 cargo；Solid 还要注入页面构建号。
run_fixture() {
	local directory=$1 stack=$2 output=$3
	(
		cd "$directory"
		if [[ "$stack" == default ]]; then
			unset AUCTION_WEB_STACK
		else
			export AUCTION_WEB_STACK=$stack
		fi
		FIXTURE_ROOT=$directory CARGO_CALL_LOG="$directory/run-calls.log" \
			PATH="$directory/bin:$PATH" bash ci/release/release.sh run-web public
	) >"$output" 2>&1
}
if run_fixture "$missing" invalid "$missing/invalid.out"; then
	fail '非法 AUCTION_WEB_STACK 不应启动 cargo'
fi
[[ ! -e "$missing/run-calls.log" ]] || fail '非法 AUCTION_WEB_STACK 时已调用 cargo'
[[ $(<"$missing/invalid.out") == *'AUCTION_WEB_STACK'* ]] || fail '非法开关缺少诊断'

mkdir -p "$missing/target/web/public"
rm -f "$missing/target/web/public/WEB-BUILD-INFO"
if run_fixture "$missing" solid "$missing/no-info.out"; then
	fail 'Solid 缺少 WEB-BUILD-INFO 时不应启动 cargo'
fi
[[ ! -e "$missing/run-calls.log" ]] || fail '缺少 WEB-BUILD-INFO 时已调用 cargo'
[[ $(<"$missing/no-info.out") == *'mise run web-build'* ]] || fail '缺少 WEB-BUILD-INFO 时缺少准备命令提示'

printf 'schema=auction-web-build/1\nsurface=public\nbuild_id=0123456789abcdef0123456789abcdef\n' \
	>"$missing/target/web/public/WEB-BUILD-INFO"
run_fixture "$missing" solid "$missing/with-info.out" || fail 'Solid 本地运行应成功'
grep -q '^run AUCTION_BUILD_ID=0123456789abcdef0123456789abcdef' "$missing/run-calls.log" ||
	fail 'Solid 本地运行未注入页面构建号'
: >"$missing/run-calls.log"
run_fixture "$missing" default "$missing/with-default.out" || fail '缺省本地运行应成功'
grep -q '^run --locked -p auction-public-api --features web-embed$' "$missing/run-calls.log" ||
	fail '缺省本地运行未保持原 cargo 命令'
if grep -q '^run AUCTION_BUILD_ID=[^ ]' "$missing/run-calls.log"; then
	fail '缺省 Dioxus 本地运行不应注入页面构建号'
fi

# 依赖清单先取得 bun.lock 摘要与 bun 版本；任一失败不能写出空值后继续。
awk '/^write_dependencies\(\) \{/ { capture = 1 } capture { print } capture && /^}$/ { exit }' \
	ci/release/release.sh >"$fixture/write-dependencies.sh"
write_dependencies_case() {
	local directory=$1 destination=$2 sha_failure=$3 version_empty=$4
	mkdir -p "$directory"
	(
		source "$fixture/write-dependencies.sh"
		embedded_artifact_root=$directory
		read_build_info() { printf '%064d\n' 1; }
		web_stack_uses_solid() { return 0; }
		web_stack_uses_dioxus() { return 1; }
		web_surface_stack() { printf 'solid\n'; }
		first_version_line() {
			if [[ "$version_empty" == true && "$*" == 'bun --version' ]]; then
				return 0
			fi
			printf 'fixture\n'
		}
		sha256_file() {
			if [[ "$sha_failure" == true && "$1" == web/bun.lock ]]; then
				return 1
			fi
			printf '%064d\n' 1
		}
		cargo() { printf 'fixture-crate v1\n'; }
		write_dependencies "$destination" 0000000000000000000000000000000000000000 1700000000 clean \
			"$(printf '%064d' 2)" 9.9.9
	) >/dev/null 2>&1
}
if write_dependencies_case "$fixture/deps-sha" "$fixture/deps-sha/DEPENDENCIES.md" true false; then
	fail 'bun.lock 摘要失败时依赖清单不应成功'
fi
[[ ! -e "$fixture/deps-sha/DEPENDENCIES.md" ]] || fail 'bun.lock 摘要失败时写出了依赖清单'
if write_dependencies_case "$fixture/deps-version" "$fixture/deps-version/DEPENDENCIES.md" false true; then
	fail 'bun 版本为空时依赖清单不应成功'
fi
[[ ! -e "$fixture/deps-version/DEPENDENCIES.md" ]] || fail 'bun 版本为空时写出了依赖清单'

for key in \
	solid_public_initial_js_baseline_bytes \
	solid_public_initial_js_max_bytes \
	solid_public_payload_baseline_bytes \
	solid_public_payload_max_bytes; do
	grep -Eq "^${key}=[1-9][0-9]*$" ci/release/web-size-budget.v1 ||
		fail "体积预算缺少 $key"
done

printf '%s\n' 'Web 双栈构建入口回归检查通过'
