#!/usr/bin/env bash
#
# test-publish-sprint.sh — prove scripts/publish-sprint.sh builds one gated commit
# over origin/main from local main, moves no ref on a dry run, refuses what it
# must, and pushes the branch and opens the pull request only with --push (S-723).
#
# Every case runs in a throwaway clone under $TMPDIR whose `origin` is a local
# bare repository created next to it, with GIT_CEILING_DIRECTORIES set so no git
# command can climb into an enclosing repository. A second bare remote, `mirror`,
# stands in for any other remote the clone has: it must never change. `gh` is a
# fake first on PATH that records its arguments: nothing here reaches a network.
# The plan files (sprint-N.md, journal.md) are fixtures handed over with --plan,
# so the suite needs no private documents.
#
# As in test-check-public-terms.sh, the real term list is never used: the cases
# plant a made-up word and hand the script a term list hashed from it at run time.
#
# Usage: bash scripts/tests/test-publish-sprint.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PUBLISH="$HERE/scripts/publish-sprint.sh"

if [ ! -f "$PUBLISH" ]; then
    echo "cannot find scripts/publish-sprint.sh (looked in $HERE)" >&2
    exit 1
fi

WORK="${TMPDIR:-/tmp}/logos-publish-sprint-test.$$"
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

# The fake gh: records one line per call, fails when GH_FAIL is set.
FAKEBIN="$WORK/bin"
GH_LOG="$WORK/gh.log"
mkdir -p "$FAKEBIN"
cat >"$FAKEBIN/gh" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$GH_LOG"
[ -z "${GH_FAIL:-}" ]
EOF
chmod +x "$FAKEBIN/gh"
export GH_LOG

PASS=0
FAIL=0
N=0
# The dates of main's tip: old and fixed, so a commit that takes any other date
# (the clock, HEAD's) cannot match them.
TIP_AUTHOR_DATE="1500000000 +0000"
TIP_COMMITTER_DATE="1500000100 +0000"

g() { git -c maintenance.auto=false -c gc.auto=0 "$@"; }

# The plan, as the plan files carry it: a sprint with five stories and a journal.
sprint_file() { # sprint_file <title-of-S-1>
    cat <<EOF
# Sprint 7

## 4. Execution Plan

### S-99: not a story section of this plan

- **Tracker:** github#999

## 5. Stories

### S-1: $1
- **Status:** DONE
- **Refs:** [FR-1](../../specs/requirements/FR-1.md)
- **Tracker:** github#12

#### Tasks

| # | Task |
|---|------|
| 1 | a task |

### S-2: The [linked](../journal.md#s-2) story   with  blanks
- **Status:** DONE
- **Refs:** none

### S-3: A private story
- **Status:** DONE
- **Tracker:** none

### S-4: An unmirrored story
- **Status:** DONE

### S-5: Mirrored in the journal only
- **Status:** DONE

## 6. References
EOF
}
journal_file() {
    cat <<'EOF'
# Journal

### S-1: A mirrored story
- **Status:** DONE
- **Tracker:** github#12

### S-2: The linked story
- **Status:** DONE
- **Refs:** none
- **Tracker:** github#13

### S-5: Mirrored in the journal only
- **Refs:** none
- **Tracker:** github#15

### S-77: In the journal, not in this sprint
- **Tracker:** github#77
EOF
}

# fixture -> sets CLONE, ORIGIN, MIRROR, PLAN. origin holds main at c1. The clone's
# main is c1 plus a merged branch sprint/7 (--no-ff), so main's tree differs from
# origin's and the sprint is merged. The plan holds the sprint and the journal.
fixture() {
    N=$((N + 1))
    local base="$WORK/case$N"
    ORIGIN="$base/origin.git"
    MIRROR="$base/mirror.git"
    CLONE="$base/clone"
    PLAN="$base/plan"
    mkdir -p "$base" "$PLAN/sprints"
    sprint_file "A mirrored story" >"$PLAN/sprints/sprint-7.md"
    journal_file >"$PLAN/journal.md"
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
        g add -A && g commit -qm c1
        g push -q origin main 2>/dev/null
        g remote add mirror "$MIRROR"
        g push -q mirror main 2>/dev/null
        g checkout -q -b sprint/7
        printf 'pub fn two() {}\n' >>src/lib.rs
        g commit -qam 'sprint work'
        g checkout -q main
        GIT_AUTHOR_DATE="$TIP_AUTHOR_DATE" GIT_COMMITTER_DATE="$TIP_COMMITTER_DATE" g merge -q --no-ff -m 'Merge sprint 7' sprint/7
    ) || {
        echo "  FAIL  could not populate the fixture clone $CLONE"
        exit 1
    }
}

