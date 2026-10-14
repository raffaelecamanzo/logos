#!/usr/bin/env bash
#
# test-check-public-terms.sh — prove scripts/check-public-terms.sh finds a listed
# term, and only a listed term, in a repository's text files (S-716).
#
# The real term list must never be written down in this repository, so these
# cases never use it. They plant made-up words instead and hand the script a term
# list hashed from those words at run time (--terms), which exercises exactly the
# code the real list does: the script never sees anything but digests.
#
# Each case builds a throwaway git repository, so nothing here reads or writes
# this one. The last cases are the controls: a gate that flagged everything would
# pass every planted-term case, and only a clean tree read as clean, with its
# denominator, rules that out.
#
# Usage: bash scripts/tests/test-check-public-terms.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
CHECK="$HERE/scripts/check-public-terms.sh"

if [ ! -f "$CHECK" ]; then
    echo "cannot find scripts/check-public-terms.sh (looked in $HERE)" >&2
    exit 1
fi

WORK="${TMPDIR:-/tmp}/logos-public-terms-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1

# The planted terms: one made-up word and one made-up pair, neither of which is
# English, so no case can pass by accident on a real word.
WORD=zorbex
PAIR="quindle frap"

sha() { # the SHA-256 of $1, computed independently of the script under test
    if command -v shasum >/dev/null 2>&1; then
        printf '%s' "$1" | shasum -a 256 | awk '{ print $1 }'
    else
        printf '%s' "$1" | sha256sum | awk '{ print $1 }'
    fi
}

TERMS="$WORK/terms.sha256"
{
    echo "# test-only digests of the planted words"
    sha "$WORD"
    sha "$PAIR"
} >"$TERMS"
EMPTY_ALLOW="$WORK/empty.allow"
: >"$EMPTY_ALLOW"

PASS=0
FAIL=0
N=0

