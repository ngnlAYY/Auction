#!/usr/bin/env perl
# Rust 导入分组检查。
#
# 稳定版 rustfmt 只在连续的 use 段内排序，不负责分组；本检查要求每个导入块按
#   1. std / core / alloc
#   2. 外部 crate（含工作区内其他 crate）
#   3. 本 crate（crate::、super::、self::、本文件声明的子模块或 `as` 别名）
# 三组排列，组间恰好一个空行、组内不留空行。导入块从文件头文档、mod 声明、空行或内联
# `mod x {` 之后开始，由私有 use（可多行）与紧贴其上的属性组成，遇到其他行即结束；
# pub use 门面、函数内 use 与含注释的块不在范围内。
#
# 用法：perl import-groups.pl，标准输入为 NUL 分隔的 Rust 源文件。

use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

my %std = map { $_ => 1 } qw(std core alloc);
my %local_root = map { $_ => 1 } qw(crate super self);
my $use_start = qr/^(\s*)use\s+(?:::)?([A-Za-z_][A-Za-z0-9_]*)/;
my $mod_decl = qr/^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([a-z_][a-z0-9_]*)\s*;/;

# 从 $start 起识别一个导入块；返回 (块结束行, 违规说明或 undef)。
sub check_block {
    my ($lines, $start, $local) = @_;
    my ($i, $indent, @groups, @gaps) = ($start);
    my ($pending_attrs, $blank_before) = (0, 0);
    while ($i < @$lines) {
        my $line = $lines->[$i];
        if ($line =~ /^\s*$/) {
            return ($i, undef) if $pending_attrs;
            $blank_before++;
            $i++;
            next;
        }
        if ($line =~ /^\s*#\[/ && $line !~ /^\s*#\[cfg\(test\)\]/) {
            $pending_attrs++;
            $i++;
            next;
        }
        my ($this_indent, $first) = $line =~ $use_start or last;
        $indent //= $this_indent;
        last if $this_indent ne $indent;
        my ($end, $depth, $comment) = ($i, 0, 0);
        while (1) {
            my $text = $lines->[$end];
            $comment ||= $text =~ m{//};
            $depth += () = $text =~ /\{/g;
            $depth -= () = $text =~ /\}/g;
            last if $depth <= 0 && $text =~ /;\s*$/;
            $end++;
        }
        return ($end + 1, undef) if $comment;
        my $group = $std{$first} ? 0 : ($local_root{$first} || $local->{$first}) ? 2 : 1;
        push @groups, $group;
        push @gaps, $blank_before;
        ($pending_attrs, $blank_before) = (0, 0);
        $i = $end + 1;
    }
    $i -= $pending_attrs;
    for my $k (1 .. $#groups) {
        my ($previous, $current, $gap) = ($groups[$k - 1], $groups[$k], $gaps[$k]);
        return ($i, '分组顺序应为 std → 外部 crate → 本 crate') if $current < $previous;
        return ($i, '不同分组之间需要恰好一个空行') if $current != $previous && $gap != 1;
        return ($i, '同一分组内不留空行') if $current == $previous && $gap;
    }
    return ($i, undef);
}

my @files = sort split /\0/, do { local $/; <STDIN> // '' };
die "导入分组：未找到 Rust 源码\n" unless @files;
my @failures;
for my $file (@files) {
    open my $handle, '<', $file or die "$file: $!\n";
    my @lines = <$handle>;
    chomp @lines;
    my $text = join "\n", @lines;
    my %local = map { $_ => 1 } ($text =~ /^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s+([a-z_][a-z0-9_]*)\s*;/mg),
        ($text =~ /\bas\s+([A-Za-z_][A-Za-z0-9_]*)\s*[;,}]/g);
    my ($i, $at_block_start) = (0, 1);
    while ($i < @lines) {
        my $line = $lines[$i];
        my $j = $i;
        $j++ while $at_block_start && $j < @lines && $lines[$j] =~ /^\s*#\[/
            && $lines[$j] !~ /^\s*#\[cfg\(test\)\]/;
        if ($at_block_start && $j < @lines && $lines[$j] =~ $use_start) {
            my ($end, $failure) = check_block(\@lines, $i, \%local);
            push @failures, sprintf('%s:%d: %s', $file, $i + 1, $failure) if $failure;
            ($i, $at_block_start) = ($end, 0);
            next;
        }
        my $stripped = $line =~ s/^\s+|\s+$//gr;
        $at_block_start = $stripped eq '' || $stripped =~ m{^//!} || $stripped =~ /^#!/
            || ($stripped =~ /\{$/ && $line =~ /^\s*(?:pub(?:\([^)]*\))?\s+)?mod\s/)
            || $line =~ $mod_decl
            || ($stripped =~ /^#\[/ && $at_block_start);
        $i++;
    }
}
if (@failures) {
    print STDERR "Rust 导入分组不符合 std → 外部 crate → 本 crate：\n";
    print STDERR "$_\n" for @failures;
    exit 1;
}