# commit <message> <file> <content>: one more local commit on the clone's main.
commit() {
    (cd "$CLONE" && mkdir -p "$(dirname "$2")" && printf '%s' "$3" >"$2" && g add -A && g commit -qm "$1")
}

commits() { (cd "$CLONE" && git cat-file --batch-all-objects --batch-check='%(objecttype)' | grep -c '^commit$'); }
refs() { git -C "$1" for-each-ref --format='%(refname) %(objectname)'; }

snapshot() {
    BEFORE_CLONE="$(refs "$CLONE")"
    BEFORE_ORIGIN="$(refs "$ORIGIN")"
    BEFORE_MIRROR="$(refs "$MIRROR")"
    BEFORE_COMMITS="$(commits)"
    : >"$GH_LOG"
}
# run <args>: the script in the clone, with the fixture plan and the test term list.
# RUN_DIR (the directory it runs in), RUN_PATH (its PATH) and PUBLISH_UNDER (the
# script) override the defaults for one call: set them on the command line.
run() {
    snapshot
    OUT="$(cd "${RUN_DIR:-$CLONE}" && PATH="${RUN_PATH:-$FAKEBIN:$PATH}" bash "${PUBLISH_UNDER:-$PUBLISH}" --plan "$PLAN" --terms "$TERMS" --allow "$ALLOW" "$@" 2>&1)"
    RC=$?
    NEW_COMMITS=$(($(commits) - BEFORE_COMMITS))
}
# run_raw: exactly the arguments given.
run_raw() {
    snapshot
    OUT="$(cd "${RUN_DIR:-$CLONE}" && PATH="${RUN_PATH:-$FAKEBIN:$PATH}" bash "${PUBLISH_UNDER:-$PUBLISH}" "$@" 2>&1)"
    RC=$?
    NEW_COMMITS=$(($(commits) - BEFORE_COMMITS))
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

# expect <name> <exit> <new-commit-objects> [<text that must appear>]
#   ... and, always: no ref of the clone, origin or mirror moved, no gh call,
#   no scratch left behind, and the working tree untouched.
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
    if [ -s "$GH_LOG" ]; then fail "$name: gh was called"; return; fi
    if ls -d "$CLONE"/.git/publish-sprint.* >/dev/null 2>&1; then fail "$name: scratch left in .git"; return; fi
    if [ -n "$(cd "$CLONE" && git status --porcelain)" ]; then fail "$name: the working tree changed"; return; fi
    ok "$name"
}

squash_of() { printf '%s\n' "$OUT" | sed -n 's/^  squash   \([0-9a-f]*\)$/\1/p'; }
message_of() { git -C "$CLONE" log -1 --format=%B "$1"; }

echo "test-publish-sprint.sh"
echo "  work $WORK"
echo

# --- a dry run builds the one commit, and moves nothing ---------------------------

fixture
run --sprint 7
expect "a dry run writes exactly one commit object and moves no ref" 0 1 "stories  S-1 S-2 S-5"
S="$(squash_of)"
(
    cd "$CLONE" || exit 1
    [ "$(git rev-parse "$S^@")" = "$(git rev-parse origin/main)" ] || { echo "parent is not origin/main"; exit 1; }
    [ "$(git rev-parse "$S^{tree}")" = "$(git rev-parse 'main^{tree}')" ] || { echo "tree is not main's tree"; exit 1; }
    [ "$(git rev-list --count "$S")" -eq 2 ] || { echo "the commit does not sit directly on origin/main"; exit 1; }
    git tag --points-at "$S" | grep -q . && { echo "a tag points at the commit"; exit 1; }
    git for-each-ref --contains "$S" | grep -q . && { echo "a ref contains the commit"; exit 1; }
    exit 0
) >"$WORK/check.out" 2>&1
CHECK_RC=$?
if [ "$CHECK_RC" -eq 0 ]; then
    ok "its parent is origin/main, its tree main's tree, and no ref reaches it"
