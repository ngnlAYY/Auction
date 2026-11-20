#!/usr/bin/env perl
# 核对应用层与 Admin 两套实现的热字段常量数组。
use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

sub cannot_read_element {
    my ($path, $source, $offset, $message) = @_;
    my $prefix = substr($source, 0, $offset);
    my $line = 1 + ($prefix =~ tr/\n/\n/);
    my $column = $offset - rindex($prefix, "\n");
    die "$path:$line:$column: $message\n";
}

sub read_paths {
    my ($path, $name) = @_;
    open my $file, '<:encoding(UTF-8)', $path or die "无法读取 $path: $!\n";
    local $/;
    my $source = <$file> // '';
    my $is_rust = $path =~ /\.rs$/;
    my $block_comment = $is_rust
        ? qr{(?<block> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&block) )* \*/ )}x
        : qr{/\*.*?\*/}s;
    # 注释换成等长空白，保留字符串、行号与列号。
    $source =~ s{
        ("(?:\\.|[^"\\])*" | '(?:\\.|[^'\\\r\n])*' | \x60(?:\\.|[^\x60\\])*\x60)
        | (//[^\r\n]* | $block_comment)
    }{$1 // ($2 =~ s/[^\r\n]/ /gr)}gsex;

    my $array = $is_rust ? qr/\&\s*\[\s*\&\s*str\s*\]\s*=\s*\&\s*\[/
                        : qr/readonly\s+string\s*\[\s*\]\s*=\s*\[/;
    $source =~ /^\h*(?:(?:pub(?:\([^)\n]*\))?|export)\s+)?const\s+\Q$name\E\s*:\s*$array/mg
        or die "无法从 $path 读取热字段表 $name\n";

    my $literal = $is_rust ? qr/"([a-z0-9_.]+)"/ : qr/(["'])([a-z0-9_.]+)\1/;
    my @paths;
    while (1) {
        $source =~ /\G\s*/gc;
        last if $source =~ /\G\]/gc;
        my $start = pos($source);
        $source =~ /\G$literal/gc
            or cannot_read_element($path, $source, $start, '热字段表项必须是字段路径字符串字面量');
        push @paths, $is_rust ? $1 : $2;
        $source =~ /\G\s*/gc;
        next if $source =~ /\G,/gc;
        last if $source =~ /\G\]/gc;
        cannot_read_element($path, $source, pos($source), '热字段表项后应为逗号或数组结束符');
    }
    $source =~ /\G\s*;/gc && @paths
        or cannot_read_element($path, $source, pos($source), '热字段表必须是非空的数组字面量');
    return @paths;
}

my $owner = 'crates/auction-application/src/ports/admin/runtime_config/hot.rs';
my @expected = read_paths($owner, 'WORKER_HOT_FIELD_PATHS');
for my $mirror (
    'crates/auction-web-admin/src/surface/config/hot_fields.rs',
    'web/admin/src/surface/config/hot-fields.ts',
) {
    my @actual = read_paths($mirror, 'HOT_FIELDS');
    my $last = $#expected > $#actual ? $#expected : $#actual;
    for my $index (0 .. $last) {
        my $want = $expected[$index] // '（无）';
        my $got = $actual[$index] // '（无）';
        next if $got eq $want;
        my $item = $index + 1;
        die "$mirror 与 $owner 的热字段表必须逐项同序，第 $item 项应为 $want，实际为 $got\n";
    }
}
