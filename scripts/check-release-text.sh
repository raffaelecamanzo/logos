#!/usr/bin/env bash
#
# check-release-text.sh — run scripts/check-public-terms.sh over text a release
# generates and publishes: the release notes, the Homebrew formula, the shell
# installer (S-732, NFR-CR-04).
#
#   bash scripts/check-release-text.sh [--terms FILE] [--allow FILE] FILE...
#
# The term gate walks a git repository, and these files are written by the
# release run and never committed. So, as scripts/lib/publish-core.sh does for a
# publish's commit message, each FILE is copied under its base name into a
# throwaway repository that holds nothing else, and the gate runs on it with
# --root. The gate's summary must then show exactly the files given, every one of
# them tracked and read as text: a walk that read fewer reads exactly like a clean
# one, so it fails instead.
#
# .github/workflows/release.yml runs this in the `host` job, after the notes,
# formula and installer exist and before the GitHub Release is created, so a hit
# stops the release before it is published: no Release, no formula pushed. --terms and --allow are handed to
# check-public-terms.sh unchanged. The terms default to the gate's own list; the
# allowlist defaults to scripts/release-text.allow, the reviewed lines of generated
# text (the repository's allowlist names repository paths, none of which is here).
# An entry is keyed by the file's base name.
#
# Exit codes:
#   0  no listed term in any FILE
#   1  a listed term in a FILE (the hits are printed as name:line:term)
#   2  usage error: no FILE, a FILE that is not a readable regular file, or two
#      FILEs with the same base name
#   3  the gate could not run, or did not read every FILE as text

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$HERE/check-public-terms.sh"
GATE_ARGS=()
ALLOW="$HERE/release-text.allow"
FILES=()

usage() {
    echo "usage: bash scripts/check-release-text.sh [--terms FILE] [--allow FILE] FILE..." >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --terms)
            [ $# -ge 2 ] || usage
            GATE_ARGS+=("$1" "$2")
            shift 2
            ;;
        --allow)
            [ $# -ge 2 ] || usage
            ALLOW="$2"
            shift 2
            ;;
        -h | --help) usage ;;
        --)
            shift
            FILES+=("$@")
            break
            ;;
        -*)
            echo "check-release-text.sh: unknown option: $1" >&2
            usage
            ;;
        *)
            FILES+=("$1")
            shift
            ;;
    esac
done
[ "${#FILES[@]}" -gt 0 ] || usage
[ -f "$CHECK" ] || { echo "check-release-text.sh: cannot find $CHECK" >&2; exit 3; }

for f in "${FILES[@]}"; do
    if [ -L "$f" ] || [ ! -f "$f" ] || [ ! -r "$f" ]; then
        echo "check-release-text.sh: not a readable regular file: $f" >&2
        exit 2
    fi
done
# bash 3.2 (macOS) has no associative arrays, so duplicates are found by sorting.
dup="$(for f in "${FILES[@]}"; do basename -- "$f"; done | sort | uniq -d | head -n 1)"
if [ -n "$dup" ]; then
    echo "check-release-text.sh: two files share the base name $dup" >&2
    exit 2
fi

WORK="${RUNNER_TEMP:-${TMPDIR:-/tmp}}/release-text.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
rm -rf "$WORK"
if ! {
    git init -q "$WORK" &&
        for f in "${FILES[@]}"; do /bin/cp -- "$f" "$WORK/$(basename -- "$f")"; done &&
        git -C "$WORK" add -A
} 2>"$WORK.err"; then
    sed 's/^/    /' "$WORK.err" >&2
    rm -f "$WORK.err"
    echo "check-release-text.sh: could not stage the files to scan" >&2
    exit 3
fi
rm -f "$WORK.err"

rc=0
out="$(bash "$CHECK" --root "$WORK" --allow "$ALLOW" ${GATE_ARGS[@]+"${GATE_ARGS[@]}"} 2>&1)" || rc=$?
summary="$(printf '%s\n' "$out" | tail -n 1)"
# The hit lines name the throwaway repository's paths, which are the base names.
case "$rc" in
    0) ;;
    1)
        echo "check-release-text.sh: the release text carries a listed term:" >&2
        printf '%s\n' "$out" | sed 's/^/    /' >&2
        exit 1
        ;;
    *)
        echo "check-release-text.sh: the term gate could not run (exit $rc):" >&2
        printf '%s\n' "$out" | sed 's/^/    /' >&2
        exit 3
        ;;
esac
n="${#FILES[@]}"
if [[ ! "$summary" =~ ^public-terms:\ tracked=([0-9]+)\ untracked=0\ scanned=([0-9]+)\  ]] ||
    [ "${BASH_REMATCH[1]}" -ne "$n" ] || [ "${BASH_REMATCH[2]}" -ne "$n" ]; then
    echo "check-release-text.sh: the term gate did not read the $n file(s) as text: $summary" >&2
    exit 3
fi
echo "release text: allowlist $ALLOW"
echo "release text: $summary"
