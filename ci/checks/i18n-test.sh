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
    fn fixture() { /* /* } */ } */ let _ = r##"} "# }"##; let _ = '}'; let _ = crate::t!("fixture-only"); }
}
fn after() { let _ = crate::t!("after-missing"); }
RUST
run_gate '内联测试模块之后的生产代码仍被检查' 1 \
	'crates/auction-web-shared/src/lib.rs:6: t!("after-missing")' 'fixture-only'

reset_fixture
cat >"$fixture/crates/auction-web-shared/src/lib.rs" <<'RUST'
#[cfg(test)]
mod external;
fn after() { let _ = crate::t!("production-missing"); }
#[cfg(test)]
mod tests {
    fn fixture() { let _ = crate::t!("fixture-only"); }
}
RUST
printf '%s\n' '// 外置测试模块。' >"$fixture/crates/auction-web-shared/src/external.rs"
run_gate '外置测试模块之后的生产代码仍被检查' 1 \
	'crates/auction-web-shared/src/lib.rs:3: t!("production-missing")' 'fixture-only'

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

# 解析结构的期望值取自重构前的解析器，覆盖调用与字面量、绝对和相对坐标。
perl -Ici/checks <<'PERL' || fail 'TypeScript 分词边界'
use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));
use Test::More;
use I18nSource qw(parse_typescript);

sub call_shape {
    return [map { [@{$_}{qw(key line offset invalid provided)}] } @{ $_[0]{calls} }];
}

sub literals {
    return [map { [@{$_}{qw(value line)}] } @{ $_[0]{literals} }];
}

my $coordinates = "雪/* 注\r\n释 */\r\n  t(\"键\", { \"名\": 值 });\r\n'外///*'";
is_deeply(parse_typescript($coordinates), {
    calls => [{
        key => '键', line => 3, offset => 15, syntax => 't', dynamic => 0, provided => { '名' => 1 },
        arguments => [
            [{ type => 'string', value => '键', start => 2, end => 5 }],
            [
                { type => 'punct', value => '{', start => 7, end => 8 },
                { type => 'string', value => '名', start => 9, end => 12 },
                { type => 'punct', value => ':', start => 12, end => 13 },
                { type => 'punct', value => '值', start => 14, end => 15 },
                { type => 'punct', value => '}', start => 16, end => 17 },
            ],
        ],
    }],
    literals => [{ value => '外///*', line => 4 }],
}, '多字节字符、跨行注释与 CRLF 保留字符坐标');

