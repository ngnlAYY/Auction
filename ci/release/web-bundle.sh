#!/usr/bin/env bash

set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
cd "$repository_root"

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

# Dioxus 会把继承 release 的自定义 profile 归入 release 中间目录；两个精确路径都清理，避免把历史
# 指纹文件再次复制进发布 payload，同时不触碰另一个 surface 或 Cargo 的共享依赖缓存。
rm -rf "$stable_root" "$internal_release_root" "$internal_profile_root"

source_epoch=$(git show -s --format=%ct HEAD)
if [[ ! "$source_epoch" =~ ^[1-9][0-9]*$ ]]; then
  printf '无法取得有效的 SOURCE_DATE_EPOCH：%s\n' "$source_epoch" >&2
  exit 1
fi

SOURCE_DATE_EPOCH=$source_epoch \
  dx bundle \
    -p "$package" \
    --platform web \
    --profile wasm-release \
    --debug-symbols false \
    --out-dir "$stable_root" \
    --locked

if [[ ! -f "$payload_root/index.html" ]]; then
  printf '%s Web payload 缺少 index.html：%s\n' "$surface" "$payload_root" >&2
  exit 1
fi
if ! grep -Fq "<title>$expected_title</title>" "$payload_root/index.html"; then
  printf '%s Web 入口标题不符合 surface 契约\n' "$surface" >&2
  exit 1
fi
if [[ -n $(find "$payload_root" -type l -print -quit) ]]; then
  printf '%s Web payload 禁止符号链接\n' "$surface" >&2
  exit 1
fi
if [[ -n $(find "$payload_root" ! -type f ! -type d -print -quit) ]]; then
  printf '%s Web payload 禁止特殊文件\n' "$surface" >&2
  exit 1
fi

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

# rust-embed 会保留资源 mtime；统一时间和权限使相同 payload 的二进制元数据可复核。
find "$payload_root" -type f -exec chmod 0644 {} +
find "$payload_root" -type d -exec chmod 0755 {} +
find "$payload_root" -exec touch -d "@$source_epoch" {} +
chmod 0644 "$payload_manifest"
touch -d "@$source_epoch" "$payload_manifest" "$stable_root"

printf '%s Web 发布 payload 已生成：sha256=%s files=%s\n' \
  "$surface" \
  "$payload_sha256" \
  "$(wc -l < "$payload_manifest" | tr -d ' ')"
