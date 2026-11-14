#!/usr/bin/env perl
# Fluent 变量一致性检查。
#
# 浏览器的 `t!` 在 Fluent 报错（引用了调用方没有传入的变量）时 panic，整个 SPA 只剩错误横幅；
# 键集合一致的检查拦不住“某个语言的译文多写了一个变量”，该语言用户打开页面才会崩溃。
# 本检查保证两点：
#   1. 每个键在其他语言里引用的变量都出现在英文译文里（子集即可，少用变量不会报错）；
#   2. 源码里以字面量键调用 `t!("key", name: value, ...)` 时，传入了英文译文引用的全部变量。
# 两条合起来，任何语言都不会引用未传入的变量。动态键调用（键不是字面量）不在第 2 条范围内，
# 它们的取值来自代码内的闭集映射，由各自的单元测试或评审保证。
#
# 用法：perl i18n-placeholders.pl <en.ftl> <其他语言.ftl>...，标准输入为 NUL 分隔的 Rust 源文件。

use strict;
use warnings;

my ($base, @locales) = @ARGV;
die "用法：i18n-placeholders.pl <en.ftl> <locale.ftl>...\n" unless defined $base && @locales;

# 返回 键 → {变量 → 1}。消息从行首的 `key =` 开始，缩进行是续行；行首注释或空行结束消息。
sub variables_by_key {
    my ($file) = @_;
    open my $handle, '<:encoding(UTF-8)', $file or die "无法读取 $file：$!\n";
    my (%variables, %text, $key);
    while (my $line = <$handle>) {
        if ($line =~ /^([A-Za-z][A-Za-z0-9_-]*)\s*=(.*)$/) {
            $key = $1;
            $variables{$key} //= {};
            $text{$key} = $2;
        } elsif ($line =~ /^\s+\S/ && defined $key) {
            $text{$key} .= "\n$line";
        } else {
            undef $key;
        }
    }
    close $handle;
    for my $message (keys %text) {
        # 只看占位符内部：译文正文里的字面量 `$` 不是变量。
        while ($text{$message} =~ /\{([^{}]*)\}/g) {
            my $placeable = $1;
            $variables{$message}{$1} = 1 while $placeable =~ /\$([A-Za-z][A-Za-z0-9_-]*)/g;
        }
    }
    return \%variables;
}

my $expected = variables_by_key($base);
my @failures;

for my $locale (@locales) {
    my $actual = variables_by_key($locale);
    for my $key (sort keys %$actual) {
        my @extra = grep { !$expected->{$key}{$_} } sort keys %{ $actual->{$key} };
        push @failures, "$locale: $key 引用了英文译文没有的变量 " . join(', ', map { "\$$_" } @extra)
            if @extra;
    }
}

# 以平衡括号读取 `t!( ... )` 的参数；字符串内的括号与逗号不计入层级。
sub macro_arguments {
    my ($source, $start) = @_;
    my ($depth, $in_string, $escaped, $current, @arguments) = (1, 0, 0, '');
    for (my $index = $start; $index < length $source; $index++) {
        my $character = substr $source, $index, 1;
        if ($in_string) {
            if ($escaped) { $escaped = 0 }
            elsif ($character eq '\\') { $escaped = 1 }
            elsif ($character eq '"') { $in_string = 0 }
            $current .= $character;
            next;
        }
        if ($character eq '"') { $in_string = 1 }
        elsif ($character =~ /[\(\[\{]/) { $depth++ }
        elsif ($character =~ /[\)\]\}]/) {
            $depth--;
            if ($depth == 0) { push @arguments, $current; return @arguments }
        } elsif ($character eq ',' && $depth == 1) { push @arguments, $current; $current = ''; next }
        $current .= $character;
    }
    return;
}

local $/ = "\0";
while (my $path = <STDIN>) {
    chomp $path;
    next unless length $path;
    open my $handle, '<:encoding(UTF-8)', $path or die "无法读取 $path：$!\n";
    my $source = do { local $/; <$handle> };
    close $handle;
    while ($source =~ /\bt!\(/g) {
        my @arguments = macro_arguments($source, pos $source) or next;
        my ($key) = $arguments[0] =~ /^\s*"([A-Za-z0-9_-]+)"\s*$/ or next;
        next unless exists $expected->{$key};
        my %provided;
        for my $argument (@arguments[1 .. $#arguments]) {
            $provided{$1} = 1 if $argument =~ /^\s*([A-Za-z_][A-Za-z0-9_]*)\s*:/;
        }
        my @missing = grep { !$provided{$_} } sort keys %{ $expected->{$key} };
        if (@missing) {
            my $line = 1 + (() = substr($source, 0, pos $source) =~ /\n/g);
            push @failures, "$path:$line: t!(\"$key\") 未传入译文引用的变量 " . join(', ', map { "\$$_" } @missing);
        }
    }
}

if (@failures) {
    print STDERR "Fluent 变量不一致（运行时会使对应语言的页面崩溃）：\n";
    print STDERR "  $_\n" for @failures;
    exit 1;
}
print "Fluent 变量一致性检查通过（$base）\n";