my $templates = parse_typescript(<<'TS');
const outer = `前${ /* ` t("ghost") */ t("outer", {value: `深${ "// }"; /* } */ t("inner") }尾`}) }终`;
const line = "/* not a comment */ //";
const static = `plain\${still}`;
TS
is_deeply(call_shape($templates), [
    ['outer', 1, 38, undef, { value => 1 }], ['inner', 1, 78, undef, {}],
], '嵌套模板插值里的字符串、注释与调用顺序');
is_deeply($templates->{calls}[0]{arguments}[1][3],
    { type => 'template', value => '深尾', interpolated => 1, start => 19, end => 54 }, '插值模板 token');
is_deeply(literals($templates), [['/* not a comment */ //', 2], ['plain${still}', 3]], '字面量中的注释符和转义插值');

my $template_line_comment = parse_typescript(<<'TS');
const label = `${"value" // t("ghost")
  + t("inside")}`;
t("after");
TS
is_deeply(
    [map { $_->{key} } @{ $template_line_comment->{calls} }],
    ['inside', 'after'],
    '模板插值里的行注释不会遮蔽后续调用',
);

my $crossed = parse_typescript(<<'TS');
`${ "一
`" } ${ "二
`" } ${ t("三") }`
TS
is_deeply(call_shape($crossed), [map { ['三', 3, 26, undef, {}] } 1 .. 4], '跨行引号触发插值容错时保留重复调用');
is_deeply(literals($crossed), [], '插值里的字面量沿用原扫描范围');

my $properties = parse_typescript(<<'TS');
12t("after-number"); prefix12t("not-a-call");
t("shape", { one, 'two-two': 12abc, three: "...", four: (...rest) });
t("spread", { ...rest });
t("computed", { [key]: 2 });
t("number", { 12abc: 1 });
t("method", { name() {} });
t("template", { `key`: 1 });
t("duplicate", {a, "a": 2,});
t("holes", {a,, b});
TS
my $ordinary_only = '对象里只允许普通属性与简写属性';
is_deeply(call_shape($properties), [
    ['after-number', 1, 2, undef, {}],
    ['shape', 2, 46, undef, { one => 1, 'two-two' => 1, three => 1, four => 1 }],
    ['spread', 3, 116, '对象里有展开属性', undef],
    ['computed', 4, 142, '对象里有计算属性名', undef],
    ['number', 5, 171, $ordinary_only, undef],
    ['method', 6, 198, $ordinary_only, undef],
    ['template', 7, 226, $ordinary_only, undef],
    ['duplicate', 8, 255, undef, { a => 1 }],
    ['holes', 9, 285, '对象字面量里有空属性', undef],
], '数字后紧跟 t 与各种属性名和分隔符');
is_deeply(literals($properties), [['not-a-call', 1]], '标识符里的 t 与独立字面量');
is_deeply([grep { $_->{type} eq 'number' || $_->{value} eq '...' } @{ $properties->{calls}[1]{arguments}[1] }], [
    { type => 'number', value => '12abc', start => 29, end => 34 },
    { type => 'string', value => '...', start => 43, end => 48 },
    { type => 'punct', value => '...', start => 57, end => 60 },
], '数字边界与三个点的 token');

my $dynamic = parse_typescript(q~t("outer", {name: t(flag ? "one" : `two`)});~);
is_deeply(call_shape($dynamic), [
    ['outer', 1, 0, undef, { name => 1 }], [undef, 1, 18, undef, undef],
], '重叠的外层和动态调用');
is_deeply(literals($dynamic), [['one', 1], ['two', 1]], '动态键的字面量顺序');

my $unclosed = [undef, 1, undef, '调用括号未闭合', undef];
for my $sample (
    ['字符串', q~t("unclosed~, [$unclosed], []],
    ['模板和插值', q~t("k", `bad${ expr)~, [$unclosed], [['k', 1]]],
    ['块注释', q~t("k" /* unclosed)~, [$unclosed], [['k', 1]]],
    ['块注释结束符重叠', q~t("k", /*/ )~, [$unclosed], [['k', 1]]],
    ['调用切片里的未闭合块注释', "t(\"k\", \"open\n' \" /* ) */ '",
        [[undef, 1, undef, '调用内有无法识别的字符串或模板', undef]], []],
    ['花括号', q~t("k", { value: 1)~, [$unclosed], []],
    ['重叠调用中的方括号', q~t("outer", t("inner", [1))~, [$unclosed, $unclosed], []],
    ['对象括号', q~t("k", {} || {})~, [['k', 1, 0, '对象字面量括号不平衡', undef]], []],
) {
    my ($name, $source, $calls, $literals) = @$sample;
    my $parsed = parse_typescript($source);
    is_deeply([call_shape($parsed), literals($parsed), $parsed->{error}], [$calls, $literals, undef], $name);
}

my $continued = parse_typescript("t(\"k\", 'escaped\\\nline');\r\n'open\r\n t('recovered')");
is_deeply(call_shape($continued), [
    ['k', 1, 0, '第二实参不是对象字面量', undef], ['recovered', 4, 34, undef, {}],
], '转义换行与孤立引号后的调用恢复');
is_deeply($continued->{calls}[0]{arguments}[1][0],
    { type => 'string', value => "escaped\nline", start => 7, end => 22 }, '转义换行的字符串值和位置');

# 原解析器在这两种输入上各输出两条 Deep recursion 告警。解析结构沿用实测结果。
for my $sample (
    ['0', []],
    ['t("x")', [{
        key => 'x', line => 1, offset => 297, syntax => 't', dynamic => 0, provided => {},
        arguments => [[{ type => 'string', value => 'x', start => 2, end => 5 }]],
    }]],
) {
    my ($middle, $calls) = @$sample;
    my $source = ("\x60\x24\x7b" x 99) . $middle . ("\x7d\x60" x 99);
    my $parsed;
    my $stderr = '';
    {
        local *STDERR;
        open STDERR, '>', \$stderr or die "无法捕获标准错误：$!";
        $parsed = parse_typescript($source);
    }
    is_deeply([$parsed, $stderr], [{ calls => $calls, literals => [] }, ''], "99 层模板插值 $middle 的解析结果与标准错误");
}

done_testing();
PERL

timeout 10 perl -Ici/checks <<'PERL' || fail 'Rust 内联测试模块掩蔽'
use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));
use Test::More;
use I18nSource qw(strip_rust_inline_tests);

my $before = "// 前\nfn before() {}\n";
my $after = "fn after() {} // \"#\n";
my $hashes = '#' x 255;
for my $sample (
    ['普通字符串', q~let _ = "雪 } \"# // {";~],
    ['字节字符串', q~let _ = b"} \"# // {";~],
    ['零个井号的原始字符串', q~let _ = r"} // \";~],
    ['单井号的原始字符串', q~let _ = r#"} " // # {"#;~],
    ['多井号的原始字符串', q~let _ = r##"} "# // {"##;~],
    ['255 个井号的原始字符串', 'let _ = r' . $hashes . q~"} "# // {"~ . $hashes . ';'],
    ['零个井号的原始字节字符串', q~let _ = br"} // \";~],
    ['单井号的原始字节字符串', q~let _ = br#"} " // # {"#;~],
    ['多井号的原始字节字符串', q~let _ = br##"} "# // {"##;~],
    ['C 字符串', q~let _ = c"} \"# // {";~],
    ['零个井号的原始 C 字符串', q~let _ = cr"} // \";~],
    ['单井号的原始 C 字符串', q~let _ = cr#"} " // # {"#;~],
    ['多井号的原始 C 字符串', q~let _ = cr##"} "# // {"##;~],
    ['普通字符', q~let _ = ('a', '}');~],
    ['转义单引号', q~let _ = ('\'', '}');~],
    ['转义的字节字符', q~let _ = (b'\'', b'}');~],
    ['带下划线的 Unicode 字符转义', q~let _ = ('\u{7d______}', '}');~],
    ['生命周期', q~let _: &'static _=&'}';~],
    ['Unicode 生命周期', q~fn f<'Ᲊ>() {}~],
    ['静态生命周期', q~let _: &'static str = "}";~],
    ['循环标签', q~'a: loop { break 'a; }~],
    ['右花括号字节字符', q~let _ = b'}';~],
    ['Unicode 字符转义', q~let _ = '\u{7d}';~],
    ['嵌套块注释', q~/* /* } "# // */ } "# // */~],
) {
    my ($name, $body) = @$sample;
    my $module = "#[cfg(test)]\nmod tests {\n    fn fixture() { $body }\n}\n";
    my $masked = $module =~ s/[^\r\n\x{2028}\x{2029}]/ /gr;
    is(strip_rust_inline_tests($before . $module . $after), $before . $masked . $after, $name);
}

for my $sample (
    ['模块头的行注释', "#[cfg(test)]\nmod// { }\r{\n tests // { }\r{\n{ // }\r}\n}\n"],
    ['模块头的嵌套块注释', "#[cfg(test)]\nmod/* { /* } */ } */tests /* { } /* */ */ {}\n"],
    ['CRLF 属性行', "#[cfg(test)] \t\r\nmod tests {}\n"],
    ['原始标识符模块名', "#[cfg(test)]\nmod r#mod {}\n"],
) {
    my ($name, $module) = @$sample;
    my $masked = $module =~ s/[^\r\n\x{2028}\x{2029}]/ /gr;
    is(strip_rust_inline_tests($before . $module . $after), $before . $masked . $after, $name);
}

my $complete = "#[cfg(test)]\nmod complete {}\n";
my $masked = $complete =~ s/[^\r\n\x{2028}\x{2029}]/ /gr;
my $module_start = "#[cfg(test)]\nmod broken { ";
for my $sample (
    ['未闭合的原始字符串', $module_start . 'r#" }'],
    ['未闭合的原始字节字符串', $module_start . 'br#" }'],
    ['未闭合的普通字符串', $module_start . '" }'],
    ['未闭合的嵌套块注释', $module_start . '/* /* } */'],
    ['未闭合的模块花括号', $module_start],
    ['测试属性后的未闭合块注释', "#[cfg(test)]\n/*"],
) {
    my ($name, $open) = @$sample;
    my $tail = "$open\n#[cfg(test)]\nmod later {}\n";
    is(strip_rust_inline_tests($complete . $tail), $masked . $tail, $name);
}

done_testing();
PERL

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

# 清单被截断或某行写坏时会少跑用例；执行条数必须与 JSON 清单一致才算通过。
expected_samples=$(perl -MJSON::PP -e '
	use strict;
	use warnings;
	local $/;
	my $samples = decode_json(<STDIN>);
	print scalar @$samples;
' <"$samples")
[[ $expected_samples =~ ^[1-9][0-9]*$ ]] || fail '平铺边界样本清单为空或无法读取'
consumed_samples=$(wc -l <"$sample_dir/index")
[[ $consumed_samples -eq $expected_samples ]] ||
	fail "平铺边界样本执行 $consumed_samples 条，清单有 $expected_samples 条"

((test_failed == 0)) || exit 1
printf '%s\n' 'i18n 门禁回归通过'
