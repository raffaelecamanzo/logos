#!/usr/bin/env bash
#
# test-release-tag-guard.sh — prove scripts/release-tag-guard.sh lets a tag be
# released only when it is a version tag, is what the checkout holds, and has no
# GitHub Release yet (S-732).
#
# Nothing here talks to GitHub. The guard runs `$GH api --paginate
# repos/OWNER/REPO/releases… --jq '.[].tag_name'`; GH points at a stub that
# records its arguments and prints a stubbed release listing (or fails, as the
# API does). Each case runs in a throwaway repository under $WORK, with
# GIT_CEILING_DIRECTORIES set so no git command can walk up into this one.
#
# Usage: bash scripts/tests/test-release-tag-guard.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GUARD="$HERE/scripts/release-tag-guard.sh"
[ -f "$GUARD" ] || { echo "cannot find scripts/release-tag-guard.sh (looked in $HERE)" >&2; exit 1; }

WORK="${TMPDIR:-/tmp}/logos-release-tag-guard-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK/bin" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"
G=(git -c maintenance.auto=false -c gc.auto=0)

# The stub: logs its argv one word per line, then answers from $STUB_LISTING
# (exit 0) or fails with $STUB_ERROR (exit 1).
cat >"$WORK/bin/gh" <<'EOF'
#!/usr/bin/env bash
printf '%s\n' "$@" >"$STUB_LOG"
if [ -n "${STUB_ERROR:-}" ]; then
    echo "$STUB_ERROR" >&2
    exit 1
fi
cat "$STUB_LISTING"
EOF
chmod +x "$WORK/bin/gh"
export GH="$WORK/bin/gh" STUB_LOG="$WORK/gh.log" STUB_LISTING="$WORK/listing.txt"

# The repository: two commits, v1.16.1 (lightweight) on the first and v1.16.2
# (annotated, as scripts/publish-public.sh creates it) on the second.
REPO="$WORK/repo"
mkdir -p "$REPO"
"${G[@]}" -C "$REPO" init -q . || exit 1
if [ "$(cd "$REPO" && git rev-parse --show-toplevel)" != "$REPO" ]; then
    echo "could not create the fixture repository $REPO" >&2
    exit 1
fi
"${G[@]}" -C "$REPO" config user.email t@example.com
"${G[@]}" -C "$REPO" config user.name test
echo one >"$REPO/f" && "${G[@]}" -C "$REPO" add f && "${G[@]}" -C "$REPO" commit -qm one
"${G[@]}" -C "$REPO" tag v1.16.1
echo two >"$REPO/f" && "${G[@]}" -C "$REPO" commit -qam two
"${G[@]}" -C "$REPO" tag -a -m 'v1.16.2' v1.16.2
TAG_COMMIT="$("${G[@]}" -C "$REPO" rev-parse 'v1.16.2^{commit}')"
OLD_COMMIT="$("${G[@]}" -C "$REPO" rev-parse 'v1.16.1^{commit}')"

PASS=0
FAIL=0

# guard <listing> <arg>... -> sets OUT, RC, GHOUT (what was appended to
# $GITHUB_OUTPUT) and CALLED (yes when the stub ran). The listing is the stub's
# answer, one tag per line; STUB_ERROR in the environment makes it fail instead.
guard() {
    local listing="$1"
    shift
    printf '%s' "$listing" >"$STUB_LISTING"
    rm -f "$STUB_LOG" "$WORK/github-output"
    : >"$WORK/github-output"
    OUT="$(cd "$REPO" && GITHUB_OUTPUT="$WORK/github-output" GITHUB_REPOSITORY=owner/logos \
        bash "$GUARD" "$@" 2>&1)"
    RC=$?
    GHOUT="$(cat "$WORK/github-output")"
    if [ -f "$STUB_LOG" ]; then CALLED=yes; else CALLED=no; fi
}

ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() {
    echo "  FAIL  $1"
    printf '%s\n' "$OUT" | sed 's/^/        /'
    FAIL=$((FAIL + 1))
}

# expect <name> <wanted-exit> [<text the output must contain>]
expect() {
    if [ "$RC" -ne "$2" ]; then bad "$1: exit $RC, wanted $2"; return; fi
    if [ -n "${3:-}" ] && ! printf '%s\n' "$OUT" | grep -qF -- "$3"; then
        bad "$1: exit $RC correct, but the output does not contain '$3'"
        return
    fi
    ok "$1"
}

# refused <name> <text>: exit 1, the reason, and nothing written for later jobs.
refused() {
    expect "$1" 1 "$2"
    if [ "$RC" -eq 1 ] && [ -n "$GHOUT" ]; then bad "$1: a refusal still wrote '$GHOUT' to GITHUB_OUTPUT"; fi
}

checkout() { "${G[@]}" -C "$REPO" checkout -q "$1"; }

echo "test-release-tag-guard.sh"
echo "  work $WORK"
echo

# --- a tag with no release may be released -------------------------------------

checkout v1.16.2
guard "$(printf 'v1.15.8\nv1.15.7\n')" v1.16.2
expect "a tag the listing does not name may be released" 0 "release-tag-guard: v1.16.2 ($TAG_COMMIT) has no GitHub Release in owner/logos"
if [ "$GHOUT" = "$(printf 'tag=v1.16.2\ncommit=%s' "$TAG_COMMIT")" ]; then
    ok "it hands the tag and the tag's commit to the later jobs"
