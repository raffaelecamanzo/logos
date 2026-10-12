#!/usr/bin/env bash
#
# check-public-terms.sh — keep a reviewed list of terms out of this repository's
# text files (NFR-CR-04).
#
# THE LIST IS STORED ONLY AS HASHES
#   scripts/public-terms.sha256 holds one SHA-256 per listed word or word pair,
#   so the repository never carries the list in plain text and a search of the
#   tree cannot find the list by reading this gate's data. To add a term, hash
#   its normalised form and append the digest:
#       printf '%s' 'word' | shasum -a 256
#       printf '%s' 'first second' | shasum -a 256
#
# WHAT A HIT IS
#   Every line of every file in the walk is split into words and normalised:
#   lower-case, and every character that is not a letter or a digit is a word
#   separator: ASCII punctuation, hyphens and underscores included, so `a-b`,
#   `a_b` and `a b` are the same pair, and so is the common non-ASCII
#   punctuation (dashes, curly quotes, the no-break space; see PUNCT below).
#   A capital after a lower-case letter or digit starts a new word as well, so
#   `aB` is the pair `a b`, as does the last capital of a run before a
#   lower-case letter (`ABCDef` is `abc def`): identifiers are where a later
#   edit is likeliest to bring a term back. Each word, and each pair of adjacent words on the same
#   line (joined by one space), is hashed; a digest in the list is a hit,
#   reported as `path:line:term`. Matching is whole-word: a longer word that
#   contains a listed word is a different word and does not hit.
#
# THE ALLOWLIST ADMITS A LINE, NOT A TERM
#   scripts/public-terms.allow holds reviewed `path:sha256` entries, where the
#   digest is of the whole line (its bytes without the line ending). An entry
#   admits every hit on that one line of that one file, and survives the line
#   moving, but not the line changing: an edited line must be reviewed again.
#   `--emit-allow` prints the candidate entries for the current hits; append only
#   the ones a reviewer has read. Comments in the allowlist must say why a line is
#   admitted without quoting the term.
#
# THE WALK
#   The files `git ls-files` lists, plus the untracked files `git add -A` would
#   take (the tree gate.sh certifies includes them). Symlinks, submodules and
#   files deleted from the working tree are not read; a file with a NUL byte in
#   its first 8000 bytes is binary (git's own heuristic) and is not read either.
#   A walk that read zero text files fails: an empty walk reports no hits, which
#   reads exactly like a clean tree (compare names_in() in
#   cli/tests/dependency_fitness.rs and the read-count assertion beside it).
#
# Usage:
#   bash scripts/check-public-terms.sh [--root DIR] [--terms FILE] [--allow FILE]
#                                      [--emit-allow]
#
# The last line printed is always the summary, in a fixed shape gate.sh parses:
#   public-terms: tracked=N untracked=N scanned=N binary=N skipped=N hits=N allowlisted=N
#   tracked + untracked = scanned + binary + skipped.
#
# Exit codes:
#   0  no hit outside the allowlist
#   1  at least one hit outside the allowlist
#   2  usage error, not a git repository, missing tool, or unreadable/empty data
#   3  the walk read zero text files

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
ROOT=""
TERMS="$HERE/public-terms.sha256"
ALLOW="$HERE/public-terms.allow"
EMIT_ALLOW=no

usage() {
    sed -n '/^# Usage:/,/^#   bash/p; /^#  *\[--emit-allow\]/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --root)
            [ $# -ge 2 ] || usage
            ROOT="$2"
            shift 2
            ;;
        --terms)
            [ $# -ge 2 ] || usage
            TERMS="$2"
            shift 2
            ;;
        --allow)
            [ $# -ge 2 ] || usage
            ALLOW="$2"
            shift 2
            ;;
        --emit-allow)
            EMIT_ALLOW=yes
            shift
            ;;
        -h | --help) usage ;;
        *)
            echo "check-public-terms.sh: unknown argument: $1" >&2
            usage
            ;;
    esac
done

for prereq in git perl; do
    if ! command -v "$prereq" >/dev/null 2>&1; then
        echo "check-public-terms.sh: required command not found: $prereq" >&2
        exit 2
    fi
done
if ! perl -MDigest::SHA -e 1 >/dev/null 2>&1; then
    echo "check-public-terms.sh: perl's Digest::SHA module is required" >&2
    exit 2
fi

if [ -z "$ROOT" ]; then
    ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || ROOT=""
