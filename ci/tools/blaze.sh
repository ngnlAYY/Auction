#!/usr/bin/env bash
# 仅供显式调用的本地 Blaze 试验；普通 Cargo、静态门禁和发布入口保持独立。
set -euo pipefail

repository_root=$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd -P)
cd "$repository_root"

usage() {
  printf '%s\n' \
    '用法：just blaze <build|test|status|stop> [Blaze 参数]' \
    '示例：just blaze build -p auctionctl' \
    '      just blaze test -p auction-domain --lib' \
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

# XDG_RUNTIME_DIR 只在本进程树生效，隔离 daemon，继续复用用户已安装的工具链。
# CARGO_TARGET_DIR 是已核对版本支持的隔离方式；该版本拒绝 --target-dir。
umask 077
mkdir -p "$repository_root/target/blaze/runtime"
unset BLAZE_HOME RA_JIT_HOME BLAZE_REMOTE RA_JIT_REMOTE
export XDG_RUNTIME_DIR="$repository_root/target/blaze/runtime"
export CARGO_TARGET_DIR="$repository_root/target/blaze/build"
export BLAZE_CACHE="$repository_root/target/blaze/cache"
export CARGO_BUILD_JOBS="${CARGO_BUILD_JOBS:-2}"
export RUSTC_WRAPPER='' RUSTC_WORKSPACE_WRAPPER=''
# 每次显式 test 都实际运行选定测试，不复用 Blaze 的测试通过缓存。
export BLAZE_TESTS=all

blaze --version >&2
exec blaze "${arguments[@]}"
