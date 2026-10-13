#!/usr/bin/env bash
#
# test-publish-public.sh — prove scripts/publish-public.sh builds one gated commit
# over origin/main, moves no ref on a dry run, and pushes only with --push (S-717).
#
# Every case runs in a throwaway clone under $TMPDIR whose `origin` is a local
# bare repository created next to it, with GIT_CEILING_DIRECTORIES set so no git
# command can climb into an enclosing repository. A second bare remote, `mirror`,
# stands in for any other remote the clone has: it must never change.
#
# As in test-check-public-terms.sh, the real term list is never used: the cases
# plant a made-up word and hand the script a term list hashed from it at run time.
#
# S-722 adds the ancestry precondition (origin/main must already be merged
# locally), --bootstrap (record origin's published commit as merged) and
# --version X.Y.Z --tag-only (tag what origin already holds).
#
# Usage: bash scripts/tests/test-publish-public.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PUBLISH="$HERE/scripts/publish-public.sh"

if [ ! -f "$PUBLISH" ]; then
    echo "cannot find scripts/publish-public.sh (looked in $HERE)" >&2
    exit 1
fi

WORK="${TMPDIR:-/tmp}/logos-publish-public-test.$$"
case "$(cd "${TMPDIR:-/tmp}" && pwd -P)/" in
    "$(cd "$HERE" && pwd -P)"/*)
        echo "refusing to build fixtures inside the repository under test" >&2
        exit 1
        ;;
esac
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE

WORD=zorbex
sha() {
    if command -v shasum >/dev/null 2>&1; then
        printf '%s' "$1" | shasum -a 256 | awk '{ print $1 }'
    else
        printf '%s' "$1" | sha256sum | awk '{ print $1 }'
    fi
}
TERMS="$WORK/terms.sha256"
sha "$WORD" >"$TERMS"
ALLOW="$WORK/empty.allow"
: >"$ALLOW"

PASS=0
FAIL=0
N=0

g() { git -c maintenance.auto=false -c gc.auto=0 "$@"; }

cargo() { # cargo <version> -> a Cargo.toml carrying it
    printf '[workspace]\nmembers = []\n\n[workspace.package]\nversion = "%s"\n' "$1" >Cargo.toml
}

# The CHANGELOG at each release, newest first, as the real one is.
SEC_100='## [1.0.0] — 2026-01-01

### Added
- The first release.
'
SEC_101='## [1.0.1] — 2026-01-02

### Fixed
- A fix.
'
SEC_102='## [1.0.2] — 2026-01-03

### Added
- A feature, with a line of its own.
'
changelog() { # changelog <section>... -> CHANGELOG.md with these sections
    {
        printf '# Changelog\n\n## [Unreleased]\n\n'
        for s in "$@"; do printf '%s\n' "$s"; done
        printf '[1.0.0]: https://example.com/ — the link footer is not a section\n'
    } >CHANGELOG.md
}

# fixture -> sets CLONE, ORIGIN, MIRROR. origin holds main at release(1.0.0),
# tagged v1.0.0; the clone's main is two releases ahead, at release(1.0.2).
fixture() {
    N=$((N + 1))
    local base="$WORK/case$N"
    ORIGIN="$base/origin.git"
    MIRROR="$base/mirror.git"
    CLONE="$base/clone"
    mkdir -p "$base"
    g init -q --bare "$ORIGIN"
    g init -q --bare "$MIRROR"
    g -C "$ORIGIN" symbolic-ref HEAD refs/heads/main
    g clone -q "$ORIGIN" "$CLONE" 2>/dev/null
    if [ "$(cd "$CLONE" && git rev-parse --show-toplevel 2>/dev/null)" != "$CLONE" ]; then
        echo "  FAIL  could not create the fixture clone $CLONE"
        exit 1
    fi
    (
        cd "$CLONE" || exit 1
        git config user.email owner@example.com
        git config user.name owner
        g checkout -q -b main 2>/dev/null || true
        mkdir -p src
        printf 'pub fn one() {}\n' >src/lib.rs
        cargo 1.0.0
        changelog "$SEC_100"
        g add -A && g commit -qm 'release(1.0.0)'
        g tag -a v1.0.0 -m 'logos 1.0.0'
        g push -q origin main v1.0.0 2>/dev/null
        g remote add mirror "$MIRROR"
        g push -q mirror main v1.0.0 2>/dev/null
        printf 'pub fn two() {}\n' >>src/lib.rs
        g commit -qam 'work towards 1.0.1'
        cargo 1.0.1
        changelog "$SEC_101" "$SEC_100"
        g commit -qam 'release(1.0.1)'
        printf 'pub fn three() {}\n' >>src/lib.rs
        cargo 1.0.2
        changelog "$SEC_102" "$SEC_101" "$SEC_100"
        g commit -qam 'release(1.0.2)'
    ) || {
        echo "  FAIL  could not populate the fixture clone $CLONE"
        exit 1
    }
}

# commit <message> <file> <content>: one more local commit on the clone's main.
commit() {
    (cd "$CLONE" && mkdir -p "$(dirname "$2")" && printf '%s' "$3" >"$2" && g add -A && g commit -qm "$1")
}

commits() { # the number of commit objects in the clone's store
    (cd "$CLONE" && git cat-file --batch-all-objects --batch-check='%(objecttype)' | grep -c '^commit$')
}
objects() { # the number of objects of every type in the clone's store
    (cd "$CLONE" && git cat-file --batch-all-objects --batch-check='%(objectname)' | wc -l | tr -d ' ')
}
refs() { # every ref and what it points at, in <repo>
    git -C "$1" for-each-ref --format='%(refname) %(objectname)'
}

# snapshot / run: run the script in the clone, recording every ref of the clone,
# origin and mirror, and the commit-object count, before it.
snapshot() {
    BEFORE_CLONE="$(refs "$CLONE")"
    BEFORE_ORIGIN="$(refs "$ORIGIN")"
    BEFORE_MIRROR="$(refs "$MIRROR")"
    BEFORE_COMMITS="$(commits)"
    BEFORE_OBJECTS="$(objects)"
}
run() {
    snapshot
    OUT="$(cd "$CLONE" && bash "$PUBLISH" --terms "$TERMS" --allow "$ALLOW" "$@" 2>&1)"
    RC=$?
    NEW_COMMITS=$(($(commits) - BEFORE_COMMITS))
    NEW_OBJECTS=$(($(objects) - BEFORE_OBJECTS))
}

# run_raw: like run, but hands the script exactly the arguments given (the
# bootstrap takes no term options).
run_raw() {
    snapshot
    OUT="$(cd "$CLONE" && bash "$PUBLISH" "$@" 2>&1)"
    RC=$?
    NEW_COMMITS=$(($(commits) - BEFORE_COMMITS))
    NEW_OBJECTS=$(($(objects) - BEFORE_OBJECTS))
}

fail() {
    echo "  FAIL  $1"
    printf '%s\n' "$OUT" | sed 's/^/        /'
    FAIL=$((FAIL + 1))
}
ok() {
    echo "  ok    $1"
    PASS=$((PASS + 1))
}

# verdict <name>: report the subshell check that wrote $WORK/check.out and exited
# CHECK_RC.
verdict() {
    if [ "$CHECK_RC" -eq 0 ]; then
        ok "$1"
    else
        OUT="$OUT
$(cat "$WORK/check.out")"
        fail "$1"
    fi
}

# published_fixture -> a fixture whose release(1.0.2) has been squashed and landed
# on origin's main by other means (no tag): sets S, the commit origin's main now
# is, and fetches it. The clone's main does not yet contain S.
published_fixture() {
    fixture
    run --version 1.0.2
    S="$(squash_of)"
    (cd "$CLONE" && g push -q origin "$S:refs/heads/main" 2>/dev/null && g fetch -q origin)
}

# expect <name> <exit> <new-commit-objects> [<text that must appear>]
#   ... and, always: no ref of the clone, origin or mirror moved.
expect() {
    local name="$1" want="$2" made="$3" must="${4:-}"
    if [ "$RC" -ne "$want" ]; then fail "$name: exit $RC, wanted $want"; return; fi
    if [ "$NEW_COMMITS" -ne "$made" ]; then fail "$name: $NEW_COMMITS commit object(s) written, wanted $made"; return; fi
    if [ -n "$must" ] && ! printf '%s\n' "$OUT" | grep -qF -- "$must"; then
        fail "$name: exit correct, but the output does not say '$must'"
        return
    fi
    if [ "$(refs "$CLONE")" != "$BEFORE_CLONE" ]; then fail "$name: a ref of the clone moved"; return; fi
    if [ "$(refs "$ORIGIN")" != "$BEFORE_ORIGIN" ]; then fail "$name: a ref of origin moved"; return; fi
    if [ "$(refs "$MIRROR")" != "$BEFORE_MIRROR" ]; then fail "$name: a ref of the mirror moved"; return; fi
    if ls -d "$CLONE"/.git/publish-public.* >/dev/null 2>&1; then fail "$name: scratch left in .git"; return; fi
    if [ -n "$(cd "$CLONE" && git status --porcelain)" ]; then fail "$name: the working tree changed"; return; fi
    ok "$name"
}

squash_of() { printf '%s\n' "$OUT" | sed -n 's/^  squash   \([0-9a-f]*\)$/\1/p'; }

echo "test-publish-public.sh"
echo "  work $WORK"
echo

# --- a dry run builds the one commit, and moves nothing ---------------------------

fixture
run --version 1.0.2
expect "a dry run writes exactly one commit object and moves no ref" 0 1 "sections 1.0.2 1.0.1"
S="$(squash_of)"
(
    cd "$CLONE" || exit 1
    [ "$(git rev-parse "$S^@")" = "$(git rev-parse origin/main)" ] || { echo "parent is not origin/main"; exit 1; }
    [ "$(git rev-parse "$S^{tree}")" = "$(git rev-parse 'main^{tree}')" ] || { echo "tree is not the release tree"; exit 1; }
    printf '%s\n%s' "$SEC_102" "$SEC_101" >"$WORK/want.msg"
    git log -1 --format=%B "$S" | sed -e :a -e '/^[[:space:]]*$/{$d;N;ba' -e '}' >"$WORK/got.msg"
    sed -e :a -e '/^[[:space:]]*$/{$d;N;ba' -e '}' "$WORK/want.msg" | diff - "$WORK/got.msg" || { echo "message differs"; exit 1; }
    git tag --points-at "$S" | grep -q . && { echo "a tag points at the squash"; exit 1; }
    git for-each-ref --contains "$S" | grep -q . && { echo "a ref contains the squash"; exit 1; }
    exit 0
) >"$WORK/check.out" 2>&1
CHECK_RC=$?
if [ "$CHECK_RC" -eq 0 ]; then
    ok "its parent is origin/main, its tree the release tree, its message the two unpublished sections, and no ref reaches it"
else
    OUT="$(cat "$WORK/check.out")"
    fail "the squash commit is not the one specified"
fi

run --version 1.0.2
if [ "$RC" -eq 0 ] && [ "$(squash_of)" = "$S" ]; then
    ok "a second dry run builds the same commit id (the one --push would push)"
else
    fail "a second dry run built $(squash_of), not $S"
fi
if printf '%s\n' "$OUT" | grep -qxF "push: git push --atomic origin $S:refs/heads/main refs/tags/v1.0.2:refs/tags/v1.0.2"; then
    ok "it prints the exact push it would run"
else
    fail "the printed push is not the expected command"
fi

# --- a listed term aborts before any commit object is written --------------------

fixture
(cd "$CLONE" && changelog "${SEC_102/A feature/A $WORD feature}" "$SEC_101" "$SEC_100" && g commit -qam 'plant')
run --version 1.0.2
expect "a term in an unpublished CHANGELOG section aborts at the message gate" 1 0 "the message carries a listed term"

fixture
commit 'plant' src/lib.rs "pub fn $WORD() {}
"
run --version 1.0.2
expect "a term in the release tree aborts at the tree gate" 1 0 "the release tree carries a listed term"

fixture
commit 'plant deep' docs/deep/notes.md "nothing
a ${WORD}-joined line
"
run --version 1.0.2
expect "a term in a nested file of the tree aborts" 1 0 "docs/deep/notes.md:2:$WORD"

# The tree is gated as committed, not as a checkout attribute would rewrite it:
# a working-tree-encoding turns the file into UTF-16 on checkout, which the
# scanner would skip as binary.
fixture
(
    cd "$CLONE" || exit 1
    b=$(printf 'a %s line\n' "$WORD" | git hash-object -w --stdin)
    a=$(printf 'encoded.txt working-tree-encoding=UTF-16\n' | git hash-object -w --stdin)
    g update-index --add --cacheinfo "100644,$b,encoded.txt" --cacheinfo "100644,$a,.gitattributes"
    g commit -qm 'encoded on checkout'
    g reset -q --hard
)
run --version 1.0.2
expect "a term in a file a checkout attribute would re-encode still aborts" 1 0 "encoded.txt:1:$WORD"

# A tree the scratch checkout cannot write is the gate failing to run (exit 3),
# never a silent exit 1. Two paths differing only in case collide on a
# case-insensitive filesystem; on a case-sensitive one there is no collision to
# provoke, and the case says so instead of passing.
: >"$WORK/caseprobe"
if [ -e "$WORK/CASEPROBE" ]; then
    fixture
    (
        cd "$CLONE" || exit 1
        b1=$(printf 'clean one\n' | git hash-object -w --stdin)
        b2=$(printf 'clean two\n' | git hash-object -w --stdin)
        g update-index --add --cacheinfo "100644,$b1,README" --cacheinfo "100644,$b2,readme"
        g commit -qm 'case pair'
        g read-tree HEAD
    )
    snapshot
    OUT="$(cd "$CLONE" && bash "$PUBLISH" --terms "$TERMS" --allow "$ALLOW" --version 1.0.2 2>&1)"
    RC=$?
    NEW_COMMITS=$(($(commits) - BEFORE_COMMITS))
    if [ "$RC" -eq 3 ] && [ "$NEW_COMMITS" -eq 0 ] && printf '%s\n' "$OUT" | grep -qF "could not check out the release tree"; then
        ok "a release tree the scratch checkout cannot write exits 3 with the reason"
    else
        fail "a failed scratch checkout exited $RC with $NEW_COMMITS commit object(s)"
    fi
else
    echo "  skip  a failed scratch checkout (this filesystem is case-sensitive: no collision to provoke)"
fi

# A line the allowlist admits in CHANGELOG.md is admitted in the message too: the
# message is that file's lines, published as reviewed.
fixture
LINE="- A $WORD feature, reviewed."
(cd "$CLONE" && changelog "${SEC_102/- A feature, with a line of its own./$LINE}" "$SEC_101" "$SEC_100" && g commit -qam 'reviewed')
printf 'CHANGELOG.md:%s\n' "$(sha "$LINE")" >"$WORK/changelog.allow"
snapshot
OUT="$(cd "$CLONE" && bash "$PUBLISH" --terms "$TERMS" --allow "$WORK/changelog.allow" --version 1.0.2 2>&1)"
RC=$?
NEW_COMMITS=$(($(commits) - BEFORE_COMMITS))
expect "a CHANGELOG line the allowlist admits passes in the message as in the tree" 0 1 "message: public-terms: tracked=1 untracked=0 scanned=1 binary=0 skipped=0 hits=0 allowlisted=1"

# The tree is read from the commit, never the working directory: a term only in an
# uncommitted file is not published, so it does not abort.
fixture
printf 'a %s here\n' "$WORD" >"$CLONE/scratch-notes.txt"
OUT="$(cd "$CLONE" && bash "$PUBLISH" --terms "$TERMS" --allow "$ALLOW" --version 1.0.2 2>&1)"
RC=$?
rm -f "$CLONE/scratch-notes.txt"
if [ "$RC" -eq 0 ]; then
    ok "an untracked file the commit does not carry is not gated"
else
    fail "an untracked file aborted the run (exit $RC)"
fi

# --- usage errors and preconditions write nothing --------------------------------

fixture
run --version 1.0.0
expect "a version origin already has is refused" 2 0 "not newer than v1.0.0"
run --version 0.9.9
expect "an older version is refused" 2 0 "not newer than v1.0.0"
run --version 1.0
expect "a malformed version is refused" 2 0 "must be MAJOR.MINOR.PATCH"
run --version 1.0.1
expect "a version the release tree's Cargo.toml does not carry is refused" 2 0 "version is '1.0.2', not 1.0.1"
run --version 1.0.1 --from main~1
expect "--from an older release publishes that release" 0 1 "sections 1.0.1"

commit 'bump only' Cargo.toml '[workspace.package]
version = "1.0.3"
'
run --version 1.0.3
expect "a version with no CHANGELOG section is refused" 2 0 "no ## [1.0.3] section"

fixture
(cd "$CLONE" && changelog "$SEC_102" "$SEC_101" && g commit -qam 'drop 1.0.0')
run --version 1.0.2
expect "a CHANGELOG without origin's newest section is refused" 2 0 "no ## [1.0.0] section below"

fixture
(cd "$CLONE" && changelog "$SEC_102" "${SEC_101/1.0.1/0.9.0}" "$SEC_100" && g commit -qam 'misorder')
run --version 1.0.2
expect "a section out of order is refused" 2 0 "out of order"

fixture
(cd "$CLONE" && g tag -a v1.0.2 -m existing main~1)
run --version 1.0.2
expect "an existing local tag of that version is refused, and stays where it was" 2 0 "already exists locally"

fixture
(cd "$CLONE" && g update-ref -d refs/remotes/origin/main)
run --version 1.0.2
expect "a clone with no origin/main is refused" 2 0 "no origin/main"

# --- --push publishes to origin only ------------------------------------------------

fixture
MAIN_BEFORE="$(git -C "$CLONE" rev-parse main)"
V100_BEFORE="$(git -C "$CLONE" rev-parse v1.0.0)"
snapshot
OUT="$(cd "$CLONE" && bash "$PUBLISH" --terms "$TERMS" --allow "$ALLOW" --version 1.0.2 --push 2>&1)"
RC=$?
S="$(squash_of)"
(
    [ "$RC" -eq 0 ] || { echo "exit $RC"; exit 1; }
    [ "$(git -C "$ORIGIN" rev-parse main)" = "$S" ] || { echo "origin main is not the squash"; exit 1; }
    [ "$(git -C "$ORIGIN" rev-parse 'v1.0.2^{commit}')" = "$S" ] || { echo "origin v1.0.2 does not peel to the squash"; exit 1; }
    [ "$(git -C "$ORIGIN" cat-file -t v1.0.2)" = tag ] || { echo "the tag is not annotated"; exit 1; }
    [ "$(git -C "$ORIGIN" rev-list --count main)" -eq 2 ] || { echo "origin main is not one commit over the old main"; exit 1; }
    git -C "$ORIGIN" show v1.0.2:Cargo.toml | grep -qxF 'version = "1.0.2"' || { echo "the tag's tree does not carry the version"; exit 1; }
    [ "$(git -C "$CLONE" rev-parse main)" = "$MAIN_BEFORE" ] || { echo "local main moved"; exit 1; }
    [ "$(git -C "$CLONE" rev-parse v1.0.0)" = "$V100_BEFORE" ] || { echo "an existing tag moved"; exit 1; }
    [ "$(refs "$MIRROR")" = "$BEFORE_MIRROR" ] || { echo "the mirror changed"; exit 1; }
    [ "$(git -C "$CLONE" rev-parse 'v1.0.2^{commit}')" = "$S" ] || { echo "no local tag on the squash"; exit 1; }
    exit 0
) >"$WORK/check.out" 2>&1
CHECK_RC=$?
if [ "$CHECK_RC" -eq 0 ]; then
    ok "--push puts one tagged commit on origin's main and moves no local main, existing tag or other remote"
else
    OUT="$OUT
$(cat "$WORK/check.out")"
    fail "--push did not publish as specified"
fi

# The next release stacks one commit on the published one: origin now reaches
# v1.0.2, so only the newer section is unpublished. Origin's main is the squash,
# which local main does not contain; the publish refuses until that is recorded.
run_raw --bootstrap --yes
if [ "$RC" -eq 0 ] && git -C "$CLONE" merge-base --is-ancestor "$S" main; then
    ok "after a push, --bootstrap --yes records the published squash as an ancestor of main"
else
    fail "after a push, --bootstrap --yes did not record the squash"
fi
commit 'release(1.0.3)' Cargo.toml '[workspace.package]
version = "1.0.3"
'
(cd "$CLONE" && changelog '## [1.0.3] — 2026-01-04

### Fixed
- Another fix.
' "$SEC_102" "$SEC_101" "$SEC_100" && g commit -qam 'changelog 1.0.3')
run --version 1.0.3
expect "after a push, the next release carries only its own section" 0 1 "sections 1.0.3"
if [ "$(git -C "$CLONE" rev-parse "$(squash_of)^@")" = "$S" ]; then
    ok "and its parent is the published squash"
else
    fail "its parent is not the published squash"
fi
run --version 1.0.2
expect "the published version cannot be published again" 2 0 "not newer than v1.0.2"

# A stale origin/main: origin moved since the last fetch. Nothing is tagged or pushed.
fixture
(
    cd "$WORK" && g clone -q "$ORIGIN" "case$N/other" 2>/dev/null && cd "case$N/other" \
        && git config user.email o@example.com && git config user.name other \
        && printf 'x\n' >x && g add x && g commit -qm moved && g push -q origin main 2>/dev/null
)
run --version 1.0.2 --push
expect "--push refuses when origin's main moved since the last fetch" 2 1 "fetch origin and re-run"

# An origin that cannot be read is a failed precondition, not git's own status.
fixture
(cd "$CLONE" && git remote set-url origin "$WORK/case$N/gone.git")
run --version 1.0.2 --push
expect "--push refuses with exit 2 when origin cannot be read, and tags nothing" 2 1 "cannot read origin's refs"

# A push origin rejects: the tag this run created is deleted again.
fixture
printf '#!/bin/sh\nexit 1\n' >"$ORIGIN/hooks/pre-receive"
chmod +x "$ORIGIN/hooks/pre-receive"
run --version 1.0.2 --push
expect "a rejected push exits 4 and leaves no tag behind" 4 1 "deleted the local tag v1.0.2"

# A push interrupted mid-way: the hook stands in for Ctrl-C, signalling the script
# while its push runs, and the push then fails. The tag must not outlive the run.
fixture
cat >"$CLONE/.git/hooks/pre-push" <<'HOOK'
#!/bin/sh
kill -TERM "$(ps -o ppid= -p "$PPID" | tr -d ' ')"
exit 1
HOOK
chmod +x "$CLONE/.git/hooks/pre-push"
run --version 1.0.2 --push
expect "a push interrupted mid-way exits 130 and leaves no tag behind" 130 1 "deleted the local tag v1.0.2"

# A signal that reaches the script alone while its push runs is handled when the
# push returns. Here the push succeeds (the hook only waits), so the tag is on
# origin and must stay locally; the run still exits 130.
fixture
printf '#!/bin/sh\ntouch "%s/hook-started"\nsleep 3\nexit 0\n' "$WORK" >"$ORIGIN/hooks/pre-receive"
chmod +x "$ORIGIN/hooks/pre-receive"
rm -f "$WORK/hook-started"
(cd "$CLONE" && exec bash "$PUBLISH" --terms "$TERMS" --allow "$ALLOW" --version 1.0.2 --push >"$WORK/term.out" 2>&1) &
PUB_PID=$!
for _ in $(seq 1 100); do [ -e "$WORK/hook-started" ] && break; sleep 0.1; done
kill -TERM "$PUB_PID" 2>/dev/null
wait "$PUB_PID" 2>/dev/null
RC=$?
OUT="$(cat "$WORK/term.out")"
if [ "$RC" -eq 130 ] && git -C "$CLONE" rev-parse --verify --quiet refs/tags/v1.0.2 >/dev/null \
    && [ -n "$(git -C "$ORIGIN" tag -l v1.0.2)" ] && printf '%s\n' "$OUT" | grep -qF "kept the local tag"; then
    ok "a signal that lands after the push succeeded keeps the local tag the remote has"
else
    fail "a signal after a successful push (exit $RC)"
fi

# The gate failing to run is exit 3, never a pass: a copy of the script beside a
# stand-in scanner that cannot run.
STUB="$WORK/stub/scripts"
mkdir -p "$STUB/lib"
cp "$PUBLISH" "$STUB/publish-public.sh"
cp "$HERE/scripts/lib/publish-core.sh" "$STUB/lib/publish-core.sh"
stub_gate() { # stub_gate <script body>: the stand-in scanner the copy calls
    printf '#!/usr/bin/env bash\n%s\n' "$1" >"$STUB/check-public-terms.sh"
}
REAL_PUBLISH="$PUBLISH"
PUBLISH="$STUB/publish-public.sh"
fixture
stub_gate 'echo "scanner broke" >&2; exit 2'
run --version 1.0.2
expect "a term gate that cannot run exits 3, and nothing is built" 3 0 "could not check the message"
PUBLISH="$REAL_PUBLISH"

# A gate that read a different number of files than the input holds certifies
# nothing: exit 3, whether it read too few or took an untracked file.
PUBLISH="$STUB/publish-public.sh"
stub_gate 'echo "public-terms: tracked=0 untracked=0 scanned=0 binary=0 skipped=0 hits=0 allowlisted=0"'
run --version 1.0.2
expect "a term gate that walked zero of the message's one file exits 3" 3 0 "did not walk the message's 1 file"
stub_gate 'echo "public-terms: tracked=1 untracked=1 scanned=2 binary=0 skipped=0 hits=0 allowlisted=0"'
run --version 1.0.2
expect "a term gate that also walked an untracked file exits 3" 3 0 "did not walk the message's 1 file"
PUBLISH="$REAL_PUBLISH"

# GIT_DIR / GIT_WORK_TREE in the caller's environment cannot point the script at
# another repository: here they name the mirror.
fixture
snapshot
OUT="$(cd "$CLONE" && GIT_DIR="$MIRROR" GIT_WORK_TREE="$WORK" bash "$PUBLISH" --terms "$TERMS" --allow "$ALLOW" --version 1.0.2 2>&1)"
RC=$?
NEW_COMMITS=$(($(commits) - BEFORE_COMMITS))
expect "GIT_DIR and GIT_WORK_TREE in the environment are ignored" 0 1 "sections 1.0.2 1.0.1"

# Origin already holds the tag (pushed by hand, say): --push refuses before
# tagging anything.
fixture
(cd "$CLONE" && g tag -a v1.0.2 -m x main~1 && g push -q origin v1.0.2 2>/dev/null && g tag -d v1.0.2 >/dev/null)
run --version 1.0.2 --push
expect "--push refuses when origin already has the tag" 2 1 "origin already has tag v1.0.2"

# The message is compared as committed, untrimmed: a section ending in a
# whitespace-only line must not carry it into the commit.
fixture
WS_LINE="$(printf '%3s' '')" # three spaces, spelled so no editor strips them
(cd "$CLONE" && changelog "$SEC_102" "$SEC_101$WS_LINE
" "$SEC_100" && g commit -qam 'trailing whitespace')
run --version 1.0.2
expect "a section ending in a whitespace-only line still builds" 0 1 "sections 1.0.2 1.0.1"
printf '%s\n%s' "$SEC_102" "$SEC_101" >"$WORK/want.raw"
git -C "$CLONE" cat-file commit "$(squash_of)" | sed '1,/^$/d' >"$WORK/got.raw"
if diff "$WORK/want.raw" "$WORK/got.raw" >/dev/null; then
    ok "the committed message is the sections byte for byte, with no trailing blank line"
else
    OUT="$(diff "$WORK/want.raw" "$WORK/got.raw")"
    fail "the committed message differs from the sections"
fi

# The printed tag command is the one --push runs, and the tag it publishes
# carries that message.
fixture
run --version 1.0.2
S="$(squash_of)"
if printf '%s\n' "$OUT" | grep -qxF "tag:  git tag -a v1.0.2 -m logos\\ 1.0.2 $S"; then
    ok "it prints the exact tag command, shell-quoted"
else
    fail "the printed tag command is not the expected one"
fi
OUT="$(cd "$CLONE" && bash "$PUBLISH" --terms "$TERMS" --allow "$ALLOW" --version 1.0.2 --push 2>&1)"
SUBJECT="$(git -C "$ORIGIN" tag -l --format='%(contents:subject)' v1.0.2)"
if [ "$SUBJECT" = "logos 1.0.2" ]; then
    ok "the pushed tag's message is the printed one"
else
    fail "the pushed tag's message is '$SUBJECT'"
fi

# Origin's newest version is chosen by version, not by the order the tags are
# listed in: an older tag that sorts last must not become the lower bound.
fixture
(cd "$CLONE" && g tag v0.9.0 main~3 && g push -q origin v0.9.0 2>/dev/null && git config tag.sort -version:refname)
run --version 1.0.2
expect "origin's newest version is the highest, whatever order the tags list in" 0 1 "sections 1.0.2 1.0.1"


# --- origin's main must already be merged locally (S-722) ------------------------

# A commit on origin that the clone has fetched but not merged: building over
# origin's main from this tree would publish that commit reverted.
fixture
(
    cd "$WORK" && g clone -q "$ORIGIN" "case$N/other" 2>/dev/null && cd "case$N/other" \
        && git config user.email o@example.com && git config user.name other \
        && printf 'merged on public main\n' >fromorigin.txt && g add fromorigin.txt \
        && g commit -qm 'merged on public main' && g push -q origin main 2>/dev/null
)
(cd "$CLONE" && g fetch -q origin)
run --version 1.0.2
expect "a commit on origin/main absent locally is refused, naming the merge" 2 0 "git fetch origin && git merge origin/main"
if [ "$NEW_OBJECTS" -eq 0 ]; then
    ok "and the refusal writes no object of any kind"
else
    fail "the refusal wrote $NEW_OBJECTS object(s)"
fi
run --version 1.0.2 --tag-only
expect "--tag-only is refused the same way" 2 0 "git fetch origin && git merge origin/main"
(cd "$CLONE" && g merge -q --no-edit origin/main)
run --version 1.0.2
expect "after the merge the dry run passes" 0 1 "sections 1.0.2 1.0.1"
S="$(squash_of)"
(
    cd "$CLONE" || exit 1
    [ "$(git show "$S:fromorigin.txt")" = "merged on public main" ] || { echo "the squash tree lacks the origin commit's change"; exit 1; }
    [ "$(git rev-parse "$S^@")" = "$(git rev-parse origin/main)" ] || { echo "parent is not origin/main"; exit 1; }
    exit 0
) >"$WORK/check.out" 2>&1
CHECK_RC=$?
verdict "and its tree contains the change merged on public main"

# The precondition is on the commit being published, not on HEAD: the merge's
# first parent still lacks origin's commit.
run --version 1.0.2 --from main^1
expect "--from a commit that lacks origin/main is refused though HEAD has it" 2 0 "is not an ancestor of"

# --- --bootstrap ------------------------------------------------------------------

published_fixture
run_raw --bootstrap
BUILT_FROM="$(git -C "$CLONE" rev-parse main)"
expect "--bootstrap prints the merge it would run" 0 0 "merge: git merge -s ours origin/main"
if printf '%s\n' "$OUT" | grep -qF "built from   $BUILT_FROM"; then
    ok "and names the local commit origin's was built from"
else
    fail "the local commit origin's was built from is not named"
fi
if printf '%s\n' "$OUT" | grep -q 'dry run: nothing was merged'; then
    ok "and says nothing was merged"
else
    fail "the dry run does not say nothing was merged"
fi

run_raw --bootstrap --yes
(
    cd "$CLONE" || exit 1
    [ "$RC" -eq 0 ] || { echo "exit $RC"; exit 1; }
    [ "$NEW_COMMITS" -eq 1 ] || { echo "$NEW_COMMITS commit object(s) written, wanted the one merge"; exit 1; }
    [ "$(git rev-list --parents -n1 HEAD | wc -w | tr -d ' ')" -eq 3 ] || { echo "HEAD is not a two-parent merge"; exit 1; }
    [ "$(git rev-parse 'HEAD^1')" = "$BUILT_FROM" ] || { echo "the first parent is not the old main"; exit 1; }
    [ "$(git rev-parse 'HEAD^2')" = "$S" ] || { echo "the second parent is not origin/main"; exit 1; }
    [ "$(git rev-parse 'HEAD^{tree}')" = "$(git rev-parse "$BUILT_FROM^{tree}")" ] || { echo "the merge changed the tree"; exit 1; }
    [ "$(refs "$ORIGIN")" = "$BEFORE_ORIGIN" ] || { echo "origin moved: bootstrap pushed"; exit 1; }
    [ "$(refs "$MIRROR")" = "$BEFORE_MIRROR" ] || { echo "the mirror moved"; exit 1; }
    exit 0
) >"$WORK/check.out" 2>&1
CHECK_RC=$?
verdict "--bootstrap --yes makes one merge that keeps main's tree, and pushes nothing"

AFTER="$(git -C "$CLONE" rev-parse HEAD)"
run_raw --bootstrap --yes
if [ "$RC" -eq 0 ] && [ "$NEW_OBJECTS" -eq 0 ] && [ "$(git -C "$CLONE" rev-parse HEAD)" = "$AFTER" ] \
    && printf '%s\n' "$OUT" | grep -qF "already recorded"; then
    ok "on an ancestry already recorded it says so and makes no second merge"
else
    fail "a second --bootstrap --yes did not report 'already recorded' cleanly (exit $RC, $NEW_OBJECTS new object(s))"
fi

# Local work after the publish: the squash's tree is an older commit's, and the
# merge still keeps main's own tree.
published_fixture
commit 'later work' src/later.rs "pub fn later() {}
"
LATER_TREE="$(git -C "$CLONE" rev-parse 'main^{tree}')"
run_raw --bootstrap --yes
if [ "$RC" -eq 0 ] && [ "$(git -C "$CLONE" rev-parse 'HEAD^{tree}')" = "$LATER_TREE" ] \
    && git -C "$CLONE" merge-base --is-ancestor "$S" HEAD; then
    ok "--bootstrap records ancestry for a main that has moved on, keeping its tree"
else
    fail "--bootstrap on a main that moved on (exit $RC)"
fi

# Origin's main is a commit no local commit was built into: recording it as merged
# would throw its content away.
fixture
(
    cd "$WORK" && g clone -q "$ORIGIN" "case$N/other" 2>/dev/null && cd "case$N/other" \
        && git config user.email o@example.com && git config user.name other \
        && printf 'only on origin\n' >only-on-origin.txt && g add only-on-origin.txt \
        && g commit -qm 'only on origin' && g push -q origin main 2>/dev/null
)
(cd "$CLONE" && g fetch -q origin)
run_raw --bootstrap
expect "--bootstrap is refused when origin's tree is no local commit's" 2 0 "is the tree of no commit in HEAD's history"
HEAD_BEFORE="$(git -C "$CLONE" rev-parse HEAD)"
run_raw --bootstrap --yes
if [ "$RC" -eq 2 ] && [ "$NEW_OBJECTS" -eq 0 ] && [ "$(git -C "$CLONE" rev-parse HEAD)" = "$HEAD_BEFORE" ]; then
    ok "and --yes does not override the refusal"
else
    fail "--bootstrap --yes on a tree that matches no local commit (exit $RC)"
fi

published_fixture
printf 'dirty\n' >>"$CLONE/src/lib.rs"
run_raw --bootstrap --yes
if [ "$RC" -eq 2 ] && printf '%s\n' "$OUT" | grep -qF "uncommitted changes" && [ "$NEW_OBJECTS" -eq 0 ]; then
    ok "--bootstrap --yes is refused on a dirty working tree"
else
    fail "--bootstrap --yes on a dirty tree (exit $RC)"
fi
git -C "$CLONE" checkout -q -- src/lib.rs
git -C "$CLONE" checkout -q --detach
run_raw --bootstrap --yes
if [ "$RC" -eq 2 ] && printf '%s\n' "$OUT" | grep -qF "HEAD is detached"; then
    ok "and on a detached HEAD"
else
    fail "--bootstrap --yes on a detached HEAD (exit $RC)"
fi

run_raw --bootstrap --version 1.0.2
expect "--bootstrap takes no --version" 2 0 "--bootstrap takes only --yes"
run_raw --bootstrap --push
expect "--bootstrap takes no --push" 2 0 "--bootstrap takes only --yes"
run --bootstrap
expect "--bootstrap takes no term options" 2 0 "--bootstrap takes only --yes"
run --version 1.0.2 --yes
expect "--yes without --bootstrap is refused" 2 0 "--yes belongs to --bootstrap"

# The dry run refuses what --yes would refuse, so it never promises a merge that
# is then refused.
published_fixture
printf 'dirty\n' >>"$CLONE/src/lib.rs"
run_raw --bootstrap
if [ "$RC" -eq 2 ] && printf '%s\n' "$OUT" | grep -qF "uncommitted changes"; then
    ok "the --bootstrap dry run refuses a dirty working tree as --yes does"
else
    fail "the --bootstrap dry run on a dirty tree (exit $RC)"
fi
git -C "$CLONE" checkout -q -- src/lib.rs
printf 'untracked\n' >"$CLONE/untracked.txt"
run_raw --bootstrap --yes
if [ "$RC" -eq 0 ] && git -C "$CLONE" merge-base --is-ancestor "$S" HEAD; then
    ok "an untracked file does not stop --bootstrap --yes"
else
    fail "--bootstrap --yes with an untracked file (exit $RC)"
fi
rm -f "$CLONE/untracked.txt"

published_fixture
git -C "$CLONE" checkout -q --detach
run_raw --bootstrap
expect "the --bootstrap dry run refuses a detached HEAD" 2 0 "HEAD is detached"

# Origin's main shares no history with HEAD though its tree is HEAD's: git would
# refuse the merge, so the dry run does.
fixture
ROOT_COMMIT="$(git -C "$CLONE" commit-tree -m 'unrelated root' "$(git -C "$CLONE" rev-parse 'HEAD^{tree}')")"
git -C "$CLONE" update-ref refs/remotes/origin/main "$ROOT_COMMIT"
run_raw --bootstrap
expect "the --bootstrap dry run refuses a history origin/main shares nothing with" 2 0 "share no history"

# A merge a hook stops leaves MERGE_HEAD: the script puts the branch back.
published_fixture
printf '#!/bin/sh\nexit 1\n' >"$CLONE/.git/hooks/pre-merge-commit"
chmod +x "$CLONE/.git/hooks/pre-merge-commit"
HEAD_BEFORE="$(git -C "$CLONE" rev-parse HEAD)"
run_raw --bootstrap --yes
if [ "$RC" -eq 2 ] && [ ! -e "$CLONE/.git/MERGE_HEAD" ] && [ "$(git -C "$CLONE" rev-parse HEAD)" = "$HEAD_BEFORE" ] \
    && printf '%s\n' "$OUT" | grep -qF "is as it was"; then
    ok "a merge a hook stops is aborted and the branch is as it was"
else
    fail "a hook-stopped bootstrap merge (exit $RC, MERGE_HEAD $([ -e "$CLONE/.git/MERGE_HEAD" ] && echo present || echo absent))"
fi

# The merge keeps HEAD's tree (-s ours) even where HEAD and origin disagree: a plain
# merge would conflict on the file both changed.
published_fixture
commit 'rewrite' src/lib.rs "pub fn mine() {}
"
MINE_TREE="$(git -C "$CLONE" rev-parse 'HEAD^{tree}')"
run_raw --bootstrap --yes
if [ "$RC" -eq 0 ] && [ "$(git -C "$CLONE" rev-parse 'HEAD^{tree}')" = "$MINE_TREE" ] \
    && [ "$(git -C "$CLONE" show HEAD:src/lib.rs)" = "pub fn mine() {}" ]; then
    ok "--bootstrap keeps main's own tree where main and origin differ on a file"
else
    fail "--bootstrap where main and origin differ (exit $RC)"
fi

# Several commits carry origin's tree (a change and its revert): the newest is the
# one named.
published_fixture
commit 'change' src/extra.rs "pub fn extra() {}
"
(cd "$CLONE" && g revert --no-edit HEAD >/dev/null 2>&1)
NEWEST_SAME="$(git -C "$CLONE" rev-parse HEAD)"
run_raw --bootstrap
expect "--bootstrap names the newest commit that carries origin's tree" 0 0 "built from   $NEWEST_SAME"

run_raw --bootstrap --tag-only
expect "--bootstrap takes no --tag-only" 2 0 "--bootstrap takes only --yes"
run_raw --bootstrap --from HEAD
expect "--bootstrap takes no --from" 2 0 "--bootstrap takes only --yes"
run_raw --help
expect "--help prints the usage, including the new modes, with exit 2" 2 0 "--bootstrap [--yes]"
run_raw
expect "no arguments print the usage with exit 2" 2 0 "--tag-only"

# --- --version X.Y.Z --tag-only ---------------------------------------------------

# Origin's main holds the release tree but local main does not contain it.
published_fixture
run --version 1.0.2 --tag-only
expect "--tag-only is refused until origin/main is merged locally" 2 0 "is not an ancestor of"
(cd "$CLONE" && g merge -q -s ours --no-edit origin/main)

run --version 1.0.2 --tag-only
(
    cd "$CLONE" || exit 1
    [ "$RC" -eq 0 ] || { echo "exit $RC"; exit 1; }
    [ "$NEW_OBJECTS" -eq 1 ] || { echo "$NEW_OBJECTS object(s) written, wanted the tag object alone"; exit 1; }
    [ "$(git cat-file -t v1.0.2)" = tag ] || { echo "the tag is not annotated"; exit 1; }
    [ "$(git rev-parse 'v1.0.2^{commit}')" = "$S" ] || { echo "the tag does not point at origin/main"; exit 1; }
    [ "$(git tag -l --format='%(contents:subject)' v1.0.2)" = "logos 1.0.2" ] || { echo "the tag message is wrong"; exit 1; }
    [ "$(refs "$ORIGIN")" = "$BEFORE_ORIGIN" ] || { echo "origin changed without --push"; exit 1; }
    [ "$(refs "$MIRROR")" = "$BEFORE_MIRROR" ] || { echo "the mirror changed"; exit 1; }
    git ls-remote origin refs/tags/v1.0.2 | grep -q . && { echo "the tag reached origin without --push"; exit 1; }
    exit 0
) >"$WORK/check.out" 2>&1
CHECK_RC=$?
verdict "--tag-only tags origin/main with an annotated tag and pushes nothing"

run --version 1.0.2 --tag-only
expect "a second --tag-only run says the tag exists, not that the version is not newer" 2 0 "tag v1.0.2 already exists locally"

published_fixture
(cd "$CLONE" && g merge -q -s ours --no-edit origin/main)
run --version 1.0.2 --tag-only --push
(
    cd "$CLONE" || exit 1
    [ "$RC" -eq 0 ] || { echo "exit $RC"; exit 1; }
    [ "$(git -C "$ORIGIN" rev-parse 'v1.0.2^{commit}')" = "$S" ] || { echo "origin's v1.0.2 does not peel to origin/main"; exit 1; }
    [ "$(git -C "$ORIGIN" cat-file -t v1.0.2)" = tag ] || { echo "origin's tag is not annotated"; exit 1; }
    [ "$(git -C "$ORIGIN" rev-parse main)" = "$S" ] || { echo "origin's main moved"; exit 1; }
    [ "$(refs "$MIRROR")" = "$BEFORE_MIRROR" ] || { echo "the mirror changed"; exit 1; }
    exit 0
) >"$WORK/check.out" 2>&1
CHECK_RC=$?
verdict "--tag-only --push pushes the tag alone"
if printf '%s\n' "$OUT" | grep -qxF "push: git push origin refs/tags/v1.0.2:refs/tags/v1.0.2"; then
    ok "and the push it prints and runs is the tag alone, unforced"
else
    fail "the printed --tag-only push is not the tag alone"
fi

# --push's remote checks hold for --tag-only too.
published_fixture
(cd "$CLONE" && g merge -q -s ours --no-edit origin/main)
(
    cd "$WORK" && g clone -q "$ORIGIN" "case$N/other" 2>/dev/null && cd "case$N/other" \
        && git config user.email o@example.com && git config user.name other \
        && printf 'x\n' >x && g add x && g commit -qm moved && g push -q origin main 2>/dev/null
)
run --version 1.0.2 --tag-only --push
expect "--tag-only --push refuses when origin's main moved since the last fetch" 2 0 "fetch origin and re-run"

published_fixture
(cd "$CLONE" && g merge -q -s ours --no-edit origin/main && g tag -a v1.0.2 -m x main~1 && g push -q origin v1.0.2 2>/dev/null && g tag -d v1.0.2 >/dev/null)
run --version 1.0.2 --tag-only --push
expect "--tag-only --push refuses when origin already has the tag" 2 0 "origin already has tag v1.0.2"

# The squash mode's version checks hold for it too.
published_fixture
(cd "$CLONE" && g merge -q -s ours --no-edit origin/main)
run --version 1.0.0 --tag-only
expect "--tag-only refuses a version origin already has" 2 0 "not newer than v1.0.0"
run --version 1.0.1 --tag-only
expect "--tag-only refuses a version the release tree's Cargo.toml does not carry" 2 0 "version is '1.0.2', not 1.0.1"
(cd "$CLONE" && g tag -a v1.0.2 -m existing main~1)
run --version 1.0.2 --tag-only
expect "--tag-only refuses when the local tag exists, and leaves it" 2 0 "already exists locally"

# Origin's main is local main itself (so the trees match), but its CHANGELOG has
# no section for the version.
fixture
(cd "$CLONE" && changelog "$SEC_101" "$SEC_100" && g commit -qam 'no 1.0.2 section' && g push -q origin main 2>/dev/null)
run --version 1.0.2 --tag-only
expect "--tag-only refuses a version the CHANGELOG has no section for" 2 0 "no ## [1.0.2] section"

# The release tree must be the one origin holds.
fixture
run --version 1.0.2 --tag-only
expect "--tag-only is refused when the release tree is not origin/main's" 2 0 "is not origin/main's tree"

# A tag push origin rejects: the tag this run created is deleted again.
published_fixture
(cd "$CLONE" && g merge -q -s ours --no-edit origin/main)
printf '#!/bin/sh\nexit 1\n' >"$ORIGIN/hooks/pre-receive"
chmod +x "$ORIGIN/hooks/pre-receive"
run --version 1.0.2 --tag-only --push
if [ "$RC" -eq 4 ] && ! git -C "$CLONE" rev-parse --verify --quiet refs/tags/v1.0.2 >/dev/null \
    && printf '%s\n' "$OUT" | grep -qF "deleted the local tag v1.0.2"; then
    ok "a rejected --tag-only push exits 4 and leaves no tag behind"
else
    fail "a rejected --tag-only push (exit $RC)"
fi

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
