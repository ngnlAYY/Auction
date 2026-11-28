#!/usr/bin/env bash
# 仅供显式调用的本地 Blaze 试验；普通 Cargo、静态门禁和发布入口保持独立。
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
cd "$repository_root"

usage() {
  printf '%s\n' \
    '用法：mise run blaze <build|test|status|stop> [Blaze 参数]' \
    '示例：mise run blaze build -p auction-cli' \
    '      mise run blaze test -p auction-domain --lib' \
    '实验结果仍须由项目固定工具链检查；不用于 Web 或正式发布。'
}

if [[ $# == 0 || $1 == --help || $1 == -h ]]; then
  usage
  exit 0
fi

operation=$1
shift
case "$operation" in
  build | test)
    # 固定原生开发档，避免把实验产物当作 release 或浏览器 payload。
    for argument in "$@"; do
      [[ $argument == -- ]] && break
      case "$argument" in
        --release | -r | --profile | --profile=* | --target | --target=* | \
          --target-dir | --target-dir=* | --config | --config=*)
          printf 'Blaze 本地入口不接受 %s；发布、WASM 与配置覆盖请使用原有入口。\n' "$argument" >&2
          exit 2
          ;;
      esac
    done
    arguments=("$operation" --locked "$@")
    ;;
  status | stop)
    if [[ $# != 0 ]]; then
      usage >&2
      exit 2
    fi
    arguments=("$operation")
    ;;
  *)
    usage >&2
    exit 2
    ;;
esac

if ! command -v blaze >/dev/null 2>&1; then
  printf '未找到已安装的 blaze；本入口不会自动安装或接管全局 Cargo。\n' >&2
  exit 127
fi

# XDG_RUNTIME_DIR 必须是当前用户独占的真实目录，避免复用宽权限或符号链接目标。
# CARGO_TARGET_DIR 是已核对版本支持的隔离方式；该版本拒绝 --target-dir。
umask 077
runtime_parent="$repository_root/target/blaze"
runtime_directory="$runtime_parent/runtime"
# 构建目录与缓存同样导出给 Blaze，必须是普通目录，不能是指向隔离树之外的符号链接。
for directory in "$repository_root/target" "$runtime_parent" "$runtime_directory" \
	"$runtime_parent/build" "$runtime_parent/cache"; do
	if [[ -L "$directory" ]]; then
		printf 'Blaze 运行目录路径不得是符号链接：%s\n' "$directory" >&2
		exit 1
	fi
done
mkdir -p -- "$runtime_directory" "$runtime_parent/build" "$runtime_parent/cache"
for directory in "$runtime_directory" "$runtime_parent/build" "$runtime_parent/cache"; do
	[[ -d "$directory" ]] || { printf 'Blaze 运行路径不是目录：%s\n' "$directory" >&2; exit 1; }
	runtime_owner=$(stat -c '%u' -- "$directory")
	[[ "$runtime_owner" == "$EUID" ]] ||
		{ printf 'Blaze 运行目录不属于当前用户：%s\n' "$directory" >&2; exit 1; }
	chmod 700 -- "$directory"
done
unset BLAZE_HOME RA_JIT_HOME BLAZE_REMOTE RA_JIT_REMOTE
# 目标、档位与 flags 只由本入口决定；继承的同名环境变量会绕过上面的命令行拒绝。
while IFS= read -r variable; do
	case "$variable" in
	CARGO_BUILD_TARGET* | CARGO_PROFILE_* | CARGO_ENCODED_RUSTFLAGS | CARGO_BUILD_RUSTFLAGS | \
		CARGO_BUILD_RUSTDOCFLAGS | RUSTFLAGS | RUSTDOCFLAGS) unset "$variable" ;;
	esac
done < <(compgen -e)
export XDG_RUNTIME_DIR="$runtime_directory"
export CARGO_TARGET_DIR="$repository_root/target/blaze/build"
export BLAZE_CACHE="$repository_root/target/blaze/cache"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export RUSTC_WRAPPER='' RUSTC_WORKSPACE_WRAPPER=''
# 每次显式 test 都实际运行选定测试，不复用 Blaze 的测试通过缓存。
export BLAZE_TESTS=all

blaze --version >&2
exec blaze "${arguments[@]}"