else
    OUT="$(cat "$WORK/check.out")"
    fail "the commit is not the one specified"
fi
if printf '%s\n' "$OUT" | grep -qxF "push: git push origin $S:refs/heads/sprint/7" &&
    printf '%s\n' "$OUT" | grep -qxF "pr:   gh pr create --base main --head sprint/7 --fill"; then
    ok "it prints the exact push and pull request commands"
else
    fail "the printed push or pull request command is not the expected one"
fi

# --- the message ----------------------------------------------------------------

WANT='Sprint 7: 3 mirrored stories

- A mirrored story (S-1)
  Closes #12
- The linked story with blanks (S-2)
  Closes #13
- Mirrored in the journal only (S-5)
  Closes #15'
GOT="$(message_of "$S" | sed -e :a -e '/^[[:space:]]*$/{$d;N;ba' -e '}')"
if [ "$GOT" = "$WANT" ]; then
    ok "the message lists each mirrored story once with its Closes line, from the sprint file and the journal, links stripped"
else
    OUT="$(printf 'wanted:\n%s\ngot:\n%s\n' "$WANT" "$GOT")"
    fail "the message is not the one specified (a private, an unmirrored, a non-sprint or a story of the journal only must be absent)"
fi

run --sprint 7
if [ "$(squash_of)" = "$S" ]; then
    ok "a second dry run builds the same commit id"
else
    fail "a second dry run built a different commit"
fi

fixture
sprint_file "A mirrored story" | sed 's/github#12/github#12 /' >"$PLAN/sprints/sprint-7.md"
run --sprint 7
expect "a Tracker line with trailing blanks still counts" 0 1 "stories  S-1 S-2 S-5"

fixture
printf '# Sprint 7\n\n## 5. Stories\n\n### S-4: A story\n- **Status:** DONE\n' >"$PLAN/sprints/sprint-7.md"
run --sprint 7
expect "a sprint with no mirrored story gets the one-line summary" 0 1 "stories  none mirrored"
S="$(squash_of)"
if [ "$(message_of "$S" | sed -e :a -e '/^[[:space:]]*$/{$d;N;ba' -e '}')" = "Sprint 7: publish the approved sprint" ]; then
    ok "that summary is one line"
else
    fail "the one-line summary is not one line"
fi

fixture
run_raw --sprint 7 --terms "$TERMS" --allow "$ALLOW"
expect "plan files that are absent give the one-line summary and say so" 0 1 "no plan file"

fixture
run --sprint 7 --plan "$WORK/no-such-plan"
expect "a --plan directory that does not hold the sprint file is refused, not summarised" 2 0 "--plan names a directory without it"

fixture
rm -f "$PLAN/journal.md"
run --sprint 7
expect "a sprint file without a journal still gives the stories it carries" 0 1 "stories  S-1"
if printf '%s\n' "$OUT" | grep -qx "  stories  S-1"; then
    ok "...and only those whose Tracker line the sprint file holds"
else
    fail "the stories line is not exactly S-1"
fi

fixture
mkdir -p "$CLONE/docs/planning/sprints"
sprint_file "A default-plan story" >"$CLONE/docs/planning/sprints/sprint-7.md"
(cd "$CLONE" && printf 'docs/planning\n' >>.git/info/exclude)
run_raw --sprint 7 --terms "$TERMS" --allow "$ALLOW"
expect "the default plan directory is docs/planning of the repository (no journal there: the sprint file's own Tracker lines)" 0 1 "A default-plan story"

# --- the plan files disagree ------------------------------------------------------

fixture
sed -i.bak 's/github#13/github#14/' "$PLAN/journal.md"
sed -i.bak 's/^### S-2: .*/&\n- **Tracker:** github#13/' "$PLAN/sprints/sprint-7.md"
run --sprint 7
expect "a story with two different issue numbers between the sprint file and the journal is refused" 2 0 "S-2 carries Tracker github#13"

