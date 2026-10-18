#!/usr/bin/env bash
#
# stop.sh — the logos plugin's Stop hook (FR-IN-13, ADR-82): where the agent ends
# its turn, check what the turn did to the quality contract, within a budget.
#
#   off switch (LOGOS_STOP_HOOK_DISABLE=1)   exit 0, no output
#   not enrolled (no .logos/)                exit 0, no output — logos is never run
#   stop_hook_active                         exit 0, no output: it blocked once already
#   no tracked source changed since the      exit 0, no output
#     previous run
#   otherwise the LEGS run behind one budget (LOGOS_STOP_HOOK_BUDGET_MS, 15000):
#     clean                                  no output; the stop proceeds
#     a regression                           {"decision":"block","reason":…}: each new
#                                            or worsened entry with its symbol, value,
#                                            threshold and trust, each failing rule,
#                                            and a pointer to /logos:review
#     a new secret, or a new finding at      {"decision":"block","reason":…}: each with
#       high or above, in a changed file     its rule, file, line and redacted message,
#                                            and a pointer to /logos:sec-review
#     over budget, or a leg could not run    {"systemMessage":"… not measured …"}, and
#                                            the stop proceeds: a leg that was not
#                                            measured never blocks by itself (a
#                                            regression another leg did find does,
#                                            with the unmeasured leg named)
#
# Report-only unless a leg found a regression: every path exits 0.
#
# The host does not block "once" by itself (it overrides after eight continuations
# in a row), so the once-only behaviour is this script's: a payload with
# stop_hook_active true is answered with silence before anything is measured.
#
# Tracked source changed: the fingerprint of HEAD, of `git diff HEAD` and of every
# untracked file the repository does not ignore — documentation and media files
# left out — differs from the one the previous run recorded in
# .logos/stop-hook/state. With no record, only a dirty tree runs. A turn that
# changed only documentation or media runs the security leg alone, on its own
# fingerprint (only .logos/ left out). A turn that was
# not measured because the before-state is still being built records nothing, so
# the next turn is measured; a turn over budget records, so a slow repository
# is not re-measured every turn.
#
# The before-state (ADR-82) is the merge-base's: a recorded snapshot, else a cached
# shadow index, and a cold one costs one full index of that tree (about 25 s). The
# hook does not pay that inside its budget. It runs `review` only when it can tell
# that `review` will find a before-state ready:
#   - a build that finished here under the same key. A background build leaves
#     .logos/stop-hook/ready-<merge-base> holding the key it ran under: the logos
#     version and the contract and configuration files. A changed key means a
#     before-state `review` would rebuild, so it is cold again;
#   - or, when `check` evaluated no rule, a snapshot recorded at the merge-base. That
#     is what `review` reads first, but only under the thresholds it was recorded
#     under; a snapshot recorded under other thresholds makes `review` build a
#     shadow index after all, and the budget then cuts it (the build restarts in the
#     background).
# Otherwise the hook starts the build detached, at most one at a time, and reports
# "not measured". A build that fails leaves failed-<merge-base> with the same key and
# is not retried until the key changes.
# `review` does not sync, so the check leg runs first: `check` reconciles the
# working tree, and the review then reads it fresh.
#
# Legs. LEGS names the legs, run in order; each is a function leg_<name> that
#   - spends time only through run_bounded CMD… (sets RB_STATUS done|timeout|spent,
#     RB_RC and RB_OUT; it never runs past the shared budget),
#   - reports what it found with leg_finding LINE (listed in the reason),
#     leg_blocking [SUBJECT] (the finding is a regression that blocks; SUBJECT names
#     it in the reason's first line) and leg_pointer TEXT (what to run about it),
#     and what it could not measure with leg_unmeasured TEXT.
# The legs are check and review (the quality contract) and security (FR-SC-20,
# ADR-83: a scoped scan of the changed files, only with recorded consent).
# A new leg is a function and a word in LEGS: the budget, the once-only rule, the
# state, the output and the exit code are this script's, not the leg's.
#
#   off-switch: export LOGOS_STOP_HOOK_DISABLE=1
#   budget:     export LOGOS_STOP_HOOK_BUDGET_MS=<milliseconds>

set -euo pipefail
set -E
# Bytes, not characters: the JSON is parsed by length, and a multi-byte locale makes
# bash 3.2 slower at it.
export LC_ALL=C
trap 'exit 0' ERR

