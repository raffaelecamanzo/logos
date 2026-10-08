#!/usr/bin/perl
#
# run-bounded.pl — run a command with a wall-clock limit, and on timeout kill
# its WHOLE process tree (S-611). macOS ships no `timeout`, and the obvious
# `perl -e 'alarm shift; exec @ARGV'` kills only the process it exec'd: npm
# dies, while its `sh -c` child and everything under it keep running.
#
#   perl scripts/run-bounded.pl <seconds> <command> [args...]
#
# The command runs in its own process group. On timeout the group gets SIGTERM,
# then SIGKILL three seconds later, and this script exits 142 (128 + SIGALRM),
# the status a plain alarm would have produced. Otherwise it exits with the
# command's own status (128 + n when the command died of signal n).
# Exit 2 on a usage error, 127 when the command cannot be started.

use strict;
use warnings;

my $limit = shift @ARGV;
if (!defined $limit || $limit !~ /^[1-9][0-9]*$/ || !@ARGV) {
    print STDERR "usage: run-bounded.pl <seconds> <command> [args...]\n";
    exit 2;
}

my $pid = fork();
die "run-bounded.pl: fork failed: $!\n" unless defined $pid;
if ($pid == 0) {
    setpgrp(0, 0);
    exec { $ARGV[0] } @ARGV or do {
        print STDERR "run-bounded.pl: cannot run $ARGV[0]: $!\n";
        exit 127;
    };
}

$SIG{ALRM} = sub {
    print STDERR "run-bounded.pl: $ARGV[0] exceeded ${limit}s; killing its process group\n";
    kill 'TERM', -$pid;
    sleep 3;
    kill 'KILL', -$pid;
    waitpid($pid, 0);
    exit 142;
};
alarm $limit;
waitpid($pid, 0);
alarm 0;
my $status = $?;
exit(($status & 127) ? 128 + ($status & 127) : $status >> 8);
