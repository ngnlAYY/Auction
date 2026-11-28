#!/usr/bin/env bash

# 只读取源码和迁移定义，核对装备解析诊断闭集；不连接数据库或执行业务流程。
set -euo pipefail
cd "$(dirname "$0")/../.."

perl <<'PERL'
use strict;
use warnings;
use utf8;
binmode STDERR, ':encoding(UTF-8)';

sub read_source {
    my ($path) = @_;
    open my $file, '<:encoding(UTF-8)', $path or die "无法读取 $path: $!\n";
    local $/;
    my $source = <$file>;
    if ($path =~ /\.(rs|ts)\z/) {
        my $is_rust = $1 eq 'rs';
        my $quoted = qr{"(?:\\.|[^"\\])*"}s;
        my $literal = $is_rust
            ? qr{
                (?:br|cr|r)(?<hashes>\#*)".*?"\k<hashes>
                | $quoted
                | b?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\r\n])'
            }xs
            : qr{ $quoted | '(?:\\.|[^'\\])*' | \x60(?:\\.|[^\x60\\])*\x60 }xs;
        my $line_comment = $is_rust ? qr{//[^\n]*} : qr{//[^\r\n\x{2028}\x{2029}]*};
        my $block_comment = $is_rust
            ? qr{(?<block> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&block) )* \*/ )}x
            : qr{/\*.*?\*/}s;
        # 完整保留字面量，注释换成空白，避免相邻的源码记号拼接。
        $source =~ s{
            (?<literal>$literal) | (?<comment>$line_comment|$block_comment)
        }{$+{literal} // ($+{comment} =~ s/[^\r\n\x{2028}\x{2029}]/ /gr)}gex;
    } else {
        $source =~ s{/\*.*?\*/}{}gs;
        $source =~ s{^\s*(?://|--).*$}{}mg;
    }
    return $source;
}

sub equal_set {
    my ($name, $expected, $actual) = @_;
    my %seen;
    die "$name 含重复项\n" if grep { $seen{$_}++ } @$actual;
    die "$name 与装备来源原因枚举不一致\n"
        unless join("\n", sort @$expected) eq join("\n", sort @$actual);
}

sub translation_map {
    my ($name, $codes, @pairs) = @_;
    my (%translations, @actual_codes);
    while (@pairs) {
        my ($code, $key) = splice @pairs, 0, 2;
        push @actual_codes, $code;
        $translations{$code} = $key;
    }
    equal_set($name, $codes, \@actual_codes);
    return %translations;
}

my $reason = read_source(
    'crates/auction-application/src/ports/mail/equipment_diagnostics/reason.rs'
);
my @pairs = $reason =~ /Self::(\w+)\s*=>\s*"(equipment_[a-z_]+)"/g;
die "装备来源原因映射为空\n" unless @pairs;
my (@variants, @codes);
while (@pairs) {
    push @variants, shift @pairs;
    push @codes, shift @pairs;
}
my ($enum) = $reason =~ /pub enum EquipmentSourceDetailCode\s*\{([^}]+)\}/s;
my ($reverse) = $reason =~ /pub fn from_code\b.*?\[\s*(.*?)\s*\]/s;
die "装备来源原因枚举或反向映射结构变化，请同步静态门禁\n"
    unless defined $enum && defined $reverse;
my @declared = $enum =~ /^\s*(\w+)\s*,/mg;
my @reverse = $reverse =~ /Self::(\w+)/g;
equal_set('原因枚举', \@variants, \@declared);
equal_set('原因反向映射', \@variants, \@reverse);

