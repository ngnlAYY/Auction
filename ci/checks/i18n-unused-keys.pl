#!/usr/bin/env perl
# FTL 键可达性检查：en.ftl 的每个键都必须能在同一 surface 源码的字符串字面量中找到。
# 既覆盖 t!("key") 静态调用，也覆盖字面量经变量传入 t! 的动态调用（设置表单字段 ID、图例与星期标签）。
# SetupField 的 `-help` 动态键只从 Rust/TypeScript 两端实际渲染的闭集字段组生成，并核对对应渲染分支仍存在。
# 新增动态拼接模式时本检查失败关闭，须显式扩展规则，不能放宽成任意字面量前缀。
use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

use FindBin qw($Bin);
use lib $Bin;
use I18nSource qw(parse_typescript);

my $ftl = shift // die "用法：i18n-unused-keys.pl <en.ftl>（源文件列表从 STDIN 以 NUL 分隔）\n";
my @files = sort split /\0/, do { local $/; <STDIN> // '' };
die "$ftl: 未收到源码文件列表\n" unless @files;

open my $fh, '<', $ftl or die "$ftl: $!\n";
my %keys;
while (<$fh>) {
    $keys{$1} = 1 if /^([A-Za-z][A-Za-z0-9_-]*)\s*=/;
}
close $fh;
die "$ftl: 未解析到任何消息键\n" unless %keys;

my %lits;
my (%rust_variant_ids, %rust_rendered_variants, %ts_field_ids, %ts_rendered_fields);
my ($rust_setup_renderer, $rust_setup_groups, $ts_setup_renderer, $ts_setup_groups);
for my $file (@files) {
    open my $src, '<', $file or die "$file: $!\n";
    my $code = do { local $/; <$src> };
    if ($file =~ m{(?:^|/)crates/auction-web-admin/src/surface/setup/draft\.rs$}) {
        while ($code =~ /Self::([A-Za-z_]\w*)\s*=>\s*"(setup-[A-Za-z0-9_-]+)"/g) {
            die "$file: 重复首次设置字段 ID：$2\n" if $rust_variant_ids{$1} || grep { $_ eq $2 } values %rust_variant_ids;
            $rust_variant_ids{$1} = $2;
        }
        my %groups;
        while ($code =~ /pub\(super\)\s+const\s+(DATABASE|ADMIN|BASIC|LISTENER|SCHEDULE|ADVANCED):\s*\[Self;\s*\d+\]\s*=\s*\[(.*?)\];/sg) {
            my ($group, $members) = ($1, $2);
            $groups{$group} = 1;
            while ($members =~ /Self::([A-Za-z_]\w*)/g) {
                die "$file: 重复首次设置渲染字段：$1\n" if $rust_rendered_variants{$1}++;
            }
        }
        die "$file: 首次设置字段或渲染组映射缺失\n"
            unless %rust_variant_ids && keys(%groups) == 6 && %rust_rendered_variants;
    }
    if ($file =~ m{(?:^|/)crates/auction-web-admin/src/surface/setup/fields\.rs$}) {
        my $help_call_sites = () = $code =~ /\{\s*help\s*\}/g;
        $rust_setup_renderer = 1 if $code =~ /t!\s*\(\s*&format!\s*\(\s*"\{id\}-help"/ && $help_call_sites == 2;
    }
    if ($file =~ m{(?:^|/)crates/auction-web-admin/src/surface/setup/form\.rs$}) {
        $rust_setup_groups = 1 if
            $code =~ /for field in fields\.iter\(\)\.copied\(\)\s*\{\s*SetupInput/ &&
            $code =~ /\("setup-database",\s*SetupField::DATABASE\.as_slice\(\)\)/ &&
            $code =~ /\("setup-admin",\s*SetupField::ADMIN\.as_slice\(\)\)/ &&
            $code =~ /\("setup-basic",\s*SetupField::BASIC\.as_slice\(\)\)/ &&
            $code =~ /\("setup-listener",\s*SetupField::LISTENER\.as_slice\(\)\)/ &&
            $code =~ /SetupInput\s*\{\s*field:\s*SetupField::ScheduleTimezone/ &&
            $code =~ /for field in SetupField::SCHEDULE\.into_iter\(\)\.skip\(1\)\s*\{\s*SetupInput/ &&
            $code =~ /for field in SetupField::ADVANCED\s*\{\s*SetupInput/;
    }
    if ($file =~ m{(?:^|/)web/admin/src/surface/setup/draft\.ts$}) {
        while ($code =~ /case\s+"([A-Za-z0-9_]+)"\s*:\s*return\s+"(setup-[A-Za-z0-9_-]+)"/g) {
            die "$file: 重复首次设置字段 ID：$2\n" if $ts_field_ids{$1} || grep { $_ eq $2 } values %ts_field_ids;
            $ts_field_ids{$1} = $2;
        }
        my %groups;
        while ($code =~ /export\s+const\s+SETUP_FIELD_(DATABASE|ADMIN|BASIC|LISTENER|SCHEDULE|ADVANCED):\s*readonly\s+SetupField\[\]\s*=\s*\[(.*?)\];/sg) {
            my ($group, $members) = ($1, $2);
            $groups{$group} = 1;
            while ($members =~ /"([A-Za-z0-9_]+)"/g) {
                die "$file: 重复首次设置渲染字段：$1\n" if $ts_rendered_fields{$1}++;
            }
        }
        die "$file: 首次设置字段或渲染组映射缺失\n"
            unless %ts_field_ids && keys(%groups) == 6 && %ts_rendered_fields;
    }
    if ($file =~ m{(?:^|/)web/admin/src/surface/setup/fields\.tsx$}) {
        my $help_call_sites = () = $code =~ /\{\s*help\(\)\s*\}/g;
        $ts_setup_renderer = 1 if $code =~ /\bt\s*\(\s*`\$\{id\(\)\}-help`\s*\)/ && $help_call_sites == 2;
    }
    if ($file =~ m{(?:^|/)web/admin/src/surface/setup/form\.tsx$}) {
        $ts_setup_groups = 1 if
            $code =~ /\{ legend: "setup-database", fields: SETUP_FIELD_DATABASE \}/ &&
            $code =~ /\{ legend: "setup-admin", fields: SETUP_FIELD_ADMIN \}/ &&
            $code =~ /\{ legend: "setup-basic", fields: SETUP_FIELD_BASIC \}/ &&
            $code =~ /\{ legend: "setup-listener", fields: SETUP_FIELD_LISTENER \}/ &&
            $code =~ /<For each=\{SETUP_FORM_GROUPS\}[\s\S]*?<For each=\{group\(\)\.fields\}[\s\S]*?<SetupInput\b/ &&
            $code =~ /<SetupInput field="schedule_timezone"/ &&
            $code =~ /<For each=\{SETUP_FIELD_SCHEDULE\.slice\(1\)\}[\s\S]*?<SetupInput\b/ &&
            $code =~ /<For each=\{SETUP_FIELD_ADVANCED\}[\s\S]*?<SetupInput\b/;
    }
    if ($file =~ /\.tsx?$/) {
        my $parsed = parse_typescript($code);
        die "$file:1: $parsed->{error}\n" if $parsed->{error};
        for my $call (@{ $parsed->{calls} }) {
            $lits{$call->{key}} = 1 if defined $call->{key};
        }
        for my $literal (@{ $parsed->{literals} }) {
            $lits{$literal->{value}} = 1 if $literal->{value} =~ /^[A-Za-z][A-Za-z0-9_-]*$/;
        }
    } else {
        # 只从 Rust 普通字符串 token 提取键；注释、字符与原始字符串内容不参与匹配。
        my $rust_token = qr{
            (?<line_comment> //[^\n]* )
            | (?<block_comment> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&block_comment) )* \*/ )
            | (?<raw_string> (?:br|cr|r)(?<hashes>\#*)".*?"\k<hashes> )
            | (?<string> (?:b|c)?"(?:\\.|[^"\\])*" )
            | (?<char> b?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])' )
        }xs;
        while ($code =~ /$rust_token/g) {
            next unless defined $+{string};
            my $literal = substr($code, $-[0], $+[0] - $-[0]);
            $lits{$1} = 1 if $literal =~ /^"([A-Za-z][A-Za-z0-9_-]*)"$/;
        }
    }
}

my %rust_help_keys;
if (%rust_variant_ids) {
    die "Admin Rust 首次设置帮助键缺少对应的渲染器或字段组\n"
        unless $rust_setup_renderer && $rust_setup_groups;
    for my $variant (keys %rust_rendered_variants) {
        my $id = $rust_variant_ids{$variant}
            // die "Admin Rust 首次设置渲染字段没有 ID：$variant\n";
        $rust_help_keys{"$id-help"} = 1;
    }
}
my %ts_help_keys;
if (%ts_field_ids) {
    die "Admin TypeScript 首次设置帮助键缺少对应的渲染器或字段组\n"
        unless $ts_setup_renderer && $ts_setup_groups;
    for my $field (keys %ts_rendered_fields) {
        my $id = $ts_field_ids{$field}
            // die "Admin TypeScript 首次设置渲染字段没有 ID：$field\n";
        $ts_help_keys{"$id-help"} = 1;
    }
}
if (%rust_help_keys && %ts_help_keys) {
    # grep 是右向列表运算符；分别括号化两个差集，避免把非空列表误判为不一致。
    die "Admin Rust 与 TypeScript 首次设置帮助键清单不一致\n"
        if (grep { !$ts_help_keys{$_} } keys %rust_help_keys)
        || (grep { !$rust_help_keys{$_} } keys %ts_help_keys);
}
my %derived_help_keys = (%rust_help_keys, %ts_help_keys);

my @unused;
for my $key (sort keys %keys) {
    next if $lits{$key};
    # 动态 setup 帮助键只对有限字段 ID 映射中存在、且对应渲染器仍构造帮助键的项放行。
    next if $derived_help_keys{$key};
    push @unused, $key;
}
if (@unused) {
    print STDERR "en.ftl 中存在源码未引用的译文键：\n";
    print STDERR "  $_\n" for @unused;
    exit 1;
}
print "未引用译文键检查通过（$ftl，", scalar(keys %keys), " 个键）\n";
