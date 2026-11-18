# 从 Cargo metadata 提取浏览器的工作区路径依赖闭包，核对 Web 指纹清单。

use strict;
use warnings;
use utf8;
use File::Basename qw(dirname);
use JSON::PP qw(decode_json);

binmode STDERR, ':encoding(UTF-8)';

my $metadata = decode_json(do { local $/; <STDIN> });
my $workspace_root = $metadata->{workspace_root};
my %members = map { $_ => 1 } @{ $metadata->{workspace_members} };
my %packages = map { dirname($_->{manifest_path}) => $_ }
    grep { $members{$_->{id}} } @{ $metadata->{packages} };
my %package_paths = map { $packages{$_}->{name} => $_ } keys %packages;

my @pending = map {
    $package_paths{$_} // die "未找到浏览器工作区成员：$_\n"
} qw(auction-web-public auction-web-admin auction-web-shared);

# 按 Cargo 给出的绝对目录跟随依赖，依赖别名和不同目标的声明同样参与指纹。
my (%visited, %dependencies);
while (@pending) {
    my $directory = pop @pending;
    next if $visited{$directory}++;

    for my $dependency (@{ $packages{$directory}->{dependencies} }) {
        my $path = $dependency->{path};
        next unless defined $path && exists $packages{$path};
        push @pending, $path;

        $path =~ s{^\Q$workspace_root\E/}{} or die "工作区依赖超出仓库根目录：$path\n";
        $dependencies{$path} = 1;
    }
}

my %listed = map { $_ => 1 } @ARGV;
my @missing = sort grep { !exists $listed{$_} } keys %dependencies;
my @extra = sort grep { !exists $dependencies{$_} } keys %listed;
print STDERR "Web 指纹清单缺少工作区依赖：$_\n" for @missing;
print STDERR "Web 指纹清单多出工作区依赖：$_\n" for @extra;
exit(@missing || @extra ? 1 : 0);