case "${BASH_SOURCE[0]}" in
    */*) HOOK_DIR="${BASH_SOURCE[0]%/*}" ;;
    *) HOOK_DIR=. ;;
esac
# shellcheck source=SCRIPTDIR/lib/common.sh
. "$HOOK_DIR/lib/common.sh"
# Every logos call this hook makes is stamped with it (FR-OB-15): `stats --json`
# counts the calls the plugin caused apart from the agent's own.
export LOGOS_INVOKER=hook:stop

[ "${LOGOS_STOP_HOOK_DISABLE:-0}" = "1" ] && exit 0

PROJECT="$(logos_project_dir)"
logos_enrolled "$PROJECT" || exit 0

# The payload is one JSON object on stdin; only stop_hook_active is read.
PAYLOAD=""
IFS= read -r -d '' -t 5 PAYLOAD || true
ACTIVE_RE='"stop_hook_active"[[:space:]]*:[[:space:]]*true'
[[ $PAYLOAD =~ $ACTIVE_RE ]] && exit 0

command -v git >/dev/null 2>&1 || exit 0
# An ERR trap fires inside $( ) too and turns its failure into success, so a
# failing substitution is judged by what it printed, never by its status.
BIN="$(logos_bin || true)"
[ -n "$BIN" ] || exit 0
cd "$PROJECT" 2>/dev/null || exit 0
git rev-parse --verify --quiet HEAD >/dev/null 2>&1 || exit 0

STATE_DIR="$PROJECT/.logos/stop-hook"
STATE="$STATE_DIR/state"
LOCK="$STATE_DIR/build.lock"
mkdir -p "$STATE_DIR" 2>/dev/null || exit 0
# The hook's records are machine-local: the directory ignores itself, so a project
# whose .logos/.gitignore does not know it never sees them as untracked.
[ -f "$STATE_DIR/.gitignore" ] || printf '*\n' >"$STATE_DIR/.gitignore"
trap 'rm -f "$STATE_DIR"/run.$$.out "$STATE_DIR"/run.$$.err' EXIT

BUDGET_MS="${LOGOS_STOP_HOOK_BUDGET_MS:-15000}"
case "$BUDGET_MS" in *[!0-9]*) BUDGET_MS=15000 ;; esac
# Base 10: 015 is fifteen, and 08 is eight, not an octal error.
BUDGET_MS=$((10#$BUDGET_MS))
# hooks.json gives this hook 30 seconds: past that the host ends it mid-leg, which
# reads as silence rather than "not measured". The budget stops short of it.
BUDGET_MAX_MS=25000
[ "$BUDGET_MS" -le "$BUDGET_MAX_MS" ] || BUDGET_MS=$BUDGET_MAX_MS
POLL_MS=50
SPENT_MS=0
MAX_LINES=12

# --- has tracked source changed? ------------------------------------------------

# Documentation and media never reach the graph, and .logos/ is this tool's own
# state (a project that does not ignore it must not see the hook's records as a
# change); a turn that touched only those is not measured by check and review.
# The security leg still scans it (a key in a README is still a key), so it has
# a fingerprint of its own that leaves out only .logos/.
SEC_EXCLUDES=(":(exclude,glob)**/.logos/**")
EXCLUDES=("${SEC_EXCLUDES[@]}")
for g in md markdown txt rst adoc pdf png jpg jpeg gif svg ico lock; do
    EXCLUDES[${#EXCLUDES[@]}]=":(exclude,glob)**/*.$g"
done

HEAD_SHA="$(git rev-parse HEAD)"

# tree_fingerprint EXCLUDE… — TF_PRINT, the fingerprint of HEAD, of `git diff HEAD`
# and of every untracked file the repository does not ignore, the EXCLUDEd paths
# left out; TF_DIRTY yes when the tree differs from HEAD outside them.
TF_PRINT="" TF_DIRTY=no
tree_fingerprint() {
    local diff_hash untracked untracked_hash="" readable="" f
    diff_hash="$(git diff HEAD --no-ext-diff --binary -- . "$@" 2>/dev/null | git hash-object --stdin)" || diff_hash=""
    untracked="$(git -c core.quotePath=false ls-files -o --exclude-standard -- . "$@" 2>/dev/null)" || untracked=""
    if [ -n "$untracked" ]; then
        # hash-object stops at the first path it cannot open (a dangling symlink, a
        # link to a directory), and then nothing after it is hashed: only files are given.
        while IFS= read -r f; do
            if [ -f "$f" ]; then readable="$readable$f"$'\n'; fi
        done <<<"$untracked"
        if [ -n "$readable" ]; then
            untracked_hash="$(printf '%s' "$readable" | git hash-object --stdin-paths 2>/dev/null)" || untracked_hash=""
        fi
    fi
    TF_DIRTY=no
    git diff HEAD --quiet --no-ext-diff -- . "$@" 2>/dev/null || TF_DIRTY=yes
    if [ -n "$untracked" ]; then TF_DIRTY=yes; fi
    TF_PRINT="$(printf '%s\n%s\n%s\n%s\n' "$HEAD_SHA" "$diff_hash" "$untracked" "$untracked_hash" | git hash-object --stdin)"
    return 0
}
tree_fingerprint "${EXCLUDES[@]}"
FINGERPRINT="$TF_PRINT" DIRTY="$TF_DIRTY"
tree_fingerprint "${SEC_EXCLUDES[@]}"
SEC_FINGERPRINT="$TF_PRINT" SEC_DIRTY="$TF_DIRTY"

