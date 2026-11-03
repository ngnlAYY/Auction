#!/usr/bin/env perl
# 只读取本地文件，检查 Markdown 相对链接与 GitHub 标题锚点。
use strict;
use warnings;
use utf8;
use open qw(:std :encoding(UTF-8));
use File::Basename qw(dirname);
use File::Spec;
use Encode qw(decode FB_CROAK);

my $brackets = qr{(?<label>\[(?:\\.|[^\[\]\\]|(?&label))*\])};
my $inline_link = qr{
    (?<!\\)$brackets \( \s*
    (?: <(?<angle>[^<>\n]*)> | (?<bare>(?:\\[^\s]|[^\s()\\]|(?&paren))*) )
    (?: \s+ (?: "[^"]*" | '[^']*' | \([^)]*\) ) )? \s* \)
    (?(DEFINE)(?<paren>\((?:\\[^\s]|[^\s()\\]|(?&paren))*\)))
}x;
my %documents;
my $failures = 0;
my $link_count = 0;

sub blank {
    my ($text) = @_;
    $text =~ s/[^\n]/ /g;
    return $text;
}

sub entities {
    my ($text) = @_;
    my %named = (amp => '&', lt => '<', gt => '>', quot => '"', apos => "'", nbsp => ' ');
    $text =~ s/&#(?:x([0-9a-fA-F]+)|([0-9]+));/chr(defined $1 ? hex($1) : $2)/ge;
    $text =~ s/&(amp|lt|gt|quot|apos|nbsp);/$named{$1}/ge;
    return $text;
}

sub unescape {
    my ($text) = @_;
    $text =~ s/\\([[:punct:]])/$1/g;
    return entities($text);
}

sub reference_key {
    my ($text) = @_;
    $text = lc unescape($text);
    $text =~ s/^\s+|\s+$//g;
    $text =~ s/\s+/ /g;
    return $text;
}

sub slug {
    my ($heading) = @_;
    # 行内代码保留内容（包括下划线）；强调标记只在代码外去除。
    my @code;
    $heading =~ s/(?<!`)(`+)(?!`)(.*?)\1(?!`)/push(@code, $2); "\0" . $#code . "\0"/gse;
    $heading =~ s/$inline_link/substr($+{label}, 1, -1)/ge;
    $heading =~ s/\[([^\]]+)\](?:\[[^\]]*\])?/$1/g;
    $heading =~ s/<[^>]*>//g;
    $heading =~ s/(?<!\w)_+|_+(?!\w)|\*+|~~//g;
    $heading =~ s/\0([0-9]+)\0/$code[$1]/ge;
    $heading = lc unescape($heading);
    $heading =~ s/^\s+|\s+$//g;
    $heading =~ s/[^\p{L}\p{M}\p{N}_\-\s]//g;
    $heading =~ s/\s/-/g;
    return $heading;
}

