#!/usr/bin/env bash
# 验证 Web 包边界检查的放行与拒绝规则。每个用例使用独立的临时仓库，避免用例间相互影响；
# 反例匹配完整的“文件:行号: 原因”诊断，避免把检查器自身崩溃误当成越界。

set -euo pipefail
cd "$(dirname "$0")/../.."
checker=$PWD/web/tooling/check-boundaries.ts

create_fixture() {
	local fixture=$1 package
	mkdir -p "$fixture/web"/{contracts,shared,public,admin}/src
	mkdir -p "$fixture/crates/auction-web-public"/{assets,i18n}
	mkdir -p "$fixture/crates/auction-web-admin"/{assets,i18n}
	mkdir -p "$fixture/crates/auction-web-shared"/{assets,style}
	cat >"$fixture/web/package.json" <<'JSON'
{
  "name": "fixture-web",
  "private": true,
  "type": "module",
  "dependencies": {
    "@solidjs/web": "2.0.0",
    "solid-js": "2.0.0",
    "valibot": "1.5.0"
  },
  "devDependencies": {}
}
JSON
	cat >"$fixture/web/tsconfig.base.json" <<'JSON'
{
  "compilerOptions": {
    "target": "ES2023",
    "module": "ESNext",
    "moduleResolution": "bundler",
    "jsx": "preserve",
    "paths": {
      "@auction/contracts/*": ["./contracts/src/*"],
      "@auction/web-shared/*": ["./shared/src/*"]
    }
  }
}
JSON
	for package in contracts shared public admin; do
		cat >"$fixture/web/$package/tsconfig.json" <<'JSON'
{
  "extends": "../tsconfig.base.json",
  "include": ["src"]
}
JSON
	done
}

test_failed=0
check_fixture() {
	local name=$1
	local expected_status=$2
	local expected_diagnostic=$3
	local file=$4
	local fixture status=0 diagnostic path
	fixture=$(mktemp -d)
	create_fixture "$fixture"
	path="$fixture/$file"
	mkdir -p "$(dirname "$path")"
	cat >"$path"
	diagnostic=$( (cd "$fixture" && bun --no-install "$checker" "$fixture") 2>&1 ) || status=$?
	printf '%s\t预期 %s\t实际 %s\n' "$name" "$expected_status" "$status"
	if [[ $status -ne $expected_status ]]; then
		printf '%s\n' "Web 包边界检查回归失败：$name 的退出码不匹配。" "$diagnostic" >&2
		test_failed=1
	elif [[ $expected_status -eq 0 ]]; then
		if [[ -n "$diagnostic" ]]; then
			printf '%s\n' "Web 包边界检查回归失败：$name 不应报告违规。" "$diagnostic" >&2
			test_failed=1
		fi
	elif ! grep -Fqx -- "$expected_diagnostic" <<<"$diagnostic"; then
		printf '%s\n' "Web 包边界检查回归失败：$name 未给出完整诊断 $expected_diagnostic。" "$diagnostic" >&2
		test_failed=1
	fi
	rm -rf -- "$fixture"
}

check_config_failure() {
	local name=$1
	local expected_text=$2
	local fixture status=0 diagnostic
	fixture=$(mktemp -d)
	create_fixture "$fixture"
	printf '%s' "$3" >"$fixture/web/tsconfig.base.json"
	diagnostic=$( (cd "$fixture" && bun --no-install "$checker" "$fixture") 2>&1 ) || status=$?
	printf '%s\t预期 2\t实际 %s\n' "$name" "$status"
	if [[ $status -ne 2 ]] || ! grep -Fq -- "$expected_text" <<<"$diagnostic"; then
		printf '%s\n' "Web 包边界检查回归失败：$name 没有以退出码 2 报告 $expected_text。" "$diagnostic" >&2
		test_failed=1
	fi
	rm -rf -- "$fixture"
}

