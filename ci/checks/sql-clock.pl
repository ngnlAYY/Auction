#!/usr/bin/env perl
# 按 Rust 字符串字面量限制墙上时钟调用次数，输入为 NUL 分隔的源文件路径。

use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

my $token = qr{
    //[^\n]*
    | (?<comment> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&comment) )* \*/ )
    | (?<literal> (?:br|cr|r)(?<hashes>\#*)".*?"\k<hashes>
                | (?:b|c)?"(?:\\.|[^"\\])*" )
    | b?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'
}xs;

my $clock = qr/\bclock_timestamp\s*\(\s*\)/i;
my @files = sort split /\0/, do { local $/; <STDIN> // '' };
die "SQL 时钟门禁：未找到 Rust 源码\n" unless @files;

my @failures;
for my $file (@files) {
    open my $source, '<', $file or die "$file: $!\n";
    my $code = do { local $/; <$source> };
    while ($code =~ /$token/g) {
        next unless defined $+{literal};
        my $start = $-[0];
        next unless $+{literal} =~ /$clock.*$clock/s;
        my $line = 1 + (() = substr($code, 0, $start) =~ /\n/g);
        push @failures, "$file:$line";
    }
}

if (@failures) {
    print STDERR "每个 SQL 字面量只允许调用一次 clock_timestamp()，请复用同一墙钟读数：\n";
    print STDERR "$_\n" for @failures;
    exit 1;
}
print "SQL 时钟门禁通过\n";