fixture
sed -i.bak 's/^### S-1: .*/&\n- **Tracker:** github#21/' "$PLAN/sprints/sprint-7.md"
run --sprint 7
expect "two Tracker lines in one story are refused" 2 0 "more than one Tracker"

# --- the term gate ----------------------------------------------------------------

fixture
sprint_file "A $WORD story" >"$PLAN/sprints/sprint-7.md"
run --sprint 7
expect "a planted term in a story title aborts before any commit is written" 1 0 "the message carries a listed term"

fixture
commit "add a file" notes.txt "a $WORD here
"
run --sprint 7
expect "a planted term in the tree aborts before any commit is written" 1 0 "the release tree carries a listed term"

fixture
run --sprint 7 --terms "$WORK/missing.terms"
expect "a term gate that cannot run exits 3 and writes no commit" 3 0

# --- the commit takes main's dates ---------------------------------------------------

fixture
run --sprint 7
S="$(squash_of)"
WANT_DATES="$TIP_AUTHOR_DATE|$TIP_COMMITTER_DATE"
if [ "$(git -C "$CLONE" log -1 --format='%ad|%cd' --date=raw "$S")" = "$WANT_DATES" ]; then
    ok "the commit carries main's author and committer dates"
else
    OUT="$(git -C "$CLONE" log -1 --format='%ad|%cd' --date=raw "$S")"
    fail "the commit dates are not main's tip dates ($WANT_DATES)"
fi
fixture
(cd "$CLONE" && g checkout -q --detach HEAD~1)
run --sprint 7
S="$(squash_of)"
if [ "$(git -C "$CLONE" log -1 --format='%ad|%cd' --date=raw "$S")" = "$WANT_DATES" ]; then
    ok "...and they are main's, not those of a detached HEAD"
else
    fail "the commit dates follow HEAD instead of main"
fi

# --- where it runs -----------------------------------------------------------------

fixture
g -C "$CLONE" worktree add -q --detach "$WORK/case$N/wt" main
RUN_DIR="$WORK/case$N/wt" run --sprint 7
expect "it runs from a linked worktree, whose object store is the clone's" 0 1 "stories  S-1 S-2 S-5"

fixture
RUN_DIR="$CLONE/src" run_raw --sprint 7 --plan ../../plan --terms ../../../terms.sha256 --allow ../../../empty.allow
expect "from a subdirectory, relative --plan, --terms and --allow name files relative to it" 0 1 "stories  S-1 S-2 S-5"

# --- the plan files disagree or are malformed -----------------------------------------

fixture
printf '\n### S-1: One again\n- **Status:** DONE\n' >>"$PLAN/journal.md"
run --sprint 7
expect "a story the journal names twice, with a number on one of them, keeps that number" 0 1 "stories  S-1 S-2 S-5"

fixture
printf '\n### S-2: Again\n- **Tracker:** github#14\n' >>"$PLAN/journal.md"
run --sprint 7
expect "a story the journal names twice with different numbers is refused" 2 0 "more than one Tracker issue number"

fixture
sed -i.bak 's/^- \*\*Tracker:\*\* github#15$/&\n- **Tracker:** github#16/' "$PLAN/journal.md"
run --sprint 7
expect "two Tracker lines of one journal story are refused, not published as a number" 2 0 "more than one Tracker issue number"

fixture
sed -i.bak 's/^- \*\*Tracker:\*\* github#12$/- **Tracker:** github#12 (mirror)/' "$PLAN/sprints/sprint-7.md"
run --sprint 7
expect "a Tracker line of another shape is refused, not dropped" 2 0 "neither '- **Tracker:** github#N'"

fixture
sed -i.bak 's/^- \*\*Tracker:\*\* github#15$/  - **Tracker:** github#15/' "$PLAN/journal.md"
run --sprint 7
expect "...in the journal as well" 2 0 "neither '- **Tracker:** github#N'"

fixture
sprint_file "" >"$PLAN/sprints/sprint-7.md"
run --sprint 7
expect "a mirrored story with no title is refused" 2 0 "has no title"

