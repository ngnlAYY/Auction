package I18nSource;

# TypeScript 调用的闭集解析。缺键、未引用与参数三项检查都从 parse_typescript
# 取同一份调用记录，避免三套正则对注释、字符串与对象字面量给出不同结论。

use strict;
use warnings;
use utf8;

use Exporter 'import';

our @EXPORT_OK = qw(parse_typescript strip_rust_inline_tests);

sub line_index {
    my ($source) = @_;
    my @starts = (0);
    while ($source =~ /\n/g) {
        push @starts, pos($source);
    }
    return \@starts;
}

sub line_for_offset {
    my ($starts, $offset) = @_;
    my ($low, $high) = (0, $#$starts);
    while ($low < $high) {
        my $middle = int(($low + $high + 1) / 2);
        if ($starts->[$middle] <= $offset) {
            $low = $middle;
        } else {
            $high = $middle - 1;
        }
    }
    return $low + 1;
}

sub scan_string {
    my ($source, $start) = @_;
    my $quote = substr($source, $start, 1);
    my $cursor = $start + 1;
    my $value = '';
    while ($cursor < length $source) {
        my $character = substr($source, $cursor, 1);
        if ($character eq $quote) {
            return ($cursor + 1, $value);
        }
        if ($character eq '\\') {
            my $escaped = substr($source, $cursor + 1, 1);
            return (undef, undef) if $escaped eq '';
            $value .= $escaped;
            $cursor += 2;
            next;
        }
        $value .= $character;
        $cursor++;
    }
    return (undef, undef);
}

sub scan_template {
    my ($source, $start) = @_;
    my $cursor = $start + 1;
    my $value = '';
    my $interpolated = 0;
    while ($cursor < length $source) {
        my $character = substr($source, $cursor, 1);
        if ($character eq '\\') {
            my $escaped = substr($source, $cursor + 1, 1);
            return (undef, undef, undef) if $escaped eq '';
            $value .= $escaped;
            $cursor += 2;
            next;
        }
        if ($character eq '`') {
            return ($cursor + 1, $value, $interpolated);
        }
        if ($character eq '$' && substr($source, $cursor + 1, 1) eq '{') {
            $interpolated = 1;
            my $end = scan_braced_expression($source, $cursor + 2);
            return (undef, undef, undef) unless defined $end;
            $cursor = $end;
            next;
        }
        $value .= $character;
        $cursor++;
    }
    return (undef, undef, undef);
}

# 返回模板字面量里每个 ${...} 表达式的绝对范围（右花括号不包含在内），
# 供调用提取进入插值内部；模板正文仍按字面量跳过。
sub template_expression_ranges {
    my ($source, $start) = @_;
    my $cursor = $start + 1;
    my @ranges;
    while ($cursor < length $source) {
        my $character = substr($source, $cursor, 1);
        if ($character eq '\\') {
            $cursor += 2;
            next;
        }
        if ($character eq '`') {
            return @ranges;
        }
        if ($character eq '$' && substr($source, $cursor + 1, 1) eq '{') {
            my $expression_start = $cursor + 2;
            my $end = scan_braced_expression($source, $expression_start);
            return unless defined $end;
            push @ranges, { start => $expression_start, end => $end - 1 };
            $cursor = $end;
            next;
        }
        $cursor++;
    }
    return;
}

sub scan_braced_expression {
    my ($source, $start) = @_;
    my $cursor = $start;
    my $depth = 1;
    while ($cursor < length $source) {
        my $character = substr($source, $cursor, 1);
        my $next = substr($source, $cursor + 1, 1);
        if ($character eq '/' && $next eq '/') {
            $cursor += 2;
            $cursor++ while $cursor < length $source && substr($source, $cursor, 1) !~ /[\r\n]/;
            next;
        }
        if ($character eq '/' && $next eq '*') {
            $cursor += 2;
            while ($cursor < length $source && substr($source, $cursor, 2) ne '*/') {
                $cursor++;
            }
            return unless $cursor < length $source;
            $cursor += 2;
            next;
        }
        if ($character eq '"' || $character eq "'") {
            my ($end) = scan_string($source, $cursor);
            return unless defined $end;
            $cursor = $end;
            next;
        }
        if ($character eq '`') {
            my ($end) = scan_template($source, $cursor);
            return unless defined $end;
            $cursor = $end;
            next;
        }
        if ($character eq '{') {
            $depth++;
        } elsif ($character eq '}') {
            $depth--;
            return $cursor + 1 if $depth == 0;
        }
        $cursor++;
    }
    return;
}

sub tokenize_typescript {
    my ($source) = @_;
    my @tokens;
    my $cursor = 0;
    while ($cursor < length $source) {
        my $character = substr($source, $cursor, 1);
        if ($character =~ /\s/) {
            $cursor++;
            next;
        }
        if ($character eq '/' && substr($source, $cursor + 1, 1) eq '/') {
            $cursor += 2;
            $cursor++ while $cursor < length $source && substr($source, $cursor, 1) !~ /[\r\n]/;
            next;
        }
        if ($character eq '/' && substr($source, $cursor + 1, 1) eq '*') {
            $cursor += 2;
            while ($cursor < length $source && substr($source, $cursor, 2) ne '*/') {
                $cursor++;
            }
            return unless $cursor < length $source;
            $cursor += 2;
            next;
        }
        if ($character eq '"' || $character eq "'") {
            my ($end, $value) = scan_string($source, $cursor);
            return unless defined $end;
            push @tokens, { type => 'string', value => $value, start => $cursor, end => $end };
            $cursor = $end;
            next;
        }
        if ($character eq '`') {
            my ($end, $value, $interpolated) = scan_template($source, $cursor);
            return unless defined $end;
            push @tokens, {
                type => 'template',
                value => $value,
                interpolated => $interpolated,
                start => $cursor,
                end => $end,
            };
            $cursor = $end;
            next;
        }
        if ($character =~ /[A-Za-z_\$]/) {
            my $start = $cursor;
            $cursor++;
            $cursor++ while $cursor < length $source && substr($source, $cursor, 1) =~ /[A-Za-z0-9_\$]/;
            push @tokens, { type => 'identifier', value => substr($source, $start, $cursor - $start), start => $start, end => $cursor };
            next;
        }
        if ($character =~ /[0-9]/) {
            my $start = $cursor;
            $cursor++;
            $cursor++ while $cursor < length $source && substr($source, $cursor, 1) =~ /[0-9A-Za-z_.]/;
            push @tokens, { type => 'number', value => substr($source, $start, $cursor - $start), start => $start, end => $cursor };
            next;
        }
        if (substr($source, $cursor, 3) eq '...') {
            push @tokens, { type => 'punct', value => '...', start => $cursor, end => $cursor + 3 };
            $cursor += 3;
            next;
        }
        push @tokens, { type => 'punct', value => $character, start => $cursor, end => $cursor + 1 };
        $cursor++;
    }
    return \@tokens;
}

sub split_call_arguments {
    my ($tokens, $open_index) = @_;
    my $paren = 1;
    my $brace = 0;
    my $bracket = 0;
    my @arguments;
    my @current;
    for (my $index = $open_index + 1; $index < @$tokens; $index++) {
        my $token = $tokens->[$index];
        my $value = $token->{value};
        if ($token->{type} eq 'punct' && $value eq ')' && $paren == 1 && $brace == 0 && $bracket == 0) {
            # 调用末尾的尾逗号不是额外的空实参；`t("key",)` 等价于 `t("key")`。
            push @arguments, [@current] if @current > 0 || @arguments == 0;
            return (\@arguments, $index);
        }
        if ($token->{type} eq 'punct') {
            if ($value eq '(') {
                $paren++;
            } elsif ($value eq ')') {
                $paren--;
                return unless $paren > 0;
            } elsif ($value eq '{') {
                $brace++;
            } elsif ($value eq '}') {
                $brace--;
                return unless $brace >= 0;
            } elsif ($value eq '[') {
                $bracket++;
            } elsif ($value eq ']') {
                $bracket--;
                return unless $bracket >= 0;
            } elsif ($value eq ',' && $paren == 1 && $brace == 0 && $bracket == 0) {
                push @arguments, [@current];
                @current = ();
                next;
            }
        }
        push @current, $token;
    }
    return;
}

sub object_property_names {
    my ($tokens) = @_;
    return { error => '第二实参不是对象字面量' } unless @$tokens >= 2;
    return { error => '第二实参不是对象字面量' } unless $tokens->[0]{type} eq 'punct' && $tokens->[0]{value} eq '{';
    return { error => '第二实参不是对象字面量' } unless $tokens->[-1]{type} eq 'punct' && $tokens->[-1]{value} eq '}';

    my @parts;
    my @current;
    my $brace = 0;
    my $bracket = 0;
    my $paren = 0;
    for (my $index = 1; $index < @$tokens - 1; $index++) {
        my $token = $tokens->[$index];
        my $value = $token->{value};
        if ($token->{type} eq 'punct') {
            if ($value eq '{') {
                $brace++;
            } elsif ($value eq '}') {
                $brace--;
                return { error => '对象字面量括号不平衡' } if $brace < 0;
            } elsif ($value eq '[') {
                $bracket++;
            } elsif ($value eq ']') {
                $bracket--;
                return { error => '对象字面量括号不平衡' } if $bracket < 0;
            } elsif ($value eq '(') {
                $paren++;
            } elsif ($value eq ')') {
                $paren--;
                return { error => '对象字面量括号不平衡' } if $paren < 0;
            } elsif ($value eq ',' && $brace == 0 && $bracket == 0 && $paren == 0) {
                push @parts, [@current];
                @current = ();
                next;
            }
        }
        push @current, $token;
    }
    return { error => '对象字面量括号不平衡' } unless $brace == 0 && $bracket == 0 && $paren == 0;
    push @parts, [@current] if @current > 0 || @parts == 0;

    my %names;
    for (my $index = 0; $index < @parts; $index++) {
        my $part = $parts[$index];
        if (@$part == 0) {
            return { error => '对象字面量里有空属性' } unless $index == $#parts;
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

sub scan_same_line_string {
    my ($source, $start) = @_;
    my $quote = substr($source, $start, 1);
    my $cursor = $start + 1;
    while ($cursor < length $source) {
        my $character = substr($source, $cursor, 1);
        return if $character eq "\n" || $character eq "\r" || $character eq "\x{2028}" || $character eq "\x{2029}";
        if ($character eq '\\') {
            $cursor += 2;
            next;
        }
        return ($cursor + 1, substr($source, $start + 1, $cursor - $start - 1)) if $character eq $quote;
        $cursor++;
    }
    return;
}

# 去掉注释但保留字符串与模板；行号由保留的换行维持。JSX 文本里的孤立单引号
# （如 Don't）不会形成跨行字符串，按普通字符继续扫描。
sub strip_typescript_comments {
    my ($source) = @_;
    my @pieces;
    my $cursor = 0;
    my $copied = 0;
    while ($cursor < length $source) {
        my $character = substr($source, $cursor, 1);
        my $next = substr($source, $cursor + 1, 1);
        my ($comment_start, $comment_end);
        if ($character eq '/' && $next eq '/') {
            $comment_start = $cursor;
            $cursor += 2;
            $cursor++ while $cursor < length $source && substr($source, $cursor, 1) !~ /[\r\n]/;
            $comment_end = $cursor;
        } elsif ($character eq '/' && $next eq '*') {
            $comment_start = $cursor;
            $cursor += 2;
            while ($cursor < length $source && substr($source, $cursor, 2) ne '*/') {
                $cursor++;
            }
            $cursor += 2 if $cursor < length $source;
            $comment_end = $cursor;
        }
        if (defined $comment_start) {
            push @pieces, substr($source, $copied, $comment_start - $copied);
            my $removed = substr($source, $comment_start, $comment_end - $comment_start);
            $removed =~ s/[^\n]/ /g;
            push @pieces, $removed;
            $copied = $comment_end;
            next;
        }
        if ($character eq '"' || $character eq "'") {
            my ($end) = scan_same_line_string($source, $cursor);
            $cursor = defined $end ? $end : $cursor + 1;
            next;
        }
        if ($character eq '`') {
            my ($end) = scan_template($source, $cursor);
            if (defined $end) {
                my @expressions = template_expression_ranges($source, $cursor);
                for my $expression (@expressions) {
                    my $length = $expression->{end} - $expression->{start};
                    my $body = substr($source, $expression->{start}, $length);
                    my $cleaned = strip_typescript_comments($body);
                    substr($source, $expression->{start}, $length, $cleaned);
                }
                $cursor = $end;
            } else {
                $cursor++;
            }
            next;
        }
        $cursor++;
    }
    push @pieces, substr($source, $copied);
    return join('', @pieces);
}

sub balanced_call_end {
    my ($source, $open_paren) = @_;
    my $cursor = $open_paren;
    my $depth = 0;
    while ($cursor < length $source) {
        my $character = substr($source, $cursor, 1);
        if ($character eq '"' || $character eq "'") {
            my ($end) = scan_string($source, $cursor);
            return unless defined $end;
            $cursor = $end;
            next;
        }
        if ($character eq '`') {
            my ($end) = scan_template($source, $cursor);
            return unless defined $end;
            $cursor = $end;
            next;
        }
        if ($character eq '(') {
            $depth++;
        } elsif ($character eq ')') {
            $depth--;
            return $cursor if $depth == 0;
        }
        $cursor++;
    }
    return;
}

sub parse_call_tokens {
    my ($source, $call_start, $call_end, $line) = @_;
    my $call_text = substr($source, $call_start, $call_end - $call_start + 1);
    my $tokens = tokenize_typescript($call_text);
    if (!defined $tokens || @$tokens < 2 || $tokens->[0]{value} ne 't' || $tokens->[1]{value} ne '(') {
        return { line => $line, syntax => 't', invalid => '调用内有无法识别的字符串或模板', key => undef, dynamic => 0 };
    }
    my ($arguments) = split_call_arguments($tokens, 1);
    if (!defined $arguments) {
        return { line => $line, syntax => 't', invalid => '调用括号未闭合', key => undef, dynamic => 0 };
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

sub find_typescript_calls_in_range {
    my ($source, $from, $to) = @_;
    my @ranges;
    my $cursor = $from;
    while ($cursor < $to) {
        my $character = substr($source, $cursor, 1);
        if ($character eq '"' || $character eq "'") {
            my ($end) = scan_same_line_string($source, $cursor);
            $cursor = defined $end ? $end : $cursor + 1;
            next;
        }
        if ($character eq '`') {
            my ($end) = scan_template($source, $cursor);
            if (defined $end) {
                my @expressions = template_expression_ranges($source, $cursor);
                for my $expression (@expressions) {
                    push @ranges, find_typescript_calls_in_range($source, $expression->{start}, $expression->{end});
                }
                $cursor = $end;
            } else {
                $cursor++;
            }
            next;
        }
        if ($character =~ /[A-Za-z_\$]/) {
            my $start = $cursor;
            $cursor++;
            $cursor++ while $cursor < $to && substr($source, $cursor, 1) =~ /[A-Za-z0-9_\$]/;
            next unless substr($source, $start, $cursor - $start) eq 't';
            my $look = $cursor;
            $look++ while $look < $to && substr($source, $look, 1) =~ /\s/;
            next unless substr($source, $look, 1) eq '(';
            my $end = balanced_call_end($source, $look);
            push @ranges, { start => $start, end => $end };
            next;
        }
        $cursor++;
    }
    return @ranges;
}

sub find_typescript_calls {
    my ($source) = @_;
    return find_typescript_calls_in_range($source, 0, length $source);
}

sub parse_typescript {
    my ($source) = @_;
    my $line_starts = line_index($source);
    my $stripped = strip_typescript_comments($source);
    my @calls;
    my @call_ranges = find_typescript_calls($stripped);
    for my $range (@call_ranges) {
        my $call_start = $range->{start};
        my $line = line_for_offset($line_starts, $call_start);
        if (!defined $range->{end}) {
            push @calls, { line => $line, syntax => 't', invalid => '调用括号未闭合', key => undef, dynamic => 0 };
            next;
        }
        my $call = parse_call_tokens($stripped, $call_start, $range->{end}, $line);
        push @calls, $call;
    }

    my @literals;
    my $cursor = 0;
    while ($cursor < length $stripped) {
        my $character = substr($stripped, $cursor, 1);
        if ($character eq '"' || $character eq "'") {
            my ($end, $value) = scan_same_line_string($stripped, $cursor);
            if (defined $end) {
                my $inside = 0;
                for my $range (@call_ranges) {
                    if (defined $range->{end} && $cursor >= $range->{start} && $cursor <= $range->{end}) {
                        $inside = 1;
                        last;
                    }
                }
                push @literals, { value => $value, line => line_for_offset($line_starts, $cursor) } unless $inside;
                $cursor = $end;
                next;
            }
            $cursor++;
            next;
        }
        if ($character eq '`') {
            my ($end, $value, $interpolated) = scan_template($stripped, $cursor);
            if (defined $end) {
                my $inside = 0;
                for my $range (@call_ranges) {
                    if (defined $range->{end} && $cursor >= $range->{start} && $cursor <= $range->{end}) {
                        $inside = 1;
                        last;
                    }
                }
                push @literals, { value => $value, line => line_for_offset($line_starts, $cursor) } if !$inside && !$interpolated;
                $cursor = $end;
                next;
            }
            $cursor++;
            next;
        }
        $cursor++;
    }
    for my $call (@calls) {
        next unless $call->{dynamic_literals};
        for my $token (@{ $call->{dynamic_literals}[0] }) {
            next unless $token->{type} eq 'string' || ($token->{type} eq 'template' && !$token->{interpolated});
            push @literals, { value => $token->{value}, line => line_for_offset($line_starts, $call->{offset} + $token->{start}) };
        }
    }
    return { calls => \@calls, literals => \@literals };
}

sub skip_rust_trivia {
    my ($source, $cursor) = @_;
    while ($cursor < length $source) {
        my $character = substr($source, $cursor, 1);
        if ($character =~ /\s/) {
            $cursor++;
            next;
        }
        if ($character eq '/' && substr($source, $cursor + 1, 1) eq '/') {
            $cursor += 2;
            $cursor++ while $cursor < length $source && substr($source, $cursor, 1) !~ /[\r\n]/;
            next;
        }
        if ($character eq '/' && substr($source, $cursor + 1, 1) eq '*') {
            my $depth = 1;
            $cursor += 2;
            while ($cursor < length $source && $depth > 0) {
                if (substr($source, $cursor, 2) eq '/*') {
                    $depth++;
                    $cursor += 2;
                } elsif (substr($source, $cursor, 2) eq '*/') {
                    $depth--;
                    $cursor += 2;
                } else {
                    $cursor++;
                }
            }
            next;
        }
        last;
    }
    return $cursor;
}

sub skip_rust_string {
    my ($source, $cursor) = @_;
    my $prefix = substr($source, $cursor, 2);
    my $raw = 0;
    my $quote_at = $cursor;
    if ($prefix eq 'r"' || $prefix eq 'b"' || $prefix eq 'r#') {
        if (substr($source, $cursor, 1) eq 'r') {
            $raw = 1;
            $quote_at = $cursor + 1;
        } elsif ($prefix eq 'b"' && substr($source, $cursor, 2) eq 'b"') {
            $quote_at = $cursor + 1;
        }
    } elsif (substr($source, $cursor, 3) eq 'br"' || substr($source, $cursor, 3) eq 'br#') {
        $raw = 1;
        $quote_at = $cursor + 2;
    }
    if ($raw) {
        $quote_at++ while $quote_at < length $source && substr($source, $quote_at, 1) eq '#';
        return unless substr($source, $quote_at, 1) eq '"';
        my $hashes = $quote_at - ($cursor + ($prefix eq 'br#' || $prefix eq 'br"' ? 2 : 1));
        my $terminator = '"' . ('#' x $hashes);
        my $end = index($source, $terminator, $quote_at + 1);
        return defined $end ? $end + length $terminator : undef;
    }
    return unless substr($source, $quote_at, 1) eq '"';
    my $index = $quote_at + 1;
    while ($index < length $source) {
        my $character = substr($source, $index, 1);
        if ($character eq '\\') {
            $index += 2;
            next;
        }
        return $index + 1 if $character eq '"';
        $index++;
    }
    return;
}

sub rust_module_end {
    my ($source, $open_brace) = @_;
    my $cursor = $open_brace;
    my $depth = 0;
    while ($cursor < length $source) {
        my $character = substr($source, $cursor, 1);
        if ($character eq '/' && substr($source, $cursor + 1, 1) eq '/') {
            $cursor += 2;
            $cursor++ while $cursor < length $source && substr($source, $cursor, 1) !~ /[\r\n]/;
            next;
        }
        if ($character eq '/' && substr($source, $cursor + 1, 1) eq '*') {
            my $comment_depth = 1;
            $cursor += 2;
            while ($cursor < length $source && $comment_depth > 0) {
                if (substr($source, $cursor, 2) eq '/*') {
                    $comment_depth++;
                    $cursor += 2;
                } elsif (substr($source, $cursor, 2) eq '*/') {
                    $comment_depth--;
                    $cursor += 2;
                } else {
                    $cursor++;
                }
            }
            next;
        }
        if ($character eq '"' || $character eq 'r' || $character eq 'b') {
            my $end = skip_rust_string($source, $cursor);
            if (defined $end) {
                $cursor = $end;
                next;
            }
        }
        if ($character eq "'") {
            # 字符字面量有闭合引号；生命周期 `'a` 没有，按普通标点继续。
            my $end = index($source, "'", $cursor + 1);
            if ($end >= 0 && $end - $cursor <= 12) {
                $cursor = $end + 1;
                next;
            }
        }
        if ($character eq '{') {
            $depth++;
        } elsif ($character eq '}') {
            $depth--;
            return $cursor if $depth == 0;
        }
        $cursor++;
    }
    return;
}

sub strip_rust_inline_tests {
    my ($source) = @_;
    my $result = $source;
    while ($source =~ /^#\[cfg\(test\)\][ \t]*$/mg) {
        my $attribute_start = $-[0];
        my $cursor = $+[0];
        $cursor = skip_rust_trivia($source, $cursor);
        next unless substr($source, $cursor, 3) eq 'mod' && substr($source, $cursor + 3, 1) =~ /\s/;
        my $brace = index($source, '{', $cursor);
        next unless $brace >= 0;
        my $end = rust_module_end($source, $brace);
        next unless defined $end;
        my $removed = substr($result, $attribute_start, $end - $attribute_start + 1);
        $removed =~ s/[^\n]/ /g;
        substr($result, $attribute_start, $end - $attribute_start + 1) = $removed;
        pos($source) = $end + 1;
    }
    return $result;
}

1;
