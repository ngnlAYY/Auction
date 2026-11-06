#!/usr/bin/env perl
# 校验依赖升级日历并按需输出提醒。
#
# 供 .github/workflows/dependency-cadence.yml 调用，也可本地手工复现：
#   perl ci/checks/dependency-cadence.pl [--today YYYY-MM-DD] [--file 日历路径]
#
# 退出码：0 = 日历有效且没有逾期窗口；1 = 日历无效或窗口已逾期。
# 提前提醒与“窗口进行中”只输出 warning 注解，不改变退出码。
# 脚本只读取仓库文件与本地日期，不联网、不修改日历、不创建 issue。

use strict;
use warnings;
use utf8;

use Cwd qw(abs_path);
use File::Spec;
use FindBin qw($Bin);
use Getopt::Long qw(GetOptions);
use JSON::PP qw(decode_json);
use POSIX qw(strftime);
use Time::Local qw(timegm);

binmode STDOUT, ':encoding(UTF-8)';
binmode STDERR, ':encoding(UTF-8)';

my $root = abs_path(File::Spec->catdir($Bin, '..', '..'));
my $calendar_path = File::Spec->catfile($root, 'ops', 'dependencies', 'upgrade-windows.json');
my $today = strftime('%Y-%m-%d', gmtime(time()));
my $help = 0;

GetOptions(
    'today=s' => \$today,
    'file=s'  => \$calendar_path,
    'help'    => \$help,
) or usage(2);
usage(0) if $help;

my $today_epoch = parse_iso($today);
unless (defined $today_epoch) {
    fail_annotations("--today 必须是 UTC 日期 YYYY-MM-DD：$today");
    exit 1;
}

my @errors;
my @warnings;

open my $fh, '<', $calendar_path
    or fail_and_exit("无法读取升级日历 $calendar_path：$!");
my $raw = do { local $/; <$fh> };
close $fh;

my $doc = eval { decode_json($raw) };
unless (defined $doc && ref $doc eq 'HASH') {
    fail_and_exit("升级日历不是有效 JSON 对象：$calendar_path");
}

validate($doc);

if (@errors) {
    for my $error (@errors) {
        print STDERR "::error title=依赖升级日历::${error}\n";
    }
    print STDERR "升级日历校验失败（$calendar_path）。\n";
    exit 1;
}

for my $warning (@warnings) {
    print STDERR "::warning title=依赖升级窗口::${warning}\n";
}

print_summary($doc);
exit 0;

sub usage {
    my ($code) = @_;
    print STDERR <<'USAGE';
用法：perl ci/checks/dependency-cadence.pl [--today YYYY-MM-DD] [--file 日历路径]
USAGE
    exit $code;
}

sub fail_annotations {
    my ($message) = @_;
    (my $single = $message) =~ s/\s+/ /g;
    print STDERR "::error title=依赖升级日历::${single}\n";
}

sub fail_and_exit {
    my ($message) = @_;
    (my $single = $message) =~ s/\s+/ /g;
    print STDERR "::error title=依赖升级日历::${single}\n";
    exit 1;
}

sub parse_iso {
    my ($value) = @_;
    return undef unless defined $value && $value =~ /\A(\d{4})-(\d{2})-(\d{2})\z/;
    my ($year, $month, $day) = ($1 + 0, $2 + 0, $3 + 0);
    my $epoch = eval { timegm(0, 0, 0, $day, $month - 1, $year) };
    return undef unless defined $epoch;
    my @parts = gmtime($epoch);
    return undef
        unless $parts[5] + 1900 == $year
        && $parts[4] + 1 == $month
        && $parts[3] == $day;
    return $epoch;
}

sub iso {
    my ($epoch) = @_;
    return strftime('%Y-%m-%d', gmtime($epoch));
}

sub weekday {
    my ($epoch) = @_;
    return (gmtime($epoch))[6];
}

sub second_monday {
    my ($year, $month) = @_;
    my $first = timegm(0, 0, 0, 1, $month - 1, $year);
    my $offset = (8 - weekday($first)) % 7;
    return timegm(0, 0, 0, 1 + $offset + 7, $month - 1, $year);
}

sub add_error {
    my ($message) = @_;
    push @errors, $message;
}

