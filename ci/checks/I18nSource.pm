package I18nSource;

# 三项源码检查共用这套 TypeScript 解析口径和 Rust 内联测试模块扫描。

use strict;
use warnings;
use utf8;

use Exporter 'import';

our @EXPORT_OK = qw(parse_typescript strip_rust_inline_tests);

# 正则在原字符串上递增读取字符坐标。另存字节坐标，供之后的随机区间取片，
# 避免用 UTF-8 字符下标定位时重新走过前缀。
sub source_slice {
    my ($source, $from, $to) = @_;
    my $slice = do { use bytes; substr($$source, $from, $to - $from) };
    if (utf8::is_utf8($$source)) {
        utf8::decode($slice);
        utf8::upgrade($slice);
    }
    return $slice;
}

# 每个标记只填入与其种类相关的边界。
use constant {
    SYMBOL => 0,
    BYTE => 1,
    OFFSET => 2,
    LINE => 3,
    ESCAPED => 4,
    QUOTE_END => 5,
    LINE_QUOTE_END => 6,
    TEMPLATE_END => 7,
    TEMPLATE_PART => 8,
    EXPRESSION_END => 9,
    NEXT_PART => 10,
    LINE_COMMENT_AFTER => 11,
    BLOCK_COMMENT => 12,
    BLOCK_COMMENT_AFTER => 13,
    CALL_END => 14,
    CALL_BYTE => 15,
    CALL_OFFSET => 16,
    CALL_LINE => 17,
    CALL_ERROR => 18,
    CHECKED_END => 19,
};

# 没有模板时，去注释阶段只需要引号、注释与换行，找调用阶段另加圆括号。
my $basic_marks = q~"'/*\r\n\x{2028}\x{2029}~;
my @mark_patterns = map { qr/\G[^$_\\]*+(\\++|[$_])/ } (
    $basic_marks, $basic_marks . '`{}', $basic_marks . '()', $basic_marks . '()`{}', $basic_marks . '()`{}\[\]',
);

sub normalize_ascii {
    my ($source) = @_;
    utf8::downgrade($$source) if utf8::is_utf8($$source) && length($$source) == do { use bytes; length($$source) };
}

sub typescript_marks {
    my ($source, $for_calls, $with_brackets) = @_;
    my $has_templates = index($source, "`") >= 0;
    my $pattern = $mark_patterns[$with_brackets ? 4 : 2 * $for_calls + $has_templates];
    my (@marks, %at);
    my ($line, $escape_at) = (1, -1);
    while ($source =~ /$pattern/gc) {
        my $symbol = $1;
        my $after = do { use bytes; pos($source) };
        if (substr($symbol, 0, 1) eq '\\') {
            $escape_at = $after if length($symbol) % 2;
            next;
        }
        my $byte = $after - do { use bytes; length($symbol) };
        my $mark = [$symbol, $byte, undef, undef, $byte == $escape_at];
        if ($for_calls) {
            $mark->[OFFSET] = pos($source) - 1;
            $mark->[LINE] = $line;
            $at{$byte} = scalar @marks if $symbol eq '(';
            $line++ if $symbol eq "\n";
        }
        if ($has_templates && $symbol eq '{' && $byte > 0 && $byte - 1 != $escape_at
                && do { use bytes; substr($source, $byte - 1, 1) } eq '$') {
            $mark->[TEMPLATE_PART] = scalar @marks;
        }
        push @marks, $mark;
    }
    push @marks, ['', do { use bytes; length($source) }, length($source), $line];
    mark_typescript_boundaries(\@marks, $has_templates, $for_calls);
    return (\@marks, \%at);
}

