#!/usr/bin/env bash
# 为本地与 CI 建立已修复证书校验的 rustup，不改变仓库固定工具链。
set -euo pipefail

readonly minimum_version=1.29.2
readonly archive_sha256=5be191e8c70a5007074254cb74d78dbfdd25cd5a61c27de7b8e5410122ec24f4
readonly archive_url=https://static.rust-lang.org/rustup/archive/1.29.2/x86_64-unknown-linux-gnu/rustup-init

if [[ $(uname -s) != Linux || $(uname -m) != x86_64 ]]; then
  echo 'rustup 安全自举仅支持项目的 Linux x86_64 构建平台。' >&2
  exit 1
fi

if command -v rustup >/dev/null 2>&1; then
  installed_version=$(rustup --version 2>/dev/null | awk 'NR == 1 {print $2}')
  if [[ $installed_version =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] &&
    [[ $(printf '%s\n' "$minimum_version" "$installed_version" | sort -V | head -n 1) == "$minimum_version" ]]; then
    printf 'rustup %s 已满足证书修复基线。\n' "$installed_version"
    exit 0
  fi
fi

for tool in curl openssl sha256sum awk mktemp; do
  command -v "$tool" >/dev/null 2>&1 || { echo "缺少自举工具：$tool" >&2; exit 1; }
done

bootstrap_dir=$(mktemp -d)
trap 'rm -rf -- "$bootstrap_dir"' EXIT

# 旧 runner 可能仍信任失信 CA；下载修复工具时先从临时信任集合排除它们，
# 保留系统配置不变。制品另以审阅后固定的 SHA-256 校验。
awk -v directory="$bootstrap_dir" '
  /-----BEGIN CERTIFICATE-----/ {number++; path=directory "/ca-" number ".pem"}
  path != "" {print > path}
  /-----END CERTIFICATE-----/ {close(path); path=""}
' /etc/ssl/certs/ca-certificates.crt
: > "$bootstrap_dir/trusted.pem"
for certificate in "$bootstrap_dir"/ca-*.pem; do
  subject=$(openssl x509 -in "$certificate" -noout -subject -nameopt RFC2253)
  case "$subject" in
    *'CN=emSign ECC Root CA - G3'* | *'CN=AAA Certificate Services'*) continue ;;
  esac
  cat "$certificate" >> "$bootstrap_dir/trusted.pem"
done
[[ -s $bootstrap_dir/trusted.pem ]] || { echo '安全自举信任集合为空。' >&2; exit 1; }
mkdir "$bootstrap_dir/empty-ca-directory"

# 同时覆盖默认 CA 目录并忽略用户 curlrc，避免从另一信任源补回已排除的根。
curl -q --fail --silent --show-error --location --proto '=https' --proto-redir '=https' \
  --cacert "$bootstrap_dir/trusted.pem" --capath "$bootstrap_dir/empty-ca-directory" \
  --retry 3 --connect-timeout 20 --max-time 180 \
  "$archive_url" -o "$bootstrap_dir/rustup-init"
printf '%s  %s\n' "$archive_sha256" "$bootstrap_dir/rustup-init" | sha256sum --check --status
chmod 0755 "$bootstrap_dir/rustup-init"
"$bootstrap_dir/rustup-init" -y --no-modify-path --no-update-default-toolchain --default-toolchain none

installed_rustup="${CARGO_HOME:-$HOME/.cargo}/bin/rustup"
printf '%s  %s\n' "$archive_sha256" "$installed_rustup" | sha256sum --check --status
"$installed_rustup" --version