fixture
sprint_file "Stop the crash (fixes #99)" >"$PLAN/sprints/sprint-7.md"
run --sprint 7
expect "a title that would close another issue is refused" 2 0 "closing keyword"

fixture
sprint_file "A mirrored story" | sed 's/$/\r/' >"$PLAN/sprints/sprint-7.md"
run --sprint 7
expect "a sprint file with CRLF line ends reads like one with LF" 0 1 "stories  S-1 S-2 S-5"
S="$(squash_of)"
if message_of "$S" | grep -q "$(printf '\r')"; then
    fail "a carriage return reached the message"
else
    ok "...and no carriage return reaches the message"
fi

fixture
{
    sprint_file "A mirrored story" | sed '/^### S-3:/i\
```\
### S-9: Shown in a code block\
- **Tracker:** github#99\
```
'
    printf '\n## 6. Stories deferred to later\n\n### S-8: Deferred\n- **Tracker:** github#98\n'
} >"$PLAN/sprints/sprint-7.md"
run --sprint 7
if [ "$RC" -eq 0 ] && printf '%s\n' "$OUT" | grep -qx "  stories  S-1 S-2 S-5"; then
    ok "a story in a code block, or in another section whose title holds 'Stories', is not the sprint's"
else
    fail "a story outside the Stories section, or in a code block, was listed"
fi

# --- the term gate, further ---------------------------------------------------------

fixture
sprint_file "A $WORD story" >"$PLAN/sprints/sprint-7.md"
LINE_SHA="$(sha "- A $WORD story (S-1)")"
printf 'CHANGELOG.md:%s\n' "$LINE_SHA" >"$WORK/changelog.allow"
printf 'SPRINT.md:%s\n' "$LINE_SHA" >"$WORK/sprint.allow"
run_raw --sprint 7 --plan "$PLAN" --terms "$TERMS" --allow "$WORK/changelog.allow"
expect "an allowlist entry for the same line of CHANGELOG.md does not admit the message" 1 0 "the message carries a listed term"
run_raw --sprint 7 --plan "$PLAN" --terms "$TERMS" --allow "$WORK/sprint.allow"
expect "control: an entry for the message's own file name does" 0 1 "stories  S-1 S-2 S-5"

fixture
FAKE="$WORK/case$N/fake"
mkdir -p "$FAKE"
cp -R "$HERE/scripts" "$FAKE/scripts"
printf '#!/usr/bin/env bash\necho "public-terms: tracked=2 untracked=0 scanned=2 binary=0 skipped=0 hits=0 allowlisted=0"\n' >"$FAKE/scripts/check-public-terms.sh"
PUBLISH_UNDER="$FAKE/scripts/publish-sprint.sh" run --sprint 7
expect "a scanner that read more files than the message holds is refused" 3 0 "did not walk the message's 1 file(s)"
printf '#!/usr/bin/env bash\necho "public-terms: tracked=1 untracked=1 scanned=2 binary=0 skipped=0 hits=0 allowlisted=0"\n' >"$FAKE/scripts/check-public-terms.sh"
PUBLISH_UNDER="$FAKE/scripts/publish-sprint.sh" run --sprint 7
expect "a scanner that read an untracked file is refused" 3 0 "did not walk the message's 1 file(s)"

# --- the refusals -----------------------------------------------------------------

fixture
(cd "$CLONE" && g checkout -q -b other origin/main 2>/dev/null && printf 'x\n' >extra.txt && g add -A && g commit -qm 'public change' &&
    g push -q origin other:main 2>/dev/null && g fetch -q origin && g checkout -q main)
run --sprint 7
expect "origin/main not an ancestor of main is refused" 2 0 "is not an ancestor of"

fixture
(cd "$CLONE" && g branch -q sprint/8 main~1 && g checkout -q sprint/8 && printf 'y\n' >y.txt && g add -A && g commit -qm 'unmerged work' && g checkout -q main)
run --sprint 8
expect "a sprint branch not merged into main is refused" 2 0 "is not merged into main"

fixture
run --sprint 9
expect "a sprint without a local branch is refused" 2 0 "no local branch sprint/9"

