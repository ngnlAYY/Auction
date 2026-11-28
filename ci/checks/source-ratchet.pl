#!/usr/bin/env perl
# Rust 源码模式计数与只降不升的基线比较；口径与更新步骤见本目录 README。
use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

my $counts_only = @ARGV == 1 && $ARGV[0] eq '--counts';
die "用法：source-ratchet.pl [--counts]\n" if @ARGV && !$counts_only;
my @layers = qw(jobs application http storage integrations apps);
my @keys = ('all:#[path]', 'all:use_super::*', 'all:let_=_error',
    (map { ("$_:Err(_)", "$_:map_err(|_|)") } @layers), 'service-apps:english_message');
my %counts = map { $_ => 0 } @keys;
my %files_per_layer;
my %service_files;

# 去掉注释和字面量，防止文档示例、错误消息及原始字符串影响计数。
# 块注释可嵌套；字符字面量必须有闭合引号，因此不会吞掉 Rust 生命周期。
my $non_code = qr{
    //[^\n]*
    | (?<comment> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&comment) )* \*/ )
    | (?:br|cr|r)(?<hashes>\#*)".*?"\k<hashes>
    | (?:b|c)?"(?:\\.|[^"\\])*"
    | b?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])'
}xs;

# 英文消息（错误消息与 tracing 消息）只统计四个服务装配根的生产范围：tests.rs 与 *_tests.rs
# 整个文件不计；文件内截止口径与 source-layout.pl 一致（首个行首 #[cfg(test)]，可隔属性与注释行，
# 紧跟内联 `mod 名 {`）。
my @service_apps = qw(server worker admin-api public-api);
my $service_app = qr{^apps/auction-(server|worker|admin-api|public-api)/src/};
# 截止位置在掩码文本上匹配：注释或字面量里的同形文本已换成空白，不会提前截止；cfg 与 mod
# 之间可隔属性、注释行或空行。
my $inline_test = qr{
    ^\#\[cfg\(test\)\][ \t]*\n
    (?:[ \t]*\n|[ \t]*\#[^\n]*\][ \t]*\n)*
    [ \t]*(?:pub(?:\([^)]*\))?\s+)?mod\s+(?:r\#)?\w+\s*\{
}xm;
# 注释与字符字面量换成空白；字符串换成带序号的占位符，正文另存，只在消息位置取出判定。
my $comment_or_literal = qr{
    (?<skip> //[^\n]*
        | (?<block> /\* (?: [^*/]++ | /(?!\*) | \*(?!/) | (?&block) )* \*/ )
        | b?'(?:\\(?:u\{[0-9a-fA-F_]+\}|x[0-9a-fA-F]{2}|.)|[^'\\\n])' )
    | (?:br|cr|r)(?<hashes>\#*)"(?<raw>.*?)"\k<hashes>
    | (?:b|c)?"(?<text>(?:\\.|[^"\\])*)"
}xs;
# 消息位置：`.context(`、`.with_context(||`（两者可再包一层 `format!(`）、`bail!(`、`anyhow!(`、
# `#[error(` 之后紧跟的字符串字面量，以及 `ensure!(` 第一个顶层逗号之后紧跟的字符串字面量。
my $message_literal = qr{
    (?: \.context \s*\( \s* (?: format! \s*\( \s* )?
      | \.with_context \s*\( \s* (?: move \s+ )? \|\| \s* (?: format! \s*\( \s* )?
      | \b(?:bail|anyhow)! \s*\( \s*
      | \bensure! \s*\( (?&condition) , \s*
      | \#\[ \s* error \s*\( \s*
    )
    \x{E000}(?<index>\d+)\x{E001}
    (?(DEFINE)
        (?<condition> (?: [^()\[\]{},]++ | (?&group) )* )
        (?<group> \( (?&nested) \) | \[ (?&nested) \] | \{ (?&nested) \} )
        (?<nested> (?: [^()\[\]{}]++ | (?&group) )* )
    )
}x;
# tracing 消息位置：`trace!`/`debug!`/`info!`/`warn!`/`error!`/`event!`（可带路径前缀）顶层参数中第一个
# 单独成项的字符串字面量。`字段 = 值`、`%字段`/`?字段`、`target:`/`parent:`/`name:` 与级别参数都不
# 单独成项，其中的字面量（字段值、target、事件名）不计。
my $tracing_call = qr{
    \b(?:trace|debug|info|warn|error|event)! \s* \( (?<arguments> (?&nested) ) \)
    (?(DEFINE)
        (?<group> \( (?&nested) \) | \[ (?&nested) \] | \{ (?&nested) \} )
        (?<nested> (?: [^()\[\]{}]++ | (?&group) )* )
    )
}x;

# 按括号深度为 0 的逗号切分宏参数；字符串已换成占位符、字符字面量已换成空白，不会干扰深度。
sub top_level_arguments {
    my ($arguments) = @_;
    my @items = ('');
    my $depth = 0;
    for my $char (split //, $arguments) {
        if ($char eq ',' && $depth == 0) {
            push @items, '';
            next;
        }
        $depth++ if $char =~ /[(\[{]/;
        $depth-- if $char =~ /[)\]}]/;
        $items[-1] .= $char;
    }
    return @items;
}

# 消息字面量去掉转义、`{{`/`}}` 与 `{…}` 占位后仍含 ASCII 字母，且全文不含汉字，即计为英文。
sub is_english {
    my ($text) = @_;
    (my $words = $text) =~ s/\\(?:u\{[0-9a-fA-F_]*\}|x[0-9a-fA-F]{2}|.)//gs;
    $words =~ s/\{\{|\}\}//g;
    $words =~ s/\{[^{}]*\}//g;
    return $words =~ /[A-Za-z]/ && $text !~ /\p{Han}/;
}

sub english_messages {
    my ($code) = @_;
    if ($code =~ /^\#\[cfg\(test\)\]/m) {
        (my $masked = $code) =~ s{$non_code}{ (my $blank = $&) =~ tr/\n/ /c; $blank }ge;
        if ($masked =~ $inline_test) {
            my $cut = $-[0];
            $code = substr($code, 0, $cut);
        }
    }
    my @literals;
    $code =~ s{$comment_or_literal}{
        defined $+{skip} ? ' '
            : do { push @literals, $+{raw} // $+{text}; "\x{E000}$#literals\x{E001}" }
    }ge;
    my $count = 0;
    while ($code =~ /$message_literal/g) {
        $count++ if is_english($literals[$+{index}]);
    }
    while ($code =~ /$tracing_call/g) {
        my @arguments = top_level_arguments($+{arguments});
        for my $argument (@arguments) {
            next unless $argument =~ /^\s*\x{E000}(\d+)\x{E001}\s*$/;
            $count++ if is_english($literals[$1]);
            last;
        }
    }
    return $count;
}

my @files = sort split /\0/, do { local $/; <STDIN> // '' };
die "计数棘轮：未找到 Rust 源码\n" unless @files;
for my $file (@files) {
    open my $source, '<', $file or die "$file: $!\n";
    my $code = do { local $/; <$source> };
    if ($file =~ $service_app && $file !~ /tests\.rs$/) {
        $service_files{$1}++;
        $counts{'service-apps:english_message'} += english_messages($code);
    }
    $code =~ s/$non_code/ /g;
    $counts{'all:#[path]'} += () = $code =~ /\#\s*!?\s*\[\s*path\s*=/g;
    # `use super::*` 与花括号第一层中单独的 `*`（`use super::{a, *}`）都是对父模块的 glob。
    while ($code =~ /\buse\s+super\s*::\s*(?:(\*)|\{((?:[^{}]++|\{(?2)\})*)\})/g) {
        if (defined $1) {
            $counts{'all:use_super::*'}++;
            next;
        }
        (my $items = $2) =~ s/\{(?:[^{}]++|(?R))*\}//g;
        $counts{'all:use_super::*'} += grep { /^\s*\*\s*$/ } split /,/, $items;
    }
    # 绑定错误后立即丢弃，等价于被统计的 `_` 形态；不得用它绕过下面两项错误模式。
    $counts{'all:let_=_error'} += () = $code =~ /\blet\s+_\s*=\s*(?:error|err|e)\s*;/g;

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
# 逐个要求命中：装配根改名或移动时失败关闭，不让它的消息悄悄移出统计范围。
for my $app (@service_apps) {
    die "计数棘轮：服务装配根 auction-$app 未找到源码\n" unless $service_files{$app};
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
