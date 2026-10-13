#!/usr/bin/env bash
#
# publish-sprint.sh — publish one approved sprint to origin as a single commit
# over origin/main, to be reviewed as one pull request (NFR-CR-04).
#
# WHAT IT BUILDS
#   One commit whose parent is origin/main and whose tree is the tree of local
#   main, the sprint already merged into it. Its message is a neutral summary of
#   the sprint: the title of each story that carries a `- **Tracker:** github#N`
#   line, with `Closes #N`. A sprint with no such story gets a one-line summary.
#   Stories are the `### S-NNN: title` sections of the sprint's plan file
#   (docs/planning/sprints/sprint-N.md); a Tracker line is read from the story's
#   section there and in the journal (docs/planning/journal.md), and the two must
#   agree. `Tracker: none` marks a story that is not mirrored. The plan files are
#   private and absent from a public checkout: without them the sprint gets the
#   one-line summary and the output says so. The commit carries the author and
#   committer dates of local main's tip, so a dry run and the later --push build
#   the same commit id.
#
# WHAT IT CHECKS FIRST (exit 2, before any object is written)
#   - origin/main is an ancestor of local main: a commit built over origin/main
#     from a tree that lacks something merged there would publish it reverted;
#   - the local branch sprint/N exists and is merged into local main (see
#     --unbranched for a sprint that ran before sprint/N branches);
#   - local main's tree differs from origin/main's: otherwise there is nothing to
#     review;
#   - the plan files are sound: a --plan directory holds sprint-N.md; a story's
#     Tracker bullet is `github#N` or `none`; its two sources agree on one number;
#     a mirrored story has a title, and none that names an issue after a closing
#     keyword (it would close that issue when the pull request merges).
#   Then scripts/check-public-terms.sh runs on the message, then on the tree,
#   exactly as they would be published (read from the commit, never from the
#   working directory). A hit aborts before any commit object is written (exit 1).
#   Last, once the commit id is known, the remote-tracking branch origin/sprint/N,
#   when present, must be that commit (exit 2). That one check follows the build,
#   so its refusal leaves the unreferenced commit object behind; with --push the
#   branch on origin itself is read instead and must be absent or that commit.
#
# DRY RUN BY DEFAULT
#   Without --push the script writes one unreferenced commit object and nothing
#   else: no branch, no tag, no remote call. It prints the commit id, the message
#   and the exact `git push` and `gh pr create` it would run. Local main, every
#   existing ref, local history and every remote are never moved in either mode.
#
# --push
#   Checks that origin can be read and its main still equals the local
#   origin/main (so the parent is current) and that its branch sprint/N is absent
#   or already the commit built; pushes the commit to refs/heads/sprint/N (never
#   forced), then opens the pull request against main with `gh pr create --fill`
#   (one commit, so title and body are its message). A push that already landed
#   (a re-run) skips straight to the pull request. Only the owner runs --push.
#   The pull request is merged on GitHub with "Rebase and merge"; afterwards run
#   `git fetch origin && git merge origin/main` to restore the local ancestry.
#
# Usage:
#   bash scripts/publish-sprint.sh --sprint N [--push] [--unbranched]
#                                  [--plan DIR] [--terms FILE] [--allow FILE]
#
#   Run it from inside the repository (a subdirectory or a linked worktree will
#   do). --plan names the directory holding journal.md and sprints/ (docs/planning
#   of the repository by default; when absent there, the one-line summary). --terms
#   and --allow are handed to check-public-terms.sh unchanged (its own defaults
#   otherwise). Relative paths among the options are relative to where you stand.
#   --unbranched accepts a sprint without a local sprint/N branch, for one that
#   ran before sprint branches; it is the only check it relaxes.
#
# Exit codes:
#   0  dry run printed, or the push and the pull request succeeded
#   1  the term gate found a listed term in the message or the tree; nothing built
#   2  usage error, or a precondition above does not hold
#   3  the term gate could not run, or read a different number of files than
#      the message or the tree holds
#   4  the push failed
#   5  the branch is on origin but the pull request could not be created; re-run
#      to retry it

set -euo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
CHECK="$HERE/check-public-terms.sh"
CORE="$HERE/lib/publish-core.sh"
REMOTE=origin
SPRINT=""
PUSH=no
UNBRANCHED=no
PLAN=""
GATE_ARGS=()
PUBLISH_PROG=publish-sprint.sh
MESSAGE_FILE=SPRINT.md

