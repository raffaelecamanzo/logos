#!/usr/bin/env bash
#
# publish-core.sh — the checks and the commit builder that scripts/publish-public.sh
# and scripts/publish-sprint.sh share, for a tree published as one commit over
# origin/main (NFR-CR-04).
# Sourced, never run: it defines functions and runs nothing.
#
# The caller sets these variables before calling anything here:
#   REMOTE RELEASE TREE PARENT               the commit published: RELEASE's tree
#                                            over PARENT (origin/main)
#   VERSION TAG                              the release version (publish-public.sh)
#   MESSAGE_FILE                             optional; see publish_commit_message
#   CHECK GATE_ARGS                          the term scanner and its options
#   SCRATCH OBJECTS                          a private directory under the git
#                                            directory; the object store
# and receives NEWEST, MESSAGE and SQUASH from the functions that compute them.
# `quoted` prints a command as one line a shell reads back as the same words.
#
# Every refusal is `die <exit> <message>`: exit 2 for a failed precondition,
# 1 for a term hit, 3 for a term gate that could not run (the contract of both
# scripts).

die() { # die <exit> <message>
    echo "${PUBLISH_PROG:-publish-public.sh}: $2" >&2
    exit "$1"
}

# version_gt A B: true when MAJOR.MINOR.PATCH A is newer than B.
version_gt() {
    local a1 a2 a3 b1 b2 b3
    IFS=. read -r a1 a2 a3 <<<"$1"
    IFS=. read -r b1 b2 b3 <<<"$2"
    [ "$a1" -ne "$b1" ] && { [ "$a1" -gt "$b1" ]; return; }
    [ "$a2" -ne "$b2" ] && { [ "$a2" -gt "$b2" ]; return; }
    [ "$a3" -gt "$b3" ]
}

# publish_require_merged <rev>: origin/main must be an ancestor of <rev>. A commit
# built over origin/main from a tree that lacks a change merged there would
# publish that change reverted, so the change has to be merged locally first.
# Runs before any object is written.
publish_require_merged() {
    git merge-base --is-ancestor "$PARENT" "$1" 2>/dev/null \
        || die 2 "$REMOTE/main ($PARENT) is not an ancestor of $1: a publish would revert what $REMOTE/main holds and $1 lacks. Run: git fetch $REMOTE && git merge $REMOTE/main  (if $REMOTE/main is the commit an earlier publish built from this tree, record it with: bash scripts/publish-public.sh --bootstrap)"
}

# publish_newest_version: sets NEWEST to the highest vMAJOR.MINOR.PATCH tag
# reachable from origin/main.
publish_newest_version() {
    local t v
    NEWEST=""
    while IFS= read -r t; do
        [[ "$t" =~ ^v([0-9]+\.[0-9]+\.[0-9]+)$ ]] || continue
        v="${BASH_REMATCH[1]}"
        if [ -z "$NEWEST" ] || version_gt "$v" "$NEWEST"; then NEWEST="$v"; fi
    done < <(git tag --merged "$PARENT" --list 'v*')
    [ -n "$NEWEST" ] || die 2 "no vMAJOR.MINOR.PATCH tag is reachable from $REMOTE/main: nothing marks what is already published"
}

# publish_check_version: the version is newer than origin's newest, no local tag
# of that name exists, and the release tree's Cargo.toml carries it (the tag
# names the product version; a workspace version that differs would name another).
publish_check_version() {
    local cargo_version
    version_gt "$VERSION" "$NEWEST" || die 2 "$VERSION is not newer than v$NEWEST, the newest version on $REMOTE"
    if git rev-parse --verify --quiet "refs/tags/$TAG" >/dev/null; then
        die 2 "tag $TAG already exists locally; publishing would move it"
    fi
    cargo_version="$(git show "$RELEASE:Cargo.toml" 2>/dev/null | awk '
        /^\[workspace\.package\]/ { f = 1; next }
        /^\[/ { f = 0 }
        f && /^version *=/ { gsub(/[" ]/, ""); sub(/version=/, ""); print; exit }')" || true
    [ "$cargo_version" = "$VERSION" ] \
        || die 2 "the release tree's Cargo.toml [workspace.package] version is '${cargo_version}', not $VERSION"
}

# publish_changelog_message: sets MESSAGE to every CHANGELOG.md section origin has
# not seen, from `## [VERSION]` down to, not including, `## [NEWEST]`.
publish_changelog_message() {
    git show "$RELEASE:CHANGELOG.md" >"$SCRATCH/CHANGELOG.md" 2>/dev/null \
        || die 2 "the release tree has no CHANGELOG.md"
    # Exits 3 when the first section is missing, 4 when the second is, 5 when a
    # section in between is not a version strictly between the two (the file is
    # newest first).
    local rc=0
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
        }' "$SCRATCH/CHANGELOG.md")" || rc=$?
    case "$rc" in
        0) ;;
        3) die 2 "CHANGELOG.md has no ## [$VERSION] section" ;;
        4) die 2 "CHANGELOG.md has no ## [$NEWEST] section below ## [$VERSION]: cannot tell where unpublished history ends" ;;
        5) die 2 "CHANGELOG.md is out of order: a section between [$VERSION] and [$NEWEST] is not a version between them" ;;
        *) die 2 "could not read the sections of CHANGELOG.md (awk exited $rc)" ;;
    esac
    # $(...) already dropped the trailing newlines; drop trailing blank lines too.
    MESSAGE="$(printf '%s\n' "$MESSAGE" | sed -e :a -e '/^[[:space:]]*$/{$d;N;ba' -e '}')"
    [ -n "$MESSAGE" ] || die 2 "the CHANGELOG sections for $VERSION are empty"
}