sub document {
    my ($file) = @_;
    return $documents{$file} if exists $documents{$file};
    open my $input, '<', $file or die "$file: $!\n";
    my $text = do { local $/; <$input> };
    $text =~ s/<!--.*?(?:-->|\z)/blank($&)/gse;
    $text =~ s{<pre\b[^>]*>.*?</pre>}{blank($&)}gise;
    my @lines = split /\n/, $text, -1;
    my @heading_lines;
    my @list_indents;
    my ($fence, $fence_length);
    for my $line (@lines) {
        my $content = $line;
        $content =~ s/^ {0,3}(?:> ?)+//;
        # 列表内的缩进先按容器扣除，不能把嵌套列表链接误当成代码块跳过。
        if (!defined $fence && $content =~ /^( *)(?:[-+*]|[0-9]{1,9}[.)])[ \t]+(?=\S)/ &&
            (length($1) < 4 || (@list_indents && length($1) < $list_indents[-1] + 4))) {
            my ($indent, $width) = (length($1), length($&));
            pop @list_indents while @list_indents && $indent < $list_indents[-1];
            push @list_indents, $width;
            $content = substr($content, $width);
        } else {
            if (!defined $fence && $content =~ /^( *)\S/) {
                my $indent = length($1);
                pop @list_indents while @list_indents && $indent < $list_indents[-1];
            }
            $content =~ s/^ {$list_indents[-1]}// if @list_indents;
        }
        if (defined $fence) {
            undef $fence if $content =~ /^ {0,3}\Q$fence\E{$fence_length,}[ \t]*$/;
            $line = blank($line);
        } elsif ($content =~ /^ {0,3}(`{3,}|~{3,})/) {
            ($fence, $fence_length) = (substr($1, 0, 1), length $1);
            $line = blank($line);
        } elsif ($content =~ /^(?: {4}|\t)/) {
            $line = blank($line);
        }
        push @heading_lines, $line =~ /\S/ ? $content : '';
    }
    $text = join "\n", @lines;
    my %anchors;
    my @paragraph;
    for my $line (@heading_lines) {
        my $heading;
        if ($line =~ /^ {0,3}\#{1,6}(?:[ \t]+(.*?)|[ \t]*)$/) {
            $heading = $1 // '';
            $heading =~ s/[ \t]+\#+[ \t]*$//;
        } elsif ($line =~ /^ {0,3}(?:=+|-+)[ \t]*$/ && @paragraph) {
            $heading = join ' ', @paragraph;
        }
        if (defined $heading) {
            my $base = slug($heading);
            my $anchor = $base;
            my $suffix = 0;
            $anchor = $base . '-' . ++$suffix while exists $anchors{$anchor};
            $anchors{$anchor} = 1;
        }
        if (defined $heading || $line !~ /\S/ || $line =~ /^ {0,3}(?:[-*_][ \t]*){3,}$/) {
            @paragraph = ();
        } else {
            $line =~ s/^\s+|\s+$//g;
            push @paragraph, $line;
        }
    }
    # 代码示例中的 HTML id 不能成为真实锚点。
    $text =~ s/(?<!`)(`+)(?!`)(.*?)\1(?!`)/blank($&)/gse;
    $text =~ s{<code\b[^>]*>.*?</code>}{blank($&)}gise;
    while ($text =~ /<(?:a|h[1-6])\b[^>]*\b(?:id|name)\s*=\s*(["'])(.*?)\1[^>]*>/gis) {
        $anchors{entities($2)} = 1;
    }
    my %references;
    $text =~ s{^ {0,3}\[([^\]\n]+)\]:[ \t]*(?:<([^>\n]+)>|(\S+))[^\n]*}{
        my ($label, $target) = ($1, defined $2 ? $2 : $3);
        my $key = reference_key($label);
        $references{$key} //= $target;
        blank($&);
    }gme;
    return $documents{$file} = { text => $text, anchors => \%anchors, references => \%references };
}

sub fail_link {
    my ($file, $line, $target, $reason) = @_;
    print STDERR "$file:$line: Markdown 断链 [$target]：$reason\n";
    $failures++;
}

sub percent_decode {
    my ($text) = @_;
    $text =~ s{((?:%[0-9a-fA-F]{2})+)}{
        my $bytes = $1;
        $bytes =~ s/%([0-9a-fA-F]{2})/chr(hex($1))/ge;
        decode('UTF-8', $bytes, FB_CROAK);
    }ge;
    return $text;
}

sub check_link {
    my ($file, $text, $offset, $target) = @_;
    $target = unescape($target);
    # 外部 URI、站点绝对路径与协议相对链接不属于仓库相对链接；从不发起请求。
    return if $target =~ m{^(?:[a-z][a-z0-9+.-]*:|/)}i;
    my $line = 1 + (() = substr($text, 0, $offset) =~ /\n/g);
    my ($path, $fragment) = split /#/, $target, 2;
    $path =~ s/\?.*$//;
    eval {
        $path = percent_decode($path);
        $fragment = percent_decode($fragment) if defined $fragment;
    };
    return fail_link($file, $line, $target, '无效的 UTF-8 百分号编码') if $@;
    my $destination = length($path) ? File::Spec->catfile(dirname($file), $path) : $file;
    $link_count++;
    return fail_link($file, $line, $target, "目标不存在：$destination") unless -e $destination;
    return unless defined $fragment && length $fragment;
    $destination .= '/README.md' if -d $destination;
    if ($destination =~ /\.md$/i && -f $destination) {
        return if exists document($destination)->{anchors}{$fragment};
    } elsif (-f $destination && $fragment =~ /^L([1-9][0-9]*)(?:-L([1-9][0-9]*))?$/) {
        my ($start, $end) = ($1, $2 // $1);
        open my $source, '<', $destination or die "$destination: $!\n";
        my $length = 0;
        $length++ while <$source>;
        return if $start <= $end && $end <= $length;
    }
    fail_link($file, $line, $target, "锚点不存在：$destination#$fragment");
}

my @files = sort split /\0/, do { local $/; <STDIN> // '' };
die "Markdown 检查：未找到文档\n" unless @files;
for my $file (@files) {
    my $doc = document($file);
    my $text = $doc->{text};
    $text =~ s/$inline_link/
        my ($offset, $target, $matched) = ($-[0], defined $+{angle} ? $+{angle} : $+{bare}, $&);
        check_link($file, $doc->{text}, $offset, $target);
        blank($matched);
    /gex;
    while ($text =~ /(?<!\\)$brackets(?:[ \t]*\[(?<reference>[^\]\n]*)\])?/g) {
        my ($label, $reference, $offset) = (substr($+{label}, 1, -1), $+{reference}, $-[0]);
        my $key = reference_key(defined $reference && length $reference ? $reference : $label);
        next unless exists $doc->{references}{$key};
        check_link($file, $doc->{text}, $offset, $doc->{references}{$key});
    }
}
exit 1 if $failures;
printf "Markdown 链接检查通过（%d 个文档，%d 个相对链接）\n", scalar(@files), $link_count;
