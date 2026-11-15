#!/usr/bin/env perl
# 生产源码的行数上限与私有 glob 导入检查（layout 门禁的一部分）；口径见本目录 README。
#
# 生产范围从文件开头数到首个“行首 #[cfg(test)] 之后（可隔属性与注释行）紧跟内联 `mod 名 {`”为止；
# `#[cfg(test)] use …;`、`#[cfg(test)] mod tests;` 这类外置声明不截止，不能靠前置 cfg(test) 条目绕过
# 行数或导入检查。
# 调用方已排除 tests/ 目录与 tests.rs、*_tests.rs。
#
# 用法：perl source-layout.pl，标准输入为 NUL 分隔的生产 Rust 源文件。

use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

# 截止口径与 audit-writes.pl 一致：只有内联测试模块之后不是生产代码。
my $inline_test = qr{
    ^\#\[cfg\(test\)\][ \t]*\n
    (?:[ \t]*(?:\#\[[^\n]*\]|//[^\n]*)[ \t]*\n)*
    [ \t]*(?:pub(?:\([^)]*\))?\s+)?mod\s+\w+\s*\{
}xm;
# 注释、字符串与字符字面量换成等长空白（保留换行），文档示例与字面量里的 `use …::*` 不计入。
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
my %local_root = map { $_ => 1 } qw(crate self super);

# 返回 use 树中每个 glob 的完整前缀路径与写法：`a::{b::*, *}` 得到叶子 [a, b] 与花括号内单独的 [a]。
sub glob_paths {
    my ($tree) = @_;
    my (@stack, @current, @globs) = ([]);
    for my $token ($tree =~ /(\w+|::|[{},*])/g) {
        if ($token eq '{') {
            push @stack, [@{$stack[-1]}, @current];
            @current = ();
        } elsif ($token eq '}') {
            pop @stack;
            @current = ();
        } elsif ($token eq ',') {
            @current = ();
        } elsif ($token eq '*') {
            push @globs, { path => [@{$stack[-1]}, @current], standalone => !@current };
        } elsif ($token ne '::') {
            push @current, $token;
        }
    }
    return @globs;
}

# 只放行外部 crate 的 `prelude::*` 叶子与单段枚举变体 glob（`use Enum::*;`）；模块名是 snake_case，
# 大写开头的单段前缀只能是已导入的类型。花括号内单独的 `*` 一律不放行。
sub allowed_glob {
    my ($glob) = @_;
    my $path = $glob->{path};
    return 0 if $glob->{standalone} || !@$path;
    return 1 if @$path == 1 && $path->[0] =~ /^[A-Z]/;
    return $path->[-1] eq 'prelude' && !$local_root{$path->[0]};
}

my @files = sort split /\0/, do { local $/; <STDIN> // '' };
die "源码布局：未找到 Rust 源码\n" unless @files;
my (@oversized, @globs);
for my $file (@files) {
    open my $handle, '<', $file or die "$file: $!\n";
    my $code = do { local $/; <$handle> };
    $code = substr($code, 0, $-[0]) if $code =~ $inline_test;

    my $lines = () = $code =~ /\n/g;
    $lines++ if length $code && substr($code, -1) ne "\n";
    my $limit = $file =~ m{(?:^|/)mod\.rs$} ? 500 : 600;
    push @oversized, "$file ($lines 行，上限 $limit)" if $lines > $limit;

    (my $masked = $code) =~ s{$non_code}{ (my $blank = $&) =~ tr/\n/ /c; $blank }ge;
    # 整条语句匹配，跨行与花括号嵌套都按一条 use 处理；pub / pub(..) use 是门面再导出，不检查，
    # 但 pub(self)、pub(in self) 等同私有。
    while ($masked =~ /(?<![\w\$])(pub(?:\s*\([^()]*\))?\s+)?use\s+([^;]*);/g) {
        my ($visibility, $tree, $start) = ($1, $2, $-[0]);
        next if defined $visibility && $visibility !~ /^pub\s*\(\s*(?:in\s+)?self\s*\)/;
        next unless grep { !allowed_glob($_) } glob_paths($tree);
        my $line = 1 + (substr($masked, 0, $start) =~ tr/\n//);
        push @globs, sprintf('%s:%d: use %s;', $file, $line, $tree =~ s/\s+/ /gr =~ s/^ | $//gr);
    }
}
if (@oversized) {
    print STDERR "生产源码文件超过行数上限（mod.rs 500 行、其他 600 行），请按职责拆分子模块：\n";
    print STDERR "$_\n" for @oversized;
}
if (@globs) {
    print STDERR "生产代码不得使用私有 glob 导入，请改为显式导入：\n";
    print STDERR "$_\n" for @globs;
}
exit 1 if @oversized || @globs;
