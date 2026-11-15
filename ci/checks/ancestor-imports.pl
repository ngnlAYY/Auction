#!/usr/bin/env perl
# 祖先模块私有导入借用检查（layout 门禁的一部分）；口径见本目录 README。
#
# 子模块经 `super::`、多级 `super::super::` 或 `crate::` 路径取用祖先模块私有 `use` 导入的名字，等于让
# 祖先替子模块囤积导入：祖先删掉或改名这条只服务自己的导入时，错误出现在远处的子模块。本检查从每个
# crate 根（lib.rs/main.rs）按 `mod` 声明建立模块树，展开 use 树（含嵌套分组、`self` 与 `as` 别名）并
# 收集表达式中的 `super::`/`crate::` 路径；路径落到某模块中只是非 pub `use` 导入的名字（`pub(self)`、
# `pub(in self)` 同私有），而该模块既没有同名定义、也没有 pub/pub(crate)/pub(super) 等再导出时报错。
#
# 生产范围：`#[cfg(test)]` 修饰的模块（内联或文件）及其子树整体不计；不像 source-layout.pl 那样按首个
# 内联测试模块截断文件，测试模块之后声明的模块与条目照常检查，挪动声明位置不能绕过检查。tests.rs、
# *_tests.rs 与 tests/ 目录下的文件不计。注释、字符串与字符字面量先换成等长空白，其中的路径不计。
# 不展开宏、不求值其他 cfg：各平台分支的模块都按生产代码检查。模块文件缺失或花括号无法配对时失败
# 关闭，不静默缩小检查范围。
#
# 可达性：输入中除测试文件与 `#[cfg(test)]` 声明的子树外，每个源文件都必须能从某个 crate 根经 `mod`
# 声明到达，孤儿文件（不参与编译的死代码）或未被解析的声明都失败关闭。
#
# 用法：perl ancestor-imports.pl，标准输入为 NUL 分隔的 apps/*/src、crates/*/src 下全部 .rs 文件；
# 其中 src 根的 lib.rs、main.rs 作为 crate 根。

use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

use File::Basename qw(basename dirname);

# 与 source-layout.pl 相同的非代码片段写法。
my $non_code = qr{
    (?=[/"'bcr])
    (?: //[^\n]*
    | (?<comment> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&comment) )* \*/ )
    | (?:br|cr|r)(?<hashes>\#*)".*?"\k<hashes>
    | (?:b|c)?"(?:\\.|[^"\\])*"
    | b?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'
    )
}xs;
my $item_name = qr{(?:(?:async|unsafe|const|extern|default)\s+)*
    (?:fn|struct|enum|trait|type|const|static|union)\s+(?:mut\s+)?([A-Za-z_]\w*)}x;

my (@modules, @failures, %reached, %test_owned, @test_dirs);

