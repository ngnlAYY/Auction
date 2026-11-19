#!/usr/bin/env bash
# i18n 门禁回归：在最小仓库骨架上运行真实的 `check.sh i18n`，覆盖两套实现、共享层、
# 平铺子集与源码枚举失败；平铺分行边界与 TypeScript 加载器共用 ci/checks/i18n-subset-samples.json。

set -euo pipefail
cd "$(dirname "$0")/../.."
gate=$PWD/ci/checks/check.sh
flat_checker=$PWD/ci/checks/i18n-flat-subset.pl
samples=$PWD/ci/checks/i18n-subset-samples.json
fixture=$(mktemp -d)
fakebin=$(mktemp -d)
sample_dir=$(mktemp -d)
trap 'rm -rf -- "$fixture" "$fakebin" "$sample_dir"' EXIT
real_rg=$(command -v rg)
cat >"$fakebin/rg" <<EOF
#!/usr/bin/env bash
if [[ \${I18N_FAIL_RG:-0} == 1 ]]; then
	for argument in "\$@"; do
		if [[ \$argument == "\${I18N_FAIL_RG_DIR:-}" ]]; then
			printf '%s\n' '模拟 rg 枚举失败' >&2
			exit 2
		fi
	done
fi
exec "$real_rg" "\$@"
EOF
chmod +x "$fakebin/rg"

test_failed=0
gate_env=()
fail() {
	printf '%s\n' "i18n 门禁回归失败：$*" >&2
	test_failed=1
}