# A path argument names a file relative to the caller, who may stand in a
# subdirectory: the script moves to the repository root before it reads any.
absolute() {
    case "$1" in
        /*) printf '%s\n' "$1" ;;
        *) printf '%s/%s\n' "$PWD" "$1" ;;
    esac
}

usage() {
    sed -n '/^# Usage:/,/it is the only check it relaxes/p' "${BASH_SOURCE[0]}" | sed 's/^# \{0,1\}//' >&2
    exit 2
}

# The shared checks and the commit builder; it defines die.
[ -f "$CORE" ] || { echo "publish-sprint.sh: cannot find $CORE" >&2; exit 2; }
# shellcheck source=lib/publish-core.sh
. "$CORE"

while [ $# -gt 0 ]; do
    case "$1" in
        --sprint)
            [ $# -ge 2 ] || usage
            SPRINT="$2"
            shift 2
            ;;
        --plan)
            [ $# -ge 2 ] || usage
            PLAN="$(absolute "$2")"
            shift 2
            ;;
        --push)
            PUSH=yes
            shift
            ;;
        --unbranched)
            UNBRANCHED=yes
            shift
            ;;
        --terms | --allow)
            [ $# -ge 2 ] || usage
            GATE_ARGS+=("$1" "$(absolute "$2")")
            shift 2
            ;;
        -h | --help) usage ;;
        *)
            echo "publish-sprint.sh: unknown argument: $1" >&2
            usage
            ;;
    esac
done

[ -n "$SPRINT" ] || usage
[[ "$SPRINT" =~ ^[1-9][0-9]*$ ]] || die 2 "--sprint must be a sprint number (got '$SPRINT')"
[ -f "$CHECK" ] || die 2 "cannot find $CHECK"

# The repository is the one the caller stands in; nothing in the environment may
# point git somewhere else.
unset GIT_DIR GIT_WORK_TREE GIT_INDEX_FILE GIT_OBJECT_DIRECTORY GIT_ALTERNATE_OBJECT_DIRECTORIES
ROOT="$(git rev-parse --show-toplevel 2>/dev/null)" || die 2 "not inside a git repository"
cd "$ROOT"
PLAN_GIVEN=yes
[ -n "$PLAN" ] || { PLAN="$ROOT/docs/planning"; PLAN_GIVEN=no; }

BRANCH="sprint/$SPRINT"
RELEASE="$(git rev-parse --verify --quiet "refs/heads/main^{commit}")" || die 2 "no local main branch"
TREE="$(git rev-parse "$RELEASE^{tree}")"
PARENT="$(git rev-parse --verify --quiet "refs/remotes/$REMOTE/main^{commit}")" \
    || die 2 "no $REMOTE/main: fetch $REMOTE first"

# --- the preconditions ------------------------------------------------------------
publish_require_merged "$RELEASE"
if LOCAL_BRANCH="$(git rev-parse --verify --quiet "refs/heads/$BRANCH^{commit}")"; then
    git merge-base --is-ancestor "$LOCAL_BRANCH" "$RELEASE" \
        || die 2 "$BRANCH ($LOCAL_BRANCH) is not merged into main: merge the sprint into main first"
elif [ "$UNBRANCHED" = no ]; then
    die 2 "no local branch $BRANCH, so nothing shows sprint $SPRINT is merged into main (a sprint that ran before sprint branches: --unbranched)"
fi
[ "$TREE" != "$(git rev-parse "$PARENT^{tree}")" ] \
    || die 2 "main's tree equals $REMOTE/main's: there is nothing to publish"

# --- the message ------------------------------------------------------------------
US=$'\037'
# sprint_stories <file> [<heading-word>]: one `S-NNN<US>title<US>number<US>bad`
# line per story section of a plan file (US is the unit separator, which `read`
# does not collapse the way it collapses runs of blanks). number is empty without
# a `- **Tracker:** github#N` line and CONFLICT with two different ones; bad is 1
# when the section holds a Tracker bullet of any other shape (a mirrored story
# dropped for a typo must not pass unseen). With a heading word, only the `## `
# section titled "<n>. <word>" is read (a sprint file lists its stories under
# "Stories"; its other sections mention stories too). Fenced code is skipped,
# CRLF line ends are read as LF, and titles lose markdown links (the text stays)
# and runs of blanks.
sprint_stories() {
    awk -v scope="${2:-}" '
        BEGIN { on = (scope == ""); US = "\037" }
        function flush() { if (id != "") print id US title US num US bad; id = ""; num = ""; bad = "" }
        { sub(/\r$/, "") }
        /^[ \t]*(```|~~~)/ { fence = !fence; next }
        fence { next }
        $1 == "##" { flush(); on = (scope == "" || $0 ~ ("^## +([0-9.]+ +)?" scope " *$")); next }
        $1 ~ /^#+$/ && length($1) <= 3 && !/^### S-[0-9]+: / { flush(); next }
        /^---[ \t]*$/ { flush(); next }
        /^### S-[0-9]+: / {
            flush()
            if (!on) next
            id = $2; sub(/:$/, "", id)
            title = $0; sub(/^### S-[0-9]+: +/, "", title); sub(/[ \t]+$/, "", title)
            while (match(title, /\[[^]]*\]\([^)]*\)/)) {
                link = substr(title, RSTART, RLENGTH)
                text = link; sub(/\]\(.*$/, "", text); sub(/^\[/, "", text)
                title = substr(title, 1, RSTART - 1) text substr(title, RSTART + RLENGTH)
            }
            gsub(/[ \t]+/, " ", title)
            next
        }
        id != "" && /^- \*\*Tracker:\*\* +github#[0-9]+[ \t]*$/ {
            n = $0; sub(/^- \*\*Tracker:\*\* +github#/, "", n); sub(/[ \t]+$/, "", n)
            if (num != "" && num != n) num = "CONFLICT"; else num = n
            next
        }
        id != "" && /^- \*\*Tracker:\*\* +none[ \t]*$/ { next }
        id != "" && /^[ \t]*[-*] +\*\*Tracker/ { bad = 1 }
        END { flush() }
    ' "$1"
}

SPRINT_FILE="$PLAN/sprints/sprint-$SPRINT.md"
JOURNAL="$PLAN/journal.md"
STORIES=""
PLAN_NOTE=""
if [ -f "$SPRINT_FILE" ]; then
    # The journal is read only for the stories the sprint lists.
    SPRINT_ROWS="$(sprint_stories "$SPRINT_FILE" Stories)"
    JOURNAL_ROWS=""
    [ -f "$JOURNAL" ] && JOURNAL_ROWS="$(sprint_stories "$JOURNAL")"
    while IFS=$US read -r id title num bad; do
        [ -n "$id" ] || continue
        # The journal's rows for this story: its issue numbers (a story named twice
        # may carry a number on one of them only) and whether any is malformed.
        jnum="$(printf '%s\n' "$JOURNAL_ROWS" | awk -F"$US" -v id="$id" '$1 == id && $3 != "" { print $3 }' | sort -u)"
        jbad="$(printf '%s\n' "$JOURNAL_ROWS" | awk -F"$US" -v id="$id" '$1 == id && $4 != "" { print "bad" }' | head -n 1)"
        [ -z "$bad$jbad" ] || die 2 "$id has a Tracker line that is neither '- **Tracker:** github#N' nor '- **Tracker:** none' (in $SPRINT_FILE or $JOURNAL): fix it so the story is not dropped unseen"
        [ "$num" != CONFLICT ] && [ "$(printf '%s\n' "$jnum" | grep -c .)" -le 1 ] && [ "$jnum" != CONFLICT ] \
            || die 2 "$id carries more than one Tracker issue number between $SPRINT_FILE and $JOURNAL"
        if [ -n "$num" ] && [ -n "$jnum" ] && [ "$num" != "$jnum" ]; then
            die 2 "$id carries Tracker github#$num in $SPRINT_FILE and github#$jnum in $JOURNAL"
        fi
        num="${num:-$jnum}"
        [ -n "$num" ] || continue
        [ -n "$title" ] || die 2 "$id carries Tracker github#$num but has no title in $SPRINT_FILE"
        # A closing keyword in a title would close another issue when the pull request merges.
        if printf '%s\n' "$title" | grep -Eiq '(clos(e|es|ed)|fix(es|ed)?|resolv(e|es|ed))[: ]+#[0-9]+'; then
            die 2 "the title of $id names an issue after a closing keyword and would close it when the pull request merges: $title"
        fi
        STORIES="${STORIES}${id}${US}${title}${US}${num}"$'\n'
    done <<<"$SPRINT_ROWS"
elif [ "$PLAN_GIVEN" = yes ]; then
    die 2 "no plan file $SPRINT_FILE: --plan names a directory without it"
else
    PLAN_NOTE="no plan file $SPRINT_FILE: the message is the one-line summary"
fi

MESSAGE="Sprint $SPRINT: publish the approved sprint"
STORY_IDS="none mirrored"
if [ -n "$STORIES" ]; then
    count="$(printf '%s' "$STORIES" | grep -c .)"
    MESSAGE="Sprint $SPRINT: $count mirrored stor$([ "$count" -eq 1 ] && echo y || echo ies)"$'\n'
    STORY_IDS=""
    while IFS=$US read -r id title num; do
        [ -n "$id" ] || continue
        MESSAGE="$MESSAGE"$'\n'"- $title ($id)"$'\n'"  Closes #$num"
        STORY_IDS="${STORY_IDS:+$STORY_IDS }$id"
    done <<<"$STORIES"
fi

# Scratch files live under this repository's git directory, so nothing appears in
# the working tree and nothing is read from it.
SCRATCH="$(git rev-parse --absolute-git-dir)/publish-sprint.$$"
OBJECTS="$(cd "$(git rev-parse --git-common-dir)" && pwd -P)/objects"
cleanup() { rm -rf "$SCRATCH"; }
trap cleanup EXIT
trap 'exit 130' INT TERM
rm -rf "$SCRATCH"
mkdir -p "$SCRATCH"

echo "publish-sprint.sh: sprint $SPRINT over $REMOTE/main"
echo "  release  $RELEASE (main, tree $TREE)"
echo "  parent   $PARENT ($REMOTE/main)"
echo "  stories  $STORY_IDS"
[ -z "$PLAN_NOTE" ] || echo "  note     $PLAN_NOTE"
publish_commit_message
echo "  squash   $SQUASH"

# The branch origin's tracking ref already holds must be this commit. Offline: the
# dry run makes no remote call, and --push reads origin itself below.
TRACKED="$(git rev-parse --verify --quiet "refs/remotes/$REMOTE/$BRANCH^{commit}" || true)"
[ -z "$TRACKED" ] || [ "$TRACKED" = "$SQUASH" ] \
    || die 2 "$REMOTE/$BRANCH is $TRACKED, not the commit built ($SQUASH): refusing to move or reuse a published sprint branch"

PUSH_CMD=(git push "$REMOTE" "$SQUASH:refs/heads/$BRANCH")
PR_CMD=(gh pr create --base main --head "$BRANCH" --fill)

echo
echo "message:"
printf '%s\n' "$MESSAGE" | sed 's/^/    /'
echo
echo "push: $(quoted "${PUSH_CMD[@]}")"
echo "pr:   $(quoted "${PR_CMD[@]}")"

if [ "$PUSH" = no ]; then
    echo
    echo "dry run: the commit above is unreferenced; no branch, pull request or remote was touched."
    echo "Re-run with --push to push it and open the pull request."
    exit 0
fi

# --- --push ---------------------------------------------------------------------------
# Every read is checked: an unreachable remote is a failed precondition (exit 2),
# never git's own status.
# The pull request is the last step and cannot be undone by a re-run, so what it
# needs is checked before the branch leaves this machine.
command -v gh >/dev/null 2>&1 || die 2 "gh is not on PATH: nothing was pushed"
REMOTE_MAIN="$(git ls-remote "$REMOTE" refs/heads/main)" || die 2 "cannot read $REMOTE's refs"
REMOTE_MAIN="${REMOTE_MAIN%%[[:space:]]*}"
[ "$REMOTE_MAIN" = "$PARENT" ] \
    || die 2 "$REMOTE's main is ${REMOTE_MAIN:-absent}, not the local $REMOTE/main $PARENT: fetch $REMOTE and re-run"
REMOTE_BRANCH="$(git ls-remote "$REMOTE" "refs/heads/$BRANCH")" || die 2 "cannot read $REMOTE's branches"
REMOTE_BRANCH="${REMOTE_BRANCH%%[[:space:]]*}"
if [ -z "$REMOTE_BRANCH" ]; then
    "${PUSH_CMD[@]}" || die 4 "the push failed"
    echo "pushed $BRANCH ($SQUASH) to $REMOTE"
elif [ "$REMOTE_BRANCH" = "$SQUASH" ]; then
    echo "$REMOTE already has $BRANCH at $SQUASH; not pushing again"
else
    die 2 "$REMOTE's $BRANCH is $REMOTE_BRANCH, not the commit built ($SQUASH): refusing to move or reuse a published sprint branch"
fi
"${PR_CMD[@]}" || die 5 "$BRANCH is on $REMOTE but the pull request could not be created: re-run to retry it"
echo "opened the pull request for $BRANCH"
