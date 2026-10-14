#!/usr/bin/env bash
#
# release-tag-guard.sh — may this tag be released? (S-732)
#
#   GH_TOKEN=… bash scripts/release-tag-guard.sh [--repo OWNER/REPO] [--commit SHA] <tag>
#
# The first step of .github/workflows/release.yml whenever a run publishes (a
# pushed release tag, or a manual dispatch naming an existing one). It runs after
# the checkout and before dist is installed, anything is built or any artifact is
# uploaded, and the `host` job runs it again just before the GitHub Release is
# created. Every check fails closed:
#
# 1. The tag is shaped like a version tag: vMAJOR.MINOR.PATCH, optionally with a
#    `-` suffix of letters, digits, dots and hyphens. Every later job reads the
#    tag from this script's output, so nothing else reaches a later `run:` line.
# 2. The checkout holds that tag and HEAD is the tag's commit: a dispatch builds
#    the tag it names, never the branch it was dispatched from. With --commit
#    (the `host` job passes the commit `plan` validated, which every build job
#    checked out), the tag must still point at that commit: a tag moved while the
#    run was building is refused rather than released over different artifacts.
# 3. The repository has no GitHub Release for the tag yet, drafts included. A tag
#    that already has one keeps it: whoever released it first owns its names. The
#    listing is every release the API returns, page by page; a listing that
#    fails is a refusal, never a pass.
#
# With $GITHUB_OUTPUT set, appends `tag=<tag>` and `commit=<sha>` to it.
#
# Needs: git, gh (authenticated by GH_TOKEN). $GH names the gh binary to run
# (scripts/tests/test-release-tag-guard.sh points it at a stub).
# Exit: 0 the tag may be released; 1 it may not (the reason on stderr); 2 usage.

set -euo pipefail

GH="${GH:-gh}"
REPO="${GITHUB_REPOSITORY:-}"
TAG=""
EXPECT=""

usage() {
    echo "usage: bash scripts/release-tag-guard.sh [--repo OWNER/REPO] [--commit SHA] <tag>" >&2
    exit 2
}
refuse() {
    echo "::error::release-tag-guard: $*" >&2
    exit 1
}

while [ $# -gt 0 ]; do
    case "$1" in
        --repo)
            [ $# -ge 2 ] || usage
            REPO="$2"
            shift 2
            ;;
        --commit)
            [ $# -ge 2 ] || usage
            EXPECT="$2"
            shift 2
            ;;
        -h | --help) usage ;;
        -*)
            echo "release-tag-guard.sh: unknown option: $1" >&2
            usage
            ;;
        *)
            [ -z "$TAG" ] || usage
            TAG="$1"
            shift
            ;;
    esac
done
[ -n "$TAG" ] || usage
[[ "$REPO" =~ ^[A-Za-z0-9_.-]+/[A-Za-z0-9_.-]+$ ]] ||
    { echo "release-tag-guard.sh: no OWNER/REPO (--repo or GITHUB_REPOSITORY): '${REPO}'" >&2; exit 2; }
[ -z "$EXPECT" ] || [[ "$EXPECT" =~ ^[0-9a-f]{40}$ ]] ||
    { echo "release-tag-guard.sh: --commit is not a full commit id: '${EXPECT}'" >&2; exit 2; }

# 1. The shape.
[[ "$TAG" =~ ^v[0-9]+\.[0-9]+\.[0-9]+(-[0-9A-Za-z.-]+)?$ ]] ||
    refuse "'$TAG' is not a release tag (vMAJOR.MINOR.PATCH, optionally -SUFFIX). Nothing was built."

# 2. The checkout is the tag.
tag_commit="$(git rev-parse -q --verify "refs/tags/$TAG^{commit}" 2>/dev/null)" ||
    refuse "the checkout has no tag $TAG: a dispatch must name a tag that already exists. Nothing was built."
head_commit="$(git rev-parse -q --verify 'HEAD^{commit}' 2>/dev/null)" ||
    refuse "the checkout has no HEAD commit. Nothing was built."
[ "$tag_commit" = "$head_commit" ] ||
    refuse "the checkout is $head_commit, not $TAG ($tag_commit): the run would build something other than the tag. Nothing was built."
[ -z "$EXPECT" ] || [ "$tag_commit" = "$EXPECT" ] ||
    refuse "$TAG now points at $tag_commit, not $EXPECT, the commit this run validated and built: the tag moved during the run. Nothing was released."

# 3. No release yet.
listing="$("$GH" api --paginate "repos/$REPO/releases?per_page=100" --jq '.[].tag_name' 2>&1)" ||
    refuse "cannot list the releases of $REPO, so cannot tell whether $TAG has one: $listing"
if printf '%s\n' "$listing" | grep -Fxq -- "$TAG"; then
    refuse "$REPO already has a GitHub Release for $TAG: a released tag is never released again. Nothing was built."
fi

if [ -n "${GITHUB_OUTPUT:-}" ]; then
    printf 'tag=%s\ncommit=%s\n' "$TAG" "$tag_commit" >>"$GITHUB_OUTPUT"
fi
echo "release-tag-guard: $TAG ($tag_commit) has no GitHub Release in $REPO; it may be released"