# publish_gate <label> <dir> <expected-files>: run check-public-terms.sh on <dir>
# and insist it read exactly the files the input holds, all of them tracked.
publish_gate() {
    local label="$1" dir="$2" expected="$3" out rc summary
    rc=0
    out="$(bash "$CHECK" --root "$dir" ${GATE_ARGS[@]+"${GATE_ARGS[@]}"} 2>&1)" || rc=$?
    summary="$(printf '%s\n' "$out" | tail -n 1)"
    if [ "$rc" -eq 1 ]; then
        echo "${PUBLISH_PROG:-publish-public.sh}: the $label carries a listed term; nothing was built:" >&2
        printf '%s\n' "$out" | sed 's/^/    /' >&2
        exit 1
    fi
    if [ "$rc" -ne 0 ]; then
        echo "${PUBLISH_PROG:-publish-public.sh}: the term gate could not check the $label (exit $rc):" >&2
        printf '%s\n' "$out" | sed 's/^/    /' >&2
        exit 3
    fi
    if [[ ! "$summary" =~ ^public-terms:\ tracked=([0-9]+)\ untracked=0\  ]] || [ "${BASH_REMATCH[1]}" -ne "$expected" ]; then
        echo "${PUBLISH_PROG:-publish-public.sh}: the term gate did not walk the $label's $expected file(s): $summary" >&2
        exit 3
    fi
    echo "  $label: $summary"
}

# publish_build_commit: the CHANGELOG message, then publish_commit_message.
publish_build_commit() {
    publish_changelog_message

    echo "${PUBLISH_PROG:-publish-public.sh}: $TAG over $REMOTE/main"
    echo "  release  $RELEASE (tree $TREE)"
    echo "  parent   $PARENT ($REMOTE/main, newest published v$NEWEST)"
    echo "  sections $(printf '%s\n' "$MESSAGE" | sed -n 's/^## \[\([^]]*\)\].*/\1/p' | paste -sd ' ' -)"
    publish_commit_message
}

# publish_commit_message: with MESSAGE set by the caller, the term gate on the
# message and on the release tree, and the commit over origin/main. Sets SQUASH;
# writes one unreferenced commit object and nothing else. The message is scanned
# as the file MESSAGE_FILE (CHANGELOG.md by default: the scanner keys its
# allowlist on path and line, and the publish-public message is lines of that file).
publish_commit_message() {
    local author_date committer_date message_file="${MESSAGE_FILE:-CHANGELOG.md}"
    # The message, as the only file of a repository.
    git init -q "$SCRATCH/message"
    printf '%s\n' "$MESSAGE" >"$SCRATCH/message/$message_file"
    git -C "$SCRATCH/message" add "$message_file"
    publish_gate message "$SCRATCH/message" 1

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
    publish_gate "release tree" "$SCRATCH/tree" "$(git ls-tree -r "$TREE" | wc -l | tr -d ' ')"

    author_date="$(git log -1 --format=%ad --date=raw "$RELEASE")"
    committer_date="$(git log -1 --format=%cd --date=raw "$RELEASE")"
    printf '%s\n' "$MESSAGE" >"$SCRATCH/message.txt"
    SQUASH="$(GIT_AUTHOR_DATE="$author_date" GIT_COMMITTER_DATE="$committer_date" \
        git commit-tree -p "$PARENT" -F "$SCRATCH/message.txt" "$TREE")"
}

# Each command printed as one line a shell reads back as the same words.
quoted() {
    local q
    q="$(printf '%q ' "$@")"
    printf '%s\n' "${q% }"
}
