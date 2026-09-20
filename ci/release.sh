#!/usr/bin/env bash

set -euo pipefail

repository_root=$(git rev-parse --show-toplevel)
cd "$repository_root"

artifact_root="$repository_root/target/release-artifacts"
api_artifact_root="$artifact_root/api"
embedded_artifact_root="$artifact_root/embedded"
api_build_root="$repository_root/target/release-build/api"
embedded_build_root="$repository_root/target/release-build/embedded"
single_artifact_root="$artifact_root/single"
single_build_root="$repository_root/target/release-build/single"
# 合并变体只声明函数，底层构建及归档门禁仍由本脚本拥有。
source "$repository_root/ci/release-single.sh"
package_output_root="$repository_root/target/release-package"
api_only_public_root="$repository_root/target/auction-api-only-assets/public"
api_only_admin_root="$repository_root/target/auction-api-only-assets/admin"
assets_missing_message="web assets are not included in this binary; run \`just build-release\`"

binaries=(
  auction-public-api
  auction-admin-api
  auction-worker
  auctionctl
)

require_command() {
  local command_name=$1
  if ! command -v "$command_name" >/dev/null 2>&1; then
    printf '发布门禁缺少命令：%s\n' "$command_name" >&2
    exit 1
  fi
}

for required in cargo date diff dx find git gzip install sha256sum sort stat tar; do
  require_command "$required"
done

sha256_file() {
  sha256sum "$1" | awk '{print $1}'
}

worktree_state() {
  if [[ -z $(git status --porcelain=v1 --untracked-files=all --ignore-submodules=none) ]]; then
    printf 'clean\n'
  else
    printf 'dirty\n'
  fi
}

# 指纹覆盖全部已跟踪及未忽略的未跟踪文件，同时记录删除和 mode 变化；target 与敏感忽略文件不进入。
source_fingerprint() {
  {
    printf 'head\0%s\0' "$(git rev-parse --verify 'HEAD^{commit}')"
    git status --porcelain=v1 -z --untracked-files=all --ignore-submodules=none
    while IFS= read -r -d '' path; do
      printf 'path\0%s\0' "$path"
      if [[ -L "$path" ]]; then
        printf 'symlink\0%s\0' "$(readlink "$path")"
      elif [[ -f "$path" ]]; then
        printf 'file\0%s\0' "$(stat -c '%a:%s' "$path")"
        sha256sum "$path"
      elif [[ ! -e "$path" ]]; then
        printf 'missing\0'
      else
        printf 'special\0%s\0' "$(stat -c '%F:%a' "$path")"
      fi
    done < <(git ls-files -z --cached --others --exclude-standard | LC_ALL=C sort -z)
  } | sha256sum | awk '{print $1}'
}

read_build_info() {
  local file=$1 key=$2
  awk -F= -v wanted="$key" '$1 == wanted { print substr($0, length($1) + 2); exit }' "$file"
}

write_build_info() {
  local destination=$1 variant=$2 features=$3
  local commit state fingerprint lock_sha
  commit=$(git rev-parse --verify 'HEAD^{commit}')
  state=$(worktree_state)
  fingerprint=$(source_fingerprint)
  lock_sha=$(sha256_file Cargo.lock)
  printf '%s\n' \
    'schema=auction-release-build/1' \
    "variant=$variant" \
    "source_commit=$commit" \
    "source_worktree=$state" \
    "source_fingerprint=$fingerprint" \
    "cargo_lock_sha256=$lock_sha" \
    "features=$features" \
    >"$destination"
  chmod 0644 "$destination"
}

replace_generated_directory() {
  local next=$1 current=$2
  local backup="$current.previous.$$"
  rm -rf "$backup"
  if [[ -e "$current" ]]; then
    mv "$current" "$backup"
  fi
  if ! mv "$next" "$current"; then
    if [[ -e "$backup" ]]; then
      mv "$backup" "$current"
    fi
    return 1
  fi
  rm -rf "$backup"
}