check_fixture allowed-public 0 '' web/public/src/allowed.ts <<'TS'
// 公共页可以导入 contracts、shared、自身与已登记资产；静态导入用登记导入名。
import type { Response } from "@auction/contracts/dto/response";
import { ok } from "@auction/web-shared/result";
import { helper } from "./helper";
import { For } from "solid-js";
import "../../../crates/auction-web-public/assets/public.css";
import en from "../../../crates/auction-web-public/i18n/en.ftl?raw";
import ja from "../../../crates/auction-web-public/i18n/ja.ftl?url";
import "../../../crates/auction-web-shared/style/base.css";
import icon from "../../../crates/auction-web-shared/assets/favicon.svg?url";
export { helper } from "@auction/web-shared/result";
const lazy = () => import("@auction/contracts/wire");
const meta = import.meta.url;
// import { banned } from "../../admin/src/banned";
const comment = 'import { banned } from "../../admin/src/banned"';
const template = `import { banned } from "../../admin/src/banned"`;
void lazy; void meta; void comment; void template; void en; void ja; void icon;
void For; void ok; void Response;
TS

check_fixture allowed-contracts 0 '' web/contracts/src/allowed.ts <<'TS'
// contracts 只引用外部依赖、自身与允许的共享资产。
import { string } from "valibot";
import { schema } from "@auction/contracts/wire";
import { local } from "./wire";
import "../../../crates/auction-web-shared/style/base.css";
void string; void schema; void local;
TS

check_fixture allowed-shared 0 '' web/shared/src/allowed.ts <<'TS'
// shared 只可反向依赖 contracts，包内相对路径不受限制。
import type { Response } from "@auction/contracts/dto/response";
import { local } from "./local";
import { other } from "./other";
import icon from "../../../crates/auction-web-shared/assets/favicon.svg?url";
void Response; void local; void other; void icon;
TS

check_fixture allowed-admin 0 '' web/admin/src/allowed.ts <<'TS'
// Admin 可以导入 contracts、shared、自身与自己的资产。
import type { Response } from "@auction/contracts/dto/response";
import { ok } from "@auction/web-shared/result";
import { local } from "./local";
import "../../../crates/auction-web-admin/assets/admin.css";
import en from "../../../crates/auction-web-admin/i18n/en.ftl?raw";
void Response; void ok; void local; void en;
TS

check_fixture allowed-test-virtual-module 0 '' web/public/src/surface/shell/navigation-fixture.test.ts <<'TS'
// 测试插件在运行期声明虚拟模块；只有点名的两个夹具文件可以非字面量 import()。
const NAMESPACE = "public-test";
const virtualModule = `${NAMESPACE}:${import.meta.dir}/fixture`;
export const load = () => import(virtualModule);
TS

check_fixture denied-other-file-virtual-module 1 "web/public/src/other.test.ts:3: 动态导入的模块路径无法静态判定；必须使用字符串字面量或无插值模板字面量" web/public/src/other.test.ts <<'TS'
const NAMESPACE = "public-test";
const virtualModule = `${NAMESPACE}:${import.meta.dir}/fixture`;
export const load = () => import(virtualModule);
TS

check_fixture denied-test-nonliteral 1 "web/public/src/bad.test.ts:2: 动态导入的模块路径无法静态判定；必须使用字符串字面量或无插值模板字面量" web/public/src/bad.test.ts <<'TS'
const target = "../../admin/src/banned";
export const load = () => import(target);
TS

check_fixture denied-require-nonliteral 1 "web/public/src/require.test.ts:2: 动态导入的模块路径无法静态判定；必须使用字符串字面量或无插值模板字面量" web/public/src/require.test.ts <<'TS'
const target = "../../admin/src/banned";
export const load = require(target);
TS

check_fixture denied-public-to-admin 1 "web/public/src/to-admin.ts:3: 跨包导入必须使用登记的导入名，不能使用相对路径：../../admin/src/banned" web/public/src/to-admin.ts <<'TS'
// 注释占两行，诊断应指向导入所在行。
// 第二个注释。
import { banned } from "../../admin/src/banned";
TS

check_fixture denied-shared-to-public 1 "web/shared/src/to-public.ts:1: 跨包导入必须使用登记的导入名，不能使用相对路径：../../public/src/banned" web/shared/src/to-public.ts <<'TS'
export { banned } from "../../public/src/banned";
TS

