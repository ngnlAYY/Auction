package I18nFluent;

# Fluent 平铺子集的共享词法：变量检查与平铺检查从这里取同一份占位解析，
# 避免两边对 `{"$USD"}`、转义与空白给出不同结论。

use strict;
use warnings;
use utf8;

use Exporter 'import';

our @EXPORT_OK = qw(
    body_fits_subset
    message_line
    message_variables
    placeholder_at
    placeholder_failure
    split_lines
);

sub split_lines {
    my ($text) = @_;
    return split /\r\n|\n|\r/, $text, -1;
}

sub message_line {
    my ($line) = @_;
    # TypeScript 加载器按 CRLF/LF/CR 分行；U+2028/U+2029 留在行内，而正则的 `.`
    # 不能跨过它们，因此这类行不会被当作消息行。
    return if $line =~ /[\x{2028}\x{2029}]/;
    return unless $line =~ /^([A-Za-z][A-Za-z0-9_-]*)( *)=( *)(.+)$/;
    return ($1, $2, $3, $4);
}

sub skip_placeholder_whitespace {
    my ($text, $start) = @_;
    my $cursor = $start;
    $cursor++ while $cursor < length $text && (substr($text, $cursor, 1) eq ' ' || substr($text, $cursor, 1) eq "\t");
    return $cursor;
}

sub identifier_start {
    my ($character) = @_;
    return $character =~ /[A-Za-z]/;
}

sub identifier_continue {
    my ($character) = @_;
    return $character =~ /[A-Za-z0-9_-]/;
}

sub string_end {
    my ($text, $open_quote) = @_;
    my $cursor = $open_quote + 1;
    while ($cursor < length $text) {
        my $character = substr($text, $cursor, 1);
        return $cursor + 1 if $character eq '"';
        if ($character ne '\\') {
            $cursor++;
            next;
        }
        my $escaped = substr($text, $cursor + 1, 1);
        if ($escaped eq '\\' || $escaped eq '"') {
            $cursor += 2;
            next;
        }
        if ($escaped eq 'u' && substr($text, $cursor + 2, 4) =~ /\A[0-9A-Fa-f]{4}\z/) {
            $cursor += 6;
            next;
        }
        if ($escaped eq 'U' && substr($text, $cursor + 2, 6) =~ /\A[0-9A-Fa-f]{6}\z/) {
            $cursor += 8;
            next;
        }
        return;
    }
    return;
}

# 成功时返回 { kind => variable|string, name, end }；不合法时返回 { error => 1 }。
sub placeholder_at {
    my ($text, $start) = @_;
    my $cursor = skip_placeholder_whitespace($text, $start + 1);
    my $first = substr($text, $cursor, 1);
    my $kind;
    my $name;
    if ($first eq '$') {
        $cursor++;
        return { error => 1 } unless identifier_start(substr($text, $cursor, 1));
        my $name_start = $cursor;
        $cursor++;
        $cursor++ while identifier_continue(substr($text, $cursor, 1));
        $name = substr($text, $name_start, $cursor - $name_start);
        $kind = 'variable';
    } elsif ($first eq '"') {
        my $end = string_end($text, $cursor);
        return { error => 1 } unless defined $end;
        $cursor = $end;
        $kind = 'string';
    } else {
        return { error => 1 };
    }
    $cursor = skip_placeholder_whitespace($text, $cursor);
    return { error => 1 } unless substr($text, $cursor, 1) eq '}';
    return { kind => $kind, name => $name, end => $cursor + 1 };
}

sub placeholder_failure {
    my ($text, $start) = @_;
    my $rest = substr($text, $start);
    return '选择表达式（->）' if $rest =~ /\A\{[^{}]*->/;
    return '术语引用（{ -term }）' if $rest =~ /\A\{\s*-/;
    return '函数调用（FUNC(...)）' if $rest =~ /\A\{\s*[A-Za-z][A-Za-z0-9_-]*\s*\(/;
    return '消息引用（{ other-key }）' if $rest =~ /\A\{\s*[A-Za-z][A-Za-z0-9_-]*\s*\}/;
    return '子集之外的占位符（只允许 { $变量 } 与 {"字符串"}）';
}

sub body_fits_subset {
    my ($body) = @_;
    my $index = 0;
    while ($index < length $body) {
        my $character = substr($body, $index, 1);
        if ($character eq '}') {
            return { error => '子集之外的裸右花括号' };
        }
        if ($character ne '{') {
            $index++;
            next;
        }
        my $placeholder = placeholder_at($body, $index);
        return { error => placeholder_failure($body, $index) } if $placeholder->{error};
        $index = $placeholder->{end};
    }
    return { ok => 1 };
}

sub message_variables {
    my ($body) = @_;
    my %variables;
    my $index = 0;
    while ($index < length $body) {
        if (substr($body, $index, 1) eq '{') {
            my $placeholder = placeholder_at($body, $index);
            if (!$placeholder->{error}) {
                $variables{$placeholder->{name}} = 1 if $placeholder->{kind} eq 'variable';
                $index = $placeholder->{end};
                next;
            }
        }
        $index++;
    }
    return \%variables;
}

1;