build_variant() {
  local variant=$1 build_root=$2 artifact_directory=$3 features=$4
  local selected_binaries=("${@:5}")
  if [[ ${#selected_binaries[@]} -eq 0 ]]; then selected_binaries=("${binaries[@]}"); fi
  local next="$artifact_root/.${variant}.next.$$"

  if [[ "$variant" == api-only ]] && { [[ -e "$api_only_public_root" ]] || [[ -e "$api_only_admin_root" ]]; }; then
    printf 'API-only 固定空资源目录不得存在：%s 或 %s\n' \
      "$api_only_public_root" "$api_only_admin_root" >&2
    exit 1
  fi
  mkdir -p "$artifact_root"
  rm -rf "$next"
  mkdir -p "$next/bin"

  # 在任何 Web/native 构建前冻结源码身份；构建期间的修改不得被事后标记成产物来源。
  write_build_info "$next/BUILD-INFO" "$variant" "$features"
  if [[ "$variant" == embedded ]]; then
    bash ci/web-bundle.sh public
    verify_build_info "$next" "$variant"
    bash ci/web-bundle.sh admin
    verify_web_root public >/dev/null
    verify_web_root admin >/dev/null
  fi
  verify_build_info "$next" "$variant"

  # 强制重新展开两个 RustEmbed 类型，避免 Cargo 复用前一份 Web payload 的过程宏结果。
  if [[ -f "$build_root/CACHEDIR.TAG" ]]; then
    cargo clean --target-dir "$build_root" -p auction-public-api -p auction-admin-api
  elif [[ -e "$build_root" ]]; then
    # 该路径由本脚本独占；没有 Cargo 标记时只可能是中断后留下的未初始化生成目录。
    rm -rf "$build_root"
  fi
  local packages=() package
  for package in "${selected_binaries[@]}"; do packages+=(-p "$package"); done
  cargo build \
    --locked \
    --release \
    --target-dir "$build_root" \
    --no-default-features \
    "${packages[@]}" \
    --features "$features"
  verify_build_info "$next" "$variant"

  local binary
  for binary in "${selected_binaries[@]}"; do
    install -m 0755 "$build_root/release/$binary" "$next/bin/$binary"
  done
  if [[ "$variant" == embedded || "$variant" == single ]]; then
    install -m 0644 \
      target/dx/build/auction-web-public/web/PAYLOAD-SHA256 \
      "$next/WEB-PUBLIC-SHA256"
    install -m 0644 \
      target/dx/build/auction-web-admin/web/PAYLOAD-SHA256 \
      "$next/WEB-ADMIN-SHA256"
    install -m 0644 \
      target/dx/build/auction-web-public/web/public/auction-web-manifest.v1 \
      "$next/WEB-PUBLIC-MANIFEST.v1"
    install -m 0644 \
      target/dx/build/auction-web-admin/web/public/auction-web-manifest.v1 \
      "$next/WEB-ADMIN-MANIFEST.v1"
  fi

  verify_build_info "$next" "$variant"
  replace_generated_directory "$next" "$artifact_directory"
  printf '%s 发布二进制已生成：%s\n' "$variant" "$artifact_directory"
}

verify_web_root() {
  local surface=$1 package other expected_title other_title
  case "$surface" in
  public)
    package=auction-web-public
    other='admin'
    expected_title='Auction 公开站点'
    other_title='Auction 管理站点'
    ;;
  admin)
    package=auction-web-admin
    other='public'
    expected_title='Auction 管理站点'
    other_title='Auction 公开站点'
    ;;
  *)
    printf '未知 Web surface：%s\n' "$surface" >&2
    return 2
    ;;
  esac

  local stable_root="$repository_root/target/dx/build/$package/web"
  local root="$stable_root/public"
  local marker="$root/auction-web-manifest.v1"
  local expected_payload="$stable_root/PAYLOAD-SHA256"
  local actual_payload marker_surface marker_profile marker_sha actual_sha wasm_count js_count

  for required_file in "$root/index.html" "$marker" "$expected_payload"; do
    if [[ ! -f "$required_file" || -L "$required_file" ]]; then
      printf '%s Web payload 缺少普通文件：%s\n' "$surface" "$required_file" >&2
      return 1
    fi
  done
  if [[ -n $(find "$root" -type l -print -quit) ]] ||
    [[ -n $(find "$root" ! -type f ! -type d -print -quit) ]]; then
    printf '%s Web payload 含符号链接或特殊文件\n' "$surface" >&2
    return 1
  fi
  if ! grep -Fq "<title>$expected_title</title>" "$root/index.html" ||
    grep -Fq "$other_title" "$root/index.html"; then
    printf '%s Web 入口混入错误 surface 标题\n' "$surface" >&2
    return 1
  fi
  if grep -Fq "auction-web-$other" < <(find "$root" -type f -print); then
    printf '%s Web payload 混入 %s 文件\n' "$surface" "$other" >&2
    return 1
  fi

  marker_surface=$(read_build_info "$marker" surface)
  marker_profile=$(read_build_info "$marker" profile)
  marker_sha=$(read_build_info "$marker" payload_sha256)
  if [[ $(read_build_info "$marker" schema) != auction-web-assets/1 ||
  "$marker_surface" != "$surface" ||
  "$marker_profile" != wasm-release ||
  ! "$marker_sha" =~ ^[0-9a-f]{64}$ ]]; then
    printf '%s Web 制品标记无效\n' "$surface" >&2
    return 1
  fi

  actual_payload=$(mktemp)
  (
    cd "$root"
    find . -type f ! -name auction-web-manifest.v1 -print0 |
      LC_ALL=C sort -z |
      xargs -0 -r sha256sum
  ) >"$actual_payload"
  if ! cmp -s "$expected_payload" "$actual_payload"; then
    rm -f "$actual_payload"
    printf '%s Web payload 与固定清单不一致\n' "$surface" >&2
    return 1
  fi
  actual_sha=$(sha256_file "$actual_payload")
  rm -f "$actual_payload"
  if [[ "$actual_sha" != "$marker_sha" ]]; then
    printf '%s Web payload 哈希与内嵌标记不一致\n' "$surface" >&2
    return 1
  fi

  wasm_count=$(find "$root" -type f -name "auction-web-${surface}_bg*.wasm" | wc -l | tr -d ' ')
  js_count=$(find "$root" -type f -name "auction-web-${surface}-*.js" | wc -l | tr -d ' ')
  if [[ "$wasm_count" != 1 || "$js_count" != 1 ]]; then
    printf '%s Web payload 必须且只能包含一组优化 JS/WASM：js=%s wasm=%s\n' \
      "$surface" "$js_count" "$wasm_count" >&2
    return 1
  fi

  printf '%s\n' "$marker_sha"
}