fixture
run_raw --sprint 9 --unbranched --terms "$TERMS" --allow "$ALLOW"
expect "--unbranched accepts a sprint that ran before sprint branches" 0 1 "sprint 9 over origin/main"

fixture
(cd "$CLONE" && g branch -q sprint/8 main~1 && g checkout -q sprint/8 && printf 'y\n' >y.txt && g add -A && g commit -qm 'unmerged work' && g checkout -q main)
run --sprint 8 --unbranched
expect "--unbranched relaxes only the missing branch, not an unmerged one" 2 0 "is not merged into main"

fixture
(cd "$CLONE" && g reset -q --hard origin/main && g branch -q -D sprint/7)
run --sprint 7 --unbranched
expect "a main whose tree equals origin/main's has nothing to publish" 2 0 "nothing to publish"

fixture
(cd "$CLONE" && g checkout -q --detach HEAD~1)
run --sprint 7
expect "a detached HEAD does not matter: the published tree is local main's" 0 1 "stories  S-1 S-2 S-5"
S="$(squash_of)"
if [ "$(git -C "$CLONE" rev-parse "$S^{tree}")" = "$(git -C "$CLONE" rev-parse 'main^{tree}')" ]; then
    ok "...and that tree is main's, not HEAD's"
else
    fail "the tree is not local main's"
fi

fixture
run --sprint 7
S="$(squash_of)"
(cd "$CLONE" && g update-ref refs/remotes/origin/sprint/7 origin/main)
run --sprint 7
expect "a remote-tracking sprint branch at a different commit is refused" 2 0 "refusing to move or reuse a published sprint branch"
(cd "$CLONE" && g update-ref refs/remotes/origin/sprint/7 "$S")
run --sprint 7
expect "a remote-tracking sprint branch at the commit built is accepted" 0 0 "sprint 7 over origin/main"

fixture
run_raw
expect "no --sprint is a usage error" 2 0 "Usage:"
run_raw --sprint 0 --terms "$TERMS"
expect "--sprint 0 is refused" 2 0 "must be a sprint number"
run_raw --sprint seven --terms "$TERMS"
expect "--sprint seven is refused" 2 0 "must be a sprint number"
run_raw --sprint 7 --bogus
expect "an unknown option is refused" 2 0 "unknown argument: --bogus"
run_raw --sprint
expect "an option without its value is a usage error" 2 0 "Usage:"

# --- --push -----------------------------------------------------------------------

fixture
run --sprint 7
S="$(squash_of)"
run --sprint 7 --push
if [ "$RC" -ne 0 ]; then
    fail "--push: exit $RC"
elif [ "$(git -C "$ORIGIN" rev-parse refs/heads/sprint/7)" != "$S" ]; then
    fail "--push: origin's sprint/7 is not the commit the dry run built"
elif [ "$(git -C "$ORIGIN" rev-parse refs/heads/main)" != "$(git -C "$CLONE" rev-parse origin/main)" ]; then
    fail "--push: origin's main moved"
elif [ "$(refs "$MIRROR")" != "$BEFORE_MIRROR" ]; then
    fail "--push: the mirror moved"
elif [ "$(git -C "$ORIGIN" for-each-ref refs/tags | wc -l | tr -d ' ')" -ne 0 ]; then
    fail "--push: a tag was pushed"
elif [ "$(cat "$GH_LOG")" != "pr create --base main --head sprint/7 --fill" ]; then
    OUT="$(cat "$GH_LOG")"
    fail "--push: gh was not called exactly once with the printed arguments"
else
    ok "--push pushes the commit to sprint/7 on origin, never main, and opens one pull request"
fi

run --sprint 7 --push
if [ "$RC" -eq 0 ] && printf '%s\n' "$OUT" | grep -q "already has sprint/7" && [ "$(wc -l <"$GH_LOG" | tr -d ' ')" -eq 1 ]; then
    ok "a re-run after the push lands skips the push and retries the pull request"
else
    fail "a re-run after the push did not skip the push"
fi