# The record: the fingerprint, the HEAD it was taken at (the security leg scans
# what was committed since, when it has no scan of its own to start from), and
# the security leg's fingerprint.
LAST="" LAST_HEAD="" LAST_SEC=""
if [ -f "$STATE" ]; then { IFS= read -r LAST; IFS= read -r LAST_HEAD; IFS= read -r LAST_SEC; } <"$STATE" || true; fi
record_state() { printf '%s\n%s\n%s\n' "$FINGERPRINT" "$HEAD_SHA" "$SEC_FINGERPRINT" >"$STATE.tmp.$$" && mv -f "$STATE.tmp.$$" "$STATE"; }

# Source changed: every leg. Only documentation or media changed: the security
# leg alone.
RUN_LEGS=""
if [ -z "$LAST" ]; then
    if [ "$DIRTY" = no ]; then
        if [ "$SEC_DIRTY" = no ]; then record_state; exit 0; fi
        RUN_LEGS=security
    fi
elif [ "$LAST" = "$FINGERPRINT" ]; then
    [ "$LAST_SEC" = "$SEC_FINGERPRINT" ] && exit 0
    RUN_LEGS=security
fi

# --- the budget -----------------------------------------------------------------

nap() { sleep 0.05 2>/dev/null || sleep 1 2>/dev/null || :; }

# run_bounded CMD… — CMD's stdout in RB_OUT, its stderr in RB_ERR and its exit
# status in RB_RC, unless the budget left cannot cover it. The wait is counted in
# nominal polls and in the whole seconds the shell reports, so a slow nap cannot
# stretch the budget.
RB_STATUS="" RB_RC=0 RB_OUT="" RB_ERR=""
run_bounded() {
    RB_STATUS="" RB_RC=0 RB_OUT="" RB_ERR=""
    local left=$((BUDGET_MS - SPENT_MS)) out="$STATE_DIR/run.$$.out" err="$STATE_DIR/run.$$.err" pid waited=0 t0=$SECONDS
    if [ "$left" -le 0 ]; then RB_STATUS=spent; return 0; fi
    "$@" >"$out" 2>"$err" </dev/null &
    pid=$!
    while :; do
        nap
        waited=$((waited + POLL_MS))
        kill -0 "$pid" 2>/dev/null || break
        if [ "$waited" -ge "$left" ] || [ $(((SECONDS - t0) * 1000)) -gt $((left + 1000)) ]; then
            kill "$pid" 2>/dev/null || true
            wait "$pid" 2>/dev/null || true
            SPENT_MS=$((SPENT_MS + left))
            RB_STATUS=timeout
            return 0
        fi
    done
    wait "$pid" 2>/dev/null && RB_RC=0 || RB_RC=$?
    [ "$waited" -gt "$left" ] && waited=$left
    SPENT_MS=$((SPENT_MS + waited))
    RB_STATUS="done"
    RB_OUT="$(<"$out")"
    RB_ERR="$(<"$err")"
}

# --- what a leg reports ---------------------------------------------------------

FINDINGS=()
FINDING_COUNT=0
BLOCK=no
UNMEASURED=()
NO_RECORD=no

SUBJECTS=$'\n'
POINTERS=$'\n'