else
    bad "GITHUB_OUTPUT holds '$GHOUT'"
fi
want_argv="$(printf '%s\n' api --paginate 'repos/owner/logos/releases?per_page=100' --jq '.[].tag_name')"
if [ "$(cat "$STUB_LOG" 2>/dev/null)" = "$want_argv" ]; then
    ok "the listing is every release of the repository, page by page"
else
    bad "the stub was called as: $(tr '\n' ' ' <"$STUB_LOG" 2>/dev/null)"
fi

guard "" v1.16.2
expect "a repository with no release at all may release the tag" 0

guard "$(printf 'v1.15.8\n')" --repo other/repo v1.16.2
expect "--repo names the repository instead of GITHUB_REPOSITORY" 0 "has no GitHub Release in other/repo"

checkout v1.16.1
guard "" v1.16.1
expect "a lightweight tag is resolved to its commit like an annotated one" 0 "v1.16.1 ($OLD_COMMIT)"

# --- a tag that already has a release is refused before anything else ----------

checkout v1.16.2
guard "$(printf 'v1.16.3\nv1.16.2\nv1.15.8\n')" v1.16.2
refused "a tag the listing names is refused" "owner/logos already has a GitHub Release for v1.16.2"

# Near misses: a longer tag, a prerelease of the same version, padded and
# substring forms. None of them is the tag, so none of them refuses it.
guard "$(printf 'v1.16.20\nv1.16.2-rc.1\nxv1.16.2\n v1.16.2\nv1.16.2 \nv1.16\n')" v1.16.2
expect "a listing naming only near misses of the tag lets it through" 0

STUB_ERROR="HTTP 401: Bad credentials" guard "" v1.16.2
refused "a listing that fails is a refusal, never a pass" "cannot list the releases of owner/logos"

# --- the tag must be a version tag, and the one the checkout holds ---------------

# shellcheck disable=SC2016  # '$(id)' is a literal: a tag that tries to expand.
# The prefixed forms are valid git tag names the push trigger's glob matches
# (`git check-ref-format` accepts them): only the leading anchor refuses them, and
# the guard's output is what later jobs interpolate into `run:` lines.
for t in v1.16 1.16.2 'v1.16.2;id' 'v1.16.2 x' 'v1.16.2-' 'v1.16.2-a b' '$(id)' v01.16.2x \
    ';id;v1.16.2' '$(id)v1.16.2' '`id`v1.16.2' xv1.16.2; do
    guard "" "$t"
    if [ "$RC" -eq 1 ] && [ "$CALLED" = no ] && printf '%s\n' "$OUT" | grep -qF "is not a release tag"; then
        ok "the shape '$t' is refused before the listing is read"
    else
        bad "the shape '$t': exit $RC, listing read: $CALLED"
    fi
done

# A newline would let the tag write a second key into GITHUB_OUTPUT.
guard "" "$(printf 'v1.16.2\ncommit=0000')"
if [ "$RC" -eq 1 ] && [ "$CALLED" = no ] && [ -z "$GHOUT" ]; then
    ok "a tag with an embedded newline is refused and writes nothing"
else
    bad "a tag with an embedded newline: exit $RC, output '$GHOUT'"
fi

guard "" v1.16.2-rc.1
refused "a well-shaped prerelease tag the checkout lacks is refused" "the checkout has no tag v1.16.2-rc.1"

guard "" v9.9.9
refused "a tag the repository does not have is refused" "the checkout has no tag v9.9.9"
if [ "$CALLED" = no ]; then ok "  ...without reading the listing"; else bad "the listing was read for a missing tag"; fi

# A branch with a version-shaped name is not a tag, even when HEAD is on it.
"${G[@]}" -C "$REPO" branch v9.9.8 v1.16.2
checkout v9.9.8
guard "" v9.9.8
refused "a branch named like a version is not a tag" "the checkout has no tag v9.9.8"

checkout v1.16.1
guard "" v1.16.2
refused "a checkout that is not the tag's commit is refused" "the checkout is $OLD_COMMIT, not v1.16.2 ($TAG_COMMIT)"

# --- --commit: the tag must still be the commit the run validated -------------

checkout v1.16.2
guard "" --commit "$TAG_COMMIT" v1.16.2
expect "--commit naming the tag's commit lets it through" 0

# The tag moved after `plan` validated it: the host job's checkout of the tag and
# its HEAD agree with each other, so only --commit can see the move.
guard "" --commit "$OLD_COMMIT" v1.16.2
refused "a tag that moved away from the validated commit is refused" "v1.16.2 now points at $TAG_COMMIT, not $OLD_COMMIT"

guard "" --commit "${TAG_COMMIT:0:12}" v1.16.2
expect "--commit that is not a full commit id is a usage error" 2 "not a full commit id"

# --- usage ---------------------------------------------------------------------

checkout v1.16.2
guard ""
expect "no tag is a usage error" 2 "usage:"
guard "" v1.16.2 v1.16.1
expect "two tags are a usage error" 2 "usage:"
guard "" --bogus v1.16.2
expect "an unknown option is a usage error" 2 "unknown option: --bogus"
OUT="$(cd "$REPO" && env -u GITHUB_REPOSITORY bash "$GUARD" v1.16.2 2>&1)"
RC=$?
expect "no repository is a usage error" 2 "no OWNER/REPO"

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