# fixture <file> <content> [<file> <content> ...] -> sets REPO: a fresh git
# repository holding exactly these committed files.
fixture() {
    N=$((N + 1))
    REPO="$WORK/repo$N"
    mkdir -p "$REPO"
    git -C "$REPO" init -q . >/dev/null 2>&1
    # A repository that failed to initialise would let every git command below
    # walk up into whatever repository encloses $WORK.
    if [ "$(cd "$REPO" && git rev-parse --show-toplevel 2>/dev/null)" != "$(cd "$REPO" && pwd -P)" ]; then
        echo "  FAIL  could not create the fixture repository $REPO"
        exit 1
    fi
    git -C "$REPO" config user.email t@example.com
    git -C "$REPO" config user.name test
    while [ $# -ge 2 ]; do
        mkdir -p "$(dirname "$REPO/$1")"
        printf '%s' "$2" >"$REPO/$1"
        shift 2
    done
    git -C "$REPO" -c maintenance.auto=false -c gc.auto=0 add -A >/dev/null 2>&1
    git -C "$REPO" -c maintenance.auto=false -c gc.auto=0 commit -qm fixture >/dev/null 2>&1
}

# run [<allow-file>] -> sets OUT and RC
run() {
    OUT="$(bash "$CHECK" --root "$REPO" --terms "$TERMS" --allow "${1:-$EMPTY_ALLOW}" 2>&1)"
    RC=$?
}

# expect <name> <wanted-exit> [<line that must appear>] [<text that must NOT appear>]
expect() {
    local name="$1" want="$2" must="${3:-}" mustnt="${4:-}"
    if [ "$RC" -ne "$want" ]; then
        echo "  FAIL  $name: exit $RC, wanted $want"
        printf '%s\n' "$OUT" | sed 's/^/        /'
        FAIL=$((FAIL + 1))
        return
    fi
    if [ -n "$must" ] && ! printf '%s\n' "$OUT" | grep -qxF -- "$must"; then
        echo "  FAIL  $name: exit $RC correct, but no line reading '$must'"
        printf '%s\n' "$OUT" | sed 's/^/        /'
        FAIL=$((FAIL + 1))
        return
    fi
    if [ -n "$mustnt" ] && printf '%s\n' "$OUT" | grep -qF -- "$mustnt"; then
        echo "  FAIL  $name: output names '$mustnt'"
        printf '%s\n' "$OUT" | sed 's/^/        /'
        FAIL=$((FAIL + 1))
        return
    fi
    echo "  ok    $name"
    PASS=$((PASS + 1))
}

echo "test-check-public-terms.sh"
echo "  work $WORK"
echo

# --- a planted term is found, with its file, line and term ---------------------

fixture notes.txt "first line
a $WORD here
"
run
expect "a planted single word fails with file:line:term" 1 "notes.txt:2:$WORD"

fixture docs/a.md "nothing
nothing
the quindle frap again
"
run
expect "a planted pair fails with file:line:term" 1 "docs/a.md:3:$PAIR"

fixture a.rs "// quindle-frap
"
run
expect "a pair written across a hyphen is the pair" 1 "a.rs:1:$PAIR"

fixture a.rs "let quindle_frap = 1;
"
run
expect "a pair written across an underscore is the pair" 1 "a.rs:1:$PAIR"

fixture a.ts "const quindleFrap = 1;
"
run
expect "a camelCase identifier is read as its words" 1 "a.ts:1:$PAIR"

fixture a.ts "type ZORBEXPath = 1;
let x9Zorbex = 2;
"
run
expect "a run of capitals ends before a capitalised word" 1 "a.ts:1:$WORD"
expect "a digit before a capital is a word boundary" 1 "a.ts:2:$WORD"

fixture a.txt "ZorBex
"
run
expect "matching is case-insensitive" 1 "a.txt:1:$WORD"

# Non-ASCII punctuation separates words like ASCII punctuation does: an em dash,
# curly quotes, a curly apostrophe, a no-break space inside a pair.
fixture a.txt "$(printf 'one\nthe %s\342\200\224tool\n\342\200\234%s\342\200\235\n%s\342\200\231s\nquindle\302\240frap\n' "$WORD" "$WORD" "$WORD")"
run
expect "an em dash separates words" 1 "a.txt:2:$WORD"
expect "curly quotes separate words" 1 "a.txt:3:$WORD"
expect "a curly apostrophe separates words" 1 "a.txt:4:$WORD"
expect "a no-break space joins a pair" 1 "a.txt:5:$PAIR"

# ...while a non-ASCII letter is part of its word.
fixture a.txt "$(printf '%s\303\251 and \303\251%s\n' "$WORD" "$WORD")"
run
expect "an accented letter is part of its word" 0 "" "a.txt:"

# The tree gate.sh certifies includes untracked files `git add -A` would take.
fixture a.txt "clean
"
printf 'a %s\n' "$WORD" >"$REPO/new.txt"
run
expect "an untracked file is in the walk" 1 "new.txt:1:$WORD"
expect "an untracked file is counted apart from the tracked ones" 1 "public-terms: tracked=1 untracked=1 scanned=2 binary=0 skipped=0 hits=1 allowlisted=0"

# --- near misses are not hits --------------------------------------------------

fixture a.txt "${WORD}es and pre${WORD} and ${WORD}2
quindle
frap
quindlefrap
"
run
expect "a longer word containing a term, or a pair split over two lines, is not a hit" 0 "" "a.txt:"

# --- the allowlist admits a reviewed line, and only that line as it stands ------

fixture LICENSE "preamble
a $WORD in a verbatim text
"
ALLOW="$WORK/one.allow"
printf '# a reviewed line\nLICENSE:%s\n' "$(sha "a $WORD in a verbatim text")" >"$ALLOW"
run "$ALLOW"
expect "an allowlisted line passes" 0 "public-terms: tracked=1 untracked=0 scanned=1 binary=0 skipped=0 hits=0 allowlisted=1"

printf 'preamble\na %s in an edited text\n' "$WORD" >"$REPO/LICENSE"
run "$ALLOW"
expect "an allowlist entry lapses when its line changes" 1 "LICENSE:2:$WORD"

fixture other.txt "a $WORD in a verbatim text
"
run "$ALLOW"
expect "an allowlist entry admits its own path only" 1 "other.txt:1:$WORD"

# --- what the walk does not read -----------------------------------------------

# A symlink may point outside the repository — on a development checkout the
# private docs directories are symlinks — so it is never followed.
printf 'a %s\n' "$WORD" >"$WORK/outside.txt"
fixture a.txt "clean
"
ln -s "$WORK/outside.txt" "$REPO/linked.txt"
git -C "$REPO" add linked.txt >/dev/null 2>&1
git -C "$REPO" commit -qm link >/dev/null 2>&1
run
expect "a symlink is not followed" 0 "public-terms: tracked=2 untracked=0 scanned=1 binary=0 skipped=1 hits=0 allowlisted=0"

# A tracked directory replaced by a link: git still lists the file it held, and
# reading it would read outside the repository.
mkdir -p "$WORK/outside-dir"
printf 'a %s\n' "$WORD" >"$WORK/outside-dir/f.txt"
fixture a.txt "clean
" d/f.txt "clean
"
rm -rf "$REPO/d"
ln -s "$WORK/outside-dir" "$REPO/d"
run
# The link itself is an untracked path, skipped like any symlink.
expect "a file under a linked directory is not read" 0 "public-terms: tracked=2 untracked=1 scanned=1 binary=0 skipped=2 hits=0 allowlisted=0"

fixture a.txt "clean
"
printf 'a %s\0 binary\n' "$WORD" >"$REPO/blob.bin"
git -C "$REPO" add blob.bin >/dev/null 2>&1
git -C "$REPO" commit -qm blob >/dev/null 2>&1
run
expect "a binary file is not read" 0 "public-terms: tracked=2 untracked=0 scanned=1 binary=1 skipped=0 hits=0 allowlisted=0"

# --- a walk that reads nothing is not a pass -----------------------------------

fixture blob.bin "placeholder"
printf 'a\0b' >"$REPO/blob.bin"
git -C "$REPO" add blob.bin >/dev/null 2>&1
git -C "$REPO" commit -qm blob >/dev/null 2>&1
run
expect "a walk that read zero text files fails" 3 "public-terms: tracked=1 untracked=0 scanned=0 binary=1 skipped=0 hits=0 allowlisted=0"

fixture a.txt "clean
"
: >"$WORK/no-terms.sha256"
OUT="$(bash "$CHECK" --root "$REPO" --terms "$WORK/no-terms.sha256" --allow "$EMPTY_ALLOW" 2>&1)"
RC=$?
expect "a term list with no digest is refused" 2 "check-public-terms.sh: $WORK/no-terms.sha256 lists no digest: a gate over zero terms checks nothing"

# A term list line that is not a bare digest is refused, never skipped: a digest
# pasted with shasum's trailing `  -` would otherwise stop gating its term.
fixture a.txt "a $WORD
"
printf '%s  -\n' "$(sha "$WORD")" >"$WORK/pasted.sha256"
OUT="$(bash "$CHECK" --root "$REPO" --terms "$WORK/pasted.sha256" --allow "$EMPTY_ALLOW" 2>&1)"
RC=$?
expect "a term list line that is not a digest is refused" 2 "check-public-terms.sh: $WORK/pasted.sha256:1: not a SHA-256 digest"

# Every other refusal exits 2 too. CI reads nothing but the exit code, so a
# refusal that exited 0 would pass the step having scanned nothing.
fixture a.txt "clean
"
run "$WORK/no-such.allow"
expect "a missing allowlist is refused" 2 "check-public-terms.sh: cannot read $WORK/no-such.allow"

printf 'not an entry\n' >"$WORK/bad.allow"
run "$WORK/bad.allow"
expect "a malformed allowlist entry is refused" 2 "check-public-terms.sh: $WORK/bad.allow:1: not a path:sha256 entry"

mkdir -p "$WORK/not-a-repo"
OUT="$(GIT_CEILING_DIRECTORIES="$WORK" bash "$CHECK" --root "$WORK/not-a-repo" --terms "$TERMS" --allow "$EMPTY_ALLOW" 2>&1)"
RC=$?
expect "a root outside any git repository is refused" 2 "check-public-terms.sh: not inside a git repository: $WORK/not-a-repo"

OUT="$(bash "$CHECK" --root "$REPO" --bogus 2>&1)"
RC=$?
expect "an unknown argument is refused" 2 "check-public-terms.sh: unknown argument: --bogus"

# An UNTRACKED symlink has no index mode to give it away: only the walk's own
# lstat keeps it from being followed out of the repository.
fixture a.txt "clean
"
ln -s "$WORK/outside.txt" "$REPO/loose-link.txt"
run
expect "an untracked symlink is not followed" 0 "public-terms: tracked=1 untracked=1 scanned=1 binary=0 skipped=1 hits=0 allowlisted=0"

# The untracked half of the walk is what `git add -A` would take: an ignored
# file (build output, dependencies) is not in it.
fixture .gitignore "ignored.txt
"
printf 'a %s\n' "$WORD" >"$REPO/ignored.txt"
run
expect "an ignored file is not in the walk" 0 "public-terms: tracked=1 untracked=0 scanned=1 binary=0 skipped=0 hits=0 allowlisted=0"

# --emit-allow prints the entry a reviewer would add, and that entry, added
# verbatim, admits the line: the documented way to grow the allowlist works.
fixture a.txt "clean
a $WORD in reviewed prose
"
OUT="$(bash "$CHECK" --root "$REPO" --terms "$TERMS" --allow "$EMPTY_ALLOW" --emit-allow 2>&1)"
RC=$?
expect "--emit-allow prints the line's entry" 1 "a.txt:$(sha "a $WORD in reviewed prose")"
printf '%s\n' "$OUT" | grep -E '^a\.txt:[0-9a-f]{64}$' >"$WORK/emitted.allow"
run "$WORK/emitted.allow"
expect "the emitted entry, added verbatim, admits the line" 0 "public-terms: tracked=1 untracked=0 scanned=1 binary=0 skipped=0 hits=0 allowlisted=1"

# The line digest is of the line without its ending, CRLF included, so an entry
# written as documented admits a line in a file with Windows line endings.
fixture dos.txt "$(printf 'a %s here\r\nnext\r\n' "$WORD")"
printf 'dos.txt:%s\n' "$(sha "a $WORD here")" >"$WORK/dos.allow"
run "$WORK/dos.allow"
expect "a CRLF line's entry is the digest of the line without its ending" 0 "public-terms: tracked=1 untracked=0 scanned=1 binary=0 skipped=0 hits=0 allowlisted=1"

# --- the text a release generates (scripts/check-release-text.sh) --------------
#
# .github/workflows/release.yml runs the gate over the release notes, the Homebrew
# formula and the shell installer before the GitHub Release is created. Those
# files are never committed, so check-release-text.sh stages them in a throwaway
# repository; these cases run it on files shaped like the ones dist writes.

RELTEXT="$HERE/scripts/check-release-text.sh"
REL="$WORK/release"
mkdir -p "$REL/a" "$REL/b"

# formula <file> <desc>: a formula shaped like the one dist generates.
formula() {
    cat >"$1" <<EOF
class Logos < Formula
  desc "$2"
  homepage "https://github.com/example/logos"
  version "1.16.2"
  if OS.mac? && Hardware::CPU.arm?
      url "https://github.com/example/logos/releases/download/v1.16.2/logos-aarch64-apple-darwin.tar.xz"
      sha256 "0000000000000000000000000000000000000000000000000000000000000000"
  end
  license "Apache-2.0"

  def install
    bin.install "logos"
  end
end
EOF
}
formula "$REL/logos.rb" "Logos — structural code intelligence"
printf '## Install logos 1.16.2\n\n### Added\n\n- a release, built from the tag\n' >"$REL/notes.txt"
printf '#!/bin/sh\n# installer for logos 1.16.2\nAPP_NAME="logos"\n' >"$REL/logos-installer.sh"

# reltext [<allow-file>] <file>... -> sets OUT and RC
reltext() {
    local allow="$EMPTY_ALLOW"
    case "${1:-}" in *.allow) allow="$1"; shift ;; esac
    OUT="$(bash "$RELTEXT" --terms "$TERMS" --allow "$allow" "$@" 2>&1)"
    RC=$?
}