check_fixture denied-contracts-to-shared 1 "web/contracts/src/to-shared.ts:1: 跨包导入必须使用登记的导入名，不能使用相对路径：../../shared/src/banned" web/contracts/src/to-shared.ts <<'TS'
import { banned } from "../../shared/src/banned";
TS

check_fixture denied-public-to-backend 1 "web/public/src/to-backend.ts:1: 不允许导入 web/ 之外的源码或资产：../../../crates/auction-web-public/src/backend" web/public/src/to-backend.ts <<'TS'
import { source } from "../../../crates/auction-web-public/src/backend";
TS

check_fixture denied-public-to-admin-dynamic 1 "web/public/src/to-admin-dynamic.ts:1: 跨包导入必须使用登记的导入名，不能使用相对路径：../../admin/src/banned" web/public/src/to-admin-dynamic.ts <<'TS'
export const load = () => import("../../admin/src/banned");
TS

check_fixture denied-contracts-to-public-alias 1 "web/contracts/src/to-public-alias.ts:1: 未登记的 @auction 包名：@auction/web-public/banned" web/contracts/src/to-public-alias.ts <<'TS'
import { banned } from "@auction/web-public/banned";
TS

check_fixture denied-type-alias-import 1 "web/public/src/to-admin-type.ts:1: 跨包导入必须使用登记的导入名，不能使用相对路径：../../admin/src/banned" web/public/src/to-admin-type.ts <<'TS'
export type Banned = import("../../admin/src/banned").Banned;
TS

check_fixture denied-after-default-export 1 "web/public/src/to-admin-after-export.ts:4: 跨包导入必须使用登记的导入名，不能使用相对路径：../../admin/src/banned" web/public/src/to-admin-after-export.ts <<'TS'
export default function page() {
  return null;
}
import { banned } from "../../admin/src/banned";
TS

check_fixture denied-raw-backend-source 1 "web/public/src/to-backend-raw.ts:1: 不允许导入 web/ 之外的源码或资产：../../../crates/auction-web-public/src/backend?raw" web/public/src/to-backend-raw.ts <<'TS'
import { source } from "../../../crates/auction-web-public/src/backend?raw";
TS

check_fixture denied-unqueried-svg 1 "web/public/src/to-unqueried-svg.ts:1: 不允许导入 web/ 之外的源码或资产：../../../crates/auction-web-shared/assets/favicon.svg" web/public/src/to-unqueried-svg.ts <<'TS'
import icon from "../../../crates/auction-web-shared/assets/favicon.svg";
TS

check_fixture denied-unknown-alias 1 "web/public/src/to-unknown-alias.ts:1: 未登记的 @auction 包名：@auction/unknown/banned" web/public/src/to-unknown-alias.ts <<'TS'
import { banned } from "@auction/unknown/banned";
TS

check_fixture denied-relative-cross-package 1 "web/public/src/to-shared.ts:1: 跨包导入必须使用登记的导入名，不能使用相对路径：../../shared/src/helper" web/public/src/to-shared.ts <<'TS'
import { helper } from "../../shared/src/helper";
TS

check_fixture denied-alias-traversal 1 "web/contracts/src/to-public-traversal.ts:1: 不允许从 contracts 导入 public：@auction/contracts/../../public/src/main" web/contracts/src/to-public-traversal.ts <<'TS'
import { main } from "@auction/contracts/../../public/src/main";
TS

check_fixture denied-unicode-escape 1 "web/public/src/to-admin-unicode.ts:1: 跨包导入必须使用登记的导入名，不能使用相对路径：../../admin/src/banned" web/public/src/to-admin-unicode.ts <<'TS'
import { banned } from "../../\u0061dmin/src/banned";
TS

check_fixture denied-backtick-dynamic 1 "web/public/src/to-admin-backtick.ts:1: 跨包导入必须使用登记的导入名，不能使用相对路径：../../admin/src/banned" web/public/src/to-admin-backtick.ts <<'TS'
export const load = () => import(`../../admin/src/banned`);
TS

check_fixture denied-template-interpolation 1 "web/public/src/template-interpolation.ts:1: 跨包导入必须使用登记的导入名，不能使用相对路径：../../admin/src/banned" web/public/src/template-interpolation.ts <<'TS'
export const value = `${import("../../admin/src/banned")}`;
TS

