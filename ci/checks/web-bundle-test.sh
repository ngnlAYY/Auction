#!/usr/bin/env bash
# shellcheck disable=SC1091,SC2030,SC2031,SC2032,SC2034
# 验证双栈构建入口：未支持的组合在清理前失败、Solid 依赖预检、Dioxus 原路径与发布拒绝。

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

# 发布入口只允许 verify-web-size 选择 Solid；缺省入口仍可执行只读源码指纹。
if diagnostic=$(AUCTION_WEB_STACK=solid bash ci/release/release.sh build-embedded 2>&1 >/dev/null); then
	fail 'Solid 发布入口应被拒绝'
fi
[[ "$diagnostic" == *'Solid 版页面尚未接入发布构建'* ]] || fail 'Solid 发布拒绝缺少明确诊断'
default_fingerprint=$(bash ci/release/release.sh source-fingerprint 2>/dev/null)
[[ "$default_fingerprint" =~ ^[0-9a-f]{64}$ ]] || fail '缺省发布入口不应被 Solid 拒绝'

for key in \
	solid_public_initial_js_baseline_bytes \
	solid_public_initial_js_max_bytes \
	solid_public_payload_baseline_bytes \
	solid_public_payload_max_bytes; do
	grep -Eq "^${key}=[1-9][0-9]*$" ci/release/web-size-budget.v1 ||
		fail "体积预算缺少 $key"
done

printf '%s\n' 'Web 双栈构建入口回归检查通过'
