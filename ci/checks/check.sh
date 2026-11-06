#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/../.."

check_layout() {
	local flat clusters invalid missing_subdirectories source_files

	# 所有规则共用一次文件清单，避免按每个 crate 反复遍历源码树。
	source_files=$(rg --files apps crates)
	flat=$(awk -F/ '/\/src\/[^/]+$/ { name=$NF; if (name != "lib.rs" && name != "main.rs") print }' <<<"$source_files")
	if [[ -n "$flat" ]]; then
		printf '%s\n' "源码实现不得平铺在 src 根目录：" >&2
		printf '%s\n' "$flat" >&2
		return 1
	fi

	missing_subdirectories=$(
		awk '
        BEGIN {
          for (i = 1; i < ARGC - 1; i++) {
            roots[i] = ARGV[i]
            ARGV[i] = ""
          }
        }
        /^(apps|crates)\/[^/]+\/src\/[^/]+\// {
          root = $0
          sub(/\/src\/.*/, "/src", root)
          found[root] = 1
        }
        END {
          for (i = 1; i < ARGC - 1; i++) {
            if (!(roots[i] in found)) print roots[i]
          }
        }
      ' apps/*/src crates/*/src - <<<"$source_files"
	)
	if [[ -n "$missing_subdirectories" ]]; then
		printf '%s\n' "每个模块的 src 都必须包含按职责划分的源码子目录：" >&2
		printf '%s\n' "$missing_subdirectories" >&2
		return 1
	fi

	clusters=$(awk -F/ '
    /\.rs$/ {
      file=$NF
      dir=$0
      sub("/[^/]+$", "", dir)
      stem=file
      sub(/\.rs$/, "", stem)
      split(stem, parts, "_")
      if (file != "mod.rs" && file != "lib.rs" && file != "main.rs") {
        count[dir SUBSEP parts[1]]++
      }
    }
    END {
      for (key in count) {
        if (count[key] >= 3) {
          split(key, values, SUBSEP)
          print values[1] ": " values[2] "_* (" count[key] " files)"
        }
      }
    }
  ' <<<"$source_files" | sort)
	if [[ -n "$clusters" ]]; then
		printf '%s\n' "同一目录存在至少三个同前缀源码文件，请按职责建立子目录：" >&2
		printf '%s\n' "$clusters" >&2
		return 1
	fi

	invalid=$(awk -F/ '
    /\.rs$/ {
      file=$NF
      if (file !~ /^[a-z][a-z0-9_]*\.rs$/) {
        print
      }
      seen=0
      for (i=1; i<NF; i++) {
        if (seen && $i !~ /^[a-z][a-z0-9_]*$/) {
          print
        }
        if ($i == "src") {
          seen=1
        }
      }
    }
  ' <<<"$source_files" | sort -u)
	if [[ -n "$invalid" ]]; then
		printf '%s\n' "src 下的 Rust 文件和子目录必须使用 snake_case：" >&2
		printf '%s\n' "$invalid" >&2
		return 1
	fi
}

check_repository_contracts() {
	local file failures=""

	for file in AGENTS.md .gitignore .editorconfig README.md Cargo.lock; do
		if ! git ls-files --error-unmatch "$file" >/dev/null 2>&1; then
			failures+="$file 未纳入版本控制"$'\n'
		elif git check-ignore --no-index -q "$file"; then
			failures+="$file 被仓库 .gitignore 忽略"$'\n'
		fi
	done
	if [[ -n "$failures" ]]; then
		printf '%s\n' "项目契约文件的 Git 规则无效：" >&2
		printf '%s' "$failures" >&2
		return 1
	fi
}

check_systemd_templates() {
	local failures

	failures=$(
		for file in ops/deploy/systemd/auction-public-api.service.example \
			ops/deploy/systemd/auction-admin-api.service.example; do
			awk -v file="$file" '
        BEGIN { section = ""; bad = 0 }
        /^[[:space:]]*\[/ { section = $0; next }
        section == "[Unit]" && /^[[:space:]]*(After|Wants|Requires)[[:space:]]*=/ {
          key = $0
          sub(/[[:space:]]*=.*/, "", key)
          gsub(/[[:space:]]/, "", key)
          value = $0
          sub(/^[^=]*=[[:space:]]*/, "", value)
          count = split(value, deps, /[[:space:]]+/)
          for (i = 1; i <= count; i++) {
            dep = deps[i]
            if (dep == "") continue
            # 角色之间只能 Wants=：Requires= 会把停止与失败传播到 Public/Admin。
            if (key == "Requires" && (dep == "auction-worker.service" || dep == "auction-admin-api.service")) {
              printf "%s:%d: 角色依赖不得使用 Requires=：%s=%s\n", file, FNR, key, dep
              bad = 1
            }
            if (key == "After" || key == "Wants") seen[key SUBSEP dep] = 1
          }
        }
        END {
          targets = (file ~ /public-api/) ? "auction-worker.service auction-admin-api.service" : "auction-worker.service"
          n = split(targets, required, " ")
          for (i = 1; i <= n; i++) {
            if (!(("After" SUBSEP required[i]) in seen)) {
              printf "%s: [Unit] After= 缺少 %s\n", file, required[i]
              bad = 1
            }
            if (!(("Wants" SUBSEP required[i]) in seen)) {
              printf "%s: [Unit] Wants= 缺少 %s\n", file, required[i]
              bad = 1
            }
          }
          exit bad
        }
      ' "$file"
		done
	)
	if [[ -n "$failures" ]]; then
		printf '%s\n' "systemd 角色模板的依赖与顺序约束无效：" >&2
		printf '%s' "$failures" >&2
		return 1
	fi

	# W9.6：每个 service 模板都必须声明资源上限并关闭核心转储。
	# 上限值属于主机容量判断，这里只要求存在、非空且 LimitCORE 恰为 0，
	# 避免新增 unit 或复制模板时悄悄丢掉这些指令。
	failures=$(
		for file in ops/deploy/systemd/*.service.example; do
			awk -v file="$file" '
        BEGIN { section = ""; bad = 0 }
        /^[[:space:]]*\[/ { section = $0; next }
        section ~ /^\[Service\]/ {
          key = $0
          sub(/[[:space:]]*=.*/, "", key)
          gsub(/[[:space:]]/, "", key)
          value = $0
          sub(/^[^=]*=[[:space:]]*/, "", value)
          gsub(/[[:space:]]/, "", value)
          if (key != "MemoryMax" && key != "TasksMax" && key != "LimitCORE") next
          seen[key] = 1
          if (value == "") {
            printf "%s:%d: %s= 不能为空\n", file, FNR, key
            bad = 1
          } else if (key == "LimitCORE" && value != "0") {
            printf "%s:%d: LimitCORE= 必须为 0\n", file, FNR
            bad = 1
          }
        }
        END {
          if (!("MemoryMax" in seen)) { printf "%s: [Service] 缺少 MemoryMax=\n", file; bad = 1 }
          if (!("TasksMax" in seen)) { printf "%s: [Service] 缺少 TasksMax=\n", file; bad = 1 }
          if (!("LimitCORE" in seen)) { printf "%s: [Service] 缺少 LimitCORE=0\n", file; bad = 1 }
          exit bad
        }
      ' "$file"
		done
	)
	if [[ -n "$failures" ]]; then
		printf '%s\n' "systemd 服务模板缺少资源上限（W9.6）：" >&2
		printf '%s' "$failures" >&2
		return 1
	fi
}