fi
if [ -z "$ROOT" ] || ! git -C "$ROOT" rev-parse --git-dir >/dev/null 2>&1; then
    echo "check-public-terms.sh: not inside a git repository${ROOT:+: $ROOT}" >&2
    exit 2
fi
for data in "$TERMS" "$ALLOW"; do
    if [ ! -r "$data" ]; then
        echo "check-public-terms.sh: cannot read $data" >&2
        exit 2
    fi
done

# The scan. perl, because hashing every word and word pair of the tree one
# process per digest would take minutes; Digest::SHA is a core module. The
# script arrives on stdin, so the arguments are the only input it trusts.
exec perl - "$ROOT" "$TERMS" "$ALLOW" "$EMIT_ALLOW" <<'PL'
use strict;
use warnings;
use Digest::SHA qw(sha256_hex);

my ($root, $terms_path, $allow_path, $emit_allow) = @ARGV;

sub fail_usage { print STDERR "check-public-terms.sh: $_[0]\n"; exit 2; }

# --- the term digests ----------------------------------------------------------
my %term;
{
    open(my $fh, '<', $terms_path) or fail_usage("cannot read $terms_path: $!");
    while (my $l = <$fh>) {
        $l =~ s/\r?\n\z//;
        next if $l =~ /^\s*(#|\z)/;
        $l =~ /^([0-9a-f]{64})\s*\z/ or fail_usage("$terms_path:$.: not a SHA-256 digest");
        $term{$1} = 1;
    }
}
# An empty list matches nothing, which reads exactly like a clean tree.
fail_usage("$terms_path lists no digest: a gate over zero terms checks nothing")
    unless %term;

# --- the allowlist ---------------------------------------------------------------
my %allow;    # "path\0digest" -> 0 until it admits a hit
{
    open(my $fh, '<', $allow_path) or fail_usage("cannot read $allow_path: $!");
    while (my $l = <$fh>) {
        $l =~ s/\r?\n\z//;
        next if $l =~ /^\s*(#|\z)/;
        $l =~ /^(.+):([0-9a-f]{64})\s*\z/
            or fail_usage("$allow_path:$.: not a path:sha256 entry");
        $allow{"$1\0$2"} = 0;
    }
}

# --- the walk --------------------------------------------------------------------
# Index entries first (mode, so symlinks and submodules are never followed), then
# the untracked files `git add -A` would take.
my (@paths, %seen, %mode);
{
    open(my $git, '-|', 'git', '-C', $root, 'ls-files', '-s', '-z')
        or fail_usage("git ls-files failed: $!");
    local $/ = "\0";
    while (my $e = <$git>) {
        chomp $e;
        my ($meta, $path) = split /\t/, $e, 2;
        next unless defined $path;
        next if $seen{$path}++;    # one entry per path, whatever its stage
        $mode{$path} = (split / /, $meta)[0];
        push @paths, $path;
    }
    close($git) or fail_usage("git ls-files exited " . ($? >> 8));

    open($git, '-|', 'git', '-C', $root, 'ls-files', '--others', '--exclude-standard', '-z')
        or fail_usage("git ls-files --others failed: $!");
    while (my $path = <$git>) {
        chomp $path;
        next if $seen{$path}++;
        $mode{$path} = 'untracked';
        push @paths, $path;
    }
    close($git) or fail_usage("git ls-files --others exited " . ($? >> 8));
}

my ($tracked, $untracked, $scanned, $binary, $skipped, $hits, $allowlisted) = (0) x 7;

# True when a directory on the way to $path is a symlink: git still lists the
# files a tracked directory held after that directory is replaced by a link, and
# reading them would read wherever the link points.
my %dir_is_link;
sub in_linked_dir {
    my @parts = split m{/}, $_[0];
    pop @parts;
    my $dir = $root;
    for my $part (@parts) {
        $dir .= "/$part";
        return 1 if $dir_is_link{$dir} //= (-l $dir ? 1 : 0);
    }
    return 0;
}
my %digest;    # a word or pair seen before is not hashed again

# PUNCT: the UTF-8 encodings of the non-ASCII punctuation prose actually uses.
# The text is read as bytes, and a byte >= 0x80 is otherwise a word character
# (so an accented word stays one word); these sequences are read as spaces
# first, so a term glued to an em dash or inside curly quotes is still a word.
#   \xc2[\xa0-\xbf]    U+00A0-U+00BF: no-break space, guillemets, section sign...
#   \xe2\x80[\x80-\xbf] U+2000-U+203F: typographic spaces, dashes, quotes, bullets
#   \xe2\x81[\x80-\xaf] U+2040-U+206F: the rest of General Punctuation
my $PUNCT = qr/\xc2[\xa0-\xbf]|\xe2\x80[\x80-\xbf]|\xe2\x81[\x80-\xaf]/;

# Records in %$found every listed word, and every listed pair of adjacent words,
# in one lower-cased line.
sub scan_words {
    my ($found, $text) = @_;
    $text =~ s/$PUNCT/ /g;
    my @w = split /[^a-z0-9\x80-\xff]+/, $text;
    shift @w if @w && $w[0] eq '';
    for my $i (0 .. $#w) {
        my $one = $w[$i];
        $found->{$one} = 1
            if $term{ $digest{$one} //= sha256_hex($one) };
        next if $i == $#w;
        my $two = "$one $w[$i + 1]";
        $found->{$two} = 1
            if $term{ $digest{$two} //= sha256_hex($two) };
    }
}
my @emit;

for my $path (@paths) {
    if ($mode{$path} eq 'untracked') { $untracked++ } else { $tracked++ }
    my $full = "$root/$path";
    # Regular files only: a symlink may point outside the repository (the private
    # docs directories are symlinks on a development checkout), and so may any
    # directory above the file; a submodule is a directory, and a file deleted
    # from the working tree has nothing to read. `-l` must stay last but one:
    # `-f _` reads the lstat it leaves behind.
    if ($mode{$path} =~ /^(120000|160000)\z/ || in_linked_dir($path) || -l $full || !-f _) {
        $skipped++;
        next;
    }
    open(my $fh, '<:raw', $full) or do { $skipped++; next };
    my $text = do { local $/; <$fh> };
    close($fh);
    $text = '' unless defined $text;
    if (index(substr($text, 0, 8000), "\0") >= 0) {
        $binary++;
        next;
    }
    $scanned++;

    my $n = 0;
    for my $line (split /\n/, $text, -1) {
        $n++;
        my %found;
        scan_words(\%found, lc $line);
        # A lower-case letter or digit followed by a capital is a word boundary
        # too, and so is the last capital of a run followed by a lower-case
        # letter (`ABCDef` is `ABC Def`), so a camelCase identifier is also read
        # as the words it is made of. Both readings are scanned: split alone, a
        # mixed-case word stops being the word it spells.
        if ($line =~ /[a-z0-9][A-Z]|[A-Z][A-Z][a-z]/) {
            (my $humps = $line) =~ s/([a-z0-9])([A-Z])/$1 $2/g;
            $humps =~ s/([A-Z])([A-Z][a-z])/$1 $2/g;
            scan_words(\%found, lc $humps);
        }
        next unless %found;
        (my $bare = $line) =~ s/\r\z//;
        my $key = "$path\0" . sha256_hex($bare);
        if (exists $allow{$key}) {
            $allow{$key}++;
            $allowlisted += scalar keys %found;
            next;
        }
        for my $t (sort keys %found) {
            print "$path:$n:$t\n";
            $hits++;
        }
        push @emit, "$path:" . sha256_hex($bare);
    }
    # The cache saves rehashing, but left alone it would hold every distinct pair
    # in the tree (~250 MB resident on this repository). Clearing it past a bound
    # keeps the peak near 120 MB, for ~0.7 s more over the whole walk.
    %digest = () if keys %digest > 200_000;
}

my $unused = grep { $allow{$_} == 0 } keys %allow;
if ($emit_allow eq 'yes' && @emit) {
    print "\n# candidate allowlist entries (review each line before adding it):\n";
    my %once;
    print "$_\n" for grep { !$once{$_}++ } @emit;
}
print STDERR "check-public-terms.sh: $unused allowlist entr"
    . ($unused == 1 ? "y admits" : "ies admit")
    . " no line in the walk (stale; remove after review)\n"
    if $unused;
if ($scanned == 0) {
    print STDERR "check-public-terms.sh: the walk read zero text files under $root\n";
}
print "public-terms: tracked=$tracked untracked=$untracked scanned=$scanned binary=$binary "
    . "skipped=$skipped hits=$hits allowlisted=$allowlisted\n";
exit 3 if $scanned == 0;
exit($hits ? 1 : 0);
PL
