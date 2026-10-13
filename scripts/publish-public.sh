#!/usr/bin/env bash
#
# publish-public.sh — publish one release to origin as a single commit over
# origin/main, whatever local history led to it (NFR-CR-04).
#
# WHAT IT BUILDS
#   One commit whose parent is origin/main, whose tree is the release tree (the
#   tree of --from, HEAD by default), and whose message is every CHANGELOG.md
#   section origin has not seen yet: from `## [X.Y.Z]` down to, and excluding,
#   the section of the newest version already tagged on origin, newest first.
#   The newest version on origin is the highest vMAJOR.MINOR.PATCH tag reachable
#   from origin/main. The commit carries the release commit's author and
#   committer dates, so a dry run and the later --push build the same commit id.
#
# WHAT IT CHECKS FIRST
#   - X.Y.Z is newer than origin's newest version, and the release tree's
#     Cargo.toml carries version X.Y.Z (the tag names the product version, and a
#     tag the workspace version does not match would name a different one);
#   - CHANGELOG.md in the release tree has a `## [X.Y.Z]` section and a section
#     for origin's newest version below it;
#   - no local tag vX.Y.Z exists (creating it would move an existing tag);
#   - scripts/check-public-terms.sh passes on the message, then on the release
#     tree, exactly as it would be published (read from the commit, never from the
#     working directory). A hit aborts before any commit object is written.
#
# DRY RUN BY DEFAULT
#   Without --push the script writes one unreferenced commit object and nothing
#   else: no tag, no branch, no remote call. It prints the commit id and the exact
#   `git tag` and `git push` it would run. Local main, every existing tag, local
#   history and every other remote are never moved or rewritten, in either mode.
#
# --push
#   Checks that origin can be read and its main still equals the local
#   origin/main (so the parent is current), creates the annotated tag vX.Y.Z on the squash commit, and pushes
#   the commit to origin's main and the tag in one atomic push, never forced. A
#   push that fails or is interrupted deletes the tag this run created. Only the
#   owner runs --push.
#
# WHAT A PUBLIC TAG IS
#   A source release. No workflow in this repository runs on a pushed tag, builds
#   a binary, creates a GitHub Release or updates a package tap: the tag marks the
#   squash commit, and the commit is what a build pins. Binaries are built and
#   published elsewhere, from a pin of this tag.
#
# Usage:
#   bash scripts/publish-public.sh --version X.Y.Z [--from REV] [--push]
#                                  [--terms FILE] [--allow FILE]
#
#   Run it from inside the repository. --terms and --allow are handed to
#   check-public-terms.sh unchanged (its own defaults otherwise).
#
# Exit codes:
#   0  dry run printed, or the push succeeded
#   1  the term gate found a listed term in the message or the tree; nothing built
#   2  usage error, or a precondition above does not hold
#   3  the term gate could not run, or read a different number of files than
#      the message or the tree holds
#   4  the push failed; the tag this run created was deleted

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$HERE/check-public-terms.sh"
REMOTE=origin
VERSION=""
FROM=HEAD
PUSH=no
GATE_ARGS=()

usage() {
    sed -n '/^# Usage:/,/^#   check-public-terms.sh unchanged/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 2
}
die() { # die <exit> <message>
    echo "publish-public.sh: $2" >&2
    exit "$1"
}

while [ $# -gt 0 ]; do
    case "$1" in
        --version)
            [ $# -ge 2 ] || usage
            VERSION="$2"
            shift 2
            ;;
        --from)
            [ $# -ge 2 ] || usage
            FROM="$2"
            shift 2
            ;;
        --push)
            PUSH=yes
            shift
            ;;
        --terms | --allow)
            [ $# -ge 2 ] || usage
            GATE_ARGS+=("$1" "$2")
            shift 2
            ;;
        -h | --help) usage ;;
        *)
            echo "publish-public.sh: unknown argument: $1" >&2
            usage
            ;;
    esac
done

