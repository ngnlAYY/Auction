#!/usr/bin/env perl
# W6.2 存储端口方法覆盖率报告：统计 auction-storage 为应用端口实现的方法，有多少在
# PostgreSQL 18 集成测试中被调用过。只输出报告、不作门禁；口径见本目录 README。
use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

my $list_uncovered = @ARGV == 1 && $ARGV[0] eq '--uncovered';
die "用法：port-coverage.pl [--uncovered]\n" if @ARGV && !$list_uncovered;

my $source_root = 'crates/auction-storage/src';
my $test_root = 'crates/auction-storage/tests/postgres';

sub rust_files {
    my ($root) = @_;
    my @files = sort split /\n/, `rg --files "$root" -g '*.rs'`;
    die "端口覆盖率：$root 下未找到 Rust 源码\n" unless @files;
    return @files;
}

# 去掉注释与字符串字面量，避免文档示例或 SQL 文本被当成实现或调用。
my $non_code = qr{
    //[^\n]*
    | (?<comment> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&comment) )* \*/ )
    | (?:br|cr|r)(?<hashes>\#*)".*?"\k<hashes>
    | (?:b|c)?"(?:\\.|[^"\\])*"
}xs;

sub read_code {
    my ($file) = @_;
    open my $handle, '<', $file or die "$file: $!\n";
    my $code = do { local $/; <$handle> };
    $code =~ s/$non_code/ /g;
    return $code;
}

# 只统计“应用端口 trait for 存储类型”的实现块：trait 路径以端口名结尾，实现者是
# `DatabaseStore` 或 `Database*` 适配器；测试模块与私有辅助 impl 不计入。
my %methods;
for my $file (rust_files($source_root)) {
    next if $file =~ m{/tests?/};
    my $code = read_code($file);
    $code =~ s/\#\[cfg\(test\)\]\s*mod\s+tests\s*\{.*//s;
    while ($code =~ /\bimpl(?:<[^>{]*>)?\s+((?:[A-Za-z_][A-Za-z0-9_]*::)*([A-Z][A-Za-z0-9_]*))(?:<[^>{]*>)?\s+for\s+(Database[A-Za-z0-9_]*)(?:<[^>{]*>)?\s*\{/g) {
        my ($trait, $implementer) = ($2, $3);
        my $start = pos($code);
        my ($depth, $index) = (1, $start);
        while ($depth > 0 && $index < length $code) {
            my $char = substr($code, $index, 1);
            $depth++ if $char eq '{';
            $depth-- if $char eq '}';
            $index++;
        }
        my $body = substr($code, $start, $index - $start - 1);
        # 只取 impl 块顶层的方法：去掉方法体后再匹配 `fn name`。
        my $top = '';
        my $level = 0;
        for my $char (split //, $body) {
            $level-- if $char eq '}';
            $top .= $char if $level == 0;
            $level++ if $char eq '{';
        }
        while ($top =~ /\bfn\s+([a-z_][a-z0-9_]*)/g) {
            $methods{"$trait\::$1"} = { trait => $trait, method => $1, implementer => $implementer };
        }
        pos($code) = $index;
    }
}
die "端口覆盖率：未找到任何端口实现\n" unless %methods;

my $tests = join "\n", map { read_code($_) } rust_files($test_root);
my (@covered, @uncovered);
for my $key (sort keys %methods) {
    my $method = $methods{$key}{method};
    # 方法调用或 UFCS 形式都算被执行过；只按名称匹配，同名方法会被一并计为覆盖。
    if ($tests =~ /(?:\.|::)\Q$method\E\s*(?:::<[^>]*>)?\s*\(/) {
        push @covered, $key;
    } else {
        push @uncovered, $key;
    }
}
my %traits = map { $methods{$_}{trait} => 1 } keys %methods;
my $total = keys %methods;
my $hit = @covered;
printf "存储端口覆盖率：%d / %d 个方法（%.1f%%），涉及 %d 个端口 trait\n",
    $hit, $total, 100 * $hit / $total, scalar keys %traits;
print "口径：按方法名在 $test_root 中出现调用即计为覆盖；只报告、不作门禁。\n";
if ($list_uncovered) {
    print "未覆盖：\n";
    print "  $_\n" for @uncovered;
}