sub validate {
    my ($doc) = @_;

    my $version = $doc->{version};
    add_error('version 必须为整数 1') unless defined $version && $version =~ /\A1\z/;

    my $lead = $doc->{lead_days};
    add_error('lead_days 必须是 1–90 的整数')
        unless defined $lead && $lead =~ /\A\d+\z/ && $lead >= 1 && $lead <= 90;

    my %blocks;
    my $ranges = $doc->{blocked_ranges};
    add_error('blocked_ranges 必须是数组')
        unless ref $ranges eq 'ARRAY';
    if (ref $ranges eq 'ARRAY') {
        for my $index (0 .. $#$ranges) {
            my $range = $ranges->[$index];
            unless (ref $range eq 'HASH') {
                add_error("blocked_ranges[$index] 必须是对象");
                next;
            }
            my $name = $range->{name};
            add_error("blocked_ranges[$index].name 必须是非空字符串")
                unless defined $name && $name =~ /\S/;
            my $start = parse_iso($range->{starts_on});
            my $end = parse_iso($range->{ends_on});
            if (!defined $start || !defined $end) {
                add_error("blocked_ranges[$index] 的日期必须是 UTC 日期 YYYY-MM-DD");
                next;
            }
            add_error("blocked_ranges[$index] 的结束早于开始") if $end < $start;
            $blocks{$index} = [$start, $end, defined $name ? $name : "blocked_ranges[$index]"];
        }
    }

    my $windows = $doc->{windows};
    add_error('windows 必须是非空数组')
        unless ref $windows eq 'ARRAY' && @$windows;
    return unless ref $windows eq 'ARRAY' && @$windows;

    my %status = map { $_ => 1 } qw(scheduled completed skipped deferred);
    my %seen_id;
    my @parsed;

    for my $index (0 .. $#$windows) {
        my $window = $windows->[$index];
        unless (ref $window eq 'HASH') {
            add_error("windows[$index] 必须是对象");
            next;
        }

        my $id = $window->{id};
        unless (defined $id && $id =~ /\A(\d{4})-Q([1-4])\z/) {
            add_error("windows[$index].id 必须形如 2027-Q1");
            next;
        }
        my ($year, $quarter) = ($1 + 0, $2 + 0);
        add_error("windows[$index].id 重复：$id") if $seen_id{$id}++;
        my $quarter_index = $year * 4 + $quarter - 1;

        my $label = $window->{id};
        my $state = $window->{status};
        add_error("$label 的 status 必须是 " . join('、', sort keys %status))
            unless defined $state && $status{$state};

        my $start = parse_iso($window->{starts_on});
        my $end = parse_iso($window->{ends_on});
        unless (defined $start && defined $end) {
            add_error("$label 的日期必须是 UTC 日期 YYYY-MM-DD");
            next;
        }
        add_error("$label 的结束早于开始") if $end < $start;

        my $note = $window->{note};
        add_error("$label 的 note 必须是字符串")
            if defined $note && ref $note;

        my $shifted = $window->{shifted_reason};
        if (defined $shifted) {
            add_error("$label 的 shifted_reason 必须是非空字符串")
                unless ref $shifted eq '' && $shifted =~ /\S/;
        }

        if (weekday($start) != 1) {
            add_error("$label 必须从周一开始：" . iso($start));
        }
        if ($end != $start + 4 * 86400) {
            add_error("$label 必须是周一到周五共 5 个工作日（结束应为 "
                . iso($start + 4 * 86400) . "）：" . iso($end));
        }

        unless (defined $shifted && $shifted =~ /\S/) {
            my $expected = second_monday($year, ($quarter - 1) * 3 + 1);
            if ($start != $expected) {
                add_error("$label 必须是该季度第二个周一（默认 "
                    . iso($expected) . "）；如已顺延，请填写 shifted_reason");
            }
        }

        for my $block (sort { $a <=> $b } keys %blocks) {
            my ($block_start, $block_end, $block_name) = @{ $blocks{$block} };
            next if $end < $block_start || $start > $block_end;
            add_error("$label 与冻结区间「$block_name」（"
                . iso($block_start) . " 至 " . iso($block_end)
                . "）重叠；请顺延并填写 shifted_reason");
        }

        push @parsed, {
            id            => $id,
            status        => $state,
            start         => $start,
            end           => $end,
            quarter_index => $quarter_index,
            note          => defined $note ? $note : '',
        };
    }

    if (ref $doc->{deferred} eq 'ARRAY') {
        for my $index (0 .. $#{ $doc->{deferred} }) {
            my $entry = $doc->{deferred}[$index];
            unless (ref $entry eq 'HASH') {
                add_error("deferred[$index] 必须是对象");
                next;
            }
            add_error("deferred[$index].item 必须是非空字符串")
                unless defined $entry->{item} && $entry->{item} =~ /\S/;
            add_error("deferred[$index].reason 必须是非空字符串")
                unless defined $entry->{reason} && $entry->{reason} =~ /\S/;
        }
    } elsif (defined $doc->{deferred}) {
        add_error('deferred 必须是数组');
    }

    return unless @parsed == @$windows;

    for my $index (1 .. $#parsed) {
        my $previous = $parsed[ $index - 1 ];
        my $current  = $parsed[$index];
        add_error("窗口顺序必须按时间递增：$current->{id} 早于 $previous->{id}")
            if $current->{start} <= $previous->{start};
        add_error("窗口必须按季度连续：$previous->{id} 之后应为下一个季度，实际为 $current->{id}")
            if $current->{quarter_index} != $previous->{quarter_index} + 1;
    }

    my @scheduled = sort { $a->{start} <=> $b->{start} }
        grep { $_->{status} eq 'scheduled' } @parsed;

    for my $window (@scheduled) {
        add_error("窗口 $window->{id} 已逾期但状态仍为 scheduled；请更新状态并追加下一季度")
            if $window->{end} < $today_epoch;
    }

    my ($next) = grep { $_->{end} >= $today_epoch } @scheduled;
    unless ($next) {
        add_error('缺少下一次升级窗口：请在 ops/dependencies/upgrade-windows.json 追加下一个季度');
        return;
    }

    my $lead_days = $doc->{lead_days} || 14;
    my $days = int(($next->{start} - $today_epoch) / 86400);
    if ($today_epoch >= $next->{start}) {
        push @warnings, "窗口 $next->{id} 正在进行（" . iso($next->{start}) . ' 至 '
            . iso($next->{end}) . '）；按 ops/dependencies.md 执行并按依赖族留证。';
    } elsif ($days <= $lead_days) {
        push @warnings, "窗口 $next->{id} 将在 $days 天后开始（"
            . iso($next->{start}) . ' 至 ' . iso($next->{end})
            . "）；提前 $lead_days 天按 issue 模板建立窗口并核对冻结日历。";
    }
}