assert_binary_contains() {
  local binary=$1 marker=$2 label=$3
  if ! LC_ALL=C grep -aFq "$marker" "$binary"; then
    printf '%s 缺少内嵌标记：%s\n' "$label" "$marker" >&2
    return 1
  fi
}

assert_binary_excludes() {
  local binary=$1 marker=$2 label=$3
  if LC_ALL=C grep -aFq "$marker" "$binary"; then
    printf '%s 意外包含禁用标记：%s\n' "$label" "$marker" >&2
    return 1
  fi
}

verify_build_info() {
  local root=$1 expected_variant=$2
  local info="$root/BUILD-INFO"
  if [[ ! -f "$info" || -L "$info" ]]; then
    printf '缺少发布构建信息：%s\n' "$info" >&2
    return 1
  fi
  local current_commit current_fingerprint current_lock
  current_commit=$(git rev-parse --verify 'HEAD^{commit}')
  current_fingerprint=$(source_fingerprint)
  current_lock=$(sha256_file Cargo.lock)
  if [[ $(read_build_info "$info" schema) != auction-release-build/1 ||
  $(read_build_info "$info" variant) != "$expected_variant" ||
  $(read_build_info "$info" source_commit) != "$current_commit" ||
  $(read_build_info "$info" source_fingerprint) != "$current_fingerprint" ||
  $(read_build_info "$info" cargo_lock_sha256) != "$current_lock" ]]; then
    printf '发布构建信息与当前源码不一致，请重新构建：%s\n' "$root" >&2
    return 1
  fi
}

verify_dependency_boundaries() {
  local worker_tree public_tree admin_tree
  worker_tree=$(cargo tree --locked -e normal,build -p auction-worker)
  public_tree=$(cargo tree --locked -e normal,build -p auction-public-api)
  admin_tree=$(cargo tree --locked -e normal,build -p auction-admin-api)
  if grep -Eq 'auction-web-(public|admin|shared)' <<<"$worker_tree"; then
    printf 'Worker 依赖图不得包含浏览器 crate\n' >&2
    return 1
  fi
  if grep -Fq 'auction-web-admin' <<<"$public_tree"; then
    printf 'Public 依赖图不得包含 Admin Web crate\n' >&2
    return 1
  fi
  if grep -Fq 'auction-web-public' <<<"$admin_tree"; then
    printf 'Admin 依赖图不得包含 Public Web crate\n' >&2
    return 1
  fi
}

