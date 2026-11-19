#!/usr/bin/env perl
# Fluent 变量一致性与调用参数检查。
#
# 两套实现共用同一份译文：Rust 的 `t!` 与 TypeScript 的 `t("键", { 参数: … })` 都必须传入
# 英文译文引用的全部变量，也不能多传译文没有引用的变量。变量只从 `{ $变量 }` 占位收集；
# `{"$USD"}` 这样的字符串字面量不是变量。
# 本检查保证：
#   1. 同一个键在五种语言里引用的变量集合完全相同；
#   2. 源码里以字面量键调用时，传入的参数集合与英文译文的变量集合一致。
# 动态键调用（键不是字面量）不在第 2 条范围内，它们的取值来自代码内的闭集映射，由各自的
# 单元测试或评审保证。TypeScript 调用形态只接受 `t(键)` 与 `t(键, { 名字: 值, 简写名 })`，
# 其它形态无法静态核对，直接报错并给出文件与行号。
#
# 用法：perl i18n-placeholders.pl <en.ftl> <其他语言.ftl>...
# 标准输入为 NUL 分隔的 Rust / TypeScript 源文件。

use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

use FindBin qw($Bin);
use lib $Bin;
use I18nFluent qw(message_line message_variables split_lines);
use I18nSource qw(parse_typescript);

my ($base, @locales) = @ARGV;
die "用法：i18n-placeholders.pl <en.ftl> <locale.ftl>...\n" unless defined $base && @locales;

# 返回 键 → { 变量 → 1, 行号 → n }。消息从行首的 `key =` 开始，缩进行是续行；行首注释或
# 空行结束消息。
sub variables_by_key {
    my ($file) = @_;
    open my $handle, '<:encoding(UTF-8)', $file or die "无法读取 $file：$!\n";
    my $text = do { local $/; <$handle> };
    close $handle;
    my (%variables, %bodies, %lines);
    my $key;
    my @lines = split_lines($text);
    for my $index (0 .. $#lines) {
        my $line = $lines[$index];
        my ($message_key, $before, $after, $body) = message_line($line);
        if (defined $message_key) {
            $key = $message_key;
            $variables{$key} //= {};
            $bodies{$key} = $body;
            $lines{$key} = $index + 1;
        } elsif ($line =~ /^\s+\S/ && defined $key) {
            $bodies{$key} .= "\n$line";
        } else {
            undef $key;
        }
    }
    for my $message (keys %bodies) {
        $variables{$message} = message_variables($bodies{$message});
    }
    return { map { $_ => { vars => $variables{$_}, line => $lines{$_} } } keys %variables };
}

my $expected = variables_by_key($base);
my @failures;

for my $locale (@locales) {
    my $actual = variables_by_key($locale);
    for my $key (sort keys %$expected) {
        my $locale_entry = $actual->{$key};
        next unless defined $locale_entry;
        my $locale_variables = $locale_entry->{vars};
        my @missing = grep { !$locale_variables->{$_} } sort keys %{ $expected->{$key}{vars} };
        my @extra = grep { !$expected->{$key}{vars}{$_} } sort keys %$locale_variables;
        push @failures, "$locale:$locale_entry->{line}: $key 缺少英文译文使用的变量 " . join(', ', map { "\$$_" } @missing)
            if @missing;
        push @failures, "$locale:$locale_entry->{line}: $key 引用了英文译文没有的变量 " . join(', ', map { "\$$_" } @extra)
            if @extra;
    }
}

# Rust 的 `t!` 参数用平衡括号读取；字符串内的括号与逗号不计入层级。
sub argument_list {
    my ($source, $start) = @_;
    my ($depth, $quote, $escaped, $current, @arguments) = (1, '', 0, '', ());
    for (my $index = $start; $index < length $source; $index++) {
        my $character = substr $source, $index, 1;
        if ($quote ne '') {
            $current .= $character;
            if ($escaped) { $escaped = 0 }
            elsif ($character eq '\\') { $escaped = 1 }
            elsif ($character eq $quote) { $quote = '' }
            next;
        }
        if ($character eq '"') {
            $quote = $character;
            $current .= $character;
            next;
        }
        if ($character =~ /[\(\[\{]/) { $depth++ }
        elsif ($character =~ /[\)\]\}]/) {
            $depth--;
            if ($depth == 0) { push @arguments, $current; return @arguments }
        } elsif ($character eq ',' && $depth == 1) { push @arguments, $current; $current = ''; next }
        $current .= $character;
    }
    return;
}

sub report_arguments {
    my ($path, $line, $call, $key, $provided) = @_;
    my %provided = %$provided;
    my @missing = grep { !$provided{$_} } sort keys %{ $expected->{$key}{vars} };
    my @extra = grep { !$expected->{$key}{vars}{$_} } sort keys %provided;
    push @failures, "$path:$line: $call(\"$key\") 调用传入的参数与译文需要的变量不一致：缺少 " . join(', ', map { "\$$_" } @missing)
        if @missing;
    push @failures, "$path:$line: $call(\"$key\") 调用传入的参数与译文需要的变量不一致：多出 " . join(', ', map { "\$$_" } @extra)
        if @extra;
}

local $/ = "\0";
while (my $path = <STDIN>) {
    chomp $path;
    next unless length $path;
    open my $handle, '<:encoding(UTF-8)', $path or die "无法读取 $path：$!\n";
    my $source = do { local $/; <$handle> };
    close $handle;
    if ($path =~ /\.rs$/) {
        while ($source =~ /\bt!\(/g) {
            my @arguments = argument_list($source, pos $source) or next;
            my ($key) = $arguments[0] =~ /^\s*"([A-Za-z0-9_-]+)"\s*$/ or next;
            next unless exists $expected->{$key};
            my %provided;
            for my $argument (@arguments[1 .. $#arguments]) {
                $provided{$1} = 1 if $argument =~ /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/;
            }
            my $line = 1 + (() = substr($source, 0, pos $source) =~ /\n/g);
            report_arguments($path, $line, 't!', $key, \%provided);
        }
    } elsif ($path =~ /\.tsx?$/) {
        my $parsed = parse_typescript($source);
        if ($parsed->{error}) {
            push @failures, "$path:1: $parsed->{error}";
            next;
        }
        for my $call (@{ $parsed->{calls} }) {
            my $key = $call->{key};
            my $label = defined $key ? "t(\"$key\")" : 't(...)';
            if (defined $call->{invalid}) {
                push @failures, "$path:$call->{line}: $label 调用形态无法静态核对：$call->{invalid}";
                next;
            }
            next if $call->{dynamic};
            next unless defined $key && exists $expected->{$key};
            report_arguments($path, $call->{line}, 't', $key, $call->{provided} // {});
        }
    }
}

if (@failures) {
    print STDERR "Fluent 变量或调用参数检查失败：\n";
    print STDERR "  $_\n" for @failures;
    exit 1;
}
print "Fluent 变量一致性检查通过（$base）\n";
