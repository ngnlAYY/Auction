#!/usr/bin/env bash

set -euo pipefail
cd "$(dirname "$0")/../.."

check_layout() {
	local catch_all flat clusters grouped_uses invalid missing_subdirectories source_files split_entries

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

	# 模块按职责命名；common/helpers/shared/misc/utils 这类兜底名会让无关辅助持续堆积。
	catch_all=$({ grep -E '/src/(.+/)?(common|helpers?|shared|misc|utils?)(/|\.rs$)' <<<"$source_files" || true; } | sort)
	if [[ -n "$catch_all" ]]; then
		printf '%s\n' "模块须按职责命名，不得使用 common/helpers/shared/misc/utils 兜底：" >&2
		printf '%s\n' "$catch_all" >&2
		return 1
	fi

	# 有子模块的模块只用 name/mod.rs 作入口；name.rs 与 name/ 并存会让同一模块的入口和实现分处两层目录。
	split_entries=$(awk '
    /\/src\// {
      if ($0 ~ /\.rs$/) files[$0] = 1
      path = $0
      while (sub("/[^/]+$", "", path)) dirs[path] = 1
    }
    END {
      for (file in files) {
        stem = file
        sub(/\.rs$/, "", stem)
        if (stem in dirs) print file " -> " stem "/mod.rs"
      }
    }
  ' <<<"$source_files" | sort)
	if [[ -n "$split_entries" ]]; then
		printf '%s\n' "目录模块的入口必须是 mod.rs，不得与同名 .rs 文件并存：" >&2
		printf '%s\n' "$split_entries" >&2
		return 1
	fi

	# 生产源码单文件不超过 600 行，目录模块入口 mod.rs 不超过 500 行；mod.rs 是声明与装配入口，
	# 实现按职责放进子模块。生产代码只用显式导入：私有 glob（含 `use super::{a::*, *}` 这类花括号
	# 形式）会让模块悄悄依赖别处的私有导入，删掉那边的导入时才在远处报错；只放行外部
	# `prelude::*`、枚举变体 glob 与 pub use 门面。两项共用 source-layout.pl 的生产范围：截止于首个
	# 后接内联 `mod 名 {` 的行首 #[cfg(test)]，tests/ 与 tests.rs、*_tests.rs 不计。
	grep -v -e '/tests/' -e 'tests\.rs$' <<<"$source_files" | grep '\.rs$' | tr '\n' '\0' |
		perl ci/checks/source-layout.pl || return 1

	# 父模块不为子模块囤积导入：子模块经 super::/crate:: 路径借用祖先模块的私有 use 导入时，祖先删掉
	# 只服务自己的导入就会在远处报错。从每个 crate 根按 mod 声明建模块树；#[cfg(test)] 修饰的模块整体
	# 不计，但不按首个内联测试模块截断文件，测试模块之后声明的生产模块照常检查。输入全部 src 源文件：
	# 其余非测试文件都必须能从某个 crate 根到达，孤儿文件失败关闭。
	grep -E '^(apps|crates)/[^/]+/src/.+\.rs$' <<<"$source_files" | tr '\n' '\0' |
		perl ci/checks/ancestor-imports.pl || return 1

	# 每个 use 单独成句；`use { a::b, c::d };` 总括分组让 rustfmt 无法按路径排序，增删时也易留空行。
	grouped_uses=$(grep '\.rs$' <<<"$source_files" |
		xargs grep -nE '^[[:space:]]*(pub(\([^)]*\))?[[:space:]]+)?use[[:space:]]*\{' || true)
	if [[ -n "$grouped_uses" ]]; then
		printf '%s\n' "不得使用 use { … } 总括分组，请拆成逐条 use：" >&2
		printf '%s\n' "$grouped_uses" >&2
		return 1
	fi
}

check_repository_contracts() {
	local file failures=""

	for file in AGENTS.md .gitignore .editorconfig README.md Cargo.lock mise.toml mise.lock; do
		if ! git ls-files --error-unmatch "$file" >/dev/null 2>&1; then
			failures+="$file 未纳入版本控制"$'\n'
		elif git check-ignore --no-index -q "$file"; then
			failures+="$file 被仓库 .gitignore 忽略"$'\n'
		fi
	done
	# 已跟踪文件一旦命中忽略规则，rg 类门禁会静默跳过它；强制添加的本地配置也由此暴露。
	while IFS= read -r file; do
		[[ -n "$file" ]] && failures+="$file 已纳入版本控制却被 .gitignore 忽略"$'\n'
	done < <(git ls-files -ci --exclude-standard)
	if [[ -n "$failures" ]]; then
		printf '%s\n' "项目契约文件的 Git 规则无效：" >&2
		printf '%s' "$failures" >&2
		return 1
	fi
}