leg_finding() {
    FINDING_COUNT=$((FINDING_COUNT + 1))
    if [ "${#FINDINGS[@]}" -lt "$MAX_LINES" ]; then FINDINGS[${#FINDINGS[@]}]="$1"; fi
}
# leg_blocking [SUBJECT] — what the regression is, for the reason's first line.
leg_blocking() {
    local subject="${1:-a quality regression against the merge-base}"
    BLOCK=yes
    case "$SUBJECTS" in *$'\n'"$subject"$'\n'*) ;; *) SUBJECTS="$SUBJECTS$subject"$'\n' ;; esac
}
# leg_pointer TEXT — what to run about the findings a leg listed; once each.
leg_pointer() {
    case "$POINTERS" in *$'\n'"$1"$'\n'*) ;; *) POINTERS="$POINTERS$1"$'\n' ;; esac
}
leg_unmeasured() { UNMEASURED[${#UNMEASURED[@]}]="$1"; }

# over_budget NAME — the shared wording for a leg the budget did not cover.
over_budget() {
    if [ "$RB_STATUS" = spent ]; then
        leg_unmeasured "$1 was not run: the ${BUDGET_MS} ms budget was spent"
    else
        leg_unmeasured "$1 did not finish within the ${BUDGET_MS} ms budget"
    fi
}

# --- the before-state ------------------------------------------------------------

# The merge-base of HEAD and the repository's default branch, as `review` takes
# them: origin/HEAD, else main, else master. Empty when there is none.
MERGE_BASE=""
default_base() {
    local ref
    ref="$(git symbolic-ref -q --short refs/remotes/origin/HEAD 2>/dev/null)" || ref=""
    if [ -n "$ref" ] && git rev-parse --verify --quiet "$ref^{commit}" >/dev/null 2>&1; then
        printf '%s\n' "$ref"
        return 0
    fi
    for ref in main master; do
        if git rev-parse --verify --quiet "$ref^{commit}" >/dev/null 2>&1; then
            printf '%s\n' "$ref"
            return 0
        fi
    done
    return 1
}
BASE_REF="$(default_base)" || BASE_REF=""
[ -n "$BASE_REF" ] && { MERGE_BASE="$(git merge-base HEAD "$BASE_REF" 2>/dev/null)" || MERGE_BASE=""; }
# The key a before-state is built under: what `review` keys its cache on, as far as
# a shell can see it — the logos version and the contract and configuration files.
KEY=""
build_key() {
    local version
    version="$("$BIN" --version 2>/dev/null || true)"
    KEY="$({ printf '%s\n' "$version"; cat "$PROJECT/.logos/rules.toml" "$PROJECT/.logos/config.toml" "$PROJECT/.logos/rules-baseline.json" 2>/dev/null || true; } | git hash-object --stdin)"
}
READY="$STATE_DIR/ready-$MERGE_BASE"
FAILED="$STATE_DIR/failed-$MERGE_BASE"
CHECKED_RULES=""

# marked FILE — 0 when FILE holds the current key.
marked() {
    [ -f "$1" ] && [ "$(<"$1")" = "$KEY" ]
}

# before_state_warm — 0 when `review` can be run now without building a shadow
# index (see the header). The snapshot probe is one cheap call, bounded like every
# other, and only asked when `check` evaluated no rule: with a rule to evaluate,
# `review` ignores a recorded snapshot.
before_state_warm() {
    [ -n "$MERGE_BASE" ] || return 1
    marked "$READY" && return 0
    [ "$CHECKED_RULES" = 0 ] || return 1
    run_bounded "$BIN" evolution --json --limit 100
    [ "$RB_STATUS" = "done" ] && [[ $RB_OUT == *"\"commit_sha\":\"$MERGE_BASE\""* ]]
}

# Build the before-state detached: every descriptor closed, so the host does not
# wait for it, SIGHUP ignored, and (where the shell has job control) a process group
# of its own. One at a time. A host that ends the hook's whole process group may
# still end the build; the lock then names a dead pid and the next turn starts it
# again. The outcome is recorded under the key the build started with.
start_build() {
    [ -n "$MERGE_BASE" ] || return 0
    local pid=""
    [ -f "$LOCK" ] && pid="$(<"$LOCK")"
    if [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; then return 0; fi
    rm -f "$READY" "$FAILED"
    (
        set -m 2>/dev/null || true
        trap '' HUP
        {
            if "$BIN" review --json; then printf '%s\n' "$KEY" >"$READY"; else printf '%s\n' "$KEY" >"$FAILED"; fi
            rm -f "$LOCK"
        } </dev/null >/dev/null 2>&1 &
        echo "$!" >"$LOCK"
    ) 2>/dev/null || true
}

# --- the legs -------------------------------------------------------------------

REVIEW_POINTER="Run /logos:review to judge each entry and prove a fix by re-measurement."

# One entry of the branch delta as one line, from CHUNK (an offender or a violation).
describe_entry() {
    local what="$1" name measure rule message file line before after threshold status trust value
    logos_json_str "$CHUNK" status && status="$JSON_VALUE" || status="?"
    logos_json_str "$CHUNK" trust && trust="$JSON_VALUE" || trust="?"
    logos_json_str "$CHUNK" file && file="$JSON_VALUE" || file=""
    logos_json_num "$CHUNK" line && line=":$JSON_VALUE" || line=""
    logos_json_num "$CHUNK" before && before="$JSON_VALUE" || before=""
    logos_json_num "$CHUNK" after && after="$JSON_VALUE" || after="?"
    logos_json_num "$CHUNK" threshold && threshold="threshold $JSON_VALUE" || threshold="no threshold"
    value="$after"
    [ -n "$before" ] && value="$before -> $after"
    if [ "$what" = offender ]; then
        logos_json_str "$CHUNK" name && name="$JSON_VALUE" || name="?"
        if logos_json_str "$CHUNK" measure; then measure="$JSON_VALUE"
        elif logos_json_str "$CHUNK" dimension; then measure="$JSON_VALUE"
        else measure="value"; fi
        LINE="- $name ($file$line): $measure $value, $threshold [$status, $trust]"
    else
        logos_json_str "$CHUNK" rule && rule="$JSON_VALUE" || rule="rule"
        logos_json_str "$CHUNK" message && message="$JSON_VALUE" || message="broken"
        LINE="- $rule ($file): $message (value $value, $threshold) [$status, $trust]"
    fi
}

# Entries are `new` or `worsened` to count at all; one the graph could not fully
# resolve (any trust but "resolved", `stale` among them) is listed and never alone
# blocks.
review_entries() {
    local what="$1" region_start="$2" region_end="$3" marker="$4" status trust i
    logos_json_region "$RB_OUT" "$region_start" "$region_end" || return 0
    logos_json_split "$REGION" "$marker"
    for ((i = 0; i < ${#JSON_CHUNKS[@]}; i++)); do
        CHUNK="${JSON_CHUNKS[$i]}"
        logos_json_str "$CHUNK" status && status="$JSON_VALUE" || continue
        case "$status" in new | worsened) ;; *) continue ;; esac
        describe_entry "$what"
        leg_finding "$LINE"
        leg_pointer "$REVIEW_POINTER"
        logos_json_str "$CHUNK" trust && trust="$JSON_VALUE" || trust=""
        [ "$trust" = resolved ] && leg_blocking
    done
    return 0
}

# Whether the check answer in RB_OUT holds a failing rule: an error-severity
# violation or a `[security]` one. Its `passed` cannot say: a missing required
# scan alone makes it false.
check_rule_failed() {
    local i
    [[ $RB_OUT == *'"security_violations":[{'* ]] && return 0
    logos_json_region "$RB_OUT" '"violations":[' '"ran_at"' || return 1
    logos_json_split "$REGION" '{"rule":'
    for ((i = 0; i < ${#JSON_CHUNKS[@]}; i++)); do
        logos_json_str "${JSON_CHUNKS[$i]}" severity && [ "$JSON_VALUE" = error ] && return 0
    done
    return 1
}

# check: the contract's own verdict. It reconciles the working tree first, so it
# is also what makes the review leg's graph fresh, and it tells the review leg how
# many rules were evaluated. Exit 1 is a failing rule; 0 and 4 (no contract loaded)
# and 5 (a security scan, not this leg's, unless the JSON says a rule failed) are not.
leg_check() {
    local c i file message
    run_bounded "$BIN" check --json
    case "$RB_STATUS" in
        spent | timeout) over_budget "check"; return 0 ;;
    esac
    logos_json_num "$RB_OUT" checked_rules && CHECKED_RULES="$JSON_VALUE"
    # Exit 5 is a missing security scan, and it outranks a failing rule in the
    # binary's exit code; the verdict of the rules is then in the JSON entries.
    if [ "$RB_RC" = 5 ] && check_rule_failed; then RB_RC=1; fi
    case "$RB_RC" in
        0 | 4 | 5) return 0 ;;
        1) ;;
        *) leg_unmeasured "check could not run (exit $RB_RC)"; return 0 ;;
    esac
    leg_blocking
    leg_pointer "$REVIEW_POINTER"
    logos_json_region "$RB_OUT" '"violations":[' '"ran_at"' || true
    logos_json_split "${REGION:-}" '{"rule":'
    for ((i = 0; i < ${#JSON_CHUNKS[@]}; i++)); do
        c="${JSON_CHUNKS[$i]}"
        logos_json_str "$c" severity && [ "$JSON_VALUE" != error ] && continue
        logos_json_str "$c" file && file="$JSON_VALUE" || file=""
        logos_json_str "$c" message && message="$JSON_VALUE" || message="failing"
        logos_json_str "$c" rule || JSON_VALUE="?"
        leg_finding "- failing rule $JSON_VALUE ($file): $message"
    done
    return 0
}

# review: the branch delta against the merge-base. Not run on a before-state that is
# not known to be ready (see the header).
leg_review() {
    local reason
    if [ -z "$MERGE_BASE" ]; then
        leg_unmeasured "no merge-base was found (no origin/HEAD, main or master to compare against)"
        return 0
    fi
    build_key
    if ! before_state_warm; then
        if marked "$FAILED"; then
            leg_unmeasured "the before-state for the merge-base could not be built (run logos review to see why); it is not retried until the contract, the configuration or logos change"
            return 0
        fi
        start_build
        NO_RECORD=yes
        leg_unmeasured "the before-state for the merge-base is being built in the background; the next turn is measured"
        return 0
    fi
    run_bounded "$BIN" review --json
    case "$RB_STATUS" in
        spent | timeout)
            over_budget "review"
            # A before-state `review` does not accept after all is rebuilt, out of budget.
            [ "$RB_STATUS" = timeout ] && start_build
            return 0
            ;;
    esac
    if [[ $RB_OUT != *'"measured":true'* ]]; then
        if logos_json_str "$RB_OUT" not_measured_because; then
            reason="$JSON_VALUE"
        else
            reason="review could not run (exit $RB_RC)"
        fi
        leg_unmeasured "$reason"
        return 0
    fi
    review_entries offender '"offenders":[' '"offender_counts"' '{"dimension":'
    review_entries violation '"violations":[' '"violation_counts"' '{"rule":'
    return 0
}

# security (FR-SC-20, ADR-83): Gitleaks, and Semgrep when it is installed and
# consented, on the files the turn changed, through the scoped scan (S-685).
#
# The files: what differs in the working tree from the HEAD the last scan whose
# findings were judged saw, else the HEAD the previous measured turn saw, else
# HEAD itself (the first of them that is an ancestor of HEAD) — so a turn that committed its work is
# scanned too, and the untracked files git does not ignore. Documentation is
# included (a key in a README is still a key); .logos/ is not. Only regular files:
# the scan refuses a deleted file, a symlink, a directory and a path that opens
# with "-", and one refusal refuses the whole call.
#
# Consent is the scan's own check (ADR-80 §4): asked for Gitleaks without it, the
# scan refuses before any process starts, the version probe included, and says
# so. `security status` is never asked, because it probes every installed
# scanner's version. Without Gitleaks, or without consent for it, the leg scans
# nothing and says so. Semgrep is scanned second, when the Gitleaks scan's answer
# shows it installed and enabled (`not_selected`); a refusal for want of its
# consent is not reported, since Semgrep runs only "when installed and consented".
#
# New: a finding in a scanned file whose fingerprint the store did not hold before
# the scan. The scan decides that, not this leg: a secret it finds again keeps its
# recorded fingerprint (S-685), and only one with no recorded match is stored with
# a new one. The match is by position, so a secret that moves reads as new again:
# one only an earlier scoped run recorded, once its line moves; one a full scan
# recorded, once its line and column both move. Suppressed findings are
# never listed. Silence is "no new finding in these files", never "clean": a
# store resting on scoped runs only stays stale/scoped_only for `security status`.
#
# It blocks on a new secret-class finding (stored redacted, whatever its severity:
# Gitleaks reports secrets as medium) or a new finding at high or critical. The
# reason carries the stored message, which the ingest already redacted or masked;
# nothing is read from the scanned files.
SEC_SCANNED="$STATE_DIR/scanned-head"
SEC_PATHS=()
SEC_LIST=$'\n'
SEC_POINTER="Run /logos:sec-review to fix what is safe and escalate the rest; a secret stays in git history, so rotate it rather than only deleting it."

record_scanned() {
    printf '%s\n' "$HEAD_SHA" >"$SEC_SCANNED.tmp.$$" && mv -f "$SEC_SCANNED.tmp.$$" "$SEC_SCANNED" || true
}

security_paths() {
    local scanned="" since f skipped=$'\n' unscannable=0
    [ -f "$SEC_SCANNED" ] && scanned="$(<"$SEC_SCANNED")"
    for since in "$scanned" "$LAST_HEAD" HEAD; do
        [ -n "$since" ] && git merge-base --is-ancestor "$since" HEAD >/dev/null 2>&1 && break
    done
    while IFS= read -r -d '' f; do
        case "$f" in .logos/* | */.logos/*) continue ;; esac
        [ -f "$f" ] && [ ! -L "$f" ] || continue
        # The scan refuses a path that would read as a flag, and a line break
        # cannot cross the hook's line-based reads: such a file is not scanned,
        # and the turn says so rather than reading as clean.
        case "$f" in
            -* | *$'\n'*)
                case "$skipped" in *$'\n'"$f"$'\n'*) ;; *) skipped="$skipped$f"$'\n'; unscannable=$((unscannable + 1)) ;; esac
                continue
                ;;
        esac
        case "$SEC_LIST" in *$'\n'"$f"$'\n'*) continue ;; esac
        SEC_LIST="$SEC_LIST$f"$'\n'
        SEC_PATHS[${#SEC_PATHS[@]}]="$f"
    done < <(
        git diff --name-only --relative --no-renames -z "$since" -- . 2>/dev/null || true
        git ls-files -o --exclude-standard -z -- . 2>/dev/null || true
    )
    [ "$unscannable" -eq 0 ] ||
        leg_unmeasured "security not measured on $unscannable changed file(s) whose name begins with - or holds a line break"
    return 0
}

# scanner_chunk NAME — CHUNK is NAME's row in the scanners list of RB_OUT (a
# `security scan` answer; its ingest rows open with {"name":, never {"scanner":);
# fails when it has none.
scanner_chunk() {
    local i
    CHUNK=""
    logos_json_split "$RB_OUT" '{"scanner":'
    for ((i = 0; i < ${#JSON_CHUNKS[@]}; i++)); do
        case "${JSON_CHUNKS[$i]}" in
            "{\"scanner\":\"$1\""*) CHUNK="${JSON_CHUNKS[$i]}"; return 0 ;;
        esac
    done
    return 1
}

# The listed findings of RB_OUT (`security findings`) that lie in a scanned file,
# one call of FN per finding, with FIELDS holding it.
each_scanned_finding() {
    local fn="$1" i path
    logos_json_region "$RB_OUT" '"findings":[' '"categories":[' || return 0
    logos_json_split "$REGION" '{"scanner_name":'
    for ((i = 0; i < ${#JSON_CHUNKS[@]}; i++)); do
        FIELDS="${JSON_CHUNKS[$i]}"
        # A file the graph does not index (.env, a README) binds to nothing: its
        # path is the one the report named, which is the one the scan was given.
        if logos_json_str "$FIELDS" path; then path="$JSON_VALUE"
        elif logos_json_str "$FIELDS" report_path; then path="$JSON_VALUE"
        else continue; fi
        case "$SEC_LIST" in *$'\n'"$path"$'\n'*) ;; *) continue ;; esac
        FINDING_PATH="$path"
        "$fn"
    done
    return 0
}

# A scan stores what it finds before the hook reads it back. When the read-back
# does not happen (the budget ran out, a listing failed, the host ended the hook),
# a new finding is already in the store, and the next turn's listing would count
# it as known. So the baseline is kept in SEC_UNJUDGED from before the scan until
# the findings after it were judged: in a file it names, a stored finding it does
# not list is still new.
SEC_UNJUDGED="$STATE_DIR/scan-unjudged"
SEC_BEFORE=$'\n'
UNJ_PATHS=$'\n'
UNJ_FPS=$'\n'

load_unjudged() {
    local line
    [ -f "$SEC_UNJUDGED" ] || return 0
    while IFS= read -r line; do
        case "$line" in
            "P "*) UNJ_PATHS="$UNJ_PATHS${line#P }"$'\n' ;;
            "F "*) UNJ_FPS="$UNJ_FPS${line#F }"$'\n' ;;
        esac
    done <"$SEC_UNJUDGED"
    return 0
}

save_unjudged() {
    local p fp
    {
        for p in "${SEC_PATHS[@]}"; do printf 'P %s\n' "$p"; done
        while IFS= read -r fp; do
            if [ -n "$fp" ]; then printf 'F %s\n' "$fp"; fi
        done <<<"$SEC_BEFORE"
    } >"$SEC_UNJUDGED.tmp.$$" && mv -f "$SEC_UNJUDGED.tmp.$$" "$SEC_UNJUDGED" || true
    return 0
}

remember_finding() {
    local fp
    logos_json_str "$FIELDS" fingerprint && fp="$JSON_VALUE" || return 0
    case "$UNJ_PATHS" in
        *$'\n'"$FINDING_PATH"$'\n'*)
            case "$UNJ_FPS" in *$'\n'"$fp"$'\n'*) ;; *) return 0 ;; esac
            ;;
    esac
    SEC_BEFORE="$SEC_BEFORE$fp"$'\n'
    return 0
}

judge_finding() {
    local fp severity rule line message scanner
    logos_json_str "$FIELDS" fingerprint && fp="$JSON_VALUE" || return 0
    case "$SEC_BEFORE" in *$'\n'"$fp"$'\n'*) return 0 ;; esac
    logos_json_str "$FIELDS" severity && severity="$JSON_VALUE" || severity="?"
    case "$FIELDS" in
        *'"redacted":true'*) ;;
        *) case "$severity" in high | critical) ;; *) return 0 ;; esac ;;
    esac
    logos_json_str "$FIELDS" rule_id && rule="$JSON_VALUE" || rule="?"
    logos_json_num "$FIELDS" start_line && line=":$JSON_VALUE" || line=""
    logos_json_str "$FIELDS" message && message="$JSON_VALUE" || message="?"
    logos_json_str "$FIELDS" scanner_name && scanner="$JSON_VALUE" || scanner="?"
    leg_finding "- $rule ($FINDING_PATH$line): $message [$scanner, $severity]"
    leg_blocking "a new secret or high-severity finding in the files it changed"
    leg_pointer "$SEC_POINTER"
    return 0
}

# sec_run WHAT ARG… — run_bounded "$BIN" ARG…; 0 when it completed with exit 0,
# else the leg's not-measured line is recorded and it returns 1.
sec_run() {
    local what="$1"
    shift
    run_bounded "$BIN" "$@"
    case "$RB_STATUS" in
        spent | timeout) over_budget "the security scan"; return 1 ;;
    esac
    [ "$RB_RC" = 0 ] && return 0
    leg_unmeasured "security not measured: $what could not run (exit $RB_RC)"
    return 1
}

# sec_scan SCANNER — the scoped scan of SEC_PATHS by SCANNER. Sets SEC_SCAN to ran,
# no_consent, unmeasured (a not-measured line was recorded), or the scanner's
# outcome when it did not run (not_installed, disabled, failed…).
SEC_SCAN=""
sec_scan() {
    local args=(security scan --json --scanner "$1") f outcome="?"
    for f in "${SEC_PATHS[@]}"; do
        args[${#args[@]}]=--path
        args[${#args[@]}]="$f"
    done
    SEC_SCAN=unmeasured
    run_bounded "$BIN" "${args[@]}"
    case "$RB_STATUS" in
        spent | timeout) over_budget "the security scan"; return 0 ;;
    esac
    if [ "$RB_RC" = 2 ] && [[ $RB_ERR == *"invalid value for consent:"* ]]; then
        SEC_SCAN=no_consent
        return 0
    fi
    if [ "$RB_RC" != 0 ]; then
        leg_unmeasured "security not measured: the scan could not run (exit $RB_RC)"
        return 0
    fi
    scanner_chunk "$1" && logos_json_str "$CHUNK" outcome && outcome="$JSON_VALUE"
    SEC_SCAN="$outcome"
    return 0
}

# sec_list FN — each changed file's stored findings, one `security findings
# --path` call per file, and one call of FN per finding. The hook never reads the
# whole store: on bash 3.2 the readers cost time quadratic in the text they cut,
# outside any budget, and a store of a thousand findings took longer than the
# host's timeout. A file holding more than SEC_LIST_MAX bytes of findings is not
# read either. Returns 1 when a listing was not read (its line recorded).
SEC_LIST_MAX=131072
sec_list() {
    local fn="$1" p
    for p in "${SEC_PATHS[@]}"; do
        sec_run "security findings" security findings --json --suppression active --path "$p" || return 1
        if [ "${#RB_OUT}" -gt "$SEC_LIST_MAX" ]; then
            leg_unmeasured "security not measured: $p holds more stored findings than the hook reads (${#RB_OUT} bytes)"
            return 1
        fi
        each_scanned_finding "$fn"
    done
    return 0
}

leg_security() {
    local semgrep=no
    security_paths
    if [ "${#SEC_PATHS[@]}" -eq 0 ]; then record_scanned; return 0; fi

    load_unjudged
    sec_list remember_finding || return 0
    save_unjudged

    # A --path scan names its scanners, or the dependency scanners drop out.
    sec_scan gitleaks
    case "$SEC_SCAN" in
        ran) ;;
        unmeasured) return 0 ;;
        no_consent) leg_unmeasured "security not measured: no consent (a logos security scan --consent records it)"; return 0 ;;
        not_installed) leg_unmeasured "security not measured: Gitleaks is not installed (logos security install gitleaks installs it)"; return 0 ;;
        disabled) leg_unmeasured "security not measured: Gitleaks is disabled"; return 0 ;;
        *) leg_unmeasured "security not measured: Gitleaks did not run ($SEC_SCAN)"; return 0 ;;
    esac
    scanner_chunk semgrep && logos_json_str "$CHUNK" outcome && [ "$JSON_VALUE" = not_selected ] && semgrep=yes
    if [ "$semgrep" = yes ]; then
        sec_scan semgrep
        case "$SEC_SCAN" in
            ran | unmeasured | no_consent) ;;
            *) leg_unmeasured "security: Semgrep did not run on the changed files ($SEC_SCAN)" ;;
        esac
    fi

    sec_list judge_finding || return 0
    rm -f "$SEC_UNJUDGED"
    record_scanned
    return 0
}

LEGS="check review security"

for leg in ${RUN_LEGS:-$LEGS}; do
    "leg_$leg"
done

[ "$NO_RECORD" = yes ] || record_state

# --- the outcome ----------------------------------------------------------------

join_unmeasured() {
    local out="" u
    for u in ${UNMEASURED[@]+"${UNMEASURED[@]}"}; do
        out="${out:+$out; }$u"
    done
    printf '%s' "$out"
}

# join_lines TEXT SEP — the lines of TEXT (newline-delimited, empty lines left out),
# joined by SEP.
join_lines() {
    local out="" l
    while IFS= read -r l; do
        if [ -n "$l" ]; then out="${out:+$out$2}$l"; fi
    done <<<"$1"
    printf '%s' "$out"
}

if [ "$BLOCK" = yes ]; then
    REASON="logos: this turn left $(join_lines "$SUBJECTS" " and "). Fix it before stopping:"
    for f in ${FINDINGS[@]+"${FINDINGS[@]}"}; do
        REASON="$REASON"$'\n'"$f"
    done
    [ "$FINDING_COUNT" -gt "${#FINDINGS[@]}" ] && REASON="$REASON"$'\n'"- ... and $((FINDING_COUNT - ${#FINDINGS[@]})) more"
    POINTER="$(join_lines "$POINTERS" " ")"
    REASON="$REASON"$'\n'"${POINTER:+$POINTER }This hook blocks once; the next stop proceeds."
    NOT="$(join_unmeasured)"
    [ -n "$NOT" ] && REASON="$REASON"$'\n'"(Not measured: $NOT.)"
    logos_hook_block "$REASON"
    exit 0
fi

if [ "${#UNMEASURED[@]}" -gt 0 ]; then
    logos_hook_notice "logos: this turn is not measured: $(join_unmeasured). The stop proceeds."
fi
exit 0