check_fixture denied-nonliteral-dynamic 1 "web/public/src/nonliteral-dynamic.ts:2: 动态导入的模块路径无法静态判定；必须使用字符串字面量或无插值模板字面量" web/public/src/nonliteral-dynamic.ts <<'TS'
const target = "../../admin/src/banned";
export const load = () => import(target);
TS

check_fixture denied-jsx-regex-quotes 1 "web/public/src/quotes.tsx:3: 跨包导入必须使用登记的导入名，不能使用相对路径：../../admin/src/banned" web/public/src/quotes.tsx <<'TSX'
const text = <div>import("../../admin/src/banned") ' "</div>;
const pattern = /import\("\.\.\/admin/;
import { banned } from "../../admin/src/banned";
export { text, pattern, banned };
TSX

check_fixture denied-public-to-admin-css 1 "web/public/src/to-admin-css.ts:1: 不允许导入 web/ 之外的源码或资产：../../../crates/auction-web-admin/assets/admin.css" web/public/src/to-admin-css.ts <<'TS'
import "../../../crates/auction-web-admin/assets/admin.css";
TS

check_fixture denied-public-to-admin-ftl 1 "web/public/src/to-admin-ftl.ts:1: 不允许导入 web/ 之外的源码或资产：../../../crates/auction-web-admin/i18n/en.ftl?raw" web/public/src/to-admin-ftl.ts <<'TS'
import en from "../../../crates/auction-web-admin/i18n/en.ftl?raw";
TS

check_fixture denied-shared-to-tooling 1 "web/shared/src/to-tooling.ts:1: 不允许导入 web/tooling 的源码：../../tooling/test-runtime" web/shared/src/to-tooling.ts <<'TS'
import { runtime } from "../../tooling/test-runtime";
TS

check_fixture denied-absolute-path 1 "web/public/src/absolute.ts:1: 模块说明符不能是绝对路径或 URL：/auction/web/public/src/main" web/public/src/absolute.ts <<'TS'
import { main } from "/auction/web/public/src/main";
TS

check_fixture denied-file-url 1 "web/public/src/file-url.ts:1: 模块说明符不能是绝对路径或 URL：file:///auction/web/public/src/main" web/public/src/file-url.ts <<'TS'
import { main } from "file:///auction/web/public/src/main";
TS

check_fixture denied-unregistered-bare 1 "web/public/src/bare.ts:1: 未登记的裸包名：left-pad" web/public/src/bare.ts <<'TS'
import { leftPad } from "left-pad";
TS

check_fixture denied-backslash 1 "web/shared/src/backslash.ts:1: 说明符含反斜杠：./..\\../public/src/main" web/shared/src/backslash.ts <<'TS'
import { main } from "./..\\../public/src/main";
TS

check_fixture denied-parenthesized-require-literal 1 "web/public/src/require-literal.ts:1: 跨包导入必须使用登记的导入名，不能使用相对路径：../../admin/src/banned" web/public/src/require-literal.ts <<'TS'
const loaded = (require)("../../admin/src/banned");
void loaded;
TS

check_fixture denied-parenthesized-require-variable 1 "web/public/src/require-variable.ts:2: 动态导入的模块路径无法静态判定；必须使用字符串字面量或无插值模板字面量" web/public/src/require-variable.ts <<'TS'
const target = "../../admin/src/banned";
const loaded = (require)(target);
void loaded;
TS

check_child_paths_failure() {
	local name=$1 fixture status=0 diagnostic
	fixture=$(mktemp -d)
	create_fixture "$fixture"
	cat >"$fixture/web/public/tsconfig.json" <<'JSON'
{
  "extends": "../tsconfig.base.json",
  "compilerOptions": {
    "paths": {}
  },
  "include": ["src"]
}
JSON
	diagnostic=$( (cd "$fixture" && bun --no-install "$checker" "$fixture") 2>&1 ) || status=$?
	printf '%s\t预期 2\t实际 %s\n' "$name" "$status"
	if [[ $status -ne 2 ]] || ! grep -Fq -- "别名只能登记在 web/tsconfig.base.json" <<<"$diagnostic"; then
		printf '%s\n' "Web 包边界检查回归失败：$name 没有以退出码 2 报告子配置 paths。" "$diagnostic" >&2
		test_failed=1
	fi
	rm -rf -- "$fixture"
}

check_source_symlink_failure() {
	local name=$1 fixture status=0 diagnostic
	fixture=$(mktemp -d)
	create_fixture "$fixture"
	ln -s ../../shared/src "$fixture/web/public/src/linked"
	diagnostic=$( (cd "$fixture" && bun --no-install "$checker" "$fixture") 2>&1 ) || status=$?
	printf '%s\t预期 2\t实际 %s\n' "$name" "$status"
	if [[ $status -ne 2 ]] || ! grep -Fq -- "受检源码目录里不允许符号链接" <<<"$diagnostic"; then
		printf '%s\n' "Web 包边界检查回归失败：$name 没有以退出码 2 报告源码符号链接。" "$diagnostic" >&2
		test_failed=1
	fi
	rm -rf -- "$fixture"
}

check_target_symlink_violation() {
	local name=$1 fixture status=0 diagnostic
	fixture=$(mktemp -d)
	create_fixture "$fixture"
	printf '%s\n' "/* admin asset */" >"$fixture/crates/auction-web-admin/assets/admin.css"
	ln -s ../../auction-web-admin/assets "$fixture/crates/auction-web-shared/style/linked"
	cat >"$fixture/web/public/src/target-symlink.ts" <<'TS'
import "../../../crates/auction-web-shared/style/linked/admin.css";
TS
	diagnostic=$( (cd "$fixture" && bun --no-install "$checker" "$fixture") 2>&1 ) || status=$?
	printf '%s\t预期 1\t实际 %s\n' "$name" "$status"
	if [[ $status -ne 1 ]] || ! grep -Fqx -- "web/public/src/target-symlink.ts:1: 目标经过符号链接：../../../crates/auction-web-shared/style/linked/admin.css" <<<"$diagnostic"; then
		printf '%s\n' "Web 包边界检查回归失败：$name 没有报告目标符号链接。" "$diagnostic" >&2
		test_failed=1
	fi
	rm -rf -- "$fixture"
}

check_child_paths_failure config-child-paths
check_source_symlink_failure config-source-symlink
check_target_symlink_violation config-target-symlink

check_config_failure config-extra-alias "别名登记与检查器不一致" '{
  "compilerOptions": {
    "paths": {
      "@auction/contracts/*": ["./contracts/src/*"],
      "@auction/web-shared/*": ["./shared/src/*"],
      "@auction/web-public/*": ["./public/src/*"]
    }
  }
}'