run_gate() {
	local name=$1 expected_status=$2 expected=${3-} forbidden=${4-}
	local output status=0
	if [[ ${#gate_env[@]} -gt 0 ]]; then
		output=$(env "${gate_env[@]}" AUCTION_I18N_ROOT="$fixture" bash "$gate" i18n 2>&1) || status=$?
	else
		output=$(AUCTION_I18N_ROOT="$fixture" bash "$gate" i18n 2>&1) || status=$?
	fi
	printf '%s\t预期 %s\t实际 %s\n' "$name" "$expected_status" "$status"
	if [[ $status -ne $expected_status ]]; then
		printf '%s\n' "$output" >&2
		fail "$name 的退出码不匹配"
	elif [[ -n $expected && $output != *"$expected"* ]]; then
		printf '%s\n' "$output" >&2
		fail "$name 缺少诊断 '$expected'"
	elif [[ -n $forbidden && $output == *"$forbidden"* ]]; then
		printf '%s\n' "$output" >&2
		fail "$name 出现了不应有的诊断 '$forbidden'"
	fi
}

write_public_locales() {
	local locale
	for locale in en ja ko zh-CN zh-TW; do
		cat >"$fixture/crates/auction-web-public/i18n/$locale.ftl" <<'FTL'
greet = Hello { $name }
flat = Plain
shape = Shaped
shared-only = Shared
cost = Cost {"$USD"}
FTL
	done
}

write_admin_locales() {
	local locale
	for locale in en ja ko zh-CN zh-TW; do
		cat >"$fixture/crates/auction-web-admin/i18n/$locale.ftl" <<'FTL'
admin-label = Admin
shared-only = Shared
FTL
	done
}

reset_fixture() {
	gate_env=()
	rm -rf -- "$fixture/crates" "$fixture/web"
	mkdir -p "$fixture/crates/auction-web-public/src" "$fixture/crates/auction-web-public/i18n"
	mkdir -p "$fixture/crates/auction-web-admin/src" "$fixture/crates/auction-web-admin/i18n"
	mkdir -p "$fixture/crates/auction-web-shared/src"
	mkdir -p "$fixture/web/public/src" "$fixture/web/shared/src"
	write_public_locales
	write_admin_locales
	printf '%s\n' 'pub fn marker() -> &'"'"'static str { "greet" }' >"$fixture/crates/auction-web-public/src/lib.rs"
	printf '%s\n' 'pub fn label() -> &'"'"'static str { crate::t!("admin-label") }' >"$fixture/crates/auction-web-admin/src/lib.rs"
	printf '%s\n' 'pub fn marker() -> &'"'"'static str { "shared-only" }' >"$fixture/crates/auction-web-shared/src/lib.rs"
	cat >"$fixture/web/public/src/app.ts" <<'TS'
const greeting = t("greet", { name: "Ada" });
const flat = t("flat");
const shape = t("shape");
const cost = t("cost");
TS
	cat >"$fixture/web/shared/src/shared.ts" <<'TS'
const { t } = context;
const direct = t("shared-only");
function render(t: (id: string) => string) {
  return t("shared-only");
}
TS
	printf '%s\n' 'const fake = t("fixture-only");' >"$fixture/web/shared/src/shared.test.ts"
}

reset_fixture
run_gate '基线：web/admin/src 不存在并共用译文' 0 'Fluent 平铺子集检查通过'

reset_fixture
cat >"$fixture/web/shared/src/shared.ts" <<'TS'
const keep = t("shared-only");
const broken = t('absent');
TS
run_gate '共享层单引号缺键' 1 'web/shared/src/shared.ts:2: t("absent")'

reset_fixture
cat >"$fixture/web/shared/src/shared.ts" <<'TS'
const keep = t("shared-only");
// t("was-deleted")
const text = 't("was-deleted")';
TS
run_gate '注释与字符串里的 t 不算调用' 0 'Fluent 平铺子集检查通过'

reset_fixture
cat >"$fixture/web/shared/src/shared.ts" <<'TS'
const keep = t("shared-only");
const suffix = "-x";
const joined = t("prefix-" + suffix);
const templated = t(`prefix-${suffix}`);
TS
run_gate '拼接与插值模板按动态键处理' 0 'Fluent 平铺子集检查通过'

reset_fixture
cat >"$fixture/web/shared/src/shared.ts" <<'TS'
const label = `${ /* t("ghost") */ t("absent") }`;
TS
run_gate '模板插值里的调用与注释' 1 'web/shared/src/shared.ts:1: t("absent")' 'ghost'

reset_fixture
printf '%s\n' 't("greet");' >"$fixture/web/public/src/bad.tsx"
run_gate 'TypeScript 少传参数' 1 "web/public/src/bad.tsx:1: t(\"greet\") 调用传入的参数与译文需要的变量不一致：缺少 \$name"

reset_fixture
printf '%s\n' 't("greet", { name: "Ada", extra: 1 });' >"$fixture/web/public/src/bad.tsx"
run_gate 'TypeScript 多传参数' 1 "web/public/src/bad.tsx:1: t(\"greet\") 调用传入的参数与译文需要的变量不一致：多出 \$extra"

reset_fixture
printf '%s\n' 't("greet", { /* 名字 */ extra: 1 });' >"$fixture/web/public/src/bad.tsx"
run_gate '对象里的注释不跳过参数核对' 1 "web/public/src/bad.tsx:1: t(\"greet\") 调用传入的参数与译文需要的变量不一致：多出 \$extra"

reset_fixture
printf '%s\n' 't("greet", { /* 名字 */ name: "Ada", });' >"$fixture/web/public/src/bad.tsx"
run_gate '对象属性注释与尾逗号合法' 0 'Fluent 平铺子集检查通过'

reset_fixture
printf '%s\n' 't("greet",);' >"$fixture/web/public/src/bad.tsx"
run_gate '只有键的尾逗号仍核对参数' 1 "web/public/src/bad.tsx:1: t(\"greet\") 调用传入的参数与译文需要的变量不一致：缺少 \$name"

reset_fixture
printf '%s\n' 't("greet", { name: "Ada" },);' >"$fixture/web/public/src/bad.tsx"
run_gate '对象实参后的尾逗号合法' 0 'Fluent 平铺子集检查通过'

reset_fixture
printf '%s\n' 't("greet", { ...args });' >"$fixture/web/public/src/bad.tsx"
run_gate '对象展开属性报错' 1 'web/public/src/bad.tsx:1: t("greet") 调用形态无法静态核对：对象里有展开属性'

reset_fixture
printf '%s\n' 't("greet", { ["name"]: "Ada" });' >"$fixture/web/public/src/bad.tsx"
run_gate '对象计算属性名报错' 1 'web/public/src/bad.tsx:1: t("greet") 调用形态无法静态核对：对象里有计算属性名'

reset_fixture
printf '%s\n' 't("greet", { name: "Ada" }, 1);' >"$fixture/web/public/src/bad.tsx"
run_gate '第三实参报错' 1 'web/public/src/bad.tsx:1: t("greet") 调用形态无法静态核对：还有第三实参'

reset_fixture
printf '%s\n' 't("greet", args);' >"$fixture/web/public/src/bad.tsx"
run_gate '第二实参不是对象字面量' 1 'web/public/src/bad.tsx:1: t("greet") 调用形态无法静态核对：第二实参不是对象字面量'

reset_fixture
printf '%s\n' 'const key = "greet";' 't(key);' >"$fixture/web/public/src/bad.tsx"
run_gate '动态键沿用跳过规则' 0 'Fluent 平铺子集检查通过'

reset_fixture
sed -i "1s/.*/greet = Hello { \$name } { \$other }/" "$fixture/crates/auction-web-public/i18n/ja.ftl"
run_gate '其它语言多写变量' 1 "crates/auction-web-public/i18n/ja.ftl:1: greet 引用了英文译文没有的变量 \$other"

reset_fixture
sed -i '1s/.*/greet = Hello/' "$fixture/crates/auction-web-public/i18n/ja.ftl"
run_gate '其它语言少写变量' 1 "crates/auction-web-public/i18n/ja.ftl:1: greet 缺少英文译文使用的变量 \$name"

reset_fixture
cat >"$fixture/crates/auction-web-shared/src/lib.rs" <<'RUST'
fn before() { let _ = crate::t!("shared-only"); }
#[cfg(test)]
mod tests {
    fn fixture() { let _ = "}"; let _ = crate::t!("fixture-only"); }
}
fn after() { let _ = crate::t!("after-missing"); }
RUST
run_gate '内联测试模块之后的生产代码仍被检查' 1 \
	'crates/auction-web-shared/src/lib.rs:6: t!("after-missing")' 'fixture-only'

reset_fixture
perl -0pi -e 's/^shape = .*$/shape = first\n    second/m' "$fixture/crates/auction-web-public/i18n/en.ftl"
run_gate '平铺子集续行' 1 'crates/auction-web-public/i18n/en.ftl:4: 续行'

reset_fixture
perl -0pi -e 's/^shape = .*$/shape = { "value" ->\n    [one] one\n   *[other] other\n    }/m' \
	"$fixture/crates/auction-web-public/i18n/en.ftl"
run_gate '平铺子集选择表达式' 1 'crates/auction-web-public/i18n/en.ftl:3: 选择表达式'
run_gate '平铺子集选择表达式后的续行' 1 'crates/auction-web-public/i18n/en.ftl:4: 续行'

reset_fixture
perl -0pi -e 's/^shape = .*$/shape = text\n    .label = Label/m' "$fixture/crates/auction-web-public/i18n/en.ftl"
run_gate '平铺子集属性' 1 'crates/auction-web-public/i18n/en.ftl:4: 属性'

reset_fixture
perl -0pi -e 's/^shape = .*$/shape = Shaped\n-brand = Name/m' "$fixture/crates/auction-web-public/i18n/en.ftl"
run_gate '平铺子集术语定义' 1 'crates/auction-web-public/i18n/en.ftl:4: 术语定义'

reset_fixture
perl -0pi -e 's/^shape = .*$/shape = { -brand }/m' "$fixture/crates/auction-web-public/i18n/en.ftl"
run_gate '平铺子集术语引用' 1 'crates/auction-web-public/i18n/en.ftl:3: 术语引用'

reset_fixture
perl -0pi -e 's/^shape = .*$/shape = { other-key }/m' "$fixture/crates/auction-web-public/i18n/en.ftl"
run_gate '平铺子集消息引用' 1 'crates/auction-web-public/i18n/en.ftl:3: 消息引用'

reset_fixture
perl -0pi -e 's/^shape = .*$/shape = { NUMBER(\$value) }/m' "$fixture/crates/auction-web-public/i18n/en.ftl"
run_gate '平铺子集函数调用' 1 'crates/auction-web-public/i18n/en.ftl:3: 函数调用'

reset_fixture
perl -0pi -e 'binmode ARGVOUT, ":encoding(UTF-8)"; s/^shape = .*$/shape = first\x{2028}second/m' "$fixture/crates/auction-web-public/i18n/en.ftl"
run_gate '正文里的 U+2028' 1 'crates/auction-web-public/i18n/en.ftl:3: 正文包含 U+2028/U+2029'

reset_fixture
perl -0pi -e 's/\n/\r\n/g' "$fixture/crates/auction-web-public/i18n/en.ftl"
run_gate 'CRLF 分行与加载器一致' 0 'Fluent 平铺子集检查通过'

reset_fixture
rm -rf -- "$fixture/web/public/src"
run_gate '必需源码目录缺失' 1 'web/public/src 不存在'

reset_fixture
mkdir -p "$fixture/web/admin"
printf '%s\n' 'not a directory' >"$fixture/web/admin/src"
run_gate 'web/admin/src 存在但不是目录' 1 'web/admin/src 存在但不是目录'

reset_fixture
gate_env=(PATH="$fakebin:$PATH" I18N_FAIL_RG=1 I18N_FAIL_RG_DIR="$fixture/web/shared/src")
run_gate '源码枚举命令失败' 1 "无法读取目录 $fixture/web/shared/src（rg 退出码 2）"

perl -MJSON::PP -e '
	use strict;
	use warnings;
	local $/;
	my $samples = decode_json(<STDIN>);
	open my $index, ">:encoding(UTF-8)", "'"$sample_dir"'/index" or die "$!";
	for my $sample (@$samples) {
		open my $handle, ">:encoding(UTF-8)", "'"$sample_dir"'/" . $sample->{name} . ".ftl" or die "$!";
		print $handle $sample->{text};
		close $handle;
		print $index join("\t", $sample->{name}, $sample->{valid} ? 1 : 0, $sample->{line} // 0), "\n";
	}
	close $index;
' <"$samples"
while IFS=$'\t' read -r sample valid line; do
	status=0
	output=$(perl "$flat_checker" "$sample_dir/$sample.ftl" 2>&1) || status=$?
	if [[ $valid -eq 1 ]]; then
		[[ $status -eq 0 ]] || fail "平铺边界样本 $sample 应通过：$output"
	else
		if [[ $status -ne 1 ]]; then
			fail "平铺边界样本 $sample 应失败，实际退出码 $status"
		elif [[ $output != *"$sample_dir/$sample.ftl:$line:"* ]]; then
			fail "平铺边界样本 $sample 缺少 $line 行诊断：$output"
		fi
	fi
done <"$sample_dir/index"

((test_failed == 0)) || exit 1
printf '%s\n' 'i18n 门禁回归通过'