fixture
(
    other="$WORK/other$N"
    g clone -q "$ORIGIN" "$other" 2>/dev/null && cd "$other" && git config user.email o@example.com && git config user.name o &&
        g checkout -q -b sprint/7 && printf 'z\n' >z.txt && g add -A && g commit -qm 'someone else' && g push -q origin sprint/7 2>/dev/null
)
BEFORE_ORIGIN_SPRINT="$(git -C "$ORIGIN" rev-parse refs/heads/sprint/7)"
run --sprint 7 --push
if [ "$RC" -eq 2 ] && [ "$(git -C "$ORIGIN" rev-parse refs/heads/sprint/7)" = "$BEFORE_ORIGIN_SPRINT" ] && [ ! -s "$GH_LOG" ] &&
    printf '%s\n' "$OUT" | grep -q "refusing to move or reuse"; then
    ok "--push refuses when origin's sprint/7 is a different commit, moving nothing and opening no pull request"
else
    fail "--push over a different remote sprint/7: exit $RC"
fi

fixture
(
    other="$WORK/other$N"
    g clone -q "$ORIGIN" "$other" 2>/dev/null && cd "$other" && git config user.email o@example.com && git config user.name o &&
        printf 'w\n' >w.txt && g add -A && g commit -qm 'public change' && g push -q origin main 2>/dev/null
)
run --sprint 7 --push
if [ "$RC" -eq 2 ] && [ -z "$(git -C "$ORIGIN" for-each-ref refs/heads/sprint)" ] && [ ! -s "$GH_LOG" ] &&
    printf '%s\n' "$OUT" | grep -q "not the local origin/main"; then
    ok "--push refuses when origin's main moved since the last fetch"
else
    fail "--push over a moved origin main: exit $RC"
fi

fixture
mkdir -p "$ORIGIN/hooks"
printf '#!/bin/sh\nexit 1\n' >"$ORIGIN/hooks/pre-receive"
chmod +x "$ORIGIN/hooks/pre-receive"
run --sprint 7 --push
if [ "$RC" -eq 4 ] && [ ! -s "$GH_LOG" ] && [ -z "$(git -C "$ORIGIN" for-each-ref refs/heads/sprint)" ]; then
    ok "a rejected push exits 4 and opens no pull request"
else
    fail "a rejected push: exit $RC"
fi

fixture
GH_FAIL=1 run --sprint 7 --push
if [ "$RC" -eq 5 ] && [ -n "$(git -C "$ORIGIN" for-each-ref refs/heads/sprint/7)" ] && printf '%s\n' "$OUT" | grep -q "re-run to retry"; then
    ok "a failed pull request exits 5 with the branch pushed"
else
    fail "a failed pull request: exit $RC"
fi

fixture
if PATH=/usr/bin:/bin command -v gh >/dev/null 2>&1 || ! PATH=/usr/bin:/bin command -v git >/dev/null 2>&1; then
    ok "skipped: gh or no git on the minimal PATH of this machine"
else
    RUN_PATH=/usr/bin:/bin run --sprint 7 --push
    if [ "$RC" -eq 2 ] && [ -z "$(git -C "$ORIGIN" for-each-ref refs/heads/sprint)" ] && printf '%s\n' "$OUT" | grep -q "gh is not on PATH"; then
        ok "--push without gh on PATH refuses before pushing anything"
    else
        fail "--push without gh: exit $RC"
    fi
fi

fixture
run --sprint 7 --push --terms "$WORK/missing.terms"
if [ "$RC" -eq 3 ] && [ -z "$(git -C "$ORIGIN" for-each-ref refs/heads/sprint)" ] && [ ! -s "$GH_LOG" ]; then
    ok "--push with a term gate that cannot run pushes nothing"
else
    fail "--push with a broken term gate: exit $RC"
fi

fixture
sprint_file "A $WORD story" >"$PLAN/sprints/sprint-7.md"
run --sprint 7 --push
if [ "$RC" -eq 1 ] && [ -z "$(git -C "$ORIGIN" for-each-ref refs/heads/sprint)" ] && [ ! -s "$GH_LOG" ]; then
    ok "--push with a planted term pushes nothing"
else
    fail "--push with a planted term: exit $RC"
fi

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ]
