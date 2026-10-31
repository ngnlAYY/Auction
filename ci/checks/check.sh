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

# 本地 Justfile 与 CI 共用命令定义，避免编译参数在两处漂移。
run_checks() {
	local check
	for check in "$@"; do
		case "$check" in
		layout) check_layout ;;
		config) check_config_examples ;;
		i18n) check_i18n ;;
		equipment-contract) bash ci/checks/equipment-contract.sh ;;
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
			cargo check --locked -p auction-web-public --target wasm32-unknown-unknown
			cargo check --locked -p auction-web-admin --target wasm32-unknown-unknown
			;;
		dependency-audit) cargo audit --deny warnings ;;
		dependency-unused) cargo machete ;;
		all) run_checks repository layout config i18n equipment-contract ;;
		rust) run_checks fmt check feature lint wasm ;;
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
