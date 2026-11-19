#!/usr/bin/env perl
# 译文平铺子集检查。
#
# 两套实现共用同一份 `.ftl`，但 TypeScript 侧的加载器
# （web/shared/src/ui/i18n/resource.ts）只接受最简单的 Fluent 形式：空行、以 `#` 开头的注释，
# 以及单行的 `键 = 文本`。文本里的占位只允许 `{ $变量 }` 与 `{"字符串"}`。续行、选择表达式、
# 属性、术语、消息引用与函数调用都会让加载器拒绝整份译文；Rust 侧即使能解析，切换到 Solid
# 后也会打不开页面，因此在这里统一失败关闭。
#
# 分行与加载器一致：CRLF、LF、CR 都是换行；U+2028/U+2029 留在行内，加载器的消息行正则不
# 接受它们。
#
# 用法：perl i18n-flat-subset.pl <file.ftl>...
# 诊断包含文件与行号；注释行与空行不参与检查。

use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

use FindBin qw($Bin);
use lib $Bin;
use I18nFluent qw(body_fits_subset message_line split_lines);

my @failures;

sub fail {
    my ($file, $line, $message) = @_;
    push @failures, "$file:$line: $message";
}

for my $file (@ARGV) {
    open my $handle, '<:encoding(UTF-8)', $file or die "$file: $!\n";
    my $text = do { local $/; <$handle> };
    close $handle;
    my @lines = split_lines($text);
    for my $index (0 .. $#lines) {
        my $line = $lines[$index];
        my $line_number = $index + 1;
        next if $line eq '';
        next if $line =~ /^#/;
        if ($line =~ /[\x{2028}\x{2029}]/) {
            fail($file, $line_number, '正文包含 U+2028/U+2029（加载器按单行处理）');
            next;
        }
        if ($line =~ /^-[A-Za-z][A-Za-z0-9_-]*\s*=/) {
            fail($file, $line_number, '术语定义（-term =）');
            next;
        }
        if ($line =~ /^\s+\.[A-Za-z][A-Za-z0-9_-]*\s*=/) {
            fail($file, $line_number, '属性（.attr =）');
            next;
        }
        my ($key, $before, $after, $body) = message_line($line);
        if (defined $key) {
            if ($body =~ /^\s*$/) {
                fail($file, $line_number, '消息正文为空');
                next;
            }
            my $result = body_fits_subset($body);
            fail($file, $line_number, $result->{error}) if $result->{error};
            next;
        }
        if ($line =~ /^\s+\S/) {
            fail($file, $line_number, '续行（每条译文必须是单行 键 = 文本）');
            next;
        }
        fail($file, $line_number, '不是单行 键 = 文本');
    }
}

if (@failures) {
    print STDERR "Fluent 平铺子集检查失败（TypeScript 加载器会拒绝整份译文）：\n";
    print STDERR "  $_\n" for @failures;
    exit 1;
}
print "Fluent 平铺子集检查通过（", scalar(@ARGV), " 个文件）\n";
