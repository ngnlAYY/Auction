#!/usr/bin/env perl
# Rust 导入分组检查。
#
# 稳定版 rustfmt 只在连续的 use 段内排序，不负责分组；本检查要求导入按
#   1. std / core / alloc
#   2. 外部 crate（含工作区内其他 crate）
#   3. 本 crate（crate::、super::、self::、本文件声明的子模块或 `as` 别名）
# 三组排列，分两层检查：
#   - 导入块：组间恰好一个空行、组内不留空行、块内组序不回退。导入块从文件头文档、mod 声明、
#     空行或内联 `mod x {` 之后开始，由私有 use（可多行）与紧贴其上的属性组成，遇到其他行即
#     结束；pub use 门面与含注释的块不在范围内。
#   - 导入区：每个模块（文件根与内联 `mod x { … }`）开头的私有 use、pub(..) use、mod 声明与属性
#     组成导入区。区内全部私有 use（跳过 pub(..) use）的组序不回退；首个其他条目（fn、struct、
#     impl、const、macro_rules! 等）之后不得再出现模块级私有 use。函数体等块内的 use 不在此列。
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
# 注释、字符串与字符字面量换成等长空白（保留换行），导入区只按代码结构切分条目。
# 前置的首字符断言只是加速：每种非代码片段都以这些字符开头。
my $non_code = qr{
    (?=[/"'bcr])
    (?: //[^\n]*
    | (?<comment> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&comment) )* \*/ )
    | (?:br|cr|r)(?<hashes>\#*)".*?"\k<hashes>
    | (?:b|c)?"(?:\\.|[^"\\])*"
    | b?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'
    )
}xs;
my @group_names = ('std', '外部 crate', '本 crate');

sub group_of {
    my ($first, $local) = @_;
    return $std{$first} ? 0 : ($local_root{$first} || $local->{$first}) ? 2 : 1;
}

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
        push @groups, group_of($first, $local);
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

# 导入区检查的解析状态：当前文件的掩码文本、本地模块名与违规列表。
my ($region_file, $masked, $region_local, @region_failures);

sub line_at {
    my ($offset) = @_;
    return 1 + (substr($masked, 0, $offset) =~ tr/\n//);
}

# 跳过一个非 use/mod 条目，返回其结束位置：深度 0 的 `;`，或让深度回到 0 的 `}`（其后紧跟的 `;`
# 一并吞掉，如 `const X: T = T { … };`）。
sub skip_item {
    my ($offset) = @_;
    my $depth = 0;
    pos($masked) = $offset;
    while ($masked =~ /\G[^(){}\[\];]*([(){}\[\];])/gc) {
        my $delimiter = $1;
        if ($delimiter =~ /[(\[{]/) {
            $depth++;
        } elsif ($delimiter eq ';') {
            return pos($masked) if $depth == 0;
        } elsif (--$depth == 0 && $delimiter eq '}') {
            my $end = pos($masked);
            return $masked =~ /\G\s*;/gc ? pos($masked) : $end;
        } elsif ($depth < 0) {
            return undef;
        }
    }
    return undef;
}

# 解析一个模块体（文件根或内联 mod），返回模块结束位置；括号不平衡时返回 undef。
sub check_region {
    my ($offset, $inline) = @_;
    my ($open, $highest, $highest_line) = (1, 0, 0);
    while (1) {
        pos($masked) = $offset;
        $masked =~ /\G\s*/gc;
        $offset = pos($masked);
        return $inline ? undef : $offset if $offset >= length $masked;
        if ($masked =~ /\G\}/gc) {
            return $inline ? pos($masked) : undef;
        }
        if ($masked =~ /\G#!?\[/gc) {
            my $depth = 1;
            while ($depth > 0) {
                return undef unless $masked =~ /\G[^\[\]]*([\[\]])/gc;
                $depth += $1 eq '[' ? 1 : -1;
            }
            $offset = pos($masked);
            next;
        }
        my $start = $offset;
        # pub(self)、pub(in self) 等同私有，与普通私有 use 一样参与组序与位置检查。
        my $public = $masked =~ /\Gpub(?!\s*\(\s*(?:in\s+)?self\s*\))(?:\s*\([^()]*\))?\s+/gc;
        $masked =~ /\Gpub\s*\([^()]*\)\s+/gc unless $public;
        if ($masked =~ /\Guse\s+([^;]*);/gc) {
            my ($tree, $end) = ($1, pos($masked));
            if (!$public) {
                # `use { … }` 总括分组没有首段，由 layout 门禁拒绝；这里只按位置检查。
                my ($first) = $tree =~ /^(?:::\s*)?([A-Za-z_]\w*)/;
                my $group = defined $first ? group_of($first, $region_local) : $highest;
                my $line = line_at($start);
                if (!$open) {
                    push @region_failures,
                        "$region_file:$line: 模块级私有 use 必须位于模块开头的导入区（首个非 use/mod 条目之前）";
                } elsif ($group < $highest) {
                    push @region_failures, sprintf(
                        '%s:%d: 导入区分组顺序应为 std → 外部 crate → 本 crate（第 %d 行已是%s）',
                        $region_file, $line, $highest_line, $group_names[$highest]);
                } else {
                    ($highest, $highest_line) = ($group, $line);
                }
            }
            $offset = $end;
            next;
        }
        if ($masked =~ /\Gmod\s+\w+\s*;/gc) {
            $offset = pos($masked);
            next;
        }
        if ($masked =~ /\Gmod\s+\w+\s*\{/gc) {
            $offset = check_region(pos($masked), 1);
            return undef unless defined $offset;
            next;
        }
        $open = 0;
        $offset = skip_item($start);
        return undef unless defined $offset;
    }
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

    ($region_file, $region_local, @region_failures) = ($file, \%local);
    ($masked = "$text\n") =~ s{$non_code}{ (my $blank = $&) =~ tr/\n/ /c; $blank }ge;
    push @region_failures, "$file: 无法识别模块结构（括号不平衡），导入区未检查"
        unless defined check_region(0, 0);
    push @failures, @region_failures;
}
if (@failures) {
    print STDERR "Rust 导入分组不符合 std → 外部 crate → 本 crate：\n";
    print STDERR "$_\n" for @failures;
    exit 1;
}
