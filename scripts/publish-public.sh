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
#   - origin/main is an ancestor of the release commit: a commit built over
#     origin/main from a tree that lacks something merged there would publish that
#     thing reverted. Otherwise exit 2, naming `git fetch origin && git merge
#     origin/main`; no commit object is written. See --bootstrap for the one
#     publish whose origin/main was built from a local commit and never merged back;
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
#   (--tag-only is the exception: its result is the local tag, see below.)
#
# --push
#   Checks that origin can be read and its main still equals the local
#   origin/main (so the parent is current), creates the annotated tag vX.Y.Z on the squash commit, and pushes
#   the commit to origin's main and the tag in one atomic push, never forced. A
#   push that fails or is interrupted deletes the tag this run created. Only the
#   owner runs --push.
#
# --bootstrap [--yes]
#   Records origin/main as an ancestor of HEAD with `git merge -s ours origin/main`,
#   which keeps HEAD's tree. Only sound when origin/main's tree is the tree of a
#   commit in HEAD's history, the one the published commit was built from; any
#   other origin/main holds content HEAD never had, and the script refuses (exit
#   2). Already an ancestor: says so, creates nothing, exit 0. Prints the merge by
#   default and runs it only with --yes. Either way it needs a branch (not a
#   detached HEAD), a clean tracked tree and a history shared with origin/main, and
#   refuses (exit 2) otherwise, so the dry run never promises what --yes refuses.
#   A merge a hook stops is aborted. It never pushes and takes no other option.
#
# --tag-only
#   For a release whose commit is already on origin/main (merged there by other
#   means): creates the annotated tag vX.Y.Z on origin/main, only when the release
#   tree equals origin/main's tree and the checks above (the version and the
#   CHANGELOG ones) hold. No commit is built, so no term gate runs: nothing is
#   published but the tag. The tag is created locally; with --push it is pushed
#   (the tag alone, never forced), and a push that fails or is interrupted deletes
#   the tag this run created.
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
#   bash scripts/publish-public.sh --version X.Y.Z --tag-only [--from REV] [--push]
#   bash scripts/publish-public.sh --bootstrap [--yes]
#
#   Run it from inside the repository. --terms and --allow are handed to
#   check-public-terms.sh unchanged (its own defaults otherwise).
#
# Exit codes:
#   0  dry run printed, the push succeeded, or --bootstrap / --tag-only did its
#      work (including "already recorded")
#   1  the term gate found a listed term in the message or the tree; nothing built
#   2  usage error, or a precondition above does not hold
#   3  the term gate could not run, or read a different number of files than
#      the message or the tree holds
#   4  the push failed; the tag this run created was deleted

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$HERE/check-public-terms.sh"
CORE="$HERE/lib/publish-core.sh"
REMOTE=origin
VERSION=""
FROM=""
PUSH=no
TAG_ONLY=no
BOOTSTRAP=no
YES=no
GATE_ARGS=()

usage() {
    sed -n '/^# Usage:/,/^#   check-public-terms.sh unchanged/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

# The shared checks and the commit builder; it defines die.
[ -f "$CORE" ] || { echo "publish-public.sh: cannot find $CORE" >&2; exit 2; }
# shellcheck source=lib/publish-core.sh
. "$CORE"

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
        --tag-only)
            TAG_ONLY=yes
            shift
            ;;
        --bootstrap)
            BOOTSTRAP=yes
            shift
            ;;
        --yes)
            YES=yes
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

