#!/usr/bin/env perl
# 源码引用的译文键检查：字面量键的 `t!` / `t` 调用必须指向当前 surface 的 `en.ftl`。
#
# Rust 实现用 `t!("键")`，TypeScript 实现用 `t("键")`（`t` 可以是导入、解构或参数名）。
# 动态键（键不是字符串字面量）不在这里判断：它们由代码内的闭集映射、字符串字面量可达性
# （i18n-unused-keys.pl）与各自的单元测试保证。
#
# 共享层同时被 Public 与 Admin 使用，它的测试夹具用 `greeting`、`missing-key` 这类假键，
# 不属于任何 surface 的译文。`--skip-tests` 跳过 TypeScript 测试文件、独立的 Rust
# `tests.rs` / `tests/` 测试模块，并按花括号配对去掉 Rust 文件里的内联测试模块（保留模块
# 之后的代码与行号）；surface 自身仍按原口径扫描全部源码。
#
# 用法：perl i18n-referenced-keys.pl <en.ftl> [--skip-tests]
# 标准输入是 NUL 分隔的源码路径。

use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

use FindBin qw($Bin);
use lib $Bin;
use I18nSource qw(parse_typescript strip_rust_inline_tests);

my $ftl = shift // die "用法：i18n-referenced-keys.pl <en.ftl> [--skip-tests]\n";
my $skip_tests = 0;
for my $argument (@ARGV) {
    die "未知参数：$argument\n" unless $argument eq '--skip-tests';
    $skip_tests = 1;
}

open my $ftl_handle, '<', $ftl or die "$ftl: $!\n";
my %known;
while (my $line = <$ftl_handle>) {
    $known{$1} = 1 if $line =~ /^([A-Za-z][A-Za-z0-9_-]*)\s*=/;
}
close $ftl_handle;
die "$ftl: 未解析到任何消息键\n" unless %known;

my @failures;
local $/ = "\0";
while (my $path = <STDIN>) {
    chomp $path;
    next unless length $path;
    if ($skip_tests && ($path =~ /\.test\.tsx?$/ || $path =~ m{(?:^|/)tests(?:\.rs$|/)})) {
        next;
    }
    open my $handle, '<', $path or die "$path: $!\n";
    my $source = do { local $/; <$handle> };
    close $handle;
    if ($path =~ /\.rs$/) {
        $source = strip_rust_inline_tests($source) if $skip_tests;
        my ($line, $previous) = (1, 0);
#line 53
        while ($source =~ /\bt!\(\s*"([A-Za-z0-9_.-]+)"/g) {
            my $key = $1;
            next if $known{$key};
            my $end = do { use bytes; pos($source) };
            my $span = do { use bytes; substr($source, $previous, $end - $previous) };
            $line += $span =~ tr/\n//;
            $previous = $end;
            push @failures, "$path:$line: t!(\"$key\") 引用了 $ftl 中不存在的消息键";
        }
    } elsif ($path =~ /\.tsx?$/) {
        my $parsed = parse_typescript($source);
        if ($parsed->{error}) {
            push @failures, "$path:1: $parsed->{error}";
            next;
        }
        for my $call (@{ $parsed->{calls} }) {
            my $key = $call->{key};
            next unless defined $key;
            next if $known{$key};
            push @failures, "$path:$call->{line}: $call->{syntax}(\"$key\") 引用了 $ftl 中不存在的消息键";
        }
    }
}

if (@failures) {
    print STDERR "源码引用了不存在的 Fluent 消息键：\n";
    print STDERR "  $_\n" for @failures;
    exit 1;
}
print "引用的译文键检查通过（$ftl）\n";
