#!/usr/bin/env perl
# FTL 键可达性检查：en.ftl 的每个键都必须能在同一 surface 源码的字符串字面量中找到。
# 既覆盖 t!("key") 静态调用，也覆盖字面量经变量传入 t! 的动态调用（设置表单的字段 id、
# 图例、星期标签，以及 format!("{id}-help") 的辅助键）。新增动态拼接模式时本检查失败关闭，
# 需要显式扩展规则，而不是放行。
use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));

my $ftl = shift // die "用法：i18n-unused-keys.pl <en.ftl>（源文件列表从 STDIN 以 NUL 分隔）\n";
my @files = sort split /\0/, do { local $/; <STDIN> // '' };
die "$ftl: 未收到源码文件列表\n" unless @files;

open my $fh, '<', $ftl or die "$ftl: $!\n";
my %keys;
while (<$fh>) {
    $keys{$1} = 1 if /^([A-Za-z][A-Za-z0-9_-]*)\s*=/;
}
close $fh;
die "$ftl: 未解析到任何消息键\n" unless %keys;

my %lits;
for my $file (@files) {
    open my $src, '<', $file or die "$file: $!\n";
    my $code = do { local $/; <$src> };
    while ($code =~ /"([A-Za-z][A-Za-z0-9_-]*)"/g) {
        $lits{$1} = 1;
    }
}

my $failed = 0;
for my $key (sort keys %keys) {
    next if $lits{$key};
    # format!("{id}-help")：以字段 id 的字面量为前缀的辅助键。
    my $prefix = $key;
    next if $prefix =~ s/-help$// && $lits{$prefix};
    printf STDERR "%s: 消息键 %s 在源码中不可达\n", $ftl, $key;
    $failed = 1;
}
exit 1 if $failed;
print "未引用译文键检查通过（$ftl，", scalar(keys %keys), " 个键）\n";