# 按迁移文件名顺序跟踪同名约束的 ADD/DROP：最后一次操作必须是 ADD，否则闭集已不在库上生效。
my ($constraint, $migration, $dropped_at);
for my $path (sort glob 'migrations/postgres/*.up.sql') {
    my $source = read_source($path);
    while ($source =~ /
        ADD\s+CONSTRAINT\s+inbound_mail_equipment_diagnostic_check\s+CHECK\s*\((.*?)\);
        | DROP\s+CONSTRAINT\s+(?:IF\s+EXISTS\s+)?inbound_mail_equipment_diagnostic_check\b
    /sgx) {
        if (defined $1) {
            ($constraint, $migration) = ($1, $path);
            undef $dropped_at;
        } else {
            undef $constraint;
            $dropped_at = $path;
        }
    }
}
die "装备诊断数据库约束已由 $dropped_at 删除\n" if defined $dropped_at;
die "未找到装备诊断数据库约束\n" unless defined $constraint;
my ($sql_codes) = $constraint =~ /
    equipment_diagnostic\s*->>\s*'category'\s*=\s*'protocol_drift'\s*
    AND\s+equipment_diagnostic\s*->>\s*'stage'\s*=\s*'parse'\s*
    AND\s+equipment_diagnostic\s*->>\s*'detail_code'\s+IN\s*\(([^)]+)\)
/sx;
die "$migration 缺少 parse/protocol_drift 原因闭集分支\n" unless defined $sql_codes;
my @sql_codes = $sql_codes =~ /'([^']+)'/g;
equal_set('数据库诊断原因', \@codes, \@sql_codes);

my $decoder = read_source('crates/auction-web-admin/src/api/mail.rs');
my ($web_codes) = $decoder =~ /
    \("protocol_drift",\s*Some\(detail\)\)\s+if\s+value\.stage\s*==\s*"parse"\s*
    =>\s*matches!\(\s*detail,\s*(.*?)\s*\),
/sx;
die "Admin 缺少 parse/protocol_drift 原因闭集分支\n" unless defined $web_codes;
my @web_codes = $web_codes =~ /"([^"]+)"/g;
equal_set('Admin 诊断原因', \@codes, \@web_codes);

my $messages = read_source('crates/auction-web-admin/src/surface/mail/messages.rs');
my @translated = $messages =~ /"(equipment_[a-z_]+)"\s*=>\s*t!\("(admin-mail-detail-source-[a-z-]+)"\)/g;
my %translations = translation_map('Admin 诊断说明', \@codes, @translated);

my $typescript_decoder = read_source('web/admin/src/api/mail-response.ts');
my ($typescript_web_codes) = $typescript_decoder =~ /
    if\s*\(\s*category\s*===\s*"protocol_drift"\s*&&\s*detailCode\s*!==\s*null\s*&&\s*stage\s*===\s*"parse"\s*\)\s*\{\s*
    return\s*\(\s*(.*?)\s*\);/sx;
die "TypeScript Admin 缺少 parse/protocol_drift 原因闭集分支\n"
    unless defined $typescript_web_codes;
# 与 Rust 的 matches! 分支采用同一口径：提取该分支中的原因字符串，再与应用层闭集比较。
my @typescript_web_codes = $typescript_web_codes =~ /"([^"]+)"/g;
equal_set('TypeScript Admin 诊断原因', \@codes, \@typescript_web_codes);

my $typescript_messages = read_source('web/admin/src/surface/mail/messages.ts');
my @typescript_translated = $typescript_messages =~
    /case\s+"(equipment_[a-z_]+)":\s*return\s+t\("(admin-mail-detail-source-[a-z-]+)"\);/g;
my %typescript_translations = translation_map('TypeScript Admin 诊断说明', \@codes, @typescript_translated);
for my $code (@codes) {
    my $expected = $translations{$code};
    my $actual = $typescript_translations{$code};
    die "TypeScript Admin 诊断译文与 Rust Admin 不一致：$code 应为 $expected，实际为 $actual\n"
        unless $expected eq $actual;
}

for my $locale (qw(en ja ko zh-CN zh-TW)) {
    my $translation = read_source("crates/auction-web-admin/i18n/$locale.ftl");
    for my $id (map { $translations{$_} } @codes) {
        die "$locale 缺少诊断译文 $id\n" unless $translation =~ /^\Q$id\E\s*=/m;
    }
}
PERL