check_config_examples() {
	local missing

	missing=$(
		while IFS= read -r file; do
			awk '
        BEGIN { previous = "" }
        /^[[:space:]]*#[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*=/ {
          if (previous !~ /(可选|必填)/) {
            print FILENAME ":" FNR ":" $0
          }
          previous = $0
          next
        }
        /^[[:space:]]*#/ { previous = $0; next }
        /^[[:space:]]*$/ { previous = ""; next }
        /^[[:space:]]*\[/ { previous = $0; next }
        /^[[:space:]]*[A-Za-z0-9_-]+[[:space:]]*=/ {
          if (previous !~ /(可选|必填)/) {
            print FILENAME ":" FNR ":" $0
          }
          previous = $0
          next
        }
        { previous = $0 }
      ' "$file"
		done < <(rg --files ops/config/examples -g '*.toml' | sort)
	)
	if [[ -n "$missing" ]]; then
		printf '%s\n' "配置示例字段缺少紧邻的中文可选/必填注释：" >&2
		printf '%s\n' "$missing" >&2
		return 1
	fi
}

fluent_ids() {
	sed -nE 's/^([A-Za-z][A-Za-z0-9_-]*)[[:space:]]*=.*$/\1/p' "$1" | sort
}

check_i18n() {
	local actual_locales base candidate directory dotted duplicates expected_locales missing referenced repeated
	expected_locales=$(printf '%s\n' en.ftl ja.ftl ko.ftl zh-CN.ftl zh-TW.ftl)

	for surface in auction-web-public auction-web-admin; do
		directory="crates/$surface/i18n"
		base="$directory/en.ftl"
		actual_locales=$(rg --files "$directory" -g '*.ftl' | sed 's|.*/||' | sort)
		if [[ "$actual_locales" != "$expected_locales" ]]; then
			printf '%s\n' "$directory 必须且只能包含五种受支持语言：" >&2
			diff -u <(printf '%s\n' "$expected_locales") <(printf '%s\n' "$actual_locales") || true
			return 1
		fi

		for locale in ja ko zh-CN zh-TW; do
			candidate="$directory/$locale.ftl"
			if ! diff -u <(fluent_ids "$base") <(fluent_ids "$candidate"); then
				printf '%s\n' "$candidate 的消息键与 $base 不一致" >&2
				return 1
			fi
		done

		# $1 是 Perl 捕获组，不是 Shell 参数，必须保留单引号避免 Shell 展开。
		# shellcheck disable=SC2016
		referenced=$(
			rg --files "crates/$surface/src" -g '*.rs' |
				xargs perl -0777 -ne 'while (/\bt!\(\s*"([A-Za-z0-9_.-]+)"/g) { print "$1\n" }' |
				sort -u
		)
		missing=$(comm -23 <(printf '%s\n' "$referenced") <(fluent_ids "$base" | uniq))
		if [[ -n "$missing" ]]; then
			printf '%s\n' "$surface 引用了不存在的 Fluent 消息 ID：" >&2
			printf '%s\n' "$missing" >&2
			return 1
		fi
	done

	dotted=$(rg -n '^[A-Za-z][A-Za-z0-9_-]*\.[A-Za-z0-9_.-]*[[:space:]]*=' \
		crates/auction-web-public/i18n crates/auction-web-admin/i18n || true)
	if [[ -n "$dotted" ]]; then
		printf '%s\n' "Fluent 消息 ID 不得包含点号：" >&2
		printf '%s\n' "$dotted" >&2
		return 1
	fi

	duplicates=$(
		while IFS= read -r file; do
			repeated=$(fluent_ids "$file" | uniq -d)
			if [[ -n "$repeated" ]]; then
				while IFS= read -r key; do
					printf '%s: %s\n' "$file" "$key"
				done <<<"$repeated"
			fi
		done < <(rg --files crates/auction-web-public/i18n crates/auction-web-admin/i18n -g '*.ftl' | sort)
	)
	if [[ -n "$duplicates" ]]; then
		printf '%s\n' "Fluent 消息 ID 不得重复：" >&2
		printf '%s\n' "$duplicates" >&2
		return 1
	fi
}