verify_artifacts() {
  if [[ -e "$api_only_public_root" || -e "$api_only_admin_root" ]]; then
    printf 'API-only 固定空资源目录不得存在\n' >&2
    return 1
  fi
  verify_build_info "$api_artifact_root" api-only
  verify_build_info "$embedded_artifact_root" embedded

  local public_web_sha admin_web_sha
  public_web_sha=$(verify_web_root public)
  admin_web_sha=$(verify_web_root admin)
  if [[ "$public_web_sha" == "$admin_web_sha" ]]; then
    printf 'Public/Admin Web payload 哈希不得相同\n' >&2
    return 1
  fi
  if [[ $(read_build_info "$embedded_artifact_root/WEB-PUBLIC-MANIFEST.v1" payload_sha256) != "$public_web_sha" ||
  $(read_build_info "$embedded_artifact_root/WEB-ADMIN-MANIFEST.v1" payload_sha256) != "$admin_web_sha" ]]; then
    printf '嵌入构建保存的 Web 标记与当前 payload 不一致\n' >&2
    return 1
  fi

  local variant binary path label
  for variant in api embedded; do
    for binary in "${binaries[@]}"; do
      path="$artifact_root/$variant/bin/$binary"
      if [[ ! -f "$path" || -L "$path" || ! -s "$path" || ! -x "$path" ]]; then
        printf '发布二进制无效：%s\n' "$path" >&2
        return 1
      fi
    done
  done

  for binary in "${binaries[@]}"; do
    label="API-only $binary"
    assert_binary_excludes "$api_artifact_root/bin/$binary" 'Auction 公开站点' "$label"
    assert_binary_excludes "$api_artifact_root/bin/$binary" 'Auction 管理站点' "$label"
    assert_binary_excludes "$api_artifact_root/bin/$binary" "$public_web_sha" "$label"
    assert_binary_excludes "$api_artifact_root/bin/$binary" "$admin_web_sha" "$label"
  done
  assert_binary_contains "$api_artifact_root/bin/auction-public-api" "$assets_missing_message" 'API-only Public'
  assert_binary_contains "$api_artifact_root/bin/auction-admin-api" "$assets_missing_message" 'API-only Admin'

  assert_binary_contains "$embedded_artifact_root/bin/auction-public-api" 'Auction 公开站点' 'Embedded Public'
  assert_binary_contains "$embedded_artifact_root/bin/auction-public-api" "$public_web_sha" 'Embedded Public'
  assert_binary_excludes "$embedded_artifact_root/bin/auction-public-api" 'Auction 管理站点' 'Embedded Public'
  assert_binary_excludes "$embedded_artifact_root/bin/auction-public-api" "$admin_web_sha" 'Embedded Public'
  assert_binary_contains "$embedded_artifact_root/bin/auction-admin-api" 'Auction 管理站点' 'Embedded Admin'
  assert_binary_contains "$embedded_artifact_root/bin/auction-admin-api" "$admin_web_sha" 'Embedded Admin'
  assert_binary_excludes "$embedded_artifact_root/bin/auction-admin-api" 'Auction 公开站点' 'Embedded Admin'
  assert_binary_excludes "$embedded_artifact_root/bin/auction-admin-api" "$public_web_sha" 'Embedded Admin'
  for binary in auction-worker auctionctl; do
    label="Embedded $binary"
    assert_binary_excludes "$embedded_artifact_root/bin/$binary" 'Auction 公开站点' "$label"
    assert_binary_excludes "$embedded_artifact_root/bin/$binary" 'Auction 管理站点' "$label"
    assert_binary_excludes "$embedded_artifact_root/bin/$binary" "$public_web_sha" "$label"
    assert_binary_excludes "$embedded_artifact_root/bin/$binary" "$admin_web_sha" "$label"
  done

  if cmp -s \
    "$api_artifact_root/bin/auction-public-api" \
    "$embedded_artifact_root/bin/auction-public-api" ||
    cmp -s \
      "$api_artifact_root/bin/auction-admin-api" \
      "$embedded_artifact_root/bin/auction-admin-api"; then
    printf 'API-only 与嵌入 Web 的 surface 二进制不得相同\n' >&2
    return 1
  fi

  verify_dependency_boundaries
  printf 'API-only、双 surface 嵌入资源、缺失资源 503 标记与 Worker 依赖边界门禁通过。\n'
}

