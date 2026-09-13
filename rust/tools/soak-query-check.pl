#!/usr/bin/env perl
# One receipt-admission owner for standalone and IPC soak queries.
# Exit 0: observed; 2: valid but not yet observed; 1: cannot examine the reply.
use strict;
use warnings;
use JSON::PP qw(decode_json);

my ($transport, $mode, $name, $path) = @ARGV;
my $reply = eval { decode_json(do { local $/; <STDIN> }) };
exit 1 if $@ || ref($reply) ne 'HASH';
my $result;
if (defined($transport) && $transport eq 'ipc') {
    exit 1 unless JSON::PP::is_bool($reply->{ok}) && $reply->{ok};
    $result = $reply->{result};
} elsif (defined($transport) && $transport eq 'cli') {
    $result = $reply;
} else {
    exit 1;
}
exit 1 unless ref($result) eq 'HASH' && defined($mode);
exit 0 if $mode eq 'validate';
if ($mode eq 'fresh') {
    exit 1 unless JSON::PP::is_bool($result->{is_fresh});
    exit($result->{is_fresh} ? 0 : 2);
}
exit 1 unless defined($result->{resolution}) && !ref($result->{resolution})
    && $result->{resolution} eq 'Available';
exit 1 unless ($mode eq 'present' || $mode eq 'absent')
    && defined($name) && defined($path) && ref($result->{items}) eq 'ARRAY';
for my $key ('total', 'shown', 'hidden') {
    exit 1 unless defined($result->{$key}) && !ref($result->{$key})
        && $result->{$key} =~ /^\d+$/;
}
exit 1 unless JSON::PP::is_bool($result->{truncated});
my $found = 0;
for my $item (@{$result->{items}}) {
    exit 1 unless ref($item) eq 'HASH'
        && defined($item->{symbol_name}) && !ref($item->{symbol_name})
        && defined($item->{file_path}) && !ref($item->{file_path});
    $found = 1 if $item->{symbol_name} eq $name && $item->{file_path} eq $path;
}
exit($found ? 0 : 2) if $mode eq 'present';
# A capped prefix cannot prove absence, even when no returned row matches.
exit 2 if $result->{truncated} || $result->{hidden} != 0
    || $result->{shown} != scalar(@{$result->{items}})
    || $result->{total} != scalar(@{$result->{items}});
exit($found ? 2 : 0);