# 仓库公开：本机用户目录、192.168/16 内网地址与本机测试库名会暴露开发机与部署拓扑，一律不得入库。
# 示例与夹具改用 /srv、/var/lib 等系统路径与 example.com、RFC 5737 文档地址。本机测试库按角色命名
# （auction-test- 后接角色名、花括号展开或通配符）；crate 名 auction-test-support、指标测试的进程名与
# CI 缓存键不带这些后缀，不会命中。
# 三类扫描都遵循 .gitignore，被忽略的本地运行配置、草稿与原始采集不读取。rg 的 -g 包含型 glob
# 会覆盖 .gitignore（还会压过排在它前面的排除 glob），所以范围只能用搜索根与 --type 限定，-g 只用于排除。
# 三类扫描都跑完再失败，一次报出全部违规；rg 自身出错（退出码 > 1）立即失败关闭。
check_public_redaction() {
	local addresses hits ipv4 markdown_addresses failed=0 status=0
	local -a fixture_roots=() roots=(./docs ./ops ./ci ./.github)
	hits=$(rg --hidden --line-number --with-filename -g '!.git' -g '!**/target/**' -g '!Cargo.lock' \
		-e '/home/[a-z_][a-z0-9_-]*/' -e '/Users/[A-Za-z][A-Za-z0-9._-]*/' \
		-e '(?-u:\b)192\.168\.[0-9]{1,3}\.[0-9]{1,3}(?-u:\b)' \
		-e 'auction-test-(?:public|admin|worker|\{|\*)' -- .) || status=$?
	if ((status > 1)); then
		printf '%s\n' "公开仓库脱敏扫描失败（rg 退出码 $status）。" >&2
		return 1
	fi
	if [[ -n "$hits" ]]; then
		printf '%s\n' "公开仓库不得包含本机用户目录、内网地址或本机测试库名（文件:行）：" >&2
		printf '%s\n' "$hits" >&2
		failed=1
	fi

	# 真实公网地址会暴露服务器。只扫夹具、文档与运维/CI 文件：源码中的四段版本号等字面量不是地址。
	# 搜索根下的非 Markdown 文件与全仓 Markdown 分两次扫描，范围互不重叠；搜索根带 ./ 前缀，与其余扫描的输出路径一致。
	# (?-u:\b) 按 ASCII 判定词边界：中文紧贴数字时 Unicode 词边界不成立，地址会漏检。
	# Caddyfile 的 Cloudflare 回源网段是公开的固定配置，按文件豁免。
	shopt -s nullglob
	fixture_roots=(./{apps,crates}/*/tests/fixtures)
	roots+=("${fixture_roots[@]}" ./{apps,crates}/*/tests/assets-fixture)
	shopt -u nullglob
	ipv4='(?-u:\b)(?:[0-9]{1,3}\.){3}[0-9]{1,3}(?-u:\b)'
	status=0
	addresses=$(rg --hidden --line-number --with-filename --only-matching -g '!.git' -g '!**/target/**' \
		-g '!ops/deploy/caddy/Caddyfile' --type-not md -e "$ipv4" -- "${roots[@]}") || status=$?
	if ((status > 1)); then
		printf '%s\n' "公开仓库 IPv4 扫描失败（rg 退出码 $status）。" >&2
		return 1
	fi
	status=0
	markdown_addresses=$(rg --hidden --line-number --with-filename --only-matching -g '!.git' \
		-g '!**/target/**' --type md -e "$ipv4" -- .) || status=$?
	if ((status > 1)); then
		printf '%s\n' "公开仓库 Markdown IPv4 扫描失败（rg 退出码 $status）。" >&2
		return 1
	fi
	# 只放行回环、0.0.0.0 与 RFC 5737 文档段；空行来自某次扫描无命中，同样放行。
	# rg 无匹配（退出码 1）表示全部地址都在白名单内。
	status=0
	hits=$(printf '%s\n' "$addresses" "$markdown_addresses" | rg --invert-match -e '^$' \
		-e ':(127\.[0-9]+\.[0-9]+\.[0-9]+|0\.0\.0\.0|192\.0\.2\.[0-9]+|198\.51\.100\.[0-9]+|203\.0\.113\.[0-9]+)$' \
		-- -) || status=$?
	if ((status > 1)); then
		printf '%s\n' "公开仓库 IPv4 白名单过滤失败（rg 退出码 $status）。" >&2
		return 1
	fi
	if [[ -n "$hits" ]]; then
		printf '%s\n' "夹具、文档与运维文件的 IPv4 只允许回环、0.0.0.0 与 RFC 5737 文档段（文件:行:地址）：" >&2
		printf '%s\n' "$hits" >&2
		failed=1
	fi

	# 采集页面残留的用户脚本样式、节点与属性会公开采集账号使用的自动化工具；标记取自已清理的注入。
	# 没有夹具目录时必须跳过：不带搜索根的 rg 会扫整个仓库，命中本脚本与 README 中的标记原文。
	if ((${#fixture_roots[@]} > 0)); then
		status=0
		hits=$(rg --hidden --line-number --with-filename --only-matching --ignore-case -g '!.git' \
			-g '!**/target/**' -e 'data-aad-' -e 'id="aad' -e 'hvAutoAttack' -e 'PonySolver' \
			-e 'id="lastEncounter"' -e '<style id="jpx"' -e 'jpx-widget' -- "${fixture_roots[@]}") || status=$?
		if ((status > 1)); then
			printf '%s\n' "夹具用户脚本残留扫描失败（rg 退出码 $status）。" >&2
			return 1
		fi
		if [[ -n "$hits" ]]; then
			printf '%s\n' "采集夹具不得残留浏览器用户脚本注入的样式、节点或属性（文件:行:标记）：" >&2
			printf '%s\n' "$hits" >&2
			failed=1
		fi
	fi
	return "$failed"
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

	# 每个 service 模板都必须声明资源上限并关闭核心转储。
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
		printf '%s\n' "systemd 服务模板缺少资源上限：" >&2
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
	local actual_locales base candidate directory dotted duplicates expected_locales missing referenced repeated web_directory
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
		# Solid 实现（web/<surface>）与 Rust 实现共用同一份译文；它的 t("键") 调用同样必须指向已存在的键。
		web_directory="web/${surface#auction-web-}/src"
		referenced=$(
			{
				rg --files "crates/$surface/src" -g '*.rs' |
					xargs perl -0777 -ne 'while (/\bt!\(\s*"([A-Za-z0-9_.-]+)"/g) { print "$1\n" }'
				# 尚未编写的 surface 目录里没有源码：rg 以 1 退出，xargs -r 不启动 perl。
				if [[ -d "$web_directory" ]]; then
					{ rg --files "$web_directory" -g '*.ts' -g '*.tsx' || true; } |
						xargs -r perl -0777 -ne 'while (/\bt\(\s*"([A-Za-z0-9_.-]+)"/g) { print "$1\n" }'
				fi
			} | sort -u
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

	# FTL 键必须在同一 surface 源码的字符串字面量中可达；动态拼接规则见脚本头部说明。
	# 两套实现共用译文：键在 Rust 实现或 Solid 实现（web/<surface>、web/shared）任一处可达即可。
	for surface in auction-web-public auction-web-admin; do
		web_directory="web/${surface#auction-web-}/src"
		{
			rg --files "crates/$surface/src" -g '*.rs' --null
			rg --files crates/auction-web-shared/src -g '*.rs' --null
			if [[ -d "$web_directory" ]]; then
				rg --files "$web_directory" web/shared/src -g '*.ts' -g '*.tsx' --null || true
			fi
		} | perl ci/checks/i18n-unused-keys.pl "crates/$surface/i18n/en.ftl" || return 1
	done

	# 译文变量与调用参数一致：其他语言只能引用英文译文的变量，字面量键调用须传入全部变量。
	for surface in auction-web-public auction-web-admin; do
		directory="crates/$surface/i18n"
		{
			rg --files "crates/$surface/src" -g '*.rs' --null
			rg --files crates/auction-web-shared/src -g '*.rs' --null
		} | perl ci/checks/i18n-placeholders.pl "$directory/en.ftl" "$directory/ja.ftl" \
			"$directory/ko.ftl" "$directory/zh-CN.ftl" "$directory/zh-TW.ftl" || return 1
	done
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

# SQL 只在 auction-storage；每条 audit_records 写入须在执行语句内接入审计失败分类，口径见 audit-writes.pl。
check_audit_writes() {
	rg --files --null crates/auction-storage/src -g '*.rs' -g '!**/target/**' |
		perl ci/checks/audit-writes.pl
}

# 导入按 std → 外部 crate → 本 crate 三组排列，模块级私有 use 只出现在模块开头的导入区；稳定版
# rustfmt 只排序不分组，口径见 import-groups.pl。
check_import_groups() {
	rg --files --null apps crates -g '*.rs' -g '!**/target/**' | perl ci/checks/import-groups.pl
}

# 每个非测试源码文件以 //! 说明模块职责；测试文件由被测模块说明，不重复要求。
check_module_docs() {
	local missing
	missing=$(rg --files --null apps/*/src crates/*/src -g '*.rs' \
		-g '!**/tests/**' -g '!**/tests.rs' -g '!**/*_tests.rs' |
		xargs -0 awk 'FNR == 1 { if ($0 !~ /^\/\/!/) print FILENAME; nextfile }' | sort)
	if [[ -n "$missing" ]]; then
		printf '%s\n' "源码文件必须以 //! 模块说明开头：" >&2
		printf '%s\n' "$missing" >&2
		return 1
	fi
}

# Admin 参数页不能依赖后端 crate，只能镜像应用层的热字段闭集；两张表必须逐项同序，
# 否则参数页会把需要重启的字段标成“立即生效”，或反过来。
check_hot_fields() {
	local owner=crates/auction-application/src/ports/admin/runtime_config/hot.rs
	local mirror=crates/auction-web-admin/src/surface/config/hot_fields.rs
	local owner_paths mirror_paths
	owner_paths=$(hot_field_paths WORKER_HOT_FIELD_PATHS "$owner")
	mirror_paths=$(hot_field_paths HOT_FIELDS "$mirror")
	if [[ -z "$owner_paths" || -z "$mirror_paths" ]]; then
		printf '%s\n' "无法从 $owner 或 $mirror 读取热字段表" >&2
		return 1
	fi
	if ! diff <(printf '%s\n' "$owner_paths") <(printf '%s\n' "$mirror_paths") >&2; then
		printf '%s\n' "热字段表不一致：$owner（<）与 $mirror（>）必须逐项同序" >&2
		return 1
	fi
}

# 输出常量数组内的字符串字面量，每行一个；数组以行首 ]; 结束。
hot_field_paths() {
	awk -v name="$1" '
		$0 ~ "const " name ": " { inside = 1; next }
		inside && /^\];/ { exit }
		inside && match($0, /"[^"]+"/) { print substr($0, RSTART, RLENGTH) }
	' "$2"
}

# src 的 #[path] 由计数棘轮（all:#[path]，基线 0）覆盖；tests 不在棘轮范围内，这里单独兜底。
check_test_module_paths() {
	local failures="" file
	while IFS= read -r -d '' file; do
		if grep -HnE '#!?\[path[[:space:]]*=' "$file"; then
			failures=1
		elif [[ $? -gt 1 ]]; then
			printf '%s\n' "无法读取 $file" >&2
			return 1
		fi
	done < <(find apps crates -path '*/tests/*.rs' -print0)
	if [[ -n "$failures" ]]; then
		printf '%s\n' "测试树不得用 #[path] 重排模块；请把文件放到自然目录层级。" >&2
		return 1
	fi
}

# 历史计划台账已删除；仓库文件（含 .github/ 与 docs/）不得再引用其工作项编号或决策编号。
# rg 遵循 .gitignore 并跳过二进制文件；已发布迁移 SQL 逐字节不可改，锁文件由工具生成，
# 测试夹具是外部语料，均不扫描。W11.4 是迁移演练回执的协议固定值（模板、校验与文档一致），
# 只豁免这一个编号本身，同一行的其他编号照常报错。
# 台账的切片与决策措辞（本切片、按决策）同样不得留在 Rust 源码与 Markdown 文档中；ci/README.md
# 列出词表本身，按文件豁免。“本批”在源码中另有运行期批次含义，不列入。
check_work_item_ids() {
	local failed=0 hits phrases status=0
	# 显式传入 `.`，避免 rg 在非终端环境改读标准输入；退出码 1 只表示没有匹配。
	hits=$(rg --hidden --only-matching --line-number --with-filename \
		-g '!.git' -g '!**/target/**' -g '!migrations/postgres/*.sql' -g '!Cargo.lock' \
		-g '!mise.lock' -g '!**/tests/fixtures/**' -g '!**/tests/assets-fixture/**' \
		-e '\bW[0-9]+\.[0-9]+\b' -e '决策 ?[0-9]+' -e 'D-W[0-9]' -- .) || status=$?
	if ((status > 1)); then
		printf '%s\n' "工作项编号扫描失败（rg 退出码 $status）。" >&2
		return 1
	fi
	hits=$(grep -v ':W11\.4$' <<<"$hits" || true)
	if [[ -n "$hits" ]]; then
		printf '%s\n' "不得引用已删除计划台账的工作项或决策编号（文件:行:编号）：" >&2
		printf '%s\n' "$hits" >&2
		failed=1
	fi

	status=0
	phrases=$(rg --hidden --only-matching --line-number --with-filename --type rust --type md \
		-g '!.git' -g '!**/target/**' -g '!**/tests/fixtures/**' -g '!**/tests/assets-fixture/**' \
		-g '!ci/README.md' -e '本切片' -e '按决策' -- .) || status=$?
	if ((status > 1)); then
		printf '%s\n' "计划措辞扫描失败（rg 退出码 $status）。" >&2
		return 1
	fi
	if [[ -n "$phrases" ]]; then
		printf '%s\n' "源码与文档不得沿用已删除计划台账的切片或决策措辞（文件:行:措辞）：" >&2
		printf '%s\n' "$phrases" >&2
		failed=1
	fi
	return "$failed"
}

check_markdown_links() {
	rg --files --hidden --null -g '*.[mM][dD]' -g '!.git' -g '!**/target/**' |
		perl ci/checks/markdown-links.pl
}

check_port_coverage() {
	perl ci/checks/port-coverage.pl "$@"
}

check_migration_compatibility() {
	perl ci/checks/migration-compatibility.pl migrations/postgres/*.up.sql
}

check_published_migrations() {
	bash ci/checks/published-migrations.sh
}

check_web_build_inputs() (
	source ci/release/build-identity.sh
	cargo metadata --format-version 1 --no-deps --locked --offline |
		perl ci/checks/web-build-inputs.pl "${web_workspace_dependency_paths[@]}"
)

# 本地 mise 任务与 CI 共用命令定义，避免编译参数在两处漂移。
run_checks() {
	local check
	for check in "$@"; do
		case "$check" in
		layout) check_layout ;;
		module-docs) check_module_docs ;;
		hot-fields) check_hot_fields ;;
		config) check_config_examples ;;
		systemd-templates) check_systemd_templates ;;
		i18n) check_i18n ;;
		equipment-contract) bash ci/checks/equipment-contract.sh ;;
		ratchet) check_source_ratchet ;;
		ratchet-counts) check_source_ratchet --counts ;;
		audit-writes) check_audit_writes ;;
		import-groups) check_import_groups ;;
		test-module-paths) check_test_module_paths ;;
		work-item-ids) check_work_item_ids ;;
		markdown-links) check_markdown_links ;;
		port-coverage) check_port_coverage --uncovered ;;
		migration-compatibility) check_migration_compatibility ;;
		published-migrations) check_published_migrations ;;
		web-build-inputs) check_web_build_inputs ;;
		dependency-versions) check_dependency_versions ;;
		repository) check_repository_contracts ;;
		public-redaction) check_public_redaction ;;
		fmt) cargo fmt --all -- --check ;;
		check) cargo check --locked --workspace --all-targets ;;
		lint) cargo clippy --locked --workspace --all-targets --all-features -- -D warnings ;;
		wasm)
			# cfg(target_arch = "wasm32") 代码只在该目标下编译，宿主 lint 覆盖不到；两端分开调用以保留各自的 feature 图。
			cargo clippy --locked --target wasm32-unknown-unknown \
				-p auction-web-public -p auction-web-shared -p auction-contracts -- -D warnings
			cargo clippy --locked --target wasm32-unknown-unknown -p auction-web-admin -- -D warnings
			;;
		rustdoc)
			# 文档注释里的符号链接、HTML 标签与代码块属性须能被 rustdoc 解析；失效链接是过期注释的信号。
			RUSTDOCFLAGS="-D rustdoc::all" cargo doc --locked --workspace --no-deps --document-private-items --all-features
			;;
		dependency-audit) bash ci/checks/dependency-audit.sh Cargo.lock ;;
		release-dependency-audit) bash ci/checks/release-dependency-audit.sh ;;
		dependency-unused) cargo machete ;;
		all) run_checks repository public-redaction layout import-groups module-docs hot-fields config systemd-templates i18n equipment-contract ratchet audit-writes test-module-paths work-item-ids markdown-links migration-compatibility published-migrations web-build-inputs ;;
		rust) run_checks fmt check dependency-versions lint wasm rustdoc ;;
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
