#!/usr/bin/env bash
#
# test-security-ignore.sh — prove the tracked .logos/security-ignore.toml holds
# entries the suppression reader will accept and honour (HF-4, FR-SC-11).
#
# A fingerprint that is not 64 lowercase hex characters is rejected by the reader
# (the whole file with it); one with a typo in it parses and suppresses nothing, so
# the shape is what can be checked offline. Each entry must name exactly one of a
# `fingerprint` and a `path`, carry a non-empty `reason`, and repeat no fingerprint;
# the file stays in the canonical spelling the writer produces (fingerprint entries
# first, sorted, then the path entries) so a triage edit shows as a minimal diff.
# The checker is run over copies of the real file with one defect each and must fail
# every one of them: a check that cannot fail proves nothing.
#
# Usage: bash scripts/tests/test-security-ignore.sh
# Exit:  0 every case behaved, 1 one did not
#
# bash 3.2 compatible.

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
FILE="$HERE/.logos/security-ignore.toml"
# Removing the last suppression removes the file (suppress.rs), so its absence is not a defect.
[ -f "$FILE" ] || { echo "test-security-ignore.sh: no .logos/security-ignore.toml, so no suppression to check"; exit 0; }
python3 -c 'import tomllib' 2>/dev/null || { echo "python3 with tomllib (3.11+) is required" >&2; exit 1; }

WORK="${TMPDIR:-/tmp}/logos-security-ignore-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1

PASS=0
FAIL=0
OUT=""
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; printf '%s\n' "$OUT" | sed 's/^/        /'; FAIL=$((FAIL + 1)); }

# check_file FILE — one line per defect, exit 1 if there is any.
check_file() {
    python3 -I -c '
import re, sys, tomllib
try:
    entries = tomllib.load(open(sys.argv[1], "rb")).get("ignore", [])
except Exception as e:
    print("does not parse as TOML: %s" % e); sys.exit(1)
bad = []
if not entries:
    bad.append("no [[ignore]] entry (nothing checked is not a pass)")
seen, fps, paths_started = set(), [], False
for i, e in enumerate(entries, 1):
    has_fp, has_path = "fingerprint" in e, "path" in e
    if has_fp == has_path:
        bad.append("entry %d names %s" % (i, "both a fingerprint and a path" if has_fp else "neither a fingerprint nor a path"))
    if not isinstance(e.get("reason"), str) or not e["reason"].strip():
        bad.append("entry %d has no reason" % i)
    if has_fp:
        fp = e["fingerprint"]
        if not isinstance(fp, str) or not re.fullmatch(r"[0-9a-f]{64}", fp):
            bad.append("entry %d: fingerprint is not 64 lowercase hex characters" % i)
        if fp in seen:
            bad.append("entry %d repeats fingerprint %s" % (i, fp[:12]))
        seen.add(fp); fps.append(fp)
        if paths_started:
            bad.append("entry %d: a fingerprint entry after a path entry (not the canonical order)" % i)
    else:
        paths_started = True
if fps != sorted(fps):
    bad.append("the fingerprint entries are not sorted")
print("\n".join(bad))
sys.exit(1 if bad else 0)
' "$1"
}

echo "test-security-ignore.sh"
echo "  file $FILE"
echo

OUT="$(check_file "$FILE" 2>&1)"; RC=$?
if [ "$RC" -eq 0 ]; then ok "every entry names one selector, has a reason, and the fingerprints are 64-hex, unique and sorted"; else bad ".logos/security-ignore.toml carries a defect"; fi

NFP="$(grep -c '^fingerprint = ' "$FILE")"
if [ "$NFP" -ge 1 ]; then ok "$NFP fingerprint entries checked (a count of zero would not be a pass)"; else OUT="no fingerprint entry"; bad "no fingerprint entry in the file"; fi

FIRST="$(sed -n 's/^fingerprint = "\([0-9a-f]*\)"$/\1/p' "$FILE" | head -1)"
[ -n "$FIRST" ] || { echo "cannot read a fingerprint from the file" >&2; exit 1; }

# expect_caught NAME PERL-EXPR PATTERN — perl -0pe edit of a copy; the checker must exit 1 and print PATTERN.
expect_caught() {
    local f="$WORK/m.toml"
    perl -0pe "$2" "$FILE" >"$f"
    if cmp -s "$FILE" "$f"; then OUT="the mutation changed nothing: $2"; bad "mutation inert ($1)"; return; fi
    OUT="$(check_file "$f" 2>&1)"; RC=$?
    if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -qE -- "$3"; then ok "caught: $1"; else bad "not caught ($1): exit $RC, wanted /$3/"; fi
}

expect_caught "a 63-hex fingerprint" "s/$FIRST/substr(\$&, 0, 63)/e" 'not 64 lowercase hex'
expect_caught "a 65-hex fingerprint" "s/$FIRST/${FIRST}0/" 'not 64 lowercase hex'
expect_caught "an upper-case fingerprint" "s/$FIRST/\\U\$&/" 'not 64 lowercase hex'
expect_caught "a non-hex character in a fingerprint" "s/$FIRST/g\${\\(substr(\$&, 1))}/" 'not 64 lowercase hex'
expect_caught "an entry with no reason" 's/^(fingerprint = "[0-9a-f]{64}"\n)reason = "[^\n]*"\n/$1/m' 'has no reason'
expect_caught "an entry with a blank reason" 's/^(fingerprint = "[0-9a-f]{64}"\nreason = )"[^\n]*"/$1"  "/m' 'has no reason'
expect_caught "a fingerprint and a path in one entry" 's/^(fingerprint = "[0-9a-f]{64}"\n)/$1path = "Cargo.lock"\n/m' 'both a fingerprint and a path'
expect_caught "a repeated fingerprint" "s/(\\[\\[ignore\\]\\]\\nfingerprint = \"$FIRST\"\\n[^\\n]*\\n)/\$1\\n\$1/" 'repeats fingerprint'
expect_caught "fingerprint entries out of order" "s/^(\\[\\[ignore\\]\\]\\nfingerprint = \")$FIRST/\${1}ffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffffff/m" 'not sorted'
expect_caught "a file that is not TOML" 's/^reason = /reason: /m' 'does not parse as TOML'

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