# Axum 的 WebSocket 错误只能按同一 tungstenite 版本 downcast。服务二进制里出现第二个版本时，
# 编译、Clippy 与其它门禁都不会报错，但 v1 的 1009 close 契约会在运行期静默失效。
check_dependency_versions() {
	local versions
	if ! versions=$(cargo tree -p auction-server -e normal --locked --prefix none); then
		printf '%s\n' "无法解析 auction-server 的依赖图" >&2
		return 1
	fi
	versions=$(awk '$1 == "tungstenite" { print $2 }' <<<"$versions" | sort -u)
	if [[ -z "$versions" || $(wc -l <<<"$versions") -ne 1 ]]; then
		printf '%s\n' "auction-server 的传递依赖必须只包含一个 tungstenite 版本：" >&2
		printf '%s\n' "${versions:-<未找到>}" >&2
		return 1
	fi
}

check_source_ratchet() {
	rg --files --null apps/*/src crates/*/src -g '*.rs' -g '!**/tests/**' -g '!**/target/**' |
		perl ci/checks/source-ratchet.pl "$@"
}

check_markdown_links() {
	rg --files --hidden --null -g '*.[mM][dD]' -g '!.git' -g '!**/target/**' |
		perl ci/checks/markdown-links.pl
}

check_migration_compatibility() {
	perl ci/checks/migration-compatibility.pl migrations/postgres/*.up.sql
}

check_published_migrations() {
	bash ci/checks/published-migrations.sh
}

# 本地 Justfile 与 CI 共用命令定义，避免编译参数在两处漂移。
run_checks() {
	local check
	for check in "$@"; do
		case "$check" in
		layout) check_layout ;;
		config) check_config_examples ;;
		systemd-templates) check_systemd_templates ;;
		i18n) check_i18n ;;
		equipment-contract) bash ci/checks/equipment-contract.sh ;;
		ratchet) check_source_ratchet ;;
		ratchet-counts) check_source_ratchet --counts ;;
		markdown-links) check_markdown_links ;;
		migration-compatibility) check_migration_compatibility ;;
		published-migrations) check_published_migrations ;;
		dependency-versions) check_dependency_versions ;;
		repository) check_repository_contracts ;;
		fmt) cargo fmt --all -- --check ;;
		check) cargo check --locked --workspace --all-targets ;;
		feature)
			cargo check --locked -p auction-storage --all-targets --no-default-features --features postgres
			cargo check --locked -p auction-cache --all-targets --no-default-features --features redis
			cargo check --locked -p auction-setup --all-targets --no-default-features
			cargo check --locked -p auction-setup --all-targets --no-default-features --features postgres
			;;
		lint) cargo clippy --locked --workspace --all-targets --all-features -- -D warnings ;;
		wasm)
			# cfg(target_arch = "wasm32") 代码只在该目标下编译，宿主 lint 覆盖不到；两端分开调用以保留各自的 feature 图。
			cargo clippy --locked --target wasm32-unknown-unknown \
				-p auction-web-public -p auction-web-shared -p auction-contracts -- -D warnings
			cargo clippy --locked --target wasm32-unknown-unknown -p auction-web-admin -- -D warnings
			;;
		dependency-audit) cargo audit --file Cargo.lock --deny warnings ;;
		dependency-unused) cargo machete ;;
		all) run_checks repository layout config systemd-templates i18n equipment-contract ratchet markdown-links migration-compatibility published-migrations ;;
		rust) run_checks fmt check feature dependency-versions lint wasm ;;
		*)
			printf '未知静态检查：%s\n' "$check" >&2
			return 2
			;;
		esac
	done
}

# 无参数仍只执行策略门禁；允许 CI 一次选择多项，不隐式运行测试。
if [[ $# -eq 0 ]]; then set -- all; fi
run_checks "$@"
