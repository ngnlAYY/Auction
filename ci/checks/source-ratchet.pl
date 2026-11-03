#!/usr/bin/env perl
# Rust 源码模式计数与只降不升的基线比较；口径与更新步骤见本目录 README。
use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

my $counts_only = @ARGV == 1 && $ARGV[0] eq '--counts';
die "用法：source-ratchet.pl [--counts]\n" if @ARGV && !$counts_only;
my @layers = qw(jobs application http storage integrations apps);
my @keys = ('all:#[path]', 'all:use_super::*',
    map { ("$_:Err(_)", "$_:map_err(|_|)") } @layers);
my %counts = map { $_ => 0 } @keys;
my %files_per_layer;

# 去掉注释和字面量，防止文档示例、错误消息及原始字符串影响计数。
# 块注释可嵌套；字符字面量必须有闭合引号，因此不会吞掉 Rust 生命周期。
my $non_code = qr{
    //[^\n]*
    | (?<comment> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&comment) )* \*/ )
    | (?:br|cr|r)(?<hashes>\#*)".*?"\k<hashes>
    | (?:b|c)?"(?:\\.|[^"\\])*"
    | b?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'
}xs;
my @files = sort split /\0/, do { local $/; <STDIN> // '' };
die "计数棘轮：未找到 Rust 源码\n" unless @files;
for my $file (@files) {
    open my $source, '<', $file or die "$file: $!\n";
    my $code = do { local $/; <$source> };
    $code =~ s/$non_code/ /g;
    $counts{'all:#[path]'} += () = $code =~ /\#\s*!?\s*\[\s*path\s*=/g;
    $counts{'all:use_super::*'} += () = $code =~ /\buse\s+super\s*::\s*\*/g;

    my ($layer) = $file =~ m{^crates/auction-(jobs|application|http|storage|integrations)/src/};
    $layer = 'apps' if $file =~ m{^apps/[^/]+/src/};
    next unless defined $layer;
    $files_per_layer{$layer}++;
    $counts{"$layer:Err(_)"} += () =
        $code =~ /\bErr\s*\(\s*(?:ref\s+)?(?:mut\s+)?_\w*\s*\)/g;
    $counts{"$layer:map_err(|_|)"} += () =
        $code =~ /\bmap_err\s*\(\s*(?:move\s+)?\|\s*(?:mut\s+)?_\w*\s*\|/g;
}
for my $layer (@layers) {
    die "计数棘轮：$layer 层未找到源码\n" unless $files_per_layer{$layer};
}
if ($counts_only) {
    print "$_ $counts{$_}\n" for @keys;
    exit 0;
}

my $baseline_file = 'ci/checks/source-ratchet.baseline';
open my $baseline, '<', $baseline_file or die "$baseline_file: $!\n";
my %limits;
while (<$baseline>) {
    chomp;
    next if /^\s*(?:#|$)/;
    die "$baseline_file:$.: 无效或重复的基线条目\n"
        unless /^(\S+) (0|[1-9][0-9]*)$/ && exists $counts{$1} && !exists $limits{$1};
    $limits{$1} = $2;
}
my $failures = 0;
for my $key (@keys) {
    die "$baseline_file: 缺少 $key\n" unless exists $limits{$key};
    if ($counts{$key} > $limits{$key}) {
        printf STDERR "计数棘轮超出基线：%s 当前 %d > 基线 %d（+%d）\n",
            $key, $counts{$key}, $limits{$key}, $counts{$key} - $limits{$key};
        $failures++;
    } elsif ($counts{$key} < $limits{$key}) {
        printf "计数已下降：%s 当前 %d < 基线 %d；请更新 %s\n",
            $key, $counts{$key}, $limits{$key}, $baseline_file;
    }
}
exit 1 if $failures;
print "计数棘轮通过（", scalar(@keys), " 项）\n";
