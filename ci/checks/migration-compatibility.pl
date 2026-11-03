#!/usr/bin/env perl

use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));
use File::Basename qw(basename);

# 仅限定登记的可审阅 SQL 子集，不推断 compatible 声明是否符合业务语义。
# 旧迁移不改写；新迁移的顶层末尾必须无条件登记自身，事务由 SQLx 管理。
my $table = 'schema_migration_compatibility';
my $failures = 0;
my %versions;
@ARGV or die "必须提供迁移文件清单\n";

sub statements {
    my ($sql) = @_;
    my @statements;
    my @tokens;
    pos($sql) = 0;
    while (pos($sql) < length($sql)) {
        next if $sql =~ /\G\s+/gc;
        if ($sql =~ /\G(--[^\n]*)/gc) {
            $1 !~ /^--\s*no-transaction\b/i or die "禁止 -- no-transaction\n";
            next;
        }
        if ($sql =~ /\G\/\*/gc) {
            my $depth = 1;
            my $offset = pos($sql);
            while ($offset < length($sql) && $depth > 0) {
                my $pair = substr($sql, $offset, 2);
                if ($pair eq '/*') { $depth++; $offset += 2; }
                elsif ($pair eq '*/') { $depth--; $offset += 2; }
                else { $offset++; }
            }
            $depth == 0 or die "块注释未闭合\n";
            pos($sql) = $offset;
            next;
        }
        if ($sql =~ /\G(E'(?:''|\\.|[^'\\])*'|'(?:''|[^'])*')/gci) {
            push @tokens, { kind => 'string', text => $1 };
            next;
        }
        if ($sql =~ /\G("(?:""|[^"])*")/gc) {
            push @tokens, { kind => 'quoted', text => $1 };
            next;
        }
        if ($sql =~ /\G(\$(?:[A-Za-z_][A-Za-z_0-9]*)?\$)/gc) {
            my $tag = $1;
            my $start = pos($sql);
            my $end = index($sql, $tag, $start);
            $end >= 0 or die "dollar quote 未闭合\n";
            push @tokens, { kind => 'dollar', text => substr($sql, $start, $end - $start) };
            pos($sql) = $end + length($tag);
            next;
        }
        if ($sql =~ /\G([A-Za-z_][A-Za-z_0-9\$]*|[0-9]+)/gc) {
            push @tokens, { kind => 'word', text => $1 };
            next;
        }
        if ($sql =~ /\G;/gc) {
            push @statements, { tokens => [@tokens], terminated => 1 } if @tokens;
            @tokens = ();
            next;
        }
        if ($sql =~ /\G(.)/gcs) {
            $1 ne "'" && $1 ne '"' or die "字符串或标识符未闭合\n";
            push @tokens, { kind => 'symbol', text => $1 };
        }
    }
    push @statements, { tokens => [@tokens], terminated => 0 } if @tokens;
    return \@statements;
}

sub identifier {
    my ($token) = @_;
    return '' unless defined $token;
    my $text = $token->{text};
    if ($token->{kind} eq 'quoted') {
        $text =~ s/^"|"$//g;
        $text =~ s/""/"/g;
        return $text;
    }
    return $token->{kind} eq 'word' ? lc($text) : '';
}

sub registrations {
    my ($statements, $nested) = @_;
    my @found;
    for my $index (0 .. $#$statements) {
        my $statement = $statements->[$index];
        my $tokens = $statement->{tokens};
        my $head = join ' ', map { identifier($_) } @$tokens;
        my $procedural = $head =~ /^(?:do\b|create (?:or replace )?(?:function|procedure)\b)/;
        my $dynamic = grep { identifier($_) eq 'execute' } @$tokens;
        for my $offset (0 .. $#$tokens) {
            my $token = $tokens->[$offset];
            if ($token->{kind} eq 'dollar' && $procedural) {
                # 函数/DO 中的登记也计入次数，但不能充当合法的顶层登记。
                push @found, registrations(statements($token->{text}), 1);
            }
            if ($dynamic && $token->{kind} =~ /^(?:string|dollar)$/ && $token->{text} =~ /\b$table\b/i) {
                die "禁止在字符串中引用兼容登记表（动态登记不可审阅）\n";
            }
            next unless identifier($token) eq 'insert'
                && identifier($tokens->[$offset + 1]) eq 'into';
            my $target = $offset + 2;
            if (defined $tokens->[$target + 1] && $tokens->[$target + 1]->{text} eq '.') {
                $target += 2;
            }
            if (identifier($tokens->[$target]) eq $table) {
                push @found, { statement => $statement, index => $index, nested => $nested };
            }
        }
    }
    return @found;
}

sub check_file {
    my ($file, $version) = @_;
    open my $handle, '<', $file or die "无法读取迁移文件：$!\n";
    local $/;
    my $sql = <$handle>;
    close $handle or die "无法关闭迁移文件：$!\n";
    my $statements = statements($sql);
    for my $statement (@$statements) {
        my $tokens = $statement->{tokens};
        my $first = identifier($tokens->[0]);
        if ($first =~ /^(?:begin|commit|rollback|abort|end)$/
            || ($first =~ /^(?:start|prepare|set)$/ && identifier($tokens->[1]) eq 'transaction')) {
            die "禁止顶层事务控制；迁移与登记必须保留 SQLx 事务边界\n";
        }
    }
    my @registrations = registrations($statements, 0);
    @registrations == 1 or die "必须恰好登记一次，实际 " . scalar(@registrations) . " 次\n";
    my $registration = $registrations[0];
    !$registration->{nested} && $registration->{index} == $#$statements
        or die "登记必须是最后一条顶层 SQL\n";
    $registration->{statement}->{terminated} or die "登记必须以分号结束\n";
    my $normalized = join ' ', map { $_->{text} } @{$registration->{statement}->{tokens}};
    $normalized =~ /\A(?i:INSERT\s+INTO\s+public\s*\.\s*schema_migration_compatibility\s*\(\s*version\s*,\s*level\s*\)\s+VALUES)\s*\(\s*([0-9]+)\s*,\s*'(compatible|breaking)'\s*\)\z/
        or die "登记只接受 INSERT INTO public.$table (version, level) VALUES (版本, 'compatible' 或 'breaking')\n";
    $1 == $version or die "登记版本 $1 与文件版本 $version 不一致\n";
}

for my $file (@ARGV) {
    my $name = basename($file);
    if ($name !~ /^([0-9]{6})_[a-z][a-z0-9_]*\.up\.sql$/) {
        print STDERR "$file: 迁移文件名不符合 NNNNNN_<snake_case>.up.sql\n";
        $failures++;
        next;
    }
    my $version = 0 + $1;
    if ($versions{$version}++) {
        print STDERR "$file: 迁移版本重复\n";
        $failures++;
    }
    next if $version <= 74;
    eval { check_file($file, $version); 1 } or do {
        print STDERR "$file: $@";
        $failures++;
    };
}
my $expected = 75;
for my $version (sort { $a <=> $b } grep { $_ > 74 } keys %versions) {
    if ($version != $expected) {
        print STDERR "迁移编号不连续：期望 $expected，实际 $version\n";
        $failures++;
    }
    $expected = $version + 1;
}
exit 1 if $failures;
my $new_count = grep { $_ > 74 } keys %versions;
print "迁移兼容登记检查通过（$new_count 个新迁移，001–074 豁免）\n";
exit 0;