check_config_failure config-multiple-candidate "别名登记与检查器不一致" '{
  "compilerOptions": {
    "paths": {
      "@auction/contracts/*": ["./contracts/src/*", "./shared/src/*"],
      "@auction/web-shared/*": ["./shared/src/*"]
    }
  }
}'

check_package_failure() {
	local name=$1 fixture status=0 diagnostic
	fixture=$(mktemp -d)
	create_fixture "$fixture"
	printf '%s' '{' >"$fixture/web/package.json"
	diagnostic=$( (cd "$fixture" && bun --no-install "$checker" "$fixture") 2>&1 ) || status=$?
	printf '%s\t预期 2\t实际 %s\n' "$name" "$status"
	if [[ $status -ne 2 ]] || ! grep -Fq -- "无法读取 web/package.json" <<<"$diagnostic"; then
		printf '%s\n' "Web 包边界检查回归失败：$name 没有以退出码 2 报告 package.json 读取失败。" "$diagnostic" >&2
		test_failed=1
	fi
	rm -rf -- "$fixture"
}

check_package_failure config-broken-package-json

check_scan_failure() {
	local name=$1 expected_text=$2 remove_source=$3 fixture status=0 diagnostic
	fixture=$(mktemp -d)
	create_fixture "$fixture"
	if [[ "$remove_source" == "empty" ]]; then
		find "$fixture/web" -type f -path '*/src/*' -delete
	else
		rm -rf -- "$fixture/web/contracts/src"
	fi
	diagnostic=$( (cd "$fixture" && bun --no-install "$checker" "$fixture") 2>&1 ) || status=$?
	printf '%s\t预期 2\t实际 %s\n' "$name" "$status"
	if [[ $status -ne 2 ]] || ! grep -Fq -- "$expected_text" <<<"$diagnostic"; then
		printf '%s\n' "Web 包边界检查回归失败：$name 没有以退出码 2 报告 $expected_text。" "$diagnostic" >&2
		test_failed=1
	fi
	rm -rf -- "$fixture"
}

