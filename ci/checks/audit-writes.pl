#!/usr/bin/env perl
# 审计写入失败分类门禁：auction-storage 生产源码中每条写入 audit_records 的 SQL 字面量，
# 必须在同一执行语句内以 `.await.record_audit_failure()` 接入审计失败分类；口径见本目录 README。
#
# 用法：perl audit-writes.pl，标准输入为 NUL 分隔的 Rust 源文件。

use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

# 注释、字符串与字符字面量一次切分，避免 URL、示例或转义引号干扰语句边界。
my $token = qr{
    (?<line_comment> //[^\n]* )
    | (?<block_comment> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&block_comment) )* \*/ )
    | (?<string> (?:br|cr|r)(?<hashes>\#*)".*?"\k<hashes> | (?:b|c)?"(?:\\.|[^"\\])*" )
    | (?<char> b?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])' )
}xs;
my $insert = qr/\binsert\s+into\s+(?:public\s*\.\s*)?audit_records\b/i;
# 内联测试模块之后不是生产代码（cfg 与 mod 之间可隔属性、注释行或空行）；外部 `#[cfg(test)] mod x;`
# 声明不截断。截止位置在掩码文本上匹配，注释或字面量里的同形文本已被换成空白，不会提前截止。
my $inline_test = qr{
    ^\#\[cfg\(test\)\][ \t]*\n
    (?:[ \t]*\n|[ \t]*\#[^\n]*\][ \t]*\n)*
    [ \t]*(?:pub(?:\([^)]*\))?\s+)?mod\s+(?:r\#)?\w+\s*\{
}xm;

my @files = sort split /\0/, do { local $/; <STDIN> // '' };
die "审计写入门禁：未找到 Rust 源码\n" unless @files;
my ($sites, @failures) = (0);
for my $file (@files) {
    next if $file =~ m{(?:^|/)(?:tests\.rs|[^/]*_tests\.rs)$};
    open my $source, '<', $file or die "$file: $!\n";
    my $code = do { local $/; <$source> };

    # 掩码文本：注释与字面量换成等长空白（保留换行），只留下可判断语句边界的代码。
    my ($masked, @literals) = ($code);
    while ($code =~ /$token/g) {
        my ($start, $end) = ($-[0], $+[0]);
        my $is_sql = defined $+{string};
        my $text = substr($code, $start, $end - $start);
        (my $blank = $text) =~ s/[^\n]/ /g;
        substr($masked, $start, $end - $start) = $blank;
        push @literals, [$start, $end] if $is_sql && $text =~ $insert;
    }

    if ($masked =~ $inline_test) {
        my $cut = $-[0];
        $code = substr($code, 0, $cut);
        $masked = substr($masked, 0, $cut);
        @literals = grep { $_->[0] < $cut } @literals;
    }

    for my $literal (@literals) {
        my ($start, $end) = @$literal;
        $sites++;
        # 语句到本层或外层的 `;` 为止；已离开字面量所在调用后遇到 `}` 说明包围块结束。
        my ($depth, $stop) = (0, length $masked);
        for my $index ($end .. length($masked) - 1) {
            my $char = substr($masked, $index, 1);
            if ($char =~ /[(\[{]/) {
                $depth++;
            } elsif ($char =~ /[)\]]/) {
                $depth--;
            } elsif ($char eq '}') {
                if ($depth < 0) { $stop = $index; last }
                $depth--;
            } elsif ($char eq ';' && $depth <= 0) {
                $stop = $index;
                last;
            }
        }
        my $statement = substr($masked, $end, $stop - $end);
        next if $statement =~ /\.await\s*\.record_audit_failure\s*\(\s*\)/;
        my $line = 1 + (() = substr($code, 0, $start) =~ /\n/g);
        push @failures, "$file:$line";
    }
}

if (@failures) {
    print STDERR "audit_records 写入须在同一执行语句内 `.await.record_audit_failure()` 接入审计失败分类：\n";
    print STDERR "$_\n" for @failures;
    exit 1;
}
die "审计写入门禁：未找到任何 audit_records 写入，扫描范围可能失效\n" unless $sites;
print "审计写入门禁通过（$sites 处）\n";