if [ "$BOOTSTRAP" = yes ]; then
    # It merges into the current branch and publishes nothing: no release, no push.
    if [ -n "$VERSION" ] || [ -n "$FROM" ] || [ "$PUSH" = yes ] || [ "$TAG_ONLY" = yes ] || [ ${#GATE_ARGS[@]} -gt 0 ]; then
        die 2 "--bootstrap takes only --yes"
    fi
else
    [ "$YES" = no ] || die 2 "--yes belongs to --bootstrap"
    [ -n "$VERSION" ] || usage
    [[ "$VERSION" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]] || die 2 "--version must be MAJOR.MINOR.PATCH (got '$VERSION')"
fi
[ -f "$CHECK" ] || die 2 "cannot find $CHECK"
FROM="${FROM:-HEAD}"

# The repository is the one the caller stands in; nothing in the environment may
# point git somewhere else.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES
ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || die 2 "not inside a git repository"
cd "$ROOT"

TAG="v$VERSION"
RELEASE="$(git rev-parse --verify --quiet "$FROM^{commit}")" || die 2 "--from $FROM is not a commit"
TREE="$(git rev-parse "$RELEASE^{tree}")"
PARENT="$(git rev-parse --verify --quiet "refs/remotes/$REMOTE/main^{commit}")" \
    || die 2 "no $REMOTE/main: fetch $REMOTE first"

# --- --bootstrap --------------------------------------------------------------------
if [ "$BOOTSTRAP" = yes ]; then
    if git merge-base --is-ancestor "$PARENT" "$RELEASE"; then
        echo "publish-public.sh: already recorded: $REMOTE/main ($PARENT) is an ancestor of HEAD; no merge made"
        exit 0
    fi
    ORIGIN_TREE="$(git rev-parse "$PARENT^{tree}")"
    # The local commit origin's was built from: the newest one in HEAD's history
    # with origin's tree. Read whole, so a closed pipe cannot fail it.
    BUILT_FROM="$(git log --format='%H %T' "$RELEASE" | awk -v t="$ORIGIN_TREE" '$2 == t && !f { print $1; f = 1 }')"
    [ -n "$BUILT_FROM" ] \
        || die 2 "$REMOTE/main's tree $ORIGIN_TREE is the tree of no commit in HEAD's history: it holds content HEAD never had, and recording it as merged would discard it. Run: git fetch $REMOTE && git merge $REMOTE/main"
    # What --yes needs is checked in the dry run too, so the dry run does not
    # promise a merge --yes then refuses.
    git merge-base "$PARENT" "$RELEASE" >/dev/null 2>&1 \
        || die 2 "$REMOTE/main and HEAD share no history: git merge -s ours would refuse to join them"
    BRANCH="$(git symbolic-ref --quiet --short HEAD)" || die 2 "HEAD is detached: check out the branch to record the ancestry on"
    [ -z "$(git status --porcelain --untracked-files=no)" ] || die 2 "the working tree has uncommitted changes: commit or stash them first"
    echo "publish-public.sh: bootstrap"
    echo "  $REMOTE/main  $PARENT (tree $ORIGIN_TREE)"
    echo "  built from   $BUILT_FROM (same tree, in HEAD's history)"
    echo
    echo "merge: git merge -s ours $REMOTE/main"
    if [ "$YES" = no ]; then
        echo
        echo "dry run: nothing was merged. Re-run with --yes to record the ancestry."
        exit 0
    fi
    # A merge a hook stops after it was started leaves MERGE_HEAD behind, and the
    # next merge is then refused; put the branch back as it was.
    if ! GIT_MERGE_AUTOEDIT=no git merge -s ours "$REMOTE/main"; then
        if [ -e "$(git rev-parse --git-path MERGE_HEAD)" ]; then git merge --abort >/dev/null 2>&1 || true; fi
        die 2 "the merge failed; $BRANCH is as it was"
    fi
    echo
    echo "merged: $BRANCH is now $(git rev-parse HEAD), with $REMOTE/main as an ancestor and its tree unchanged. Nothing was pushed."
    exit 0
fi

# --- the preconditions ------------------------------------------------------------
publish_require_merged "$RELEASE"
# A --tag-only run without --push keeps its tag, on origin/main's tip: left to
# publish_newest_version it would read as origin's newest version and the re-run
# would be refused as "not newer".
if [ "$TAG_ONLY" = yes ] && [ "$(git rev-parse --verify --quiet "refs/tags/$TAG^{commit}")" = "$PARENT" ]; then
    die 2 "tag $TAG already exists locally (a --tag-only run without --push keeps it); push it with: git push $REMOTE refs/tags/$TAG:refs/tags/$TAG"
fi
publish_newest_version
publish_check_version

# Scratch files live under this repository's git directory, so nothing appears in
# the working tree and nothing is read from it.
SCRATCH="$(git rev-parse --absolute-git-dir)/publish-public.$$"
OBJECTS="$(cd "$(git rev-parse --git-common-dir)" && pwd -P)/objects"
# The EXIT trap also undoes this run's tag if the push did not complete, whether
# the push failed or the run was interrupted during it (INT/TERM exit through it).
# A --tag-only run without --push keeps the tag it made: that is its result.
TAG_MADE=no
PUSHED=no
cleanup() {
    rm -rf "$SCRATCH"
    if [ "$TAG_MADE" = yes ] && [ "$PUSHED" = no ] && [ "$PUSH" = yes ]; then
        # A signal that arrives while the push runs is acted on when the push
        # returns, so the push may have landed: the tag then stays.
        if git ls-remote "$REMOTE" "refs/tags/$TAG" 2>/dev/null | grep -q .; then
            echo "publish-public.sh: interrupted after the push landed: $REMOTE has $TAG; kept the local tag" >&2
        else
            git tag -d "$TAG" >/dev/null 2>&1 || true
            echo "publish-public.sh: the push did not complete; deleted the local tag $TAG this run created" >&2
        fi
    fi
}
trap cleanup EXIT
trap 'exit 130' INT TERM
rm -rf "$SCRATCH"
mkdir -p "$SCRATCH"

if [ "$TAG_ONLY" = yes ]; then
    # --- --tag-only ------------------------------------------------------------------
    ORIGIN_TREE="$(git rev-parse "$PARENT^{tree}")"
    [ "$TREE" = "$ORIGIN_TREE" ] \
        || die 2 "the release tree $TREE is not $REMOTE/main's tree $ORIGIN_TREE: --tag-only tags what origin already holds"
    publish_changelog_message
    SQUASH="$PARENT"
    TAG_CMD=(git tag -a "$TAG" -m "logos $VERSION" "$SQUASH")
    PUSH_CMD=(git push "$REMOTE" "refs/tags/$TAG:refs/tags/$TAG")
    echo "publish-public.sh: $TAG on $REMOTE/main"
    echo "  release  $RELEASE (tree $TREE, the tree of $REMOTE/main)"
    echo "  tagged   $SQUASH ($REMOTE/main, newest published v$NEWEST)"
else
    publish_build_commit
    TAG_CMD=(git tag -a "$TAG" -m "logos $VERSION" "$SQUASH")
    PUSH_CMD=(git push --atomic "$REMOTE" "$SQUASH:refs/heads/main" "refs/tags/$TAG:refs/tags/$TAG")
    echo "  squash   $SQUASH"
fi
echo
echo "tag:  $(quoted "${TAG_CMD[@]}")"
echo "push: $(quoted "${PUSH_CMD[@]}")"

if [ "$PUSH" = no ]; then
    echo
    if [ "$TAG_ONLY" = yes ]; then
        "${TAG_CMD[@]}"
        TAG_MADE=yes
        echo "created the local tag $TAG on $SQUASH; nothing was pushed."
        echo "Run the push above to publish it."
    else
        echo "dry run: the commit above is unreferenced; no tag, branch or remote was touched."
        echo "Re-run with --push to tag it and push."
    fi
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