workspace_version() {
  awk '
    /^\[workspace\.package\]$/ { active = 1; next }
    active && /^\[/ { active = 0 }
    active && /^version[[:space:]]*=/ {
      value = $0
      sub(/^[^=]+=[[:space:]]*"/, "", value)
      sub(/"[[:space:]]*$/, "", value)
      print value
      exit
    }
  ' Cargo.toml
}

first_version_line() {
  "$@" 2>&1 | sed -n '1p'
}

write_dependencies() {
  local destination=$1 commit=$2 epoch=$3 state=$4 fingerprint=$5 version=$6
  local public_web_sha admin_web_sha dependency_list
  public_web_sha=$(read_build_info "$embedded_artifact_root/WEB-PUBLIC-MANIFEST.v1" payload_sha256)
  admin_web_sha=$(read_build_info "$embedded_artifact_root/WEB-ADMIN-MANIFEST.v1" payload_sha256)
  dependency_list=$(mktemp)
  cargo tree --locked --workspace --all-features --prefix none --format '{p}' |
    sed "s# (${repository_root}[^)]*)# (workspace)#" |
    LC_ALL=C sort -u \
      >"$dependency_list"

  {
    printf '# 发布与依赖摘要\n\n'
    printf '## 发布证据\n\n'
    printf -- "- Schema：\`auction-release/1\`\n"
    printf -- "- Source commit：\`%s\`\n" "$commit"
    printf -- "- Application version：\`%s\`\n" "$version"
    printf -- "- Single binary：\`%s\`\n" "${single_binary:-false}"
    printf -- "- Source worktree：\`%s\`\n" "$state"
    printf -- "- Source fingerprint：\`%s\`\n" "$fingerprint"
    printf -- "- SOURCE_DATE_EPOCH：\`%s\`\n" "$epoch"
    printf -- "- Cargo.lock SHA-256：\`%s\`\n" "$(sha256_file Cargo.lock)"
    printf -- "- Public Web payload SHA-256：\`%s\`\n" "$public_web_sha"
    printf -- "- Admin Web payload SHA-256：\`%s\`\n" "$admin_web_sha"
    printf -- "- 数据库发布档位：\`PostgreSQL full\`\n"
    printf -- "- 浏览器交付：\`Public/Admin 独立 RustEmbed\`\n\n"
    printf '## 工具链\n\n'
    printf -- "- \`%s\`\n" "$(first_version_line git --version)"
    printf -- "- \`%s\`\n" "$(first_version_line rustc --version --verbose)"
    printf -- "- \`%s\`\n" "$(first_version_line cargo --version --verbose)"
    printf -- "- \`%s\`\n" "$(first_version_line dx --version)"
    printf -- "- \`%s\`\n" "$(first_version_line tar --version)"
    printf -- "- \`%s\`\n\n" "$(first_version_line gzip --version)"
    printf '## Cargo 解析依赖\n\n'
    while IFS= read -r dependency; do
      printf -- "- \`%s\`\n" "$dependency"
    done <"$dependency_list"
  } >"$destination"
  rm -f "$dependency_list"
  chmod 0644 "$destination"
}

release_targets=()
release_modes=()

install_release_file() {
  local source=$1 target=$2 mode=$3 package_root=$4
  if [[ ! -f "$source" || -L "$source" ]]; then
    printf 'release source 必须是普通文件：%s\n' "$source" >&2
    return 1
  fi
  mkdir -p "$package_root/$(dirname "$target")"
  install -m "$mode" "$source" "$package_root/$target"
  release_targets+=("$target")
  release_modes+=("$mode")
}

register_generated_file() {
  release_targets+=("$1")
  release_modes+=("$2")
}

verify_staged_package() {
  local package_root=$1 expected_list actual_list index target mode actual_mode
  expected_list=$(mktemp)
  actual_list=$(mktemp)
  printf '%s\n' "${release_targets[@]}" | LC_ALL=C sort >"$expected_list"
  (
    cd "$package_root"
    find . -type f -printf '%P\n' | LC_ALL=C sort
  ) >"$actual_list"
  if ! diff -u "$expected_list" "$actual_list"; then
    rm -f "$expected_list" "$actual_list"
    printf 'release package 文件白名单不一致\n' >&2
    return 1
  fi
  rm -f "$expected_list" "$actual_list"

  if [[ -n $(find "$package_root" -type l -print -quit) ]] ||
    [[ -n $(find "$package_root" ! -type f ! -type d -print -quit) ]]; then
    printf 'release package 禁止符号链接或特殊文件\n' >&2
    return 1
  fi
  for index in "${!release_targets[@]}"; do
    target=${release_targets[$index]}
    mode=${release_modes[$index]}
    actual_mode=$(stat -c '%a' "$package_root/$target")
    if [[ "$actual_mode" != "$mode" ]]; then
      printf 'release package mode 错误：%s=%s want=%s\n' \
        "$target" "$actual_mode" "$mode" >&2
      return 1
    fi
  done
  if [[ -n $(find "$package_root" -type f \
    \( -name '*.log' -o -name '*.log.gz' -o -name '.keyenv' -o -name '*.secret.toml' \) \
    -print -quit) ]]; then
    printf 'release package 含运行日志或真实敏感文件名\n' >&2
    return 1
  fi
}

create_archive() {
  local parent=$1 package_name=$2 destination=$3 epoch=$4
  local tar_path=${destination%.gz}
  rm -f "$tar_path" "$destination"
  LANG=C LC_ALL=C TZ=UTC tar \
    --sort=name \
    --mtime="@$epoch" \
    --owner=0 \
    --group=0 \
    --numeric-owner \
    --format=ustar \
    --create \
    --file "$tar_path" \
    --directory "$parent" \
    "$package_name"
  gzip -n -9 "$tar_path"
}

verify_archive() {
  local archive=$1 package_name=$2 epoch=$3 extraction_root=$4
  local members duplicates expected_timestamp verbose
  members=$(tar --list --gzip --file "$archive")
  if [[ -z "$members" ]]; then
    printf 'release archive 为空\n' >&2
    return 1
  fi
  while IFS= read -r member; do
    if [[ "$member" == /* || "$member" == *'/../'* || "$member" != "$package_name"/* ]]; then
      printf 'release archive member 路径非法：%s\n' "$member" >&2
      return 1
    fi
  done <<<"$members"
  duplicates=$(printf '%s\n' "$members" | LC_ALL=C sort | uniq -d)
  if [[ -n "$duplicates" ]]; then
    printf 'release archive member 重复：%s\n' "$duplicates" >&2
    return 1
  fi

  verbose=$(LANG=C LC_ALL=C TZ=UTC tar \
    --list --verbose --numeric-owner --full-time --gzip --file "$archive")
  local invalid_verbose
  invalid_verbose=$(grep -Ev '^[dl-][rwx-]{9} 0/0 ' <<<"$verbose" || true)
  if [[ -n "$invalid_verbose" ]]; then
    printf 'release archive owner 或 mode 输出异常\n' >&2
    return 1
  fi
  expected_timestamp=$(date -u -d "@$epoch" '+%Y-%m-%d %H:%M:%S')
  invalid_verbose=$(grep -Fv "$expected_timestamp" <<<"$verbose" || true)
  if [[ -n "$invalid_verbose" ]]; then
    printf 'release archive mtime 未统一为 SOURCE_DATE_EPOCH\n' >&2
    return 1
  fi

  mkdir -p "$extraction_root"
  tar --extract --gzip --file "$archive" --directory "$extraction_root"
  (
    cd "$extraction_root/$package_name"
    sha256sum --check SHA256SUMS >/dev/null
  )
}

package_release() {
  local allow_dirty=false single_binary=false argument
  for argument in "$@"; do
    case "$argument" in
    --allow-dirty) allow_dirty=true ;;
    --single-binary) single_binary=true ;;
    *)
      printf '用法：ci/release.sh package [--single-binary] [--allow-dirty]\n' >&2
      return 2
      ;;
    esac
  done
  local selected_root="$embedded_artifact_root" selected_binaries=("${binaries[@]}")
  local package_output_root="$package_output_root" variant_suffix=
  if [[ "$single_binary" == true ]]; then
    verify_single_artifacts
    selected_root="$single_artifact_root"
    selected_binaries=(auction-server auctionctl)
    package_output_root="$repository_root/target/release-package-single"
    variant_suffix=-single
  else
    verify_artifacts
  fi
  local commit state fingerprint epoch version suffix package_name temporary_root package_root
  commit=$(git rev-parse --verify 'HEAD^{commit}')
  state=$(worktree_state)
  fingerprint=$(source_fingerprint)
  epoch=$(git show -s --format=%ct "$commit")
  version=$(workspace_version)
  if [[ ! "$commit" =~ ^[0-9a-f]{40}$ || ! "$epoch" =~ ^[1-9][0-9]*$ || -z "$version" ]]; then
    printf 'release source identity 无效\n' >&2
    return 1
  fi
  if [[ "$state" == dirty && "$allow_dirty" != true ]]; then
    printf '正式发布要求干净工作树；本地证据可显式传入 --allow-dirty\n' >&2
    return 1
  fi
  suffix=
  if [[ "$state" == dirty ]]; then
    suffix=-dirty
  fi
  package_name="auction-rust-${version}-${commit:0:12}${variant_suffix}${suffix}"
  mkdir -p "$repository_root/target"
  temporary_root=$(mktemp -d "$repository_root/target/auction-release.XXXXXX")
  package_root="$temporary_root/$package_name"
  mkdir -p "$package_root"

  release_targets=()
  release_modes=()
  local binary
  for binary in "${selected_binaries[@]}"; do
    install_release_file \
      "$selected_root/bin/$binary" \
      "bin/$binary" \
      755 \
      "$package_root"
  done

  install_release_file ops/deploy/README.md docs/DEPLOYMENT.md 644 "$package_root"
  install_release_file ops/deploy/RECOVERY.md docs/RECOVERY.md 644 "$package_root"
  install_release_file ops/deploy/LEGACY_V28_ADOPTION.md docs/LEGACY_V28_ADOPTION.md 644 "$package_root"
  install_release_file ops/deploy/retention-policy.v1.json retention-policy.v1.json 644 "$package_root"
  install_release_file ops/deploy/caddy/Caddyfile caddy/Caddyfile 644 "$package_root"
  install_release_file ops/deploy/systemd/README.md systemd/README.md 644 "$package_root"
  if [[ "$single_binary" == true ]]; then
    install_release_file ops/deploy/systemd/auction-server.service.example systemd/auction-server.service.example 644 "$package_root"
    install_release_file apps/server/README.md docs/SINGLE-SERVER.md 644 "$package_root"
    install_release_file ops/deploy/caddy/setup.Caddyfile.example caddy/setup.Caddyfile.example 644 "$package_root"
  else
    install_release_file ops/deploy/systemd/auction-public-api.service.example systemd/auction-public-api.service.example 644 "$package_root"
    install_release_file ops/deploy/systemd/auction-admin-api.service.example systemd/auction-admin-api.service.example 644 "$package_root"
    install_release_file ops/deploy/systemd/auction-worker.service.example systemd/auction-worker.service.example 644 "$package_root"
    install_release_file ops/deploy/systemd/auction-stack.service.example systemd/auction-stack.service.example 644 "$package_root"
  fi
  install_release_file ops/config/README.md config/README.md 644 "$package_root"
  install_release_file ops/config/examples/common.toml config/examples/common.toml 640 "$package_root"
  install_release_file ops/config/examples/public.toml config/examples/public.toml 640 "$package_root"
  install_release_file ops/config/examples/admin.toml config/examples/admin.toml 640 "$package_root"
  install_release_file ops/config/examples/worker.toml config/examples/worker.toml 640 "$package_root"
  install_release_file ops/config/examples/runtime/admin.toml config/examples/runtime/admin.toml 640 "$package_root"
  install_release_file ops/config/examples/secrets/postgres.toml config/examples/secrets.toml.example 600 "$package_root"
  install_release_file ops/config/examples/secrets/postgres-rebuild.toml config/examples/secrets/postgres-rebuild.toml.example 600 "$package_root"
  install_release_file "$embedded_artifact_root/WEB-PUBLIC-SHA256" docs/WEB-PUBLIC-SHA256 644 "$package_root"
  install_release_file "$embedded_artifact_root/WEB-ADMIN-SHA256" docs/WEB-ADMIN-SHA256 644 "$package_root"
  install_release_file "$embedded_artifact_root/WEB-PUBLIC-MANIFEST.v1" docs/WEB-PUBLIC-MANIFEST.v1 644 "$package_root"
  install_release_file "$embedded_artifact_root/WEB-ADMIN-MANIFEST.v1" docs/WEB-ADMIN-MANIFEST.v1 644 "$package_root"

  write_dependencies \
    "$temporary_root/DEPENDENCIES.md" \
    "$commit" "$epoch" "$state" "$fingerprint" "$version"
  install_release_file "$temporary_root/DEPENDENCIES.md" docs/DEPENDENCIES.md 644 "$package_root"

  {
    printf '%s\n' \
      'schema=auction-release-manifest/1' \
      "package=$package_name" \
      "source_commit=$commit" \
      "source_fingerprint=$fingerprint" \
      'files='
    local index
    for index in "${!release_targets[@]}"; do
      printf '%s %s\n' "${release_modes[$index]}" "${release_targets[$index]}"
    done | LC_ALL=C sort
    printf '644 RELEASE-MANIFEST.v1\n'
    printf '644 SHA256SUMS\n'
  } >"$package_root/RELEASE-MANIFEST.v1"
  chmod 0644 "$package_root/RELEASE-MANIFEST.v1"
  register_generated_file RELEASE-MANIFEST.v1 644

  (
    cd "$package_root"
    printf '%s\n' "${release_targets[@]}" |
      LC_ALL=C sort |
      while IFS= read -r target; do
        printf '%s  %s\n' "$(sha256_file "$target")" "$target"
      done
  ) >"$package_root/SHA256SUMS"
  chmod 0644 "$package_root/SHA256SUMS"
  register_generated_file SHA256SUMS 644

  find "$package_root" -type d -exec chmod 0755 {} +
  find "$package_root" -exec touch -d "@$epoch" {} +
  verify_staged_package "$package_root"

  local first_archive="$temporary_root/first.tar.gz"
  local archive="$temporary_root/$package_name.tar.gz"
  create_archive "$temporary_root" "$package_name" "$first_archive" "$epoch"
  create_archive "$temporary_root" "$package_name" "$archive" "$epoch"
  if [[ $(sha256_file "$first_archive") != $(sha256_file "$archive") ]]; then
    rm -rf "$temporary_root"
    printf '同一白名单的两次 release archive 不一致\n' >&2
    return 1
  fi
  verify_archive "$archive" "$package_name" "$epoch" "$temporary_root/extracted"

  local publish_next="$repository_root/target/.release-package.next.$$"
  rm -rf "$publish_next"
  mkdir -p "$publish_next"
  install -m 0644 "$archive" "$publish_next/$package_name.tar.gz"
  install -m 0644 "$temporary_root/DEPENDENCIES.md" "$publish_next/DEPENDENCIES.md"
  (
    cd "$publish_next"
    printf '%s  %s\n' "$(sha256_file DEPENDENCIES.md)" DEPENDENCIES.md
    printf '%s  %s\n' "$(sha256_file "$package_name.tar.gz")" "$package_name.tar.gz"
  ) >"$publish_next/SHA256SUMS"
  chmod 0644 "$publish_next/SHA256SUMS"
  # 文档与配置也属于源码身份；装包期间发生变化时保留上一份发布目录，拒绝发布混合快照。
  if [[ $(git rev-parse --verify 'HEAD^{commit}') != "$commit" ||
  $(source_fingerprint) != "$fingerprint" ]]; then
    rm -rf "$temporary_root" "$publish_next"
    printf '装包期间源码发生变化，请重新构建并装包\n' >&2
    return 1
  fi
  replace_generated_directory "$publish_next" "$package_output_root"

  printf 'release package 检查通过：commit=%s state=%s fingerprint=%s\n' \
    "$commit" "$state" "$fingerprint"
  printf 'archive=%s sha256=%s\n' \
    "$package_output_root/$package_name.tar.gz" \
    "$(sha256_file "$package_output_root/$package_name.tar.gz")"
  printf 'publication_sha256=%s\n' "$(sha256_file "$package_output_root/SHA256SUMS")"
  rm -rf "$temporary_root"
}

case "${1:-}" in
release-check)
  shift
  release_check "$@"
  ;;
build-single)
  [[ $# -eq 1 ]] || exit 2
  build_single
  ;;
verify-single)
  [[ $# -eq 1 ]] || exit 2
  verify_single_artifacts
  ;;
build-api)
  if [[ $# -ne 1 ]]; then
    printf 'build-api 不接受额外参数\n' >&2
    exit 2
  fi
  build_variant api-only "$api_build_root" "$api_artifact_root" postgres
  ;;
build-embedded)
  if [[ $# -ne 1 ]]; then
    printf 'build-embedded 不接受额外参数\n' >&2
    exit 2
  fi
  # build_variant 在同一源码身份约束内重建双端 Web 和 native 制品。
  build_variant \
    embedded \
    "$embedded_build_root" \
    "$embedded_artifact_root" \
    'postgres,auction-public-api/web-embed,auction-admin-api/web-embed'
  ;;
verify-artifacts)
  if [[ $# -ne 1 ]]; then
    printf 'verify-artifacts 不接受额外参数\n' >&2
    exit 2
  fi
  verify_artifacts
  ;;
package)
  shift
  package_release "$@"
  ;;
*)
  printf '%s\n' \
    '用法：ci/release.sh release-check [--single-binary] [--allow-dirty]' \
    '      ci/release.sh build-single | verify-single' \
    '      ci/release.sh build-api' \
    '      ci/release.sh build-embedded' \
    '      ci/release.sh verify-artifacts' \
    '      ci/release.sh package [--allow-dirty]' >&2
  exit 2
  ;;
esac