check_scan_failure empty-scan "扫描结果为空" empty
check_scan_failure unreadable-source "无法读取源码目录" missing

check_impl_load_failure() {
	local name=$1 fixture status=0 diagnostic
	fixture=$(mktemp -d)
	mkdir -p "$fixture/tooling"
	cp "$checker" "$fixture/tooling/check-boundaries.ts"
	cat >"$fixture/tooling/check-boundaries-impl.ts" <<'TS'
import "missing-boundary-dependency";
export async function runBoundaryCheck(): Promise<0> {
  return 0;
}
TS
	diagnostic=$( (cd "$fixture" && bun --no-install "$fixture/tooling/check-boundaries.ts" "$fixture") 2>&1 ) || status=$?
	printf '%s\t预期 2\t实际 %s\n' "$name" "$status"
	if [[ $status -ne 2 ]] || [[ "$diagnostic" != *"Web 包边界检查无法完成"* ]]; then
		printf '%s\n' "Web 包边界检查回归失败：$name 没有以退出码 2 报告实现模块加载失败。" "$diagnostic" >&2
		test_failed=1
	fi
	rm -rf -- "$fixture"
}

check_impl_load_failure impl-load-failure

check_web_dependency_guard() {
	local name=$1 tools=$2 fixture=$3 output status=0 tool
	mkdir -p "$fixture/ci/checks" "$fixture/web/node_modules/.bin"
	cp ci/checks/check.sh "$fixture/ci/checks/check.sh"
	for tool in $tools; do
		: >"$fixture/web/node_modules/.bin/$tool"
		chmod +x "$fixture/web/node_modules/.bin/$tool"
	done
	output=$( (cd "$fixture" && bash ci/checks/check.sh web) 2>&1 ) || status=$?
	printf '%s\t预期 1\t实际 %s\n' "$name" "$status"
	if [[ $status -ne 1 ]] || [[ "$output" != *"web/ 依赖不完整"* ]] || [[ "$output" != *"mise run web-prepare"* ]]; then
		printf '%s\n' "Web 依赖门禁回归失败：$name" "$output" >&2
		test_failed=1
	fi
	rm -rf -- "$fixture"
}

fixture=$(mktemp -d)
check_web_dependency_guard missing-tsc biome "$fixture"
fixture=$(mktemp -d)
check_web_dependency_guard missing-biome tsc "$fixture"

check_web_dependency_directory_guard() {
	local name=$1 fixture=$2 output status=0
	mkdir -p "$fixture/ci/checks" "$fixture/web/node_modules/.bin/tsc"
	cp ci/checks/check.sh "$fixture/ci/checks/check.sh"
	: >"$fixture/web/node_modules/.bin/biome"
	chmod +x "$fixture/web/node_modules/.bin/biome"
	output=$( (cd "$fixture" && bash ci/checks/check.sh web) 2>&1 ) || status=$?
	printf '%s\t预期 1\t实际 %s\n' "$name" "$status"
	if [[ $status -ne 1 ]] || [[ "$output" != *"web/ 依赖不完整"* ]] || [[ "$output" != *"mise run web-prepare"* ]]; then
		printf '%s\n' "Web 依赖门禁回归失败：$name" "$output" >&2
		test_failed=1
	fi
	rm -rf -- "$fixture"
}

fixture=$(mktemp -d)
check_web_dependency_directory_guard directory-tsc "$fixture"

((test_failed == 0)) || exit 1
printf '%s\n' 'Web 包边界检查回归通过'