# 从右向左建立跳转表。引号既有可跨行的结束位置，也有普通扫描用的同一行结束位置。
# 模板正文只关心反引号和插值；插值内部认注释、引号、模板和花括号。
# 调用的外层括号沿用直接数圆括号的口径，因而与模板插值使用不同的跳转表。
sub mark_typescript_boundaries {
    my ($marks, $has_templates, $for_calls) = @_;
    my (%quote_end, %line_quote_end, @paren, @brace, @template, @template_part);
    my $limit = $#$marks;
    my $line_break = $limit;
    my ($block_after, $later_block_after);
    for (my $index = $limit - 1; $index >= 0; $index--) {
        my $mark = $marks->[$index];
        my $symbol = $mark->[SYMBOL];
        my $next = $index + 1;
        my $adjacent = '';
        if ($symbol eq '/' || $symbol eq '*') {
            $adjacent = $marks->[$next][SYMBOL] if $marks->[$next][BYTE] == $mark->[BYTE] + 1;
        }
        my ($code_next, $expression_next) = ($next, $next);

        if ($has_templates) {
            $template[$index] = $template[$next];
            $template_part[$index] = $template_part[$next];
        }
        if ($symbol eq '"' || $symbol eq "'") {
            $mark->[QUOTE_END] = $quote_end{$symbol};
            $mark->[LINE_QUOTE_END] = $line_quote_end{$symbol};
            unless ($mark->[ESCAPED]) {
                $quote_end{$symbol} = $index;
                $line_quote_end{$symbol} = $index;
            }
            $code_next = $expression_next = defined $mark->[QUOTE_END] ? $mark->[QUOTE_END] + 1 : undef;
        } elsif ($symbol eq "`") {
            $mark->[TEMPLATE_END] = $template[$next];
            $mark->[TEMPLATE_PART] = $template_part[$next];
            unless ($mark->[ESCAPED]) {
                $template[$index] = $index;
                $template_part[$index] = $index;
            }
            $code_next = $expression_next = defined $mark->[TEMPLATE_END] ? $mark->[TEMPLATE_END] + 1 : undef;
        } elsif ($symbol eq '{') {
            my $end = $brace[$next];
            $expression_next = defined $end ? $end + 1 : undef;
            if (defined $mark->[TEMPLATE_PART]) {
                $mark->[EXPRESSION_END] = $end;
                $mark->[NEXT_PART] = defined $end ? $template_part[$end + 1] : undef;
                $template[$index] = defined $end ? $template[$end + 1] : undef;
                $template_part[$index] = $index;
            }
        } elsif ($symbol eq '/' && $adjacent eq '/') {
            $mark->[LINE_COMMENT_AFTER] = $expression_next = $line_break;
        } elsif ($symbol eq '/' && $adjacent eq '*') {
            # /*/ 的星号已经属于开头，闭合符从它之后找。
            my $after = defined $block_after && $block_after == $index + 3 ? $later_block_after : $block_after;
            $mark->[BLOCK_COMMENT] = 1;
            $mark->[BLOCK_COMMENT_AFTER] = $expression_next = $after;
        } elsif ($symbol eq '*' && $adjacent eq '/') {
            ($block_after, $later_block_after) = ($index + 2, $block_after);
        } elsif ($symbol eq "\n" || $symbol eq "\r" || $symbol eq "\x{2028}" || $symbol eq "\x{2029}") {
            $line_break = $index if $symbol eq "\r" || $symbol eq "\n";
            %line_quote_end = () unless $mark->[ESCAPED];
        }

        if ($has_templates) {
            $brace[$index] = $symbol eq '}' ? $index : defined $expression_next ? $brace[$expression_next] : undef;
        }
        next unless $for_calls;
        if ($symbol eq '(') {
            my $end = $paren[$next];
            $mark->[CALL_END] = $end;
            $paren[$index] = defined $end ? $paren[$end + 1] : undef;
        } else {
            $paren[$index] = $symbol eq ')' ? $index : defined $code_next ? $paren[$code_next] : undef;
        }
    }
}