reltext "$REL/notes.txt" "$REL/logos.rb" "$REL/logos-installer.sh"
expect "release text: clean notes, formula and installer pass, with their denominator" 0 \
    "release text: public-terms: tracked=3 untracked=0 scanned=3 binary=0 skipped=0 hits=0 allowlisted=0"

formula "$REL/a/logos.rb" "Logos — structural code intelligence, the $WORD build"
reltext "$REL/notes.txt" "$REL/a/logos.rb"
expect "release text: a term planted in a fixture formula fails with name:line:term" 1 "    logos.rb:2:$WORD"

printf '## Install logos 1.16.2\n\nthe quindle frap returns\n' >"$REL/a/notes.txt"
reltext "$REL/a/notes.txt" "$REL/logos.rb"
expect "release text: a pair planted in the release notes fails" 1 "    notes.txt:3:$PAIR"

printf '#!/bin/sh\nAPP_NAME="logos-%s"\n' "$WORD" >"$REL/a/logos-installer.sh"
reltext "$REL/notes.txt" "$REL/logos.rb" "$REL/a/logos-installer.sh"
expect "release text: a term in the installer fails" 1 "    logos-installer.sh:2:$WORD"

# A file the gate does not read as text is not a pass: the summary would be clean.
printf 'a %s\0 binary\n' "$WORD" >"$REL/b/blob.bin"
reltext "$REL/notes.txt" "$REL/b/blob.bin"
expect "release text: a file not read as text fails the run" 3 "" "release text:"

