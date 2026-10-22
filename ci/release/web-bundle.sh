#!/usr/bin/env bash

set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
cd "$repository_root"

# 回执验证、工具选择和指纹与原生发布入口共用同一份实现。
source ci/release/build-identity.sh
configure_build_tools

surface=${1:-}
case "$surface" in
  public)
    package=auction-web-public
    expected_title='Auction 公开站点'
    ;;
  admin)
    package=auction-web-admin
    expected_title='Auction 管理站点'
    ;;
  *)
    printf '未知 Web surface：%s\n' "$surface" >&2
    exit 2
    ;;
esac

stable_root="$repository_root/target/dx/build/$package/web"
payload_root="$stable_root/public"
internal_release_root="$repository_root/target/dx/$package/release/web"
internal_profile_root="$repository_root/target/dx/$package/wasm-release/web"
marker_name=auction-web-manifest.v1
payload_manifest="$stable_root/PAYLOAD-SHA256"
# 复用标记放在 payload 之外：它不属于内嵌资源，也不计入 payload 哈希。
web_build_info="$stable_root/WEB-BUILD-INFO"

source_epoch=$(git show -s --format=%ct HEAD)
if [[ ! "$source_epoch" =~ ^[1-9][0-9]*$ ]]; then
  printf '无法取得有效的 SOURCE_DATE_EPOCH：%s\n' "$source_epoch" >&2
  exit 1
fi

# 复用判定还必须区分构建工具：dx 与 rustc 版本变化会改变生成的 JS/WASM。
dx_version=$(dx --version 2>/dev/null | sed -n '1p')
rustc_version=$(rustc --version --verbose 2>/dev/null | sed -n '1p')
if [[ -z "$dx_version" || -z "$rustc_version" ]]; then
  printf '无法读取 dx/rustc 版本，Web 构建不能建立复用标记\n' >&2
  exit 1
fi
# payload 指纹覆盖本 surface、共享层与契约源码；DTO 变化必须重建两个浏览器，避免内嵌旧协议。
source_identity=$(web_inputs_fingerprint "$surface")
# WASM 自定义 profile 与 rustflags 同样影响页面产物，不能只使外层原生二进制缓存失效。
build_environment=$(build_env_fingerprint)

# payload 结构门禁：入口、标题与文件类型；复用与新构建都必须通过。
verify_payload_files() {
  if [[ ! -f "$payload_root/index.html" || -L "$payload_root/index.html" ]]; then
    printf '%s Web payload 缺少 index.html：%s\n' "$surface" "$payload_root" >&2
    return 1
  fi
  if ! grep -Fq "<title>$expected_title</title>" "$payload_root/index.html"; then
    printf '%s Web 入口标题不符合 surface 契约\n' "$surface" >&2
    return 1
  fi
  if [[ -n $(find "$payload_root" -type l -print -quit) ]]; then
    printf '%s Web payload 禁止符号链接\n' "$surface" >&2
    return 1
  fi
  if [[ -n $(find "$payload_root" ! -type f ! -type d -print -quit) ]]; then
    printf '%s Web payload 禁止特殊文件\n' "$surface" >&2
    return 1
  fi
}

# 复用时重新计算 payload 哈希：标记与清单必须互相印证，缓存内容不能自证可信。
verify_payload_manifest() {
  local marker="$payload_root/$marker_name"
  local marker_sha actual actual_sha
  for required_file in "$marker" "$payload_manifest"; do
    if [[ ! -f "$required_file" || -L "$required_file" ]]; then
      printf '%s Web payload 缺少普通文件：%s\n' "$surface" "$required_file" >&2
      return 1
    fi
  done
  if [[ $(read_build_info "$marker" schema) != auction-web-assets/1 ||
    $(read_build_info "$marker" surface) != "$surface" ||
    $(read_build_info "$marker" profile) != wasm-release ]]; then
    printf '%s Web 制品标记无效\n' "$surface" >&2
    return 1
  fi
  actual=$(mktemp)
  (
    cd "$payload_root"
    find . -type f ! -name "$marker_name" -print0 |
      LC_ALL=C sort -z |
      xargs -0 -r sha256sum
  ) >"$actual"
  if ! cmp -s "$payload_manifest" "$actual"; then
    rm -f "$actual"
    printf '%s Web payload 与固定清单不一致\n' "$surface" >&2
    return 1
  fi
  actual_sha=$(sha256sum "$actual" | awk '{print $1}')
  rm -f "$actual"
  marker_sha=$(read_build_info "$marker" payload_sha256)
  if [[ ! "$marker_sha" =~ ^[0-9a-f]{64}$ || "$actual_sha" != "$marker_sha" ]]; then
    printf '%s Web payload 哈希与内嵌标记不一致\n' "$surface" >&2
    return 1
  fi
}

