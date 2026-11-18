#!/usr/bin/env perl
# 扫描 Dioxus 组件的属性类型。标准输入是 NUL 分隔的 Rust 源文件。

use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

my $literal_or_comment = qr{
    //[^\n]*
    | (?<block> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&block) )* \*/ )
    | (?:br|cr|r)(?<hashes>\#*)".*?"\k<hashes>
    | (?:b|c)?"(?:\\.|[^"\\])*"
    | b?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'
}xs;
my $brackets = qr/(?<brackets> \[ (?: [^\[\]]++ | (?&brackets) )* \] )/x;
my $parens = qr/(?<parens> \( (?: [^()]++ | (?&parens) )* \) )/x;
my $braces = qr/(?<braces> \{ (?: [^{}]++ | (?&braces) )* \} )/x;
my $generics = qr{
    (?<generics> < (?: -> | -(?!>) | [^<>{}()-]++ | $parens | $braces | (?&generics) )* > )
}x;
my $ident = qr/(?:r\#)?\w+/;
my $path = qr/(?::: \s*)? (?:$ident\s*::\s*)*/x;
my $component = qr/$path (?:r\#)?component (?:\s*$parens)?/x;
my $props = qr{
    (?:r\#)?derive \s*\(\s*
    (?:$path $ident \s*,\s*)*
    $path (?:r\#)?Props \s*
    (?:,\s*$path $ident\s*)* ,? \s*\)
}x;
my $meta_item = qr/(?: [^,()\[\]{}]++ | $parens | $brackets | $braces )+/x;
my $optional_callback = qr/\bOption\s*<\s* $path (?:EventHandler|Callback) \s*[<>]/x;

sub matches_attribute {
    my ($attribute, $wanted) = @_;
    return 1 if $attribute =~ /\A\s*$wanted\s*\z/;
    return 0 unless $attribute =~ /\A\s*(?:r\#)?cfg_attr\s*\((.*)\)\s*\z/s;
    my $items = $1;
    # 第一项是条件，只检查其后的同层属性。组内逗号不会切开当前项。
    return 0 unless $items =~ /\G$meta_item,/g;
    while ($items =~ /\G($meta_item)(?:,|\z)/g) {
        return 1 if matches_attribute($1, $wanted);
    }
    return 0;
}

my @files = sort split /\0/, do { local $/; <STDIN> // '' };
die "Web 回调属性门禁未找到 Rust 源码\n" unless @files;
my @failures;
for my $file (@files) {
    open my $source, '<', $file or die "$file: $!\n";
    my $code = do { local $/; <$source> // '' };
    # 掩码保留行号，同时排除注释和字符串中的 Rust 示例。
    $code =~ s/$literal_or_comment/$& =~ s~[^\n]~ ~gr/ge;

    while ($code =~ /
        (?<attrs> (?:\#\s*$brackets\s*)+ )
        (?:pub\b\s*(?:$parens\s*)?)?
        (?:(?:async|const|unsafe|extern)\b\s*)*
        (?<kind>fn|struct)\s+$ident
    /gx) {
        my ($attrs, $kind, $tail_start) = ($+{attrs}, $+{kind}, $+[0]);
        my $wanted = $kind eq 'fn' ? $component : $props;
        next unless grep { matches_attribute(substr($_, 1, -1), $wanted) } $attrs =~ /\#\s*$brackets/g;
        my $tail = substr($code, $tail_start);
        $tail =~ s/\A\s*(?:$generics\s*)?//;
        $tail_start = length($code) - length($tail);
        my $region;
        if ($tail =~ /\A$parens/) {
            $region = $+{parens};
        } elsif ($kind eq 'struct' &&
            $tail =~ /\A(?:where\b(?:$generics|$parens|$brackets|->|-(?!>)|[^;{(<\[\]-]++)*)?(?<fields>$braces)/) {
            $region = $+{fields};
            $tail_start += $-[0] + length($&) - length($region);
        } else {
            next;
        }

        $region =~ s/\#\s*$brackets/$& =~ s~[^\n]~ ~gr/ge;
        while ($region =~ /$optional_callback/g) {
            my $line = 1 + (() = substr($code, 0, $tail_start + $-[0]) =~ /\n/g);
            push @failures, "$file:$line";
        }
    }
}

if (@failures) {
    print STDERR "Dioxus 组件的回调属性不得使用 Option，避免更新时保留已释放的句柄。\n";
    print STDERR "$_\n" for @failures;
    exit 1;
}
print "Web 回调属性门禁通过\n";
