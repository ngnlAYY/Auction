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
    # 本门禁的目标均不含嵌入注释分隔符的字面量；注释不能充当契约证据。
    $source =~ s{/\*.*?\*/}{}gs;
    $source =~ s{^\s*(?://|--).*$}{}mg;
    return $source;
}

sub equal_set {
    my ($name, $expected, $actual) = @_;
    my %seen;
    die "$name 含重复项\n" if grep { $seen{$_}++ } @$actual;
    die "$name 与装备来源原因枚举不一致\n"
        unless join("\n", sort @$expected) eq join("\n", sort @$actual);
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

# 取最后一次追加的同名约束定义，防止只核对某份过时迁移。
my ($constraint, $migration);
for my $path (sort glob 'migrations/postgres/*.up.sql') {
    my $source = read_source($path);
    while ($source =~ /ADD\s+CONSTRAINT\s+inbound_mail_equipment_diagnostic_check\s+CHECK\s*\((.*?)\);/sg) {
        ($constraint, $migration) = ($1, $path);
    }
}
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
my (@message_codes, @message_ids);
while (@translated) {
    push @message_codes, shift @translated;
    push @message_ids, shift @translated;
}
equal_set('Admin 诊断说明', \@codes, \@message_codes);
for my $locale (qw(en ja ko zh-CN zh-TW)) {
    my $translation = read_source("crates/auction-web-admin/i18n/$locale.ftl");
    for my $id (@message_ids) {
        die "$locale 缺少诊断译文 $id\n" unless $translation =~ /^\Q$id\E\s*=/m;
    }
}
PERL