sub line_of {
    my ($text, $offset) = @_;
    return 1 + (substr($$text, 0, $offset) =~ tr/\n//);
}

sub is_test_file {
    my ($file) = @_;
    return $file =~ m{(?:^|/)tests/} || $file =~ /tests\.rs$/;
}

sub new_module {
    my (%fields) = @_;
    my $module = {
        %fields,
        children => {},
        pending_children => [],
        private => {},
        public => {},
        defs => {},
        spans => [],
        pending_test => 0,
    };
    push @modules, $module;
    return $module;
}

# 按顶层逗号切分花括号分组内容。
sub split_top {
    my ($body) = @_;
    my (@parts, $current);
    my $depth = 0;
    $current = '';
    for my $char (split //, $body) {
        $depth++ if $char eq '{';
        $depth-- if $char eq '}';
        if ($char eq ',' && $depth == 0) {
            push @parts, $current;
            $current = '';
        } else {
            $current .= $char;
        }
    }
    push @parts, $current;
    return grep { /\S/ } @parts;
}

sub path_segments {
    my ($path) = @_;
    return grep { $_ ne '' } map { s/^\s+|\s+$//gr } split /::/, $path;
}

# 展开 use 树，返回 [路径段, 绑定名]；`a::{self as b}` 绑定 b，glob 的绑定名为 `*`。
sub expand_use {
    my ($tree, @prefix) = @_;
    $tree =~ s/\s+/ /g;
    $tree =~ s/^ | $//g;
    $tree =~ s/^::\s*//;
    if ($tree =~ /^(.*?)\{(.*)\}$/s) {
        my ($head, $body) = ($1, $2);
        $head =~ s/^\s+|\s+$//g;
        if ($head eq '' || $head =~ /::$/) {
            $head =~ s/\s*::$//;
            my @segments = path_segments($head);
            return map { expand_use($_, @prefix, @segments) } split_top($body);
        }
    }
    my $alias;
    ($tree, $alias) = ($1, $2) if $tree =~ /^(.*?)\s+as\s+(\w+)$/;
    my @full = (@prefix, path_segments($tree));
    return () unless @full;
    return ([\@full, '*']) if $full[-1] eq '*';
    if ($full[-1] eq 'self') {
        my @module_path = @full[0 .. $#full - 1];
        return ([\@module_path, $alias // (@module_path ? $module_path[-1] : undef)]);
    }
    return ([\@full, $alias // $full[-1]]);
}

# 从 `$offset` 跳过一个条目（以顶层 `;` 或回到零层的 `}` 结束），返回条目之后的偏移。
sub skip_item {
    my ($text, $offset) = @_;
    my $depth = 0;
    pos($$text) = $offset;
    while ($$text =~ /\G[^()\[\]{};]*+(.)/gcs) {
        my $char = $1;
        if ($char eq '(' || $char eq '[' || $char eq '{') {
            $depth++;
        } elsif ($char eq ')' || $char eq ']' || $char eq '}') {
            $depth--;
            return undef if $depth < 0;
            if ($depth == 0 && $char eq '}') {
                my $end = pos($$text);
                return $$text =~ /\G\s*;/gc ? pos($$text) : $end;
            }
        } elsif ($char eq ';' && $depth == 0) {
            return pos($$text);
        }
    }
    return undef;
}

# 解析模块体的顶层条目：记录 use 绑定、定义、子模块与本模块自身的代码区间（不含内联子模块）。
# 文件模块返回文件末尾偏移，内联模块返回闭合 `}` 之后的偏移；无法配对时返回 undef。
sub parse_body {
    my ($module, $offset, $inline) = @_;
    my $text = $module->{text};
    my $length = length $$text;
    my $span_start = $offset;
    while (1) {
        pos($$text) = $offset;
        $$text =~ /\G\s*/gc;
        $offset = pos($$text);
        if ($offset >= $length) {
            push @{$module->{spans}}, [$span_start, $offset];
            return $inline ? undef : $offset;
        }
        if (substr($$text, $offset, 1) eq '}') {
            push @{$module->{spans}}, [$span_start, $offset];
            return $inline ? $offset + 1 : undef;
        }
        if ($$text =~ /\G\#!?\[/gc) {
            my $depth = 1;
            while ($depth > 0 && $$text =~ /\G[^\[\]]*+([\[\]])/gc) {
                $depth += $1 eq '[' ? 1 : -1;
            }
            return undef if $depth > 0;
            my $attribute = substr($$text, $offset, pos($$text) - $offset);
            $module->{pending_test} = 1 if $attribute =~ /^\#\[\s*cfg\s*\(\s*test\s*\)\s*\]$/;
            $offset = pos($$text);
            next;
        }
        my $start = $offset;
        my $public = 0;
        if ($$text =~ /\Gpub(\s*\([^()]*\))?\s+/gc) {
            my $visibility = $1 // '';
            $public = $visibility !~ /^\s*\(\s*(?:in\s+)?self\s*\)/;
            $offset = pos($$text);
        }
        pos($$text) = $offset;
        if ($$text =~ /\Guse\s+([^;]*);/gc) {
            my $end = pos($$text);
            for my $use (expand_use($1)) {
                my $bind = $use->[1];
                next if !defined $bind || $bind eq '*' || $bind eq '_';
                if ($public) {
                    $module->{public}{$bind} = 1;
                } else {
                    $module->{private}{$bind} //= $start;
                }
            }
            $module->{pending_test} = 0;
            $offset = $end;
            next;
        }
        pos($$text) = $offset;
        if ($$text =~ /\Gmod\s+(\w+)\s*;/gc) {
            my ($name, $end) = ($1, pos($$text));
            $module->{defs}{$name} = 1;
            push @{$module->{pending_children}}, [$name, $module->{pending_test}];
            $module->{pending_test} = 0;
            $offset = $end;
            next;
        }
        pos($$text) = $offset;
        if ($$text =~ /\Gmod\s+(\w+)\s*\{/gc) {
            my ($name, $body) = ($1, pos($$text));
            $module->{defs}{$name} = 1;
            push @{$module->{spans}}, [$span_start, $start];
            if ($module->{pending_test}) {
                # 内联测试模块整体不计，只跳过其花括号；其中 `mod 名;` 的文件也归测试。
                $module->{pending_test} = 0;
                push @test_dirs, "$module->{dir}/$name";
                $offset = skip_item($text, $start);
            } else {
                my $child = new_module(
                    name => $name,
                    path => [@{$module->{path}}, $name],
                    parent => $module,
                    file => $module->{file},
                    dir => "$module->{dir}/$name",
                    text => $text,
                );
                $module->{children}{$name} = $child;
                $offset = parse_body($child, $body, 1);
            }
            return undef unless defined $offset;
            $span_start = $offset;
            next;
        }
        $module->{pending_test} = 0;
        pos($$text) = $offset;
        if ($$text =~ /\G$item_name/gc || $$text =~ /\Gmacro_rules!\s*([A-Za-z_]\w*)/gc) {
            $module->{defs}{$1} = 1;
        }
        $offset = skip_item($text, $offset);
        return undef unless defined $offset;
    }
}

# 加载 `mod 名;` 声明的文件模块；`#[cfg(test)]` 声明的模块与测试文件整棵子树不计，不再解析。
sub load_children {
    my ($module) = @_;
    for my $pending (@{$module->{pending_children}}) {
        my ($name, $test) = @$pending;
        my ($file) = grep { -f } ("$module->{dir}/$name.rs", "$module->{dir}/$name/mod.rs");
        if (!defined $file) {
            push @failures, "$module->{file}: 找不到 mod $name 的源文件";
            next;
        }
        if ($test || is_test_file($file)) {
            # 测试模块的文件与其子目录整体归测试，可达性检查不把它们当孤儿。
            $test_owned{$file} = 1;
            push @test_dirs, "$module->{dir}/$name";
            next;
        }
        $module->{children}{$name} = load_file($name, [@{$module->{path}}, $name], $module, $file);
    }
    load_children($_) for grep { $_->{file} eq $module->{file} } values %{$module->{children}};
}

sub load_file {
    my ($name, $path, $parent, $file) = @_;
    open my $handle, '<', $file or die "$file: $!\n";
    $reached{$file} = 1;
    my $code = do { local $/; <$handle> };
    # 整个文件都解析：内联测试模块由 parse_body 按 `#[cfg(test)]` 整体跳过，不截断其后的生产条目。
    (my $masked = $code) =~ s{$non_code}{ (my $blank = $&) =~ tr/\n/ /c; $blank }ge;
    # `mod 名;` 的源文件位于 mod.rs/lib.rs/main.rs 所在目录，或 `名.rs` 旁的同名目录。
    my $base = basename($file);
    my $dir = dirname($file);
    $dir .= '/' . ($base =~ s/\.rs$//r) unless $base =~ /^(?:mod|lib|main)\.rs$/;
    my $module = new_module(
        name => $name,
        path => $path,
        parent => $parent,
        file => $file,
        dir => $dir,
        text => \$masked,
    );
    push @failures, "$file: 花括号无法配对，模块结构无法解析" unless defined parse_body($module, 0, 0);
    load_children($module);
    return $module;
}

# 沿 crate::/super::/self:: 路径走模块树；落到某模块（不是引用方自身）只有私有 use 绑定的名字时返回该模块。
sub resolve {
    my ($module, $root, @segments) = @_;
    my ($current, $index);
    # 宏体里的 `$crate::` 指向宏所在 crate 的根，与 `crate::` 同口径。
    if ($segments[0] eq 'crate' || $segments[0] eq '$crate') {
        ($current, $index) = ($root, 1);
    } elsif ($segments[0] eq 'super') {
        ($current, $index) = ($module->{parent}, 1);
        while ($current && $index < @segments && $segments[$index] eq 'super') {
            $current = $current->{parent};
            $index++;
        }
    } elsif ($segments[0] eq 'self') {
        ($current, $index) = ($module, 1);
    } else {
        return;
    }
    while ($current && $index < @segments) {
        my $segment = $segments[$index];
        if (my $child = $current->{children}{$segment}) {
            ($current, $index) = ($child, $index + 1);
            next;
        }
        return if $current == $module || !exists $current->{private}{$segment};
        return if $current->{defs}{$segment} || $current->{public}{$segment};
        return ($current, $segment);
    }
    return;
}

sub module_name {
    my ($module) = @_;
    return join '::', 'crate', @{$module->{path}};
}

my @inputs = sort grep { length } split /\0/, do { local $/; <STDIN> // '' };
my @roots = grep { m{^(?:apps|crates)/[^/]+/src/(?:lib|main)\.rs$} } @inputs;
die "祖先模块私有导入：未找到 crate 根文件\n" unless @roots;
my %findings;
for my $root_file (@roots) {
    die "$root_file: 不存在\n" unless -f $root_file;
    @modules = ();
    my $root = load_file('crate', [], undef, $root_file);
    for my $module (@modules) {
        my $text = $module->{text};
        my $report = sub {
            my ($offset, $path, @segments) = @_;
            my ($owner, $name) = resolve($module, $root, @segments) or return;
            my $message = sprintf '%s:%d: %s 借用祖先模块 %s 的私有导入（%s:%d；路径 %s）',
                $module->{file}, line_of($text, $offset), $name, module_name($owner),
                $owner->{file}, line_of($owner->{text}, $owner->{private}{$name}), $path;
            $findings{$message} = 1;
        };
        for my $span (@{$module->{spans}}) {
            my ($start, $end) = @$span;
            my $chunk = substr($$text, $start, $end - $start);
            my @use_ranges;
            while ($chunk =~ /(?<![\w\$])use\s+([^;]*);/g) {
                my ($tree, $from, $to) = ($1, $-[0], $+[0]);
                push @use_ranges, [$from, $to];
                for my $use (expand_use($tree)) {
                    my @segments = @{$use->[0]};
                    $report->($start + $from, join('::', @segments), @segments) if @segments;
                }
            }
            while ($chunk =~ /(?<![\w:])((?:super|crate)(?:\s*::\s*(?:super|\w+))+)/g) {
                my ($path, $from) = ($1, $-[0]);
                next if grep { $_->[0] <= $from && $from < $_->[1] } @use_ranges;
                my @segments = path_segments($path);
                $report->($start + $from, join('::', @segments), @segments);
            }
        }
    }
}
# 可达性：每个非测试源文件都必须能从某个 crate 根经 `mod` 声明到达。到达不了的是孤儿文件
# （不参与编译的死代码），或是本脚本没解析到的模块声明；两种情况都失败关闭。
for my $file (@inputs) {
    next if $reached{$file} || $test_owned{$file} || is_test_file($file);
    next if grep { index($file, "$_/") == 0 } @test_dirs;
    push @failures, "$file: 未被任何 crate 根的模块树引用（孤儿文件，或模块声明未被解析）";
}
if (@failures) {
    print STDERR "祖先模块私有导入检查无法建立完整模块树：\n";
    print STDERR "$_\n" for @failures;
}
if (%findings) {
    print STDERR "子模块不得经 super::/crate:: 路径借用祖先模块的私有 use 导入，请改为从定义处或门面导入：\n";
    print STDERR "$_\n" for sort keys %findings;
}
exit 1 if @failures || %findings;