# 普通扫描在 JSX 文本等位置容忍孤立引号。进入有效模板的插值，正文直接跳过。
# 去注释按插值的局部范围处理。
sub find_typescript_comments {
    my ($marks) = @_;
    my @ranges = ([0, $#$marks]);
    my @comments;
    while (my $range = pop @ranges) {
        my ($index, $limit) = @$range;
        while ($index < $limit) {
            my $mark = $marks->[$index];
            my $end = $mark->[LINE_QUOTE_END];
            if (defined $end && $end < $limit) {
                $index = $end + 1;
                next;
            }
            $end = $mark->[TEMPLATE_END];
            if (defined $end && $end < $limit) {
                my @expressions;
                for (my $part = $mark->[TEMPLATE_PART]; $part != $end; $part = $marks->[$part][NEXT_PART]) {
                    push @expressions, [$part + 1, $marks->[$part][EXPRESSION_END]];
                }
                push @ranges, [$end + 1, $limit] if $end + 1 < $limit;
                push @ranges, reverse @expressions;
                last;
            }
            if (defined $mark->[LINE_COMMENT_AFTER]) {
                my $after = $mark->[LINE_COMMENT_AFTER];
                $after = $limit if $after > $limit;
                push @comments, [$mark->[BYTE], $marks->[$after][BYTE]];
                $index = $after;
                next;
            }
            if ($mark->[BLOCK_COMMENT]) {
                my $after = $mark->[BLOCK_COMMENT_AFTER];
                if (defined $after && $after <= $limit) {
                    push @comments, [$mark->[BYTE], $marks->[$after - 1][BYTE] + 1];
                } else {
                    $after = $limit;
                    push @comments, [$mark->[BYTE], $marks->[$after][BYTE]];
                }
                $index = $after;
                next;
            }
            $index++;
        }
    }
    return @comments;
}

# 向前跳过空区间，并共用嵌套模板里的调用序列。
sub find_typescript_calls {
    my ($marks) = @_;
    my $limit = $#$marks;
    my @events = ($limit) x ($limit + 1);
    my (@following, @emit, @parts_with_calls);
    for (my $index = $limit - 1; $index >= 0; $index--) {
        my $mark = $marks->[$index];
        my $next = $events[$index + 1];
        if (defined $mark->[LINE_QUOTE_END]) {
            $next = $events[$mark->[LINE_QUOTE_END] + 1];
        } elsif (defined $mark->[TEMPLATE_END]) {
            $next = $events[$mark->[TEMPLATE_END] + 1];
            my $part = $parts_with_calls[$mark->[TEMPLATE_PART]];
            if (defined $part) {
                my $first = $events[$part + 1];
                my $later = $marks->[$part][NEXT_PART];
                # 容错时插值里的引号可能把扫描带到原插值之外。共用只有一个输出分支的
                # 节点，让重复进入同一段的代价取决于实际返回的调用数。
                $emit[$index] = !defined $parts_with_calls[$later] && $following[$first] >= $marks->[$part][EXPRESSION_END]
                    ? $emit[$first] : [$part];
                $following[$index] = $next;
                $next = $index;
            }
        } elsif (defined $mark->[CALL_OFFSET]) {
            $emit[$index] = $index;
            $following[$index] = $next;
            $next = $index;
        }
        $events[$index] = $next;

        my $expression_end = $mark->[EXPRESSION_END];
        if (defined $expression_end) {
            my $later = $mark->[NEXT_PART];
            $parts_with_calls[$index] = $events[$index + 1] < $expression_end ? $index
                : defined $later ? $parts_with_calls[$later] : undef;
        }
    }

    my @calls;
    my @ranges = ([$events[0], $limit]);
    while (my $range = pop @ranges) {
        my ($index, $end) = @$range;
        while ($index < $end) {
            my $emission = $emit[$index];
            if (!ref $emission) {
                push @calls, $emission;
                $index = $following[$index];
                next;
            }
            my @expressions;
            for (my $part = $emission->[0]; defined $part;) {
                push @expressions, [$events[$part + 1], $marks->[$part][EXPRESSION_END]];
                my $later = $marks->[$part][NEXT_PART];
                $part = defined $later ? $parts_with_calls[$later] : undef;
            }
            push @ranges, [$following[$index], $end] if $following[$index] < $end;
            push @ranges, reverse @expressions;
            last;
        }
    }
    return @calls;
}

sub blank_source_ranges {
    my ($source, $ranges) = @_;
    my @pieces;
    my $copied = 0;
    for my $range (@$ranges) {
        my ($start, $end) = @$range;
        push @pieces, source_slice($source, $copied, $start);
        my $removed = source_slice($source, $start, $end);
        $removed =~ s/[^\n]/ /g;
        push @pieces, $removed;
        $copied = $end;
    }
    push @pieces, source_slice($source, $copied, do { use bytes; length($$source) });
    return join('', @pieces);
}

# 以标识符为单位跳过其它名字。旧调用搜索从字母、下划线或美元符开始，
# 因此紧跟数字的 t 也可能是调用；实参分词中的数字有自己的边界。
sub mark_typescript_calls {
    my ($source, $marks, $at) = @_;
    while ($source =~ /(?<![A-Za-z0-9_\$])[0-9]*+t(\s*+)\(/g) {
        my $gap = $1;
        my $open = do { use bytes; pos($source) } - 1;
        my $mark = $marks->[$at->{$open}];
        $mark->[CALL_BYTE] = $open - 1 - do { use bytes; length($gap) };
        $mark->[CALL_OFFSET] = pos($source) - 2 - length($gap);
        $mark->[CALL_LINE] = $mark->[LINE] - ($gap =~ tr/\n//);
    }
}

sub decoded_string {
    my ($source, $from, $to) = @_;
    return '' if $from == $to;
    my $value = source_slice($source, $from, $to);
    $value =~ s/\\(.)/$1/gs;
    return $value;
}

sub template_value {
    my ($source, $marks, $index) = @_;
    my $mark = $marks->[$index];
    my $end = $mark->[TEMPLATE_END];
    my $part = $mark->[TEMPLATE_PART];
    my $interpolated = $part != $end ? 1 : 0;
    my $from = $mark->[BYTE] + 1;
    my $value = '';
    while (1) {
        $value .= decoded_string($source, $from, $marks->[$part][BYTE] - ($part != $end));
        last if $part == $end;
        $from = $marks->[$marks->[$part][EXPRESSION_END]][BYTE] + 1;
        $part = $marks->[$part][NEXT_PART];
    }
    return ($value, $interpolated);
}

sub typescript_token_after {
    my ($mark, $index, $limit) = @_;
    if (defined $mark->[LINE_COMMENT_AFTER]) {
        my $after = $mark->[LINE_COMMENT_AFTER];
        return $after < $limit ? $after + 1 : $limit;
    }
    return $mark->[BLOCK_COMMENT_AFTER] if $mark->[BLOCK_COMMENT];
    my $symbol = $mark->[SYMBOL];
    if ($symbol eq '"' || $symbol eq "'" || $symbol eq "`") {
        my $end = $symbol eq "`" ? $mark->[TEMPLATE_END] : $mark->[QUOTE_END];
        return defined $end ? $end + 1 : undef;
    }
    return $index + 1;
}

# 标记之间的源码只分词一次。生成调用结果时复制 token 并换算为各自的相对偏移。
sub typescript_token_chunk {
    my ($source, $marks, $index) = @_;
    my $mark = $marks->[$index];
    my $previous = $marks->[$index - 1];
    my $from = $previous->[BYTE] + do { use bytes; length($previous->[SYMBOL]) };
    my $offset = $previous->[OFFSET] + 1;
    my $gap = source_slice($source, $from, $mark->[BYTE]);
    my @tokens;
    while ($gap =~ /\G(?:\s++|([A-Za-z_\$][A-Za-z0-9_\$]*+)|([0-9][0-9A-Za-z_.]*+)|(\.\.\.|(?s:.)))/gc) {
        my ($identifier, $number, $punct) = ($1, $2, $3);
        next unless defined($identifier) || defined($number) || defined($punct);
        my ($type, $value) = defined $identifier ? ('identifier', $identifier)
            : defined $number ? ('number', $number) : ('punct', $punct);
        my $end = $offset + pos($gap);
        push @tokens, { type => $type, value => $value, start => $end - length($value), end => $end };
    }

    my $symbol = $mark->[SYMBOL];
    my $after = typescript_token_after($mark, $index, $#$marks);
    return [$after, \@tokens] if !defined $after || defined $mark->[LINE_COMMENT_AFTER] || $mark->[BLOCK_COMMENT];
    if ($symbol eq '"' || $symbol eq "'" || $symbol eq "`") {
        my $closing = $after - 1;
        my $token = { start => $mark->[OFFSET], end => $marks->[$closing][OFFSET] + 1, line => $mark->[LINE] };
        if ($symbol eq "`") {
            @$token{qw(value interpolated)} = template_value($source, $marks, $index);
            $token->{type} = 'template';
        } else {
            $token->{type} = 'string';
            $token->{value} = decoded_string($source, $mark->[BYTE] + 1, $marks->[$closing][BYTE]);
        }
        push @tokens, $token;
    } elsif ($symbol !~ /\s/) {
        push @tokens, { type => 'punct', value => $symbol, start => $mark->[OFFSET], end => $mark->[OFFSET] + 1 };
    }
    return [$after, \@tokens];
}

sub tokenize_typescript_call {
    my ($source, $marks, $open, $chunks, $skip_empty, $checked_end) = @_;
    my $mark = $marks->[$open];
    my $offset = $mark->[CALL_OFFSET];
    my $end = $mark->[CALL_END];
    my $limit = $checked_end // $end;
    my $wide = utf8::is_utf8($$source)
        && $marks->[$end][BYTE] - $mark->[CALL_BYTE] != $marks->[$end][OFFSET] - $offset;

    my (@tokens, @empty, %literal_lines);
    for (my $index = $open + 1; $index <= $limit;) {
        if (defined $checked_end && defined $skip_empty->[$index]) {
            $index = $skip_empty->[$index];
            next;
        }
        my ($after, $chunk) = @{ $chunks->[$index] //= typescript_token_chunk($source, $marks, $index) };
        return unless defined $after && ($after <= $end + 1 || defined $marks->[$index][LINE_COMMENT_AFTER]);
        if (@$chunk) {
            $skip_empty->[$_] = $index for @empty;
            @empty = ();
        } elsif (defined $checked_end) {
            push @empty, $index;
        }
        for my $absolute (@$chunk) {
            my %token = %$absolute;
            $token{start} -= $offset;
            $token{end} -= $offset;
            my $line = delete $token{line};
            $literal_lines{$token{start}} = $line if defined $line;
            # 原实参分词以整个调用为字符串。ASCII 调用的 token 是字节串，
            # 含多字节字符的调用保持 UTF-8 标记，固定标点 ... 与空字面量除外。
            if (length($token{value}) && ($token{type} ne 'punct' || $token{value} ne '...')) {
                if ($wide) {
                    utf8::upgrade($token{value});
                } else {
                    utf8::downgrade($token{value});
                }
            } elsif ($token{type} eq 'punct') {
                $token{value} = '...';
            }
            push @tokens, \%token;
        }
        $index = $after;
    }
    return (\@tokens, \%literal_lines);
}

# 在词法跳转上独立配对各类括号。原实参检查分别计数，不要求不同类型交叉时满足栈顺序。
sub delimiter_jumps {
    my ($marks, $lexical, $open, $close) = @_;
    my $limit = $#$marks;
    my (@matched, @jumps);
    for (my $index = $limit - 1; $index >= 0; $index--) {
        my $symbol = $marks->[$index][SYMBOL];
        if ($symbol eq $close) {
            $matched[$index] = $index;
            $jumps[$index] = $limit;
        } else {
            my $next = $lexical->[$index];
            if ($symbol eq $open) {
                $next = defined $matched[$index + 1] ? $matched[$index + 1] + 1 : $limit;
            }
            $matched[$index] = $matched[$next];
            $jumps[$index] = $next;
        }
    }
    return (\@matched, \@jumps);
}

# 查询按终点递增，已走过的跳转可以合并。保留最后一步的起点，
# 因为只有行注释允许吞掉调用切片末尾的右括号而不产生词法错误。
sub follow_jumps {
    my ($jumps, $previous, $from, $to) = @_;
    my ($index, $last) = ($from, undef);
    while ($index < $to) {
        $last = $previous->[$index] // $index;
        $index = $jumps->[$index];
    }
    while ($from < $to) {
        my $next = $jumps->[$from];
        $jumps->[$from] = $index;
        $previous->[$from] = $last;
        $from = $next;
    }
    return ($index, $last);
}

# 高度重叠的调用先验证区间，避免对每个有错误的外层调用重新遍历整份实参。
# 通过验证的调用只遍历需要写入返回值的 token，普通短调用直接使用同一份分词结果。
sub check_indexed_calls {
    my ($marks, $calls) = @_;
    my $limit = $#$marks;
    my @lexical = map { typescript_token_after($marks->[$_], $_, $limit) // $limit } 0 .. $limit - 1;
    my ($paren) = delimiter_jumps($marks, \@lexical, '(', ')');
    my (undef, $braces) = delimiter_jumps($marks, \@lexical, '{', '}');
    my (undef, $brackets) = delimiter_jumps($marks, \@lexical, '[', ']');
    my (@by_end, @by_split, @seen, @lexical_previous, @brace_previous, @bracket_previous);
    for my $index (@$calls) {
        next if $seen[$index]++;
        my $end = $marks->[$index][CALL_END];
        push @{ $by_end[$end] }, $index if defined $end;
    }
    for my $end (0 .. $#by_end) {
        for my $index (@{ $by_end[$end] // [] }) {
            my $mark = $marks->[$index];
            my ($reached, $last) = follow_jumps(\@lexical, \@lexical_previous, $index + 1, $end);
            my $close = $paren->[$index + 1];
            if ($reached != $end && !defined $marks->[$last][LINE_COMMENT_AFTER]) {
                $mark->[CALL_ERROR] = '调用内有无法识别的字符串或模板';
            } elsif (!defined $close || $close > $end) {
                $mark->[CALL_ERROR] = '调用括号未闭合';
            } else {
                push @{ $by_split[$close] }, $index;
            }
        }
    }
    for my $end (0 .. $#by_split) {
        for my $index (@{ $by_split[$end] // [] }) {
            my ($brace) = follow_jumps($braces, \@brace_previous, $index + 1, $end);
            my ($bracket) = follow_jumps($brackets, \@bracket_previous, $index + 1, $end);
            if ($brace == $end && $bracket == $end) {
                $marks->[$index][CHECKED_END] = $end;
            } else {
                $marks->[$index][CALL_ERROR] = '调用括号未闭合';
            }
        }
    }
}

my %opening_delimiter = (')' => '(', '}' => '{', ']' => '[');

# 同类型括号独立计数。调用允许在第一个合法右圆括号结束，对象要求右花括号恰好在末尾。
sub split_typescript_fields {
    my ($tokens, $from, $closing) = @_;
    my %depth = ('(' => 0, '{' => 0, '[' => 0);
    my $nested = 0;
    my @parts = ([]);
    for my $index ($from .. $#$tokens) {
        my $token = $tokens->[$index];
        if ($token->{type} eq 'punct') {
            my $value = $token->{value};
            if ($value eq $closing && $nested == 0) {
                # 有尾逗号时不把末尾当成额外的空实参或属性。
                pop @parts if @parts > 1 && !@{ $parts[-1] };
                return (\@parts, $index);
            }
            if (exists $depth{$value}) {
                $depth{$value}++;
                $nested++;
            } elsif (exists $opening_delimiter{$value}) {
                return if --$depth{$opening_delimiter{$value}} < 0;
                $nested--;
            } elsif ($value eq ',' && $nested == 0) {
                push @parts, [];
                next;
            }
        }
        push @{ $parts[-1] }, $token;
    }
    return;
}

sub object_property_names {
    my ($tokens) = @_;
    return { error => '第二实参不是对象字面量' } unless @$tokens >= 2;
    return { error => '第二实参不是对象字面量' } unless $tokens->[0]{type} eq 'punct' && $tokens->[0]{value} eq '{';
    return { error => '第二实参不是对象字面量' } unless $tokens->[-1]{type} eq 'punct' && $tokens->[-1]{value} eq '}';

    my ($parts, $end) = split_typescript_fields($tokens, 1, '}');
    return { error => '对象字面量括号不平衡' } unless defined $end && $end == $#$tokens;

    my %names;
    for (my $index = 0; $index < @$parts; $index++) {
        my $part = $parts->[$index];
        if (@$part == 0) {
            return { error => '对象字面量里有空属性' } unless $index == $#$parts;
            next;
        }
        my $first = $part->[0];
        return { error => '对象里有展开属性' } if $first->{type} eq 'punct' && $first->{value} eq '...';
        return { error => '对象里有计算属性名' } if $first->{type} eq 'punct' && $first->{value} eq '[';
        if (@$part == 1 && $first->{type} eq 'identifier') {
            $names{$first->{value}} = 1;
            next;
        }
        my $key;
        if ($first->{type} eq 'identifier' || $first->{type} eq 'string') {
            $key = $first->{value};
        } else {
            return { error => '对象里只允许普通属性与简写属性' };
        }
        return { error => '对象里只允许普通属性与简写属性' } unless @$part >= 3 && $part->[1]{type} eq 'punct' && $part->[1]{value} eq ':';
        $names{$key} = 1;
    }
    return { names => \%names };
}

sub invalid_call {
    my ($line, $reason) = @_;
    return { line => $line, syntax => 't', invalid => $reason, key => undef, dynamic => 0 };
}

sub parse_call_tokens {
    my ($tokens, $call_start, $line) = @_;
    if (!defined $tokens) {
        return invalid_call($line, '调用内有无法识别的字符串或模板');
    }
    my ($arguments) = split_typescript_fields($tokens, 0, ')');
    if (!defined $arguments) {
        return invalid_call($line, '调用括号未闭合');
    }
    my $first = $arguments->[0] // [];
    my $count = scalar @$arguments;
    my $call = { line => $line, syntax => 't', arguments => $arguments, key => undef, dynamic => 0, provided => undef, offset => $call_start };
    if (@$first == 0) {
        $call->{invalid} = '第一实参为空';
        return $call;
    }
    if (@$first == 1 && ($first->[0]{type} eq 'string' || ($first->[0]{type} eq 'template' && !$first->[0]{interpolated}))) {
        $call->{key} = $first->[0]{value};
        if ($count == 1) {
            $call->{provided} = {};
        } elsif ($count > 2) {
            $call->{invalid} = '还有第三实参';
        } else {
            my $second = $arguments->[1] // [];
            if (@$second == 0) {
                $call->{invalid} = '第二实参为空';
            } else {
                my $properties = object_property_names($second);
                if ($properties->{error}) {
                    $call->{invalid} = $properties->{error};
                } else {
                    $call->{provided} = $properties->{names};
                }
            }
        }
    } else {
        $call->{dynamic} = 1;
        my $has_concat = grep { $_->{type} eq 'punct' && $_->{value} eq '+' } @$first;
        $call->{dynamic_literals} = [$first] unless $has_concat;
    }
    return $call;
}

sub typescript_literals {
    my ($source, $marks, $covered) = @_;
    my @literals;
    my $skip_until = 0;
    my $covered_until = -1;
    for my $index (0 .. $#$marks - 1) {
        my $end = $covered->[$index];
        $covered_until = $end if defined $end && $end > $covered_until;
        next if $index < $skip_until;
        my $mark = $marks->[$index];
        if (defined $mark->[LINE_QUOTE_END]) {
            $end = $mark->[LINE_QUOTE_END];
            if ($index > $covered_until) {
                my $value = source_slice($source, $mark->[BYTE] + 1, $marks->[$end][BYTE]);
                push @literals, { value => $value, line => $mark->[LINE] };
            }
            $skip_until = $end + 1;
        } elsif (defined $mark->[TEMPLATE_END]) {
            $end = $mark->[TEMPLATE_END];
            if ($index > $covered_until && $mark->[TEMPLATE_PART] == $end) {
                my ($value) = template_value($source, $marks, $index);
                push @literals, { value => $value, line => $mark->[LINE] };
            }
            $skip_until = $end + 1;
        }
    }
    return @literals;
}

sub parse_typescript {
    my ($source) = @_;
    return parse_empty_typescript($source) unless defined $source;
    normalize_ascii(\$source);
    if ($source =~ m{/[/*]}) {
        my ($original) = typescript_marks($source, 0);
        my @comments = find_typescript_comments($original);
        if (@comments) {
            $source = blank_source_ranges(\$source, \@comments);
            normalize_ascii(\$source);
        }
    }
    my ($marks, $at) = typescript_marks($source, 1);
    mark_typescript_calls($source, $marks, $at);
    my @selected = find_typescript_calls($marks);
    my $span = 0;
    for my $index (@selected) {
        my $mark = $marks->[$index];
        $span += $marks->[$mark->[CALL_END]][BYTE] + 1 - $mark->[CALL_BYTE] if defined $mark->[CALL_END];
    }
    if ($span > $marks->[-1][BYTE]) {
        ($marks, $at) = typescript_marks($source, 1, 1);
        mark_typescript_calls($source, $marks, $at);
        @selected = find_typescript_calls($marks);
        check_indexed_calls($marks, \@selected);
    }

    my (@calls, @covered, @dynamic_literals, @chunks, @skip_empty);
    for my $index (@selected) {
        my $mark = $marks->[$index];
        my $line = $mark->[CALL_LINE];
        my $end = $mark->[CALL_END];
        if (!defined $end) {
            push @calls, invalid_call($line, '调用括号未闭合');
            next;
        }
        $covered[$index] = $end;
        if (defined $mark->[CALL_ERROR]) {
            push @calls, invalid_call($line, $mark->[CALL_ERROR]);
            next;
        }
        my ($tokens, $literal_lines) = tokenize_typescript_call(\$source, $marks, $index, \@chunks, \@skip_empty, $mark->[CHECKED_END]);
        my $call = parse_call_tokens($tokens, $mark->[CALL_OFFSET], $line);
        push @calls, $call;
        next unless $call->{dynamic_literals};
        for my $token (@{ $call->{dynamic_literals}[0] }) {
            next unless $token->{type} eq 'string' || ($token->{type} eq 'template' && !$token->{interpolated});
            push @dynamic_literals, { value => $token->{value}, line => $literal_lines->{$token->{start}} };
        }
    }
    my @literals = typescript_literals(\$source, $marks, \@covered);
    return { calls => \@calls, literals => [@literals, @dynamic_literals] };
}

# Rust 的空白字符集合固定；非 ASCII 标识符字符的合法性由编译器检查。
my $rust_space = qr/[\x09-\x0d\x20\x{85}\x{200e}\x{200f}\x{2028}\x{2029}]/;
my $rust_identifier_char = qr/[A-Za-z0-9_]|(?!$rust_space)[^\x00-\x7f]/;

sub skip_rust_block_comment {
    my ($source) = @_;
    my $depth = 1;
    while ($$source =~ m{\G[^/*]*+(/\*|\*/|[/*])}gc) {
        if ($1 eq '/*') {
            $depth++;
        } elsif ($1 eq '*/') {
            return 1 if --$depth == 0;
        }
    }
    return;
}

sub skip_rust_trivia {
    my ($source) = @_;
    while ($$source =~ m{\G(?:$rust_space++|//[^\n]*+|(/\*))}gc) {
        return if defined $1 && !skip_rust_block_comment($source);
    }
    return 1;
}

# 从左花括号之后扫描。Rust 的块注释可以嵌套，花括号出现在字符串和字符时不计入层级。
sub rust_module_end {
    my ($source) = @_;
    my $depth = 1;
    while ($$source =~ m~\G(?:
            //[^\n]*+ | (/\*) | [bc]?r(\#*+)" | ([bc]?") | (') | ([{}])
            | $rust_identifier_char++ | $rust_space++ | (?s:.)
        )~xgc) {
        my ($comment, $hashes, $quote, $apostrophe, $brace) = ($1, $2, $3, $4, $5);
        if (defined $comment) {
            return unless skip_rust_block_comment($source);
        } elsif (defined $hashes) {
            # 结束引号后的 # 数与开头相同。br"…" 的零个 # 也适用。
            my $terminator = '"' . $hashes;
            my $end = index($$source, $terminator, pos($$source));
            return if $end < 0;
            pos($$source) = $end + length($terminator);
        } elsif (defined $quote) {
            return unless $$source =~ /\G(?:[^"\\]++|\\(?s:.))*+"/gc;
        } elsif (defined $apostrophe) {
            if ($$source =~ /\G\\/gc) {
                return unless $$source =~ /\G(?: ['"\\nrt0] | x[0-9A-Fa-f]{2} | u\{(?:[0-9A-Fa-f]_*+){1,6}\} )'/xgc;
            } else {
                # 没有紧跟的结束引号时，游标留在生命周期或标签的单引号之后。
                $$source =~ /\G(?s:.)'/gc;
            }
        } elsif (defined $brace) {
            $depth += $brace eq '{' ? 1 : -1;
            return 1 if $depth == 0;
        }
    }
    return;
}

sub strip_rust_inline_tests {
    my ($source) = @_;
    return strip_empty_rust($source) unless defined $source;
    my $scan = $source;
    my @removed;
    while ($source =~ /^(#\[cfg\(test\)\][ \t]*\r?)$/mg) {
        my $start = do { use bytes; pos($source) } - length($1);
        pos($scan) = pos($source);
        # 未闭合时保留整个候选模块及其后的源码，避免把其中的文字当成新的测试模块。
        last unless skip_rust_trivia(\$scan);
        next unless $scan =~ /\Gmod(?!$rust_identifier_char)/gc;
        last unless skip_rust_trivia(\$scan);
        next unless $scan =~ /\G(?:r#)?(?![0-9])$rust_identifier_char++/gc;
        last unless skip_rust_trivia(\$scan);
        # 外置模块以分号结束，只有左花括号才开始扫描模块体。
        next unless $scan =~ /\G\{/gc;
        last unless rust_module_end(\$scan);
        push @removed, [$start, do { use bytes; pos($scan) }];
        pos($source) = pos($scan);
    }
    return @removed ? blank_source_ranges(\$source, \@removed) : $source;
}

# 空文件的 readline 返回 undef。保留旧检查器的 Perl 诊断及其位置。
sub parse_empty_typescript {
    my ($source) = @_;
#line 17
    $source =~ /\n/g;
#line 364
    while (0 < length $source) { last; }
#line 413
    return parse_typescript(substr($source, 0));
}

sub strip_empty_rust {
    my ($source) = @_;
#line 732
    $source =~ /^#\[cfg\(test\)\][ \t]*$/mg;
    return $source;
}

1;