[ -n "$VERSION" ] || usage
[[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die 2 "--version must be MAJOR.MINOR.PATCH (got '$VERSION')"
[ -f "$CHECK" ] || die 2 "cannot find $CHECK"

# The repository is the one the caller stands in; nothing in the environment may
# point git somewhere else.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES
ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || die 2 "not inside a git repository"
cd "$ROOT"

# version_gt A B: true when MAJOR.MINOR.PATCH A is newer than B.
version_gt() {
    local a1 a2 a3 b1 b2 b3
    IFS=. read -r a1 a2 a3 <<<"$1"
    IFS=. read -r b1 b2 b3 <<<"$2"
    [ "$a1" -ne "$b1" ] && { [ "$a1" -gt "$b1" ]; return; }
    [ "$a2" -ne "$b2" ] && { [ "$a2" -gt "$b2" ]; return; }
    [ "$a3" -gt "$b3" ]
}

TAG="v$VERSION"
RELEASE="$(git rev-parse --verify --quiet "$FROM^{commit}")" || die 2 "--from $FROM is not a commit"
TREE="$(git rev-parse "$RELEASE^{tree}")"
PARENT="$(git rev-parse --verify --quiet "refs/remotes/$REMOTE/main^{commit}")" \
    || die 2 "no $REMOTE/main: fetch $REMOTE first"

# --- origin's newest version -----------------------------------------------------
NEWEST=""
while IFS= read -r t; do
    [[ "$t" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]] || continue
    v="${BASH_REMATCH[1]}"
    if [ -z "$NEWEST" ] || version_gt "$v" "$NEWEST"; then NEWEST="$v"; fi
done < <(git tag --merged "$PARENT" --list 'v*')
[ -n "$NEWEST" ] || die 2 "no vMAJOR.MINOR.PATCH tag is reachable from $REMOTE/main: nothing marks what is already published"
version_gt "$VERSION" "$NEWEST" || die 2 "$VERSION is not newer than v$NEWEST, the newest version on $REMOTE"

if git rev-parse --verify --quiet "refs/tags/$TAG" >/dev/null; then
    die 2 "tag $TAG already exists locally; publishing would move it"
fi

# --- the release tree carries the version --------------------------------------
CARGO_VERSION="$(git show "$RELEASE:Cargo.toml" 2>/dev/null | awk '
    /^\[workspace\.package\]/ { f = 1; next }
    /^\[/ { f = 0 }
    f && /^version *=/ { gsub(/[" ]/, ""); sub(/version=/, ""); print; exit }')" || true
[ "$CARGO_VERSION" = "$VERSION" ] \
    || die 2 "the release tree's Cargo.toml [workspace.package] version is '${CARGO_VERSION}', not $VERSION"

# Scratch files live under this repository's git directory, so nothing appears in
# the working tree and nothing is read from it.
SCRATCH="$(git rev-parse --absolute-git-dir)/publish-public.$$"
OBJECTS="$(cd "$(git rev-parse --git-common-dir)" && pwd -P)/objects"
# The EXIT trap also undoes this run's tag if the push did not complete, whether
# the push failed or the run was interrupted during it (INT/TERM exit through it).
TAG_MADE=no
PUSHED=no
cleanup() {
    rm -rf "$SCRATCH"
    if [ "$TAG_MADE" = yes ] && [ "$PUSHED" = no ]; then
        git tag -d "$TAG" >/dev/null 2>&1 || true
        echo "publish-public.sh: the push did not complete; deleted the local tag $TAG this run created" >&2
    fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
rm -rf "$SCRATCH"
mkdir -p "$SCRATCH"

# --- the message: every section origin has not seen ----------------------------
git show "$RELEASE:CHANGELOG.md" >"$SCRATCH/CHANGELOG.md" 2>/dev/null \
    || die 2 "the release tree has no CHANGELOG.md"
# Prints the sections from `## [VERSION]` up to, not including, `## [NEWEST]`;
# exits 3 when the first is missing, 4 when the second is, 5 when a section in
# between is not a version strictly between the two (the file is newest first).
SECTIONS_RC=0
MESSAGE="$(awk -v want="$VERSION" -v stop="$NEWEST" '
    function header(line,   v) {
        if (line !~ /^## \[/) return ""
        v = line; sub(/^## \[/, "", v); sub(/\].*/, "", v); return v
    }
    function newer(a, b,   x, y, i) {
        split(a, x, "."); split(b, y, ".")
        for (i = 1; i <= 3; i++) if (x[i] + 0 != y[i] + 0) return x[i] + 0 > y[i] + 0
        return 0
    }
    {
        v = header($0)
        if (!on) { if (v == want) on = 1; else next }
        else if (v != "") {
            if (v == stop) { done = 1; exit }
            if (v !~ /^[0-9]+\.[0-9]+\.[0-9]+$/ || !newer(want, v) || !newer(v, stop)) { bad = v; exit }
        }
        print
    }
    END {
        if (!on) exit 3
        if (bad != "") { print bad > "/dev/stderr"; exit 5 }
        if (!done) exit 4
    }' "$SCRATCH/CHANGELOG.md")" || SECTIONS_RC=$?
case "$SECTIONS_RC" in
    0) ;;
    3) die 2 "CHANGELOG.md has no ## [$VERSION] section" ;;
    4) die 2 "CHANGELOG.md has no ## [$NEWEST] section below ## [$VERSION]: cannot tell where unpublished history ends" ;;
    5) die 2 "CHANGELOG.md is out of order: a section between [$VERSION] and [$NEWEST] is not a version between them" ;;
    *) die 2 "could not read the sections of CHANGELOG.md (awk exited $SECTIONS_RC)" ;;
esac
# $(...) already dropped the trailing newlines; drop trailing blank lines too.
MESSAGE="$(printf '%s\n' "$MESSAGE" | sed -e :a -e '/^[[:space:]]*$/{$d;N;ba' -e '}')"
[ -n "$MESSAGE" ] || die 2 "the CHANGELOG sections for $VERSION are empty"

# --- the term gate ------------------------------------------------------------------
# gate <label> <dir> <expected-files>: run check-public-terms.sh on <dir> and
# insist it read exactly the files the input holds, all of them tracked.
gate() {
    local label="$1" dir="$2" expected="$3" out rc summary
    rc=0
    out="$(bash "$CHECK" --root "$dir" ${GATE_ARGS[@]+"${GATE_ARGS[@]}"} 2>&1)" || rc=$?
    summary="$(printf '%s\n' "$out" | tail -n 1)"
    if [ "$rc" -eq 1 ]; then
        echo "publish-public.sh: the $label carries a listed term; nothing was built:" >&2
        printf '%s\n' "$out" | sed 's/^/    /' >&2
        exit 1
    fi
    if [ "$rc" -ne 0 ]; then
        echo "publish-public.sh: the term gate could not check the $label (exit $rc):" >&2
        printf '%s\n' "$out" | sed 's/^/    /' >&2
        exit 3
    fi
    if [[ ! "$summary" =~ ^public-terms:\ tracked=([0-9]+)\ untracked=0\  ]] || [ "${BASH_REMATCH[1]}" -ne "$expected" ]; then
        echo "publish-public.sh: the term gate did not walk the $label's $expected file(s): $summary" >&2
        exit 3
    fi
    echo "  $label: $summary"
}

echo "publish-public.sh: $TAG over $REMOTE/main"
echo "  release  $RELEASE (tree $TREE)"
echo "  parent   $PARENT ($REMOTE/main, newest published v$NEWEST)"
echo "  sections $(printf '%s\n' "$MESSAGE" | sed -n 's/^## \[\([^]]*\)\].*/\1/p' | paste -sd ' ' -)"

# The message, as the only file of a repository: the scanner keys its allowlist
# on path and line, and these are lines of CHANGELOG.md, so it carries that name.
git init -q "$SCRATCH/message"
printf '%s\n' "$MESSAGE" >"$SCRATCH/message/CHANGELOG.md"
git -C "$SCRATCH/message" add CHANGELOG.md
gate message "$SCRATCH/message" 1

# The release tree, checked out from the commit through an alternate object
# store: no object is copied, and the index lists exactly the tree's entries.
# info/attributes outranks every .gitattributes and the global attributes file:
# it switches off each attribute that rewrites a file on checkout (an encoding
# turns UTF-8 into UTF-16 the scanner would skip as binary; a filter or `ident`
# rewrites text), so the scanner reads the bytes the commit publishes.
# A checkout that fails (two paths differing only in case, on a case-insensitive
# filesystem) is reported as such: under set -e it would exit 1, which reads as
# a term hit.
if ! {
    git init -q "$SCRATCH/tree" &&
        printf '%s\n' "$OBJECTS" >"$SCRATCH/tree/.git/objects/info/alternates" &&
        printf '* -text -eol -filter -ident -working-tree-encoding\n' >"$SCRATCH/tree/.git/info/attributes" &&
        git -C "$SCRATCH/tree" read-tree "$TREE" &&
        git -C "$SCRATCH/tree" checkout-index -a
} 2>"$SCRATCH/export.err"; then
    sed 's/^/    /' "$SCRATCH/export.err" >&2
    die 3 "could not check out the release tree to scan it"
fi
gate "release tree" "$SCRATCH/tree" "$(git ls-tree -r "$TREE" | wc -l | tr -d ' ')"

# --- the squash commit ----------------------------------------------------------------
AUTHOR_DATE="$(git log -1 --format=%ad --date=raw "$RELEASE")"
COMMITTER_DATE="$(git log -1 --format=%cd --date=raw "$RELEASE")"
printf '%s\n' "$MESSAGE" >"$SCRATCH/message.txt"
SQUASH="$(GIT_AUTHOR_DATE="$AUTHOR_DATE" GIT_COMMITTER_DATE="$COMMITTER_DATE" \
    git commit-tree -p "$PARENT" -F "$SCRATCH/message.txt" "$TREE")"

TAG_CMD=(git tag -a "$TAG" -m "logos $VERSION" "$SQUASH")
PUSH_CMD=(git push --atomic "$REMOTE" "$SQUASH:refs/heads/main" "refs/tags/$TAG:refs/tags/$TAG")
# Each command printed as one line a shell reads back as the same words.
quoted() {
    local q
    q="$(printf '%q ' "$@")"
    printf '%s\n' "${q% }"
}
echo "  squash   $SQUASH"
echo
echo "tag:  $(quoted "${TAG_CMD[@]}")"
echo "push: $(quoted "${PUSH_CMD[@]}")"

if [ "$PUSH" = no ]; then
    echo
    echo "dry run: the commit above is unreferenced; no tag, branch or remote was touched."
    echo "Re-run with --push to tag it and push."
    exit 0
fi

# --- --push ---------------------------------------------------------------------------
# Both reads are checked: an unreachable remote is a failed precondition (exit
# 2), never git's own status.
REMOTE_MAIN="$(git ls-remote "$REMOTE" refs/heads/main)" || die 2 "cannot read $REMOTE's refs"
REMOTE_MAIN="${REMOTE_MAIN%%[[:space:]]*}"
[ "$REMOTE_MAIN" = "$PARENT" ] \
    || die 2 "$REMOTE's main is ${REMOTE_MAIN:-absent}, not the local $REMOTE/main $PARENT: fetch $REMOTE and re-run"
REMOTE_TAG="$(git ls-remote "$REMOTE" "refs/tags/$TAG")" || die 2 "cannot read $REMOTE's tags"
[ -z "$REMOTE_TAG" ] || die 2 "$REMOTE already has tag $TAG"
TAG_MADE=yes
"${TAG_CMD[@]}"
"${PUSH_CMD[@]}" || die 4 "the push failed"
PUSHED=yes
echo "pushed $TAG ($SQUASH) to $REMOTE"