reltext "$REL/notes.txt" "$REL/a/notes.txt"
expect "release text: two files with one base name are refused" 2 \
    "check-release-text.sh: two files share the base name notes.txt"

reltext "$REL/notes.txt" "$REL/no-such.rb"
expect "release text: a missing file is refused" 2 \
    "check-release-text.sh: not a readable regular file: $REL/no-such.rb"

reltext
expect "release text: no file at all is refused" 2

OUT="$(bash "$RELTEXT" --terms "$WORK/no-terms.sha256" --allow "$EMPTY_ALLOW" "$REL/notes.txt" 2>&1)"
RC=$?
expect "release text: a gate that cannot run fails the run" 3 "" "release text:"

printf 'logos.rb:%s\n' "$(sha "  desc \"Logos — structural code intelligence, the $WORD build\"")" >"$WORK/rel.allow"
reltext "$WORK/rel.allow" "$REL/notes.txt" "$REL/a/logos.rb"
expect "release text: an allowlist entry is keyed by the file's base name" 0 \
    "release text: public-terms: tracked=2 untracked=0 scanned=2 binary=0 skipped=0 hits=0 allowlisted=1"

OUT="$(bash "$RELTEXT" --terms "$TERMS" "$REL/notes.txt" 2>&1)"
RC=$?
expect "release text: without --allow, the reviewed release-text allowlist is the one read" 0 \
    "release text: allowlist $HERE/scripts/release-text.allow"

# --- the controls --------------------------------------------------------------

fixture a.txt "clean text
" b/c.md "more clean text
"
run
expect "a clean tree passes, with its denominator" 0 "public-terms: tracked=2 untracked=0 scanned=2 binary=0 skipped=0 hits=0 allowlisted=0"

# This repository itself, with the real list: the tree must pass as committed.
OUT="$(bash "$CHECK" --root "$HERE" 2>&1)"
RC=$?
expect "this repository passes its own gate" 0

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