# 复用条件：源码身份、dx/rustc 版本、profile 与构建环境全部一致，且已有可校验的 payload。
# 命中后既不重新运行 dx，也不改写 mtime，保证内嵌资源与上一份制品逐字节相同。
web_payload_reusable() {
  if [[ ! -d "$stable_root" || -L "$stable_root" ]]; then
    return 1
  fi
  if [[ ! -f "$web_build_info" || -L "$web_build_info" ]]; then
    return 1
  fi
  if [[ $(read_build_info "$web_build_info" schema) != auction-web-build/1 ||
    $(read_build_info "$web_build_info" surface) != "$surface" ||
    $(read_build_info "$web_build_info" profile) != wasm-release ||
    $(read_build_info "$web_build_info" dx_version) != "$dx_version" ||
    $(read_build_info "$web_build_info" rustc_version) != "$rustc_version" ||
    $(read_build_info "$web_build_info" source_fingerprint) != "$source_identity" ||
    $(read_build_info "$web_build_info" build_env_sha256) != "$build_environment" ]]; then
    return 1
  fi
  verify_payload_files || return 1
  verify_payload_manifest || return 1
  return 0
}

if web_payload_reusable; then
  printf '%s Web payload 输入未变，复用：sha256=%s files=%s\n' \
    "$surface" \
    "$(read_build_info "$payload_root/$marker_name" payload_sha256)" \
    "$(wc -l <"$payload_manifest" | tr -d ' ')"
  exit 0
fi

# Dioxus 会把继承 release 的自定义 profile 归入 release 中间目录；两个精确路径都清理，避免把历史
# 指纹文件再次复制进发布 payload，同时不触碰另一个 surface 或 Cargo 的共享依赖缓存。
rm -rf "$stable_root" "$internal_release_root" "$internal_profile_root"

SOURCE_DATE_EPOCH=$source_epoch \
  dx bundle \
    -p "$package" \
    --platform web \
    --profile wasm-release \
    --debug-symbols false \
    --out-dir "$stable_root" \
    --locked

verify_payload_files

mkdir -p "$stable_root"
payload_next=$(mktemp "$stable_root/PAYLOAD-SHA256.next.XXXXXX")
cleanup() {
  rm -f "$payload_next"
}
trap cleanup EXIT

(
  cd "$payload_root"
  find . -type f ! -name "$marker_name" -print0 \
    | LC_ALL=C sort -z \
    | xargs -0 -r sha256sum
) > "$payload_next"

if ! grep -Fq '  ./index.html' "$payload_next"; then
  printf '%s Web payload 清单没有绑定 index.html\n' "$surface" >&2
  exit 1
fi
payload_sha256=$(sha256sum "$payload_next" | awk '{print $1}')

printf '%s\n' \
  'schema=auction-web-assets/1' \
  "surface=$surface" \
  'profile=wasm-release' \
  "payload_sha256=$payload_sha256" \
  > "$payload_root/$marker_name"
install -m 0644 "$payload_next" "$payload_manifest"

# 复用标记最后写入：中断后留下的半成品 payload 没有匹配标记，下次仍会重建。
# 独立 web-build 也必须复核输入，不能把构建期间的源码或工具变化标记成可复用的旧快照。
if [[ $(web_inputs_fingerprint "$surface") != "$source_identity" ||
  $(build_env_fingerprint) != "$build_environment" ]]; then
  printf '%s Web 构建期间输入发生变化，请重新构建\n' "$surface" >&2
  exit 1
fi

printf '%s\n' \
  'schema=auction-web-build/1' \
  "surface=$surface" \
  'profile=wasm-release' \
  "dx_version=$dx_version" \
  "rustc_version=$rustc_version" \
  "source_fingerprint=$source_identity" \
  "build_env_sha256=$build_environment" \
  > "$web_build_info"
chmod 0644 "$web_build_info"

# rust-embed 会保留资源 mtime；统一时间和权限使相同 payload 的二进制元数据可复核。
find "$payload_root" -type f -exec chmod 0644 {} +
find "$payload_root" -type d -exec chmod 0755 {} +
find "$payload_root" -exec touch -d "@$source_epoch" {} +
chmod 0644 "$payload_manifest"
touch -d "@$source_epoch" "$payload_manifest" "$web_build_info" "$stable_root"

printf '%s Web 发布 payload 已生成：sha256=%s files=%s\n' \
  "$surface" \
  "$payload_sha256" \
  "$(wc -l < "$payload_manifest" | tr -d ' ')"
