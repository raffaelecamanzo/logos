#!/usr/bin/env bash
#
# merge-base.sh — resolve the base and the merge-base the branch review compares
# against, and say whether the branch differs from it at all (FR-IN-15, FR-GV-24).
#
#   merge-base.sh [--base REF] [--project DIR]
#
# The base is the stated REF, else the repository's default branch: origin/HEAD,
# then `main`, then `master` (the order `logos review` itself uses). Prints one
# `key=value` per line:
#
#   base=<ref>  base_origin=stated|default-branch  head=<sha>  merge_base=<sha>
#   branch_commits=<commits on HEAD past the merge-base>
#   dirty_paths=<paths changed in the worktree under the project; the .logos/
#                store excluded, except the quality contract inside it>
#   identical=yes|no     yes when there is no commit and no dirty path: the
#                        branch IS the merge-base and its delta is empty, so the
#                        review needs no run (and no cold shadow index)
#
# Exit: 0 resolved · 1 no base resolves, or it shares no history with HEAD ·
#       2 usage · 3 not a git repository, or no python3
#
# Reads git only; writes nothing.

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"

BASE=""
PROJECT="$(rv_project)"
while [ $# -gt 0 ]; do
    case "$1" in
        --base) [ $# -ge 2 ] || rv_die 2 "--base needs a ref"; BASE="$2"; shift 2 ;;
        --project) [ $# -ge 2 ] || rv_die 2 "--project needs a directory"; PROJECT="$2"; shift 2 ;;
        *) rv_die 2 "unknown argument '$1' (usage: merge-base.sh [--base REF] [--project DIR])" ;;
    esac
done

g() { git -C "$PROJECT" "$@"; }
g rev-parse --git-dir >/dev/null 2>&1 || rv_die 3 "$PROJECT is not a git repository"
rv_need_python
HEAD_SHA="$(g rev-parse --verify --quiet 'HEAD^{commit}')" || rv_die 3 "HEAD has no commit yet"

TRIED=0
ORIGIN=stated
if [ -n "$BASE" ]; then
    TRIED=1
    g rev-parse --verify --quiet "$BASE^{commit}" >/dev/null || {
        echo "unresolved: '$BASE' is not a commit of this repository"
        echo "denominator: 1 candidate ref tried, 0 resolved" >&2
        exit 1
    }
else
    ORIGIN=default-branch
    CANDIDATES="$(g symbolic-ref --quiet --short refs/remotes/origin/HEAD 2>/dev/null || true)"
    for c in $CANDIDATES main master; do
        TRIED=$((TRIED + 1))
        if g rev-parse --verify --quiet "$c^{commit}" >/dev/null; then BASE="$c"; break; fi
    done
    [ -n "$BASE" ] || {
        echo "unresolved: no origin/HEAD, main or master in this repository; name a base with --base"
        echo "denominator: $TRIED candidate ref(s) tried, 0 resolved" >&2
        exit 1
    }
fi

MB="$(g merge-base HEAD "$BASE" 2>/dev/null)" || {
    SHALLOW=""
    [ "$(g rev-parse --is-shallow-repository 2>/dev/null)" = true ] && SHALLOW=" (this is a shallow clone: fetch more history)"
    echo "unresolved: HEAD and '$BASE' share no history$SHALLOW"
    echo "denominator: $TRIED candidate ref(s) tried, 1 resolved, 0 merge-base" >&2
    exit 1
}

COMMITS="$(g rev-list --count "$MB..HEAD")"
DIRTY="$(rv_py dirty --project "$PROJECT" 2>/dev/null | grep -c . || true)"

IDENTICAL=no
[ "$COMMITS" -eq 0 ] && [ "$DIRTY" -eq 0 ] && IDENTICAL=yes

printf 'base=%s\nbase_origin=%s\nhead=%s\nmerge_base=%s\nbranch_commits=%s\ndirty_paths=%s\nidentical=%s\n' \
    "$BASE" "$ORIGIN" "$HEAD_SHA" "$MB" "$COMMITS" "$DIRTY" "$IDENTICAL"
echo "denominator: $TRIED candidate ref(s) tried, 1 resolved; $COMMITS commit(s) past the merge-base, $DIRTY dirty path(s)" >&2
