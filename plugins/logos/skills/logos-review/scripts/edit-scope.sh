#!/usr/bin/env bash
#
# edit-scope.sh — prove that only the `fix` symbols' files were edited
# (FR-IN-15).
#
#   edit-scope.sh begin --state FILE [--project DIR]
#   edit-scope.sh check --state FILE [--allow PATH ...] [--project DIR]
#
# `begin` records HEAD and every dirty path of the worktree under the project
# (changed, added, deleted or untracked) with a hash of its content; the .logos/
# store is not a path, except the quality contract inside it. `check` fails when
#
#   - HEAD moved (a commit would hide an edit from a worktree comparison), or
#   - a path was added, deleted or changed since, or a path dirty at `begin` is
#     clean again (reverted or committed), and it is not allowed.
#
# --allow is repeated, one project-relative path each (a leading ./ is dropped):
# the file of each fix entry, and any existing test file the fix had to update. A
# path dirty at `begin` and untouched since is not a change. Paths are one per
# line: a path holding a tab or a newline cannot be allowed.
#
# Exit: 0 nothing outside the allowed paths changed · 1 something did (each path
#       on stdout) · 2 usage · 3 not a git repository, or no python3

set -euo pipefail

case "${BASH_SOURCE[0]}" in
    */*) SCRIPT_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) SCRIPT_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/common.sh
. "$SCRIPT_DIR/lib/common.sh"

USAGE="usage: edit-scope.sh begin|check --state FILE [--allow PATH ...] [--project DIR]"
[ $# -ge 1 ] || rv_die 2 "$USAGE"
MODE="$1"
shift
case "$MODE" in begin | check) ;; *) rv_die 2 "unknown mode '$MODE' ($USAGE)" ;; esac

STATE=""
ALLOW=()
PROJECT="$(rv_project)"
while [ $# -gt 0 ]; do
    case "$1" in
        --state) [ $# -ge 2 ] || rv_die 2 "--state needs a file"; STATE="$2"; shift 2 ;;
        --allow) [ $# -ge 2 ] || rv_die 2 "--allow needs a path"; ALLOW+=("${2#./}"); shift 2 ;;
        --project) [ $# -ge 2 ] || rv_die 2 "--project needs a directory"; PROJECT="$2"; shift 2 ;;
        *) rv_die 2 "unknown argument '$1' ($USAGE)" ;;
    esac
done
[ -n "$STATE" ] || rv_die 2 "--state FILE is required"
git -C "$PROJECT" rev-parse --git-dir >/dev/null 2>&1 || rv_die 3 "$PROJECT is not a git repository"
rv_need_python

HEAD_SHA="$(git -C "$PROJECT" rev-parse --verify --quiet 'HEAD^{commit}' || echo none)"

# "<path><TAB><hash or 'deleted'>" for every dirty path, sorted; nothing when clean.
snapshot() {
    local out
    out="$(rv_py dirty --project "$PROJECT" --hash 2>/dev/null)" || rv_die 3 "cannot read the dirty paths of $PROJECT"
    [ -z "$out" ] || printf '%s\n' "$out" | LC_ALL=C sort
}

if [ "$MODE" = begin ]; then
    { printf '#head\t%s\n' "$HEAD_SHA"; snapshot; } >"$STATE.tmp.$$"
    mv "$STATE.tmp.$$" "$STATE"
    N="$(grep -c -v '^#head' "$STATE" || true)"
    echo "recorded $N dirty path(s)"
    echo "denominator: $N dirty path(s) recorded at begin" >&2
    exit 0
fi

[ -f "$STATE" ] || rv_die 2 "no state file $STATE: run 'edit-scope.sh begin' first"
BEGIN_HEAD="$(sed -n '1s/^#head\t//p' "$STATE")"
if [ "$BEGIN_HEAD" != "$HEAD_SHA" ]; then
    echo "HEAD moved from $BEGIN_HEAD to $HEAD_SHA since begin: a commit hides edits from this check"
    echo "denominator: HEAD compared; 1 moved" >&2
    exit 1
fi

NOW="$(snapshot)"
BAD=0
CHANGED=0
SEEN=$'\n'
# comm -3: the lines only in the begin record (a path reverted, committed, or about
# to change) and only in the worktree now (added, or changed since). A path that
# appears on both sides changed once, whichever side said it first.
while IFS= read -r line; do
    line="${line#$'\t'}"
    [ -n "$line" ] || continue
    path="${line%%$'\t'*}"
    case "$SEEN" in *$'\n'"$path"$'\n'*) continue ;; esac
    SEEN="$SEEN$path"$'\n'
    CHANGED=$((CHANGED + 1))
    ok=no
    for a in "${ALLOW[@]+"${ALLOW[@]}"}"; do
        [ "$a" = "$path" ] && ok=yes
    done
    if [ "$ok" = no ]; then
        echo "outside the fix set: $path"
        BAD=$((BAD + 1))
    fi
done < <(LC_ALL=C comm -3 <(tail -n +2 "$STATE") <(printf '%s\n' "$NOW" | sed '/^$/d'))
NNOW="$(printf '%s\n' "$NOW" | grep -c . || true)"
echo "denominator: $NNOW dirty path(s) now, $CHANGED changed since begin, ${#ALLOW[@]} allowed; $BAD outside" >&2
[ "$BAD" -eq 0 ]