sub print_summary {
    my ($doc) = @_;

    print "## 依赖升级窗口\n\n";
    print "| 窗口 | 状态 | 日期（UTC） | 说明 |\n";
    print "| --- | --- | --- | --- |\n";
    for my $window (@{ $doc->{windows} }) {
        my $note = defined $window->{note} ? $window->{note} : '';
        $note =~ s/\|/\\|/g;
        print "| $window->{id} | $window->{status} | $window->{starts_on} 至 $window->{ends_on} | $note |\n";
    }

    my @scheduled = sort { $a->{starts_on} cmp $b->{starts_on} }
        grep { $_->{status} eq 'scheduled' } @{ $doc->{windows} };
    my ($next) = grep { parse_iso($_->{ends_on}) >= $today_epoch } @scheduled;
    if ($next) {
        my $days = int((parse_iso($next->{starts_on}) - $today_epoch) / 86400);
        my $when = $days > 0 ? "$days 天后" : '进行中';
        print "\n下一次窗口：$next->{id}（$next->{starts_on} 至 $next->{ends_on}，$when）。\n";
    }

    print "\n门禁：`just verify` 与 `just test`；涉及数据库或 HTTP 栈加跑 W12.3 负载演练，"
        . "涉及迁移补 W10.6 演练回执；升级节奏与冻结日历见 `ops/dependencies.md`。\n";

    my $deferred = $doc->{deferred};
    if (ref $deferred eq 'ARRAY' && @$deferred) {
        print "\n延后项：\n";
        for my $entry (@$deferred) {
            print "- $entry->{item}：$entry->{reason}\n";
        }
    }
}
