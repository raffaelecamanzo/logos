#!/usr/bin/env bash
#
# test-plugin-stop.sh — prove the logos plugin's Stop hook
# (plugins/logos/hooks/stop.sh, S-682, FR-IN-13, ADR-82) checks an editing turn
# within a budget: silent when clean, blocking once with the evidence on a
# regression, never blocking when it could not measure.
#
# The hook runs as the host runs it: executed directly, under `env -i`, with a
# PATH holding a stub `logos`, a `bash`, and the system tools. Each case is a
# fixture git repository with a branch off main and a turn's edit left in the
# working tree; the stub answers `check --json` and `review --json` from files
# and records every call, so what the hook asked of logos is part of the
# assertion. The review JSON is the shape `logos review --json` writes (the
# offender and violation objects, field for field).
#
# Layers:
#   1. lib/common.sh units: the JSON readers the hook parses with;
#   2. a regression, once: the evidence, the pointer, stop_hook_active, near misses;
#   3. silence: a clean turn, no source change, the off switch, not enrolled,
#      no logos, no repository;
#   4. not measured: a 1 ms budget, a slow review, a cold before-state built in
#      the background, a failing leg, an older binary, a bad budget value;
#   5. the verdicts the legs read: trust labels, status filters, failing rules,
#      long lists, odd names;
#   6. the manifest: the Stop hook is declared once, with room for its budget;
#   8. the security leg (S-688): a new secret blocks once, redacted; no new finding
#      is silent; no consent, no Gitleaks or no budget scans nothing and says so;
#      which files are scanned, and which findings are new.
# Every end-to-end case runs under each distinct bash on this machine
# (/bin/bash is 3.2 on macOS). With LOGOS_STOP_E2E_BIN set to a built logos
# binary, one more case runs the real flow against it.
#
# Usage: bash scripts/tests/test-plugin-stop.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLUGIN="$HERE/plugins/logos"
HOOK="$PLUGIN/hooks/stop.sh"

WORK="${TMPDIR:-/tmp}/logos-plugin-stop-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi; }
has() { case "$1" in *"$2"*) echo yes ;; *) echo no ;; esac; }

for f in "$HOOK" "$PLUGIN/hooks/lib/common.sh" "$PLUGIN/hooks/hooks.json"; do
    [ -f "$f" ] || { echo "FAIL  missing $f"; echo "0 passed, 1 failed (of 1)"; exit 1; }
done

# Each distinct bash on this machine.
BASHES=()
for b in /bin/bash "$(command -v bash)"; do
    [ -x "$b" ] || continue
    r="$(cd "$(dirname "$b")" && pwd -P)/$(basename "$b")"
    dup=no
    for s in "${BASHES[@]+"${BASHES[@]}"}"; do [ "$s" = "$r" ] && dup=yes; done
    [ "$dup" = yes ] || BASHES+=("$r")
done

GITDIR="$(cd "$(dirname "$(command -v git)")" && pwd -P)"
GIT=(git -c user.name=t -c user.email=t@t -c maintenance.auto=false -c gc.auto=0 -c commit.gpgsign=false)

# --- the stub logos --------------------------------------------------------------
#
# Behaviour from $STUB_DIR: <leg>.json is what the leg prints, <leg>.rc its exit
# status, <leg>.delay the seconds it takes; while <leg>.gate exists the leg waits
# (at most 30 s), so a test decides when it ends rather than racing a clock. Every call (argv and cwd) is appended
# to $CALLS; --version answers $STUB_DIR/version, else as a current binary. The stub
# answers check, review, evolution (the hook's snapshot probe) and the security
# leg's `security findings` (leg security-findings) and `security scan --scanner
# NAME` (leg security-scan-NAME). A scan checks consent first, as the binary does:
# NAME listed in $STUB_DIR/installed (default: gitleaks) but not in
# $STUB_DIR/consent (default: gitleaks) is refused with the binary's error on
# stderr and exit 2, before anything runs. Otherwise a Gitleaks scan runs the stub
# gitleaks once per --path, and any scan then serves security-findings-after.json,
# when there is one, as the findings it stored. The stub gitleaks appends its argv
# to $STUB_DIR/gitleaks.calls: an empty file is proof that no scanner was spawned.
# `security findings --path P` lists, as the binary does, only the findings whose
# path (else report path) is P or lies under it; once a scan ran, it also takes
# $STUB_DIR/security-findings.after-scan.delay seconds, when that file exists, and
# waits while security-findings.after-scan.gate does.

STUBBIN="$WORK/stubbin"
mkdir -p "$STUBBIN"
cat >"$STUBBIN/logos" <<'EOF'
#!/bin/sh
printf '%s|%s\n' "$*" "$(pwd -P)" >>"$CALLS"
case "$1" in
    --version) cat "$STUB_DIR/version" 2>/dev/null || echo 'logos 99.0.0'; exit 0 ;;
    check) leg=check ;;
    review) leg=review ;;
    evolution) leg=evolution ;;
    security) leg="security-$2" ;;
    *) exit 2 ;;
esac
if [ "$leg" = security-scan ]; then
    sc="" prev=""
    for a in "$@"; do
        [ "$prev" = --scanner ] && sc="$a"
        prev="$a"
    done
    installed=" $(cat "$STUB_DIR/installed" 2>/dev/null || echo gitleaks) "
    consent=" $(cat "$STUB_DIR/consent" 2>/dev/null || echo gitleaks) "
    case "$installed" in
        *" $sc "*)
            case "$consent" in
                *" $sc "*) ;;
                *) echo "error: invalid value for consent: none is recorded in this project for $sc" >&2; exit 2 ;;
            esac
            ;;
    esac
    leg="security-scan-$sc"
fi
[ -f "$STUB_DIR/$leg.delay" ] && sleep "$(cat "$STUB_DIR/$leg.delay")"
gated=0
while [ -f "$STUB_DIR/$leg.gate" ] && [ "$gated" -lt 600 ]; do sleep 0.05; gated=$((gated + 1)); done
[ "$leg" = security-findings ] && [ -f "$STUB_DIR/scanned" ] && [ -f "$STUB_DIR/$leg.after-scan.delay" ] && sleep "$(cat "$STUB_DIR/$leg.after-scan.delay")"
if [ "$leg" = security-findings ] && [ -f "$STUB_DIR/scanned" ]; then
    while [ -f "$STUB_DIR/$leg.after-scan.gate" ] && [ "$gated" -lt 600 ]; do sleep 0.05; gated=$((gated + 1)); done
fi
case "$leg" in
    security-scan-*)
        : >"$STUB_DIR/scanned"
        prev=""
        for a in "$@"; do
            [ "$prev" = --path ] && [ "$sc" = gitleaks ] && case "$installed" in *" gitleaks "*) gitleaks detect --no-git --source "$a" ;; esac
            prev="$a"
        done
        [ -f "$STUB_DIR/security-findings-after.json" ] && mv "$STUB_DIR/security-findings-after.json" "$STUB_DIR/security-findings.json"
        ;;
esac
if [ "$leg" = security-findings ] && [ -f "$STUB_DIR/$leg.json" ]; then
    want="" prev=""
    for a in "$@"; do
        [ "$prev" = --path ] && want="$a"
        prev="$a"
    done
    python3 -I -c '
import json, sys
raw = open(sys.argv[1]).read()
try:
    d = json.loads(raw)
except ValueError:
    sys.stdout.write(raw); sys.exit()
want = sys.argv[2]
if want:
    def within(p):
        return p is not None and (p == want or p.startswith(want.rstrip("/") + "/"))
    d["findings"] = [f for f in d["findings"] if within(f["path"] if f["path"] is not None else f["report_path"])]
print(json.dumps(d, separators=(",", ":")))' "$STUB_DIR/$leg.json" "$want"
elif [ -f "$STUB_DIR/$leg.json" ]; then
    cat "$STUB_DIR/$leg.json"
fi
rc=0
[ -f "$STUB_DIR/$leg.rc" ] && rc="$(cat "$STUB_DIR/$leg.rc")"
exit "$rc"
EOF
chmod +x "$STUBBIN/logos"
cat >"$STUBBIN/gitleaks" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$STUB_DIR/gitleaks.calls"
exit 0
EOF
chmod +x "$STUBBIN/gitleaks"

# --- JSON the stub prints, in the shape the binary writes -------------------------

# off NAME STATUS BEFORE AFTER THRESHOLD TRUST [MEASURE] [FILE] [LINE]: one offender.
# BEFORE and THRESHOLD are a number or null.
off() {
    printf '{"dimension":"nesting","status":"%s","node_id":"src/`%s.rs`/%s().","name":"%s","file":"%s","line":%s,"measure":"%s","before":%s,"after":%s,"threshold":%s,"detail":"nesting depth %s","trust":"%s"}' \
        "$2" "${8:-app}" "$1" "$1" "src/${8:-app}.rs" "${9:-5}" "${7:-nesting_depth}" "$3" "$4" "$5" "$4" "$6"
}
# viol RULE STATUS BEFORE AFTER THRESHOLD TRUST MESSAGE: one violation.
viol() {
    printf '{"rule":"%s","rule_type":"constraint","status":"%s","subject":"src/`app.rs`/f().","file":"src/app.rs","before":%s,"after":%s,"threshold":%s,"message":"%s","trust":"%s"}' \
        "$1" "$2" "$3" "$4" "$5" "$7" "$6"
}
# review_json MEASURED OFFENDERS VIOLATIONS: a review answer (the lists are comma-joined objects).
review_json() {
    printf '{"base":"main","base_origin":"default-branch","head":"h","merge_base":"m","measured":%s,"not_measured_because":%s,"before_state":"shadow-index","offenders":[%s],"offender_counts":{"new":0,"worsened":0,"improved":0,"resolved":0,"unchanged":0},"violations":[%s],"violation_counts":{"new":0,"worsened":0,"improved":0,"resolved":0,"unchanged":0},"introduced_clone_groups":[],"stale_files":[],"sections":{},"notes":["an unchanged entry is counted in offender_counts or violation_counts and not listed"]}\n' \
        "$1" "$([ "$1" = true ] && echo null || echo '"no base ref resolves"')" "$2" "$3"
}
CHECK_OK='{"passed":true,"checked_rules":1,"rules_present":true,"violations":[],"ran_at":1,"freshness":"x","warnings":[]}'
DEEP="$(off deep new null 6 4 resolved)"

# The security leg's answers, in the shapes `logos security scan|findings --json`
# write (captured from the binary against a stub Gitleaks).
# sec_scan GITLEAKS SEMGREP: a scan answer with each scanner's outcome. Semgrep
# installed and enabled but not asked for reads not_selected.
sec_scan() {
    printf '{"consent_recorded":[],"scanners":[{"scanner":"semgrep","driver":"Semgrep OSS","version":null,"outcome":"%s"},{"scanner":"trivy","driver":"Trivy","version":null,"outcome":"not_selected"},{"scanner":"gitleaks","driver":"gitleaks","version":"8.18.4","outcome":"%s","sarif":[".logos/security/scoped/gitleaks.sarif"],"requires_build":[],"findings":1,"ingests":[{"format":"sarif","head_sha":"h","report_hash":"r","scanners":[{"name":"gitleaks","version":"v8.18.4","snapshot_id":1,"findings":1,"replaced_existing":false}],"findings":1,"bound":1,"unbound":0,"redacted":1,"unmatched":[],"already_ingested":false}],"scope":{"kind":"paths","paths":["src/app.rs"]}}],"findings":1}\n' "$2" "$1"
}
# fnd FP RULE SEVERITY REDACTED REPORT_PATH LINE MESSAGE [SCANNER]: one listed
# finding. A report path the graph indexes (under src/) binds, and its path is
# set; any other binds to nothing, and its path is null.
fnd() {
    local path=null binding=unmatched
    case "$5" in src/*) path="\"$5\"" binding=bound ;; esac
    printf '{"scanner_name":"%s","scanner_version":"v8.18.4","head_sha":"h","fingerprint":"%s","rule_id":"%s","severity":"%s","cwes":[],"message":"%s","redacted":%s,"binding":"%s","report_path":"%s","path":%s,"start_line":%s,"start_column":18,"end_line":%s,"end_column":38,"freshness":"fresh","owasp":[],"unclassified":"no_cwe","suppression":null,"context":{"line":%s,"enclosing":{"confidence":"unbound","symbol":null,"unbound_because":"no_symbol"},"reachability":{"confidence":"unbound","verdict":"unknown","route":null,"unresolved_because":null},"blast_radius":{"confidence":"unbound"},"resolution":null}}' \
        "${8:-gitleaks}" "$1" "$2" "$3" "$7" "$4" "$binding" "$5" "$path" "$6" "$6" "$6"
}
# sec_findings FINDINGS: a findings answer (FINDINGS comma-joined fnd objects).
sec_findings() {
    printf '{"edition":"2025","as_of":"2026-10-17","scanned":[{"driver":"gitleaks","version":"v8.18.4","head_sha":"h","findings":1}],"never_run":[],"notice":null,"total":1,"findings":[%s],"categories":[{"id":"A07:2025","name":"Authentication Failures","scanners":["gitleaks"],"enabled_scanners":["gitleaks"],"state":{"state":"covered","findings":0}}],"suppressions":[]}\n' "$1"
}
# secret FP PATH LINE: a Gitleaks AWS-key finding, stored redacted (as the ingest stores it).
secret() { fnd "$1" aws-access-token medium true "$2" "$3" "secret-class finding redacted: rule aws-access-token at $2:$3"; }
AWS_KEY=AKIAIOSFODNN7EXAMPLE

# --- fixtures -------------------------------------------------------------------

N=0
REPO="" MB="" STUB=""
# keyof VERSION [FILE...]: the key a before-state is built under — the version line
# and the contract and configuration files, hashed.
keyof() {
    local v="$1"
    shift
    { printf '%s\n' "$v"; [ "$#" -gt 0 ] && cat "$@"; } | git hash-object --stdin
}
# mkrepo [cold]: a repository on a branch off main with .logos/, and a stub
# directory answering clean unless a case says otherwise. Warm unless "cold": a
# background build already finished under the current key. MB is the merge-base.
mkrepo() {
    N=$((N + 1))
    REPO="$WORK/repo$N" STUB="$WORK/stub$N"
    mkdir -p "$REPO/src" "$STUB"
    (
        cd "$REPO" || exit 1
        git init -q -b main . 2>/dev/null || { git init -q . && git checkout -q -b main; }
        printf 'fn app() {}\n' >src/app.rs
        printf '# notes\n' >README.md
        "${GIT[@]}" add -A && "${GIT[@]}" commit -qm base
        git checkout -q -b feat
    ) || exit 1
    MB="$(git -C "$REPO" rev-parse HEAD)"
    mkdir -p "$REPO/.logos"
    if [ "${1:-warm}" != cold ]; then
        mkdir -p "$REPO/.logos/stop-hook"
        keyof 'logos 99.0.0' >"$REPO/.logos/stop-hook/ready-$MB"
    fi
    printf '%s\n' "$CHECK_OK" >"$STUB/check.json"
    review_json true "" "" >"$STUB/review.json"
    sec_findings "" >"$STUB/security-findings.json"
    sec_scan ran not_installed >"$STUB/security-scan-gitleaks.json"
}
# edit: a turn's edit — a source file changed. Distinct each call.
EDITS=0
edit() { EDITS=$((EDITS + 1)); printf 'fn app() { /* edit %s */ }\n' "$EDITS" >"$REPO/src/app.rs"; }

PAYLOAD='{"hook_event_name":"Stop","stop_hook_active":false,"session_id":"t"}'
CALLS="$WORK/calls"
OUT="" ERR="" RC=0
# run <bash> <repo> [VAR=value ...]: the hook as the host runs it, in $REPO's
# project. Sets OUT, ERR, RC. $CALLS holds what logos was asked.
run() {
    local bash="$1" proj="$2" sys="$WORK/sys-$$-${1//\//_}"
    shift 2
    mkdir -p "$sys" && ln -sf "$bash" "$sys/bash"
    : >"$CALLS"
    RC=0
    (cd "$WORK" && printf '%s' "$PAYLOAD" | env -i HOME="$WORK" PATH="$STUBBIN:$sys:$GITDIR:/usr/bin:/bin" \
        CLAUDE_PLUGIN_ROOT="$PLUGIN" CLAUDE_PROJECT_DIR="$proj" STUB_DIR="$STUB" CALLS="$CALLS" \
        GIT_CEILING_DIRECTORIES="$WORK" "$@" "${HOOK_UNDER_TEST:-$HOOK}") >"$WORK/out" 2>"$WORK/err" || RC=$?
    OUT="$(cat "$WORK/out")" ERR="$(cat "$WORK/err")"
}
# calls: what logos was asked, the --version the hook reads for its key left out.
calls() { grep -v '^--version|' "$CALLS" || true; }
ncalls() { grep -c "^$1 " "$CALLS" || true; }

# jf <field>: a field of the hook's JSON object in $OUT; INVALID when $OUT is not one object.
jf() {
    printf '%s' "$OUT" | python3 -I -c '
import json, sys
try:
    d = json.loads(sys.stdin.read())
    assert isinstance(d, dict)
except Exception:
    print("INVALID"); sys.exit()
print(d.get(sys.argv[1], "ABSENT"))' "$1"
}
decision() { jf decision; }
reason() { jf reason; }

# wait_for <file> <seconds>: 0 once the file exists.
wait_for() {
    local i=0 max=$(($2 * 10))
    while [ ! -e "$1" ] && [ "$i" -lt "$max" ]; do sleep 0.1; i=$((i + 1)); done
    [ -e "$1" ]
}
# quiesce: wait for a background build in $REPO to end, so nothing outlives its case.
quiesce() {
    local i=0
    while [ -f "$REPO/.logos/stop-hook/build.lock" ] && [ "$i" -lt 100 ]; do
        local pid
        pid="$(cat "$REPO/.logos/stop-hook/build.lock" 2>/dev/null)"
        { [ -n "$pid" ] && kill -0 "$pid" 2>/dev/null; } || break
        sleep 0.1; i=$((i + 1))
    done
    sleep 0.2
}

# --- 1. lib/common.sh units -----------------------------------------------------

lib() { (. "$PLUGIN/hooks/lib/common.sh" && "$@"); }
BLOB='{"offenders":[{"dimension":"nesting","name":"a\"b}{\\c","after":6,"threshold":null,"before":-2.5},{"dimension":"equality","name":"z","after":7,"threshold":4}],"offender_counts":{}}'
# chunks <blob>: the offenders of BLOB split as the hook splits them; REGION and JSON_CHUNKS are set.
chunks() {
    . "$PLUGIN/hooks/lib/common.sh"
    logos_json_region "$BLOB" '"offenders":[' '"offender_counts"'
    logos_json_split "$REGION" '{"dimension":'
}
expect "[lib] a region is what lies between two markers" \
    "$(. "$PLUGIN/hooks/lib/common.sh"; logos_json_region 'a[b]c,d' '[' ']'; printf '%s' "$REGION")" "b"
expect "[lib] ... a region with no end marker runs to the end; with no start marker it fails" \
    "$(. "$PLUGIN/hooks/lib/common.sh"; logos_json_region 'a[bc' '[' ']'; printf '%s' "$REGION"; logos_json_region 'abc' '[' ']' || printf ' none')" "bc none"
expect "[lib] split: two objects, each chunk opens with its marker and none runs into the next" \
    "$(chunks; echo "${#JSON_CHUNKS[@]}|${JSON_CHUNKS[0]:0:26}|${JSON_CHUNKS[1]:0:26}|$([[ ${JSON_CHUNKS[0]} == *equality* ]] && echo leak || echo clean)")" \
    '2|{"dimension":"nesting","na|{"dimension":"equality","n|clean'
expect "[lib] split: no marker, no chunks" "$(. "$PLUGIN/hooks/lib/common.sh"; logos_json_split 'abc' '{"x":'; echo "${#JSON_CHUNKS[@]}")" 0
expect "[lib] a string field comes back unescaped, braces and all" "$(chunks; logos_json_str "${JSON_CHUNKS[0]}" name; echo "$JSON_VALUE")" 'a"b}{\c'
expect "[lib] numbers read, a negative decimal included; null is absent, not the next object's value" \
    "$(chunks; c="${JSON_CHUNKS[0]}"; logos_json_num "$c" before && echo "before=$JSON_VALUE"; logos_json_num "$c" after && echo "after=$JSON_VALUE"; logos_json_num "$c" threshold || echo "threshold=null")" \
    "$(printf 'before=-2.5\nafter=6\nthreshold=null')"
expect "[lib] ... and the second object's own threshold" "$(chunks; logos_json_num "${JSON_CHUNKS[1]}" threshold; echo "$JSON_VALUE")" 4
expect "[lib] a key that is not there fails (near miss: nam is not name)" \
    "$(. "$PLUGIN/hooks/lib/common.sh"; logos_json_str '{"name":"x"}' nam && echo found || echo absent)" absent
expect "[lib] a block object is valid JSON: decision, and a reason that keeps its quotes and lines" \
    "$(lib logos_hook_block 'a "q" line
two' | python3 -I -c 'import json,sys; d=json.loads(sys.stdin.read()); print(d["decision"] + "|" + str(d["reason"].count("\n")) + "|" + d["reason"].split("\n")[0])')" \
    'block|1|a "q" line'
expect "[lib] a notice is valid JSON with only a systemMessage" \
    "$(lib logos_hook_notice 'n' | python3 -I -c 'import json,sys; print(sorted(json.loads(sys.stdin.read())))')" "['systemMessage']"

# --- 2-5. the hook, under each bash ---------------------------------------------

for B in "${BASHES[@]}"; do
    V="$("$B" -c 'echo "${BASH_VERSINFO[0]}.${BASH_VERSINFO[1]}"')"
    T="[bash $V]"

    # 2. a regression, once ---------------------------------------------------
    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T a 6-deep function: exit 0, one JSON object, decision block" "$RC|$(decision)" "0|block"
    R="$(reason)"
    expect "$T ... the reason names the function" "$(has "$R" '- deep (src/app.rs:5)')" yes
    expect "$T ... the value and the measure" "$(has "$R" 'nesting_depth 6')" yes
    expect "$T ... the threshold" "$(has "$R" 'threshold 4')" yes
    expect "$T ... the status and trust label" "$(has "$R" '[new, resolved]')" yes
    expect "$T ... a pointer to /logos:review" "$(has "$R" '/logos:review')" yes
    expect "$T ... nothing on stderr" "$ERR" ""
    expect "$T ... check ran once, then review once, then the security leg, as --json, in the project" \
        "$(calls)" "check --json|$REPO
review --json|$REPO
security findings --json --suppression active --path src/app.rs|$REPO
security scan --json --scanner gitleaks --path src/app.rs|$REPO
security findings --json --suppression active --path src/app.rs|$REPO"
    expect "$T ... the state is recorded under .logos/" "$([ -s "$REPO/.logos/stop-hook/state" ] && echo yes || echo no)" yes

    expect "$T ... the hook's records are ignored by git: .logos/stop-hook never shows as untracked" \
        "$(git -C "$REPO" status --porcelain --untracked-files=all | grep -c 'stop-hook')" 0
    run "$B" "$REPO"
    expect "$T the same turn stopped again with nothing changed: silent, logos not run" "$RC|$OUT|$(calls)" "0||"

    edit
    PAYLOAD='{"hook_event_name":"Stop","stop_hook_active":true,"session_id":"t"}'
    run "$B" "$REPO"
    expect "$T stop_hook_active true: the regression does not block again, no output, logos not run" "$RC|$OUT|$(calls)" "0||"
    for p in '{"stop_hook_active" :  true}' '{"session_id":"t","stop_hook_active":true,"cwd":"/x"}'; do
        PAYLOAD="$p"
        run "$B" "$REPO"
        expect "$T stop_hook_active is read through spacing and key order ($p)" "$RC|$OUT|$(calls)" "0||"
    done
    for p in '{"stop_hook_active":false}' '{"not_stop_hook_active":true}' '{"stop_hook_active":"true"}' '{"note":"stop_hook_active"}' ''; do
        PAYLOAD="$p"
        run "$B" "$REPO"
        expect "$T near miss, not an active chain ($p): the regression blocks" "$RC|$(decision)" "0|block"
        edit
    done
    PAYLOAD='{"hook_event_name":"Stop","stop_hook_active":false,"session_id":"t"}'

    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO" </dev/null
    expect "$T no payload on stdin at all: treated as not active, blocks" "$RC|$(decision)" "0|block"

    # 3. silence --------------------------------------------------------------
    mkrepo; edit
    run "$B" "$REPO"
    expect "$T a clean turn: exit 0, no output, no stderr" "$RC|$OUT|$ERR" "0||"
    expect "$T ... both legs ran" "$(ncalls check)|$(ncalls review)" "1|1"
    run "$B" "$REPO"
    expect "$T ... and the same state is not measured again" "$RC|$OUT|$(calls)" "0||"
    edit
    run "$B" "$REPO"
    expect "$T ... a new edit is" "$(ncalls check)|$(ncalls review)" "1|1"

    mkrepo
    run "$B" "$REPO"
    expect "$T no source change (a clean tree, no record): silent, logos not run" "$RC|$OUT|$(calls)" "0||"
    printf '# more notes\n' >>"$REPO/README.md"
    printf 'notes\n' >"$REPO/NOTES.txt"
    printf 'x' >"$REPO/logo.png"
    run "$B" "$REPO"
    expect "$T only documentation and media changed (tracked and untracked): silent; check and review not run, only the security leg scans them" \
        "$RC|$OUT|$(ncalls check)|$(ncalls review)|$(grep '^security scan ' "$CALLS" | sed 's/|.*//')" \
        "0||0|0|security scan --json --scanner gitleaks --path README.md --path NOTES.txt --path logo.png"
    printf 'TOKEN=x\n' >"$REPO/-secret.env"
    printf '# and more\n' >>"$REPO/README.md"
    run "$B" "$REPO"
    expect "$T a changed file named like a flag is not scanned, and the turn says so rather than reading as clean" \
        "$RC|$(has "$(jf systemMessage)" 'security not measured on 1 changed file(s) whose name begins with -')|$(has "$(grep '^security scan ' "$CALLS")" 'secret.env')" \
        "0|yes|no"
    rm -f -- "$REPO/-secret.env"
    printf 'fn extra() {}\n' >"$REPO/src/extra.rs"
    run "$B" "$REPO"
    expect "$T a new untracked source file is a source change: both legs ran" "$(ncalls check)|$(ncalls review)" "1|1"
    # An untracked file whose content changes is a change, symlinks beside it or not.
    ln -s /nonexistent/target "$REPO/src/dangling"
    ln -s src "$REPO/srclink"
    run "$B" "$REPO"
    printf 'fn extra() { /* rewritten */ }\n' >"$REPO/src/extra.rs"
    run "$B" "$REPO"
    expect "$T an edit to an untracked file is measured beside a dangling symlink and a link to a directory" "$(ncalls check)|$(ncalls review)" "1|1"
    rm -f "$REPO/src/dangling" "$REPO/srclink"
    "${GIT[@]}" -C "$REPO" add -A && "${GIT[@]}" -C "$REPO" commit -qm turn
    run "$B" "$REPO"
    expect "$T the turn's work committed and the tree clean: HEAD moved, so it is measured" "$(ncalls check)|$(ncalls review)" "1|1"
    run "$B" "$REPO"
    expect "$T ... and then silent" "$RC|$OUT|$(calls)" "0||"

    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO" LOGOS_STOP_HOOK_DISABLE=1
    expect "$T the off switch: exit 0, no output, logos not run" "$RC|$OUT|$ERR|$(calls)" "0|||"
    expect "$T ... and no state written" "$([ -e "$REPO/.logos/stop-hook/state" ] && echo written || echo none)" none
    run "$B" "$REPO" LOGOS_STOP_HOOK_DISABLE=0
    expect "$T the off switch at 0 is not off (near miss): blocks" "$(decision)" block

    P="$WORK/plain$N"
    mkdir -p "$P/src" && printf 'fn a() {}\n' >"$P/src/a.rs"
    git -C "$P" init -q . && "${GIT[@]}" -C "$P" add -A && "${GIT[@]}" -C "$P" commit -qm x && printf 'fn b() {}\n' >"$P/src/a.rs"
    run "$B" "$P"
    expect "$T not enrolled (a dirty repository with no .logos/): exit 0, no output, logos not run" "$RC|$OUT|$ERR|$(calls)" "0|||"
    expect "$T ... and no .logos/ created" "$([ -e "$P/.logos" ] && echo created || echo absent)" absent
    mkdir -p "$WORK/dotfile$N" && : >"$WORK/dotfile$N/.logos"
    run "$B" "$WORK/dotfile$N"
    expect "$T a .logos that is a file is not a store (near miss): silent" "$RC|$OUT|$(calls)" "0||"

    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    NOLOGOS="$WORK/nologos$N"; mkdir -p "$NOLOGOS"
    STUBBIN_SAVE="$STUBBIN"; STUBBIN="$NOLOGOS"
    run "$B" "$REPO"
    STUBBIN="$STUBBIN_SAVE"
    expect "$T no logos on PATH: exit 0, no output" "$RC|$OUT|$ERR" "0||"

    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    rm -rf "$REPO/.git"
    run "$B" "$REPO"
    expect "$T enrolled but not a git repository: exit 0, no output, logos not run" "$RC|$OUT|$ERR|$(calls)" "0|||"

    # 4. not measured ---------------------------------------------------------
    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    echo 1 >"$STUB/check.delay"; echo 1 >"$STUB/review.delay"
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=1
    expect "$T a 1 ms budget: exit 0, the stop is not blocked" "$RC|$(decision)" "0|ABSENT"
    expect "$T ... it reports not measured" "$(has "$(jf systemMessage)" 'not measured')" yes
    expect "$T ... naming the budget" "$(has "$(jf systemMessage)" '1 ms budget')" yes
    expect "$T ... review was not run once the budget was spent" "$(ncalls review)|$(has "$(jf systemMessage)" 'review was not run: the 1 ms budget was spent')" "0|yes"
    expect "$T ... and it did not stay running: the slow check was cut" "$(ps -ax -o command= | grep -c "[${STUBBIN:0:1}]${STUBBIN:1}/logos check")" 0
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=1
    expect "$T ... a turn over budget records its state: not re-measured when unchanged" "$RC|$OUT|$(calls)" "0||"

    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    echo 2 >"$STUB/review.delay"
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=600
    expect "$T a slow review on a warm before-state: not measured, never a block" "$RC|$(decision)|$(has "$(jf systemMessage)" 'review did not finish')" "0|ABSENT|yes"
    expect "$T ... a background rebuild was started (the cache may be for another contract)" \
        "$(wait_for "$REPO/.logos/stop-hook/ready-$MB" 8 && echo ready || echo never)" ready
    expect "$T ... review ran in the foreground attempt and the rebuild" "$(ncalls review)" 2
    quiesce

    mkrepo cold; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    # The build is held open until the assertions about its running state are made.
    : >"$STUB/review.gate"
    run "$B" "$REPO"
    expect "$T a missing before-state: exit 0, not blocked" "$RC|$(decision)" "0|ABSENT"
    expect "$T ... reported as not measured, with the reason" "$(has "$(jf systemMessage)" 'not measured')|$(has "$(jf systemMessage)" 'built in the background')" "yes|yes"
    expect "$T ... review was not run in the foreground: the hook returned before it ended" \
        "$([ -e "$REPO/.logos/stop-hook/ready-$MB" ] && echo ready || echo not-yet)" not-yet
    expect "$T ... the turn is not recorded, so the next one is measured" "$([ -e "$REPO/.logos/stop-hook/state" ] && echo recorded || echo none)" none
    run "$B" "$REPO"
    expect "$T ... a second turn while it builds starts no second build (one at a time)" "$(ncalls review)" 0
    rm -f "$STUB/review.gate"
    expect "$T ... the detached build outlives the hook and finishes: the ready marker appears" \
        "$(wait_for "$REPO/.logos/stop-hook/ready-$MB" 8 && echo ready || echo never)" ready
    quiesce
    run "$B" "$REPO"
    expect "$T ... measured next turn, on the built before-state: it blocks" "$RC|$(decision)|$(ncalls review)" "0|block|1"

    mkrepo cold; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    printf '{"limit":100,"snapshots":[{"snapshot_id":1,"commit_sha":"%s","signal":1}],"warnings":[]}' "$MB" >"$STUB/evolution.json"
    printf '%s\n' "${CHECK_OK/\"checked_rules\":1/\"checked_rules\":0}" >"$STUB/check.json"
    run "$B" "$REPO"
    expect "$T a snapshot recorded at the merge-base, no rule to evaluate: review runs in the foreground and blocks" \
        "$RC|$(decision)|$(ncalls evolution)|$(ncalls review)" "0|block|1|1"
    expect "$T ... the probe asked for 100 snapshots, in the project" "$(grep -c "^evolution --json --limit 100|$REPO\$" "$CALLS")" 1
    expect "$T ... and no background build was started" "$([ -e "$REPO/.logos/stop-hook/build.lock" ] && echo lock || echo none)" none
    mkrepo cold; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    printf '{"limit":100,"snapshots":[{"snapshot_id":1,"commit_sha":"%s","signal":1}],"warnings":[]}' "$MB" >"$STUB/evolution.json"
    run "$B" "$REPO"
    expect "$T a snapshot at the merge-base is not used when check evaluated a rule (review reads the tree then): cold, not probed" \
        "$RC|$(decision)|$(ncalls evolution)|$(has "$(jf systemMessage)" 'built in the background')" "0|ABSENT|0|yes"
    quiesce
    for other in prefix zeros; do
        mkrepo cold; edit
        case "$other" in prefix) other="${MB:0:12}" ;; *) other="0000000000000000000000000000000000000000" ;; esac
        printf '%s\n' "${CHECK_OK/\"checked_rules\":1/\"checked_rules\":0}" >"$STUB/check.json"
        printf '{"limit":100,"snapshots":[{"snapshot_id":1,"commit_sha":"%s","signal":1}],"warnings":[]}' "$other" >"$STUB/evolution.json"
        run "$B" "$REPO"
        expect "$T a snapshot at another commit ($other) is no before-state (near miss): not measured, the review's regression is not read" \
            "$RC|$(decision)|$(has "$(jf systemMessage)" 'built in the background')" "0|ABSENT|yes"
        quiesce
    done

    mkrepo cold; edit
    echo 1 >"$STUB/review.delay"
    run "$B" "$REPO"
    LOCKPID="$(cat "$REPO/.logos/stop-hook/build.lock" 2>/dev/null)"
    [ -n "$LOCKPID" ] && kill -HUP "$LOCKPID" 2>/dev/null
    expect "$T the background build survives a SIGHUP to it (the host closing the hook's session)" \
        "$(wait_for "$REPO/.logos/stop-hook/ready-$MB" 8 && echo ready || echo died)" ready
    quiesce

    mkrepo cold; edit
    echo 1 >"$STUB/review.rc"
    run "$B" "$REPO"
    quiesce
    expect "$T a background build that fails leaves no ready marker and no lock, and a failed marker under the key" \
        "$([ -e "$REPO/.logos/stop-hook/ready-$MB" ] && echo ready || echo none)|$([ -e "$REPO/.logos/stop-hook/build.lock" ] && echo lock || echo none)|$(cat "$REPO/.logos/stop-hook/failed-$MB" 2>/dev/null)" \
        "none|none|$(keyof 'logos 99.0.0')"
    edit
    run "$B" "$REPO"
    expect "$T ... the next turn says it could not be built and does not retry" \
        "$RC|$(decision)|$(ncalls review)|$(has "$(jf systemMessage)" 'could not be built')" "0|ABSENT|0|yes"
    printf '[constraints]\nmax_cc = 9\n' >"$REPO/.logos/rules.toml"
    edit
    echo 0 >"$STUB/review.rc"
    run "$B" "$REPO"
    expect "$T ... until the contract changes: then the build is tried again" \
        "$(wait_for "$REPO/.logos/stop-hook/ready-$MB" 8 && echo ready || echo never)|$(cat "$REPO/.logos/stop-hook/ready-$MB" 2>/dev/null)" \
        "ready|$(keyof 'logos 99.0.0' "$REPO/.logos/rules.toml")"
    quiesce

    # The key: a before-state built under other rules, configuration or logos is cold.
    mkrepo; edit
    printf '[constraints]\nmax_cc = 9\n' >"$REPO/.logos/rules.toml"
    review_json true "$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T a before-state built before rules.toml changed is cold: not measured, rebuilt in the background" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'built in the background')" "0|ABSENT|yes"
    quiesce
    mkrepo; edit
    printf 'logos 99.1.0\n' >"$STUB/version"
    review_json true "$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T a before-state built by another logos version is cold" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'built in the background')" "0|ABSENT|yes"
    quiesce
    mkrepo cold; edit
    mkdir -p "$REPO/.logos/review-cache" && printf '{}' >"$REPO/.logos/review-cache/$MB.json"
    review_json true "$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T a cache file nobody here built is not trusted: cold" "$RC|$(decision)|$(has "$(jf systemMessage)" 'built in the background')" "0|ABSENT|yes"
    quiesce

    mkrepo; edit
    echo 3 >"$STUB/check.rc"
    review_json true "$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T a check that cannot run (exit 3): the review leg still blocks on its regression" "$RC|$(decision)" "0|block"
    expect "$T ... and says what it could not measure" "$(has "$(reason)" 'check could not run (exit 3)')" yes
    mkrepo; edit
    echo 3 >"$STUB/check.rc"
    run "$B" "$REPO"
    expect "$T a check that cannot run and nothing else: not measured, no block" "$RC|$(decision)|$(has "$(jf systemMessage)" 'check could not run (exit 3)')" "0|ABSENT|yes"

    mkrepo; edit
    printf '%s\n' '{"passed":false,"checked_rules":1,"rules_present":true,"violations":[{"rule":"max_cc","rule_type":"constraint","severity":"error","file":"src/app.rs","node_id":3,"message":"too complex"}],"ran_at":1,"freshness":"x","warnings":[]}' >"$STUB/check.json"
    echo 1 >"$STUB/check.rc"
    echo 2 >"$STUB/review.delay"
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=900
    expect "$T a failing rule still blocks when the review leg ran over budget, and names what was not measured" \
        "$RC|$(decision)|$(has "$(reason)" '- failing rule max_cc')|$(has "$(reason)" '(Not measured: review did not finish')" "0|block|yes|yes"
    quiesce

    mkrepo; edit
    review_json false "" "" >"$STUB/review.json"; echo 1 >"$STUB/review.rc"
    run "$B" "$REPO"
    expect "$T a review that could not classify: its reason, no block" "$RC|$(decision)|$(has "$(jf systemMessage)" 'no base ref resolves')" "0|ABSENT|yes"
    mkrepo; edit
    printf 'error: unrecognized subcommand review\n' >"$STUB/review.json"; echo 2 >"$STUB/review.rc"
    run "$B" "$REPO"
    expect "$T an older binary with no review: not measured, no block" "$RC|$(decision)|$(has "$(jf systemMessage)" 'review could not run (exit 2)')" "0|ABSENT|yes"

    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=soon
    expect "$T a budget that is not a number falls back to the default: measured, blocks" "$RC|$(decision)" "0|block"
    mkrepo; edit
    echo 1 >"$STUB/check.delay"
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=0015
    expect "$T a budget with leading zeros is read in base 10: 0015 is 15 ms" "$(has "$(jf systemMessage)" 'the 0015 ms')|$(has "$(jf systemMessage)" 'check did not finish within the 15 ms budget')" "no|yes"
    edit
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=08
    expect "$T ... and 08 is not an octal error" "$ERR|$(has "$(jf systemMessage)" 'within the 8 ms budget')" "|yes"
    # Past the hook's 30-second timeout the host would end a leg silently: the
    # budget is held at 25 s, so a leg that outlasts it is reported, never cut.
    mkrepo; edit
    : >"$STUB/check.gate"
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=99999
    rm -f "$STUB/check.gate"
    expect "$T a budget past the hook's timeout is held at 25000 ms, and the leg it cuts is not measured" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'check did not finish within the 25000 ms budget')" "0|ABSENT|yes"
    mkrepo; edit
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=
    expect "$T an empty budget falls back to the default: measured" "$(ncalls check)|$(ncalls review)" "1|1"

    mkrepo; edit
    git -C "$REPO" branch -q -m main trunk
    review_json true "$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T no main, master or origin/HEAD: not measured, no review call, no block" "$RC|$(decision)|$(ncalls review)|$(has "$(jf systemMessage)" 'no merge-base')" "0|ABSENT|0|yes"
    mkrepo; edit
    git -C "$REPO" branch -q -m main master
    review_json true "$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T master is the default branch when main is absent: measured, blocks" "$RC|$(decision)" "0|block"

    # 5. the verdicts the legs read -------------------------------------------
    mkrepo; edit
    review_json true "$(off deep new null 6 4 stale),$(off other new null 5 4 some-cause-tag app 9)" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T entries the graph could not fully resolve never alone block: silent" "$RC|$OUT" "0|"

    mkrepo; edit
    review_json true "$(off deep new null 6 4 stale),$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T ... one resolved entry blocks, and the unresolved one is listed with its label" \
        "$(decision)|$(has "$(reason)" '[new, stale]')|$(has "$(reason)" '[new, resolved]')" "block|yes|yes"

    mkrepo; edit
    review_json true "$(off grew worsened 5 8 4 resolved nesting_depth app 3),$(off fixed resolved 6 null 4 resolved),$(off better improved 9 6 4 resolved),$(off same unchanged 6 6 4 resolved)" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T a worsened entry blocks and reads before -> after" "$(decision)|$(has "$(reason)" '- grew (src/app.rs:3): nesting_depth 5 -> 8, threshold 4 [worsened, resolved]')" "block|yes"
    expect "$T resolved, improved and unchanged entries are not findings" "$(has "$(reason)" 'fixed')|$(has "$(reason)" 'better')|$(has "$(reason)" 'same')" "no|no|no"
    mkrepo; edit
    review_json true "$(off fixed resolved 6 null 4 resolved),$(off better improved 9 6 4 resolved)" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T only resolved and improved entries: a fix, silent" "$RC|$OUT" "0|"

    mkrepo; edit
    review_json true "" "$(viol max_cc new null 7 3 resolved 'fn `f` has cyclomatic complexity 7 > max_cc = 3')" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T a new violation blocks, with its rule, message, value and threshold" \
        "$(decision)|$(has "$(reason)" '- max_cc (src/app.rs): fn `f` has cyclomatic complexity 7 > max_cc = 3 (value 7, threshold 3) [new, resolved]')" "block|yes"

    mkrepo; edit
    printf '%s\n' '{"passed":false,"checked_rules":2,"rules_present":true,"violations":[{"rule":"max_fn_lines","rule_type":"constraint","severity":"error","file":"src/app.rs","node_id":3,"message":"fn `f` has 120 lines > max_fn_lines = 100"},{"rule":"no_god_files","rule_type":"constraint","severity":"warn","file":"src/big.rs","node_id":4,"message":"too many symbols"}],"ran_at":1,"freshness":"x","warnings":[]}' >"$STUB/check.json"
    echo 1 >"$STUB/check.rc"
    run "$B" "$REPO"
    expect "$T a failing rule blocks, with its message and the pointer to /logos:review (no review entry beside it)" \
        "$(decision)|$(has "$(reason)" '- failing rule max_fn_lines (src/app.rs): fn `f` has 120 lines > max_fn_lines = 100')|$(has "$(reason)" 'Run /logos:review')" "block|yes|yes"
    expect "$T ... a warning-severity rule is not listed" "$(has "$(reason)" 'no_god_files')" no
    expect "$T ... review still ran after the failing check" "$(ncalls review)" 1
    for rc in 4 5; do
        mkrepo; edit
        echo "$rc" >"$STUB/check.rc"
        run "$B" "$REPO"
        expect "$T check exit $rc (no contract / a security scan, not this leg's): not a regression" "$RC|$OUT" "0|"
    done
    mkrepo; edit
    printf '%s\n' '{"passed":false,"checked_rules":2,"rules_present":true,"violations":[{"rule":"max_cc","rule_type":"constraint","severity":"error","file":"src/app.rs","node_id":3,"message":"fn `f` has complexity 9 > max_cc = 3"}],"ran_at":1,"freshness":"x","warnings":[]}' >"$STUB/check.json"
    echo 5 >"$STUB/check.rc"
    run "$B" "$REPO"
    expect "$T check exit 5 outranks a failing rule: the JSON says passed false, so it is still a failing rule" \
        "$(decision)|$(has "$(reason)" '- failing rule max_cc (src/app.rs): fn `f` has complexity 9 > max_cc = 3')" "block|yes"
    # The binary's own answer for a required scan that is only missing: `passed` is
    # false with no violation, so it is not a failing rule.
    mkrepo; edit
    printf '%s\n' '{"passed":false,"checked_rules":2,"rules_present":true,"violations":[{"rule":"no_god_files","rule_type":"constraint","severity":"warn","file":"src/big.rs","node_id":4,"message":"too many symbols"}],"ran_at":1,"freshness":"x","warnings":[],"missing_scans":[{"scanner":"semgrep","state":"stale"}]}' >"$STUB/check.json"
    echo 5 >"$STUB/check.rc"
    run "$B" "$REPO"
    expect "$T check exit 5 with only a missing scan (passed false, no error rule): not a regression" "$RC|$OUT" "0|"
    mkrepo; edit
    printf '%s\n' '{"passed":false,"checked_rules":2,"rules_present":true,"violations":[],"ran_at":1,"freshness":"x","warnings":[],"security_violations":[{"rule":"security:ratchet","rule_type":"security","severity":"error","file":"app.py","message":"a new high finding"}],"missing_scans":[{"scanner":"semgrep","state":"stale"}]}' >"$STUB/check.json"
    echo 5 >"$STUB/check.rc"
    run "$B" "$REPO"
    expect "$T check exit 5 with a [security] violation: still a failing rule" "$(decision)" "block"

    mkrepo; edit
    OFFS=""
    for i in $(seq 1 15); do OFFS="$OFFS${OFFS:+,}$(off "fn$i" new null 6 4 resolved)"; done
    review_json true "$OFFS" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T a long list is cut at twelve with the count of the rest" \
        "$(has "$(reason)" '- fn12 ')|$(has "$(reason)" '- fn13 ')|$(has "$(reason)" '... and 3 more')" "yes|no|yes"

    mkrepo; edit
    OFFS=""
    for i in $(seq 1 800); do OFFS="$OFFS${OFFS:+,}$(off "fn$i" new null 6 4 resolved)"; done
    review_json true "$OFFS" "$(viol max_cc new null 7 3 resolved 'late')" >"$STUB/review.json"
    T0=$SECONDS
    run "$B" "$REPO"
    expect "$T 800 entries (a 190 KB answer) are parsed in seconds, not minutes, and the violation after them is counted (801 findings, 12 listed)" \
        "$RC|$(decision)|$(has "$(reason)" '... and 789 more')|$([ $((SECONDS - T0)) -lt 8 ] && echo fast || echo "slow:$((SECONDS - T0))s")" "0|block|yes|fast"

    mkrepo; edit
    review_json true "$(off 'a}b{\"q' new null 6 4 resolved),$(off after new null 7 4 resolved)" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T a name with a brace and a quote is read whole, and the entry after it is not lost" \
        "$(has "$(reason)" '- a}b{"q (src/app.rs:5)')|$(has "$(reason)" '- after (src/app.rs:5)')" "yes|yes"

    # 7. what the first suite missed (found by mutation) ------------------------
    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    git -C "$REPO" checkout -q main
    printf 'fn other() {}\n' >"$REPO/src/other.rs"
    "${GIT[@]}" -C "$REPO" add src/other.rs && "${GIT[@]}" -C "$REPO" commit -qm "main moves on"
    git -C "$REPO" checkout -q feat
    run "$B" "$REPO"
    expect "$T the merge-base, not the base's tip, keys the before-state (main moved on after the branch): measured, blocks" "$RC|$(decision)" "0|block"

    mkrepo cold
    printf 'fn app() { /* on the remote default branch */ }\n' >"$REPO/src/app.rs"
    "${GIT[@]}" -C "$REPO" add -A && "${GIT[@]}" -C "$REPO" commit -qm "develop"
    DEV="$(git -C "$REPO" rev-parse HEAD)"
    git -C "$REPO" update-ref refs/remotes/origin/develop "$DEV"
    git -C "$REPO" symbolic-ref refs/remotes/origin/HEAD refs/remotes/origin/develop
    edit
    keyof 'logos 99.0.0' >"$REPO/.logos/stop-hook/ready-$DEV" 2>/dev/null || { mkdir -p "$REPO/.logos/stop-hook"; keyof 'logos 99.0.0' >"$REPO/.logos/stop-hook/ready-$DEV"; }
    review_json true "$DEEP" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T origin/HEAD is the default branch before main: its merge-base keys the before-state" "$RC|$(decision)" "0|block"

    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    echo 1 >"$STUB/check.delay"; echo 1 >"$STUB/review.delay"
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=1500
    expect "$T the budget is shared: a check that takes 1 s leaves the review less than it needs" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'review did not finish within the 1500 ms budget')" "0|ABSENT|yes"
    quiesce

    mkrepo
    run "$B" "$REPO"
    expect "$T a clean first run records its state and is silent" "$RC|$OUT|$(calls)" "0||"
    printf 'fn app() { /* committed */ }\n' >"$REPO/src/app.rs"
    "${GIT[@]}" -C "$REPO" add -A && "${GIT[@]}" -C "$REPO" commit -qm "the turn"
    run "$B" "$REPO"
    expect "$T ... then a turn that only committed (clean tree, HEAD moved) is measured" "$(ncalls check)|$(ncalls review)" "1|1"

    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    printf 'fn app() {}\nfn ofd() {}\n' >"$REPO/src/app.rs"
    git -C "$REPO" add -A
    run "$B" "$REPO"
    printf 'fn app() {}\nfn ofd() {}\nfn two() {}\n' >"$REPO/src/app.rs"
    git -C "$REPO" add -A
    run "$B" "$REPO"
    expect "$T edits that are staged are changes: a second staged edit is measured again" "$(ncalls check)|$(ncalls review)" "1|1"

    mkrepo
    printf 'target/\n' >"$REPO/.gitignore"
    "${GIT[@]}" -C "$REPO" add -A && "${GIT[@]}" -C "$REPO" commit -qm ignore
    run "$B" "$REPO"
    mkdir -p "$REPO/target" && printf 'x' >"$REPO/target/x.o"
    run "$B" "$REPO"
    expect "$T a file git ignores is not source: silent, logos not run" "$RC|$OUT|$(calls)" "0||"

    mkrepo
    run "$B" "$REPO"
    mkdir -p "$REPO/docs/a"
    for ext in md markdown txt rst adoc pdf png jpg jpeg gif svg ico lock; do printf 'x' >"$REPO/docs/a/f.$ext"; printf 'x' >"$REPO/g.$ext"; done
    run "$B" "$REPO"
    expect "$T every documentation and media extension, at the root and nested, is not source: silent, check and review not run" \
        "$RC|$OUT|$(ncalls check)|$(ncalls review)|$(ncalls 'security scan')" "0||0|0|1"

    mkrepo; edit
    OFFS=""
    for i in $(seq 1 12); do OFFS="$OFFS${OFFS:+,}$(off "fn$i" new null 6 4 resolved)"; done
    review_json true "$OFFS" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T exactly twelve findings: all listed, no 'and more' line" "$(has "$(reason)" '- fn12 ')|$(has "$(reason)" '... and')" "yes|no"
    mkrepo; edit
    OFFS=""
    for i in $(seq 1 12); do OFFS="$OFFS${OFFS:+,}$(off "fn$i" new null 6 4 resolved)"; done
    review_json true "$OFFS" "" >"$STUB/review.json"
    printf '%s\n' '{"passed":false,"checked_rules":1,"rules_present":true,"violations":[{"rule":"max_cc","rule_type":"constraint","severity":"error","file":"src/app.rs","node_id":3,"message":"m"}],"ran_at":1,"freshness":"x","warnings":[]}' >"$STUB/check.json"
    echo 1 >"$STUB/check.rc"
    run "$B" "$REPO"
    expect "$T a failing rule and twelve entries: the count is across legs, so thirteen is one more" "$(has "$(reason)" '... and 1 more')|$(has "$(reason)" '- fn12 ')" "yes|no"

    mkrepo; edit
    review_json true "" "$(viol max_cc worsened 3 7 3 resolved 'worse')" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T a worsened violation reads before -> after" "$(has "$(reason)" '(value 3 -> 7, threshold 3) [worsened, resolved]')" yes

    mkrepo; edit
    review_json true "$(off nothr new null 6 null resolved),{\"dimension\":\"nesting\",\"status\":\"new\",\"name\":\"nomeasure\",\"file\":\"src/app.rs\",\"line\":5,\"after\":6,\"threshold\":4,\"trust\":\"resolved\"}" "" >"$STUB/review.json"
    run "$B" "$REPO"
    expect "$T an entry with no threshold reads 'no threshold'; one with no measure reads its dimension" \
        "$(has "$(reason)" '- nothr (src/app.rs:5): nesting_depth 6, no threshold')|$(has "$(reason)" '- nomeasure (src/app.rs:5): nesting 6, threshold 4')" "yes|yes"

    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    echo 2 >"$STUB/review.delay"; echo 1 >"$STUB/review.rc"
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=600
    expect "$T the hook leaves no scratch file of its own behind" "$(set -- "$REPO"/.logos/stop-hook/run.*; [ -e "$1" ] && echo $# || echo 0)" 0
    quiesce
    expect "$T an over-budget review drops the ready marker it could not trust, and a failed rebuild does not bring it back" \
        "$([ -e "$REPO/.logos/stop-hook/ready-$MB" ] && echo ready || echo gone)|$([ -e "$REPO/.logos/stop-hook/failed-$MB" ] && echo failed || echo none)" "gone|failed"

    mkrepo cold; edit
    mkdir -p "$REPO/.logos/stop-hook"
    sh -c ':' & DEADPID=$!
    wait "$DEADPID" 2>/dev/null
    echo "$DEADPID" >"$REPO/.logos/stop-hook/build.lock"
    run "$B" "$REPO"
    expect "$T a lock that names a dead process does not stop the next build" \
        "$(wait_for "$REPO/.logos/stop-hook/ready-$MB" 8 && echo ready || echo never)" ready
    quiesce

    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    PAYLOAD='{
  "session_id": "t",
  "stop_hook_active": true
}
'
    run "$B" "$REPO"
    expect "$T a pretty-printed payload with stop_hook_active on its own line: silent, logos not run" "$RC|$OUT|$(calls)" "0||"
    PAYLOAD='{"hook_event_name":"Stop","stop_hook_active":false,"session_id":"t"}'

    # 8. the security leg ----------------------------------------------------
    S="$T [security]"
    # scan_call: the security scan the hook asked for, cwd left out; empty when none.
    scan_call() { grep '^security scan ' "$CALLS" | sed 's/|.*//' || true; }
    gl_calls() { cat "$STUB/gitleaks.calls" 2>/dev/null || true; }

    mkrepo
    printf 'fn app() {}\nconst KEY: &str = "%s";\n' "$AWS_KEY" >"$REPO/src/app.rs"
    sec_findings "$(secret fp-key src/app.rs 2)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    expect "$S an added AWS-key-shaped literal: exit 0, decision block" "$RC|$(decision)" "0|block"
    R="$(reason)"
    expect "$S ... the reason names the rule, file, line and the redacted message" \
        "$(has "$R" '- aws-access-token (src/app.rs:2): secret-class finding redacted: rule aws-access-token at src/app.rs:2 [gitleaks, medium]')" yes
    expect "$S ... its first line says what the turn left" "$(printf '%s\n' "$R" | head -1)" \
        "logos: this turn left a new secret or high-severity finding in the files it changed. Fix it before stopping:"
    expect "$S ... it points to /logos:sec-review, and not to /logos:review (no quality entry was listed)" \
        "$(has "$R" 'Run /logos:sec-review')|$(has "$R" '/logos:review')|$(has "$R" 'This hook blocks once')" "yes|no|yes"
    expect "$S ... the secret's value is nowhere in what the hook wrote" "$(has "$OUT$ERR" "$AWS_KEY")" no
    expect "$S ... the scan named Gitleaks and the one changed file" "$(scan_call)" "security scan --json --scanner gitleaks --path src/app.rs"
    expect "$S ... and the hook spawned no scanner itself: Gitleaks ran once, from the scan" "$(gl_calls | wc -l | tr -d ' ')" 1
    edit
    PAYLOAD='{"hook_event_name":"Stop","stop_hook_active":true,"session_id":"t"}'
    run "$B" "$REPO"
    expect "$S ... stop_hook_active: it does not block again" "$RC|$OUT|$(calls)" "0||"
    PAYLOAD='{"hook_event_name":"Stop","stop_hook_active":false,"session_id":"t"}'
    printf 'fn other() {}\n' >"$REPO/src/other.rs"
    run "$B" "$REPO"
    expect "$S ... a later turn, the secret still there and found again under its recorded fingerprint: silent" \
        "$RC|$OUT|$(scan_call)" "0||security scan --json --scanner gitleaks --path src/app.rs --path src/other.rs"

    mkrepo; edit
    sec_findings "$(secret fp-old src/app.rs 2)" >"$STUB/security-findings.json"
    run "$B" "$REPO"
    expect "$S a turn with no new finding (a recorded secret found again): silent" "$RC|$OUT|$ERR" "0||"
    expect "$S ... the scan ran" "$(scan_call)" "security scan --json --scanner gitleaks --path src/app.rs"
    for sev in info low medium; do
        mkrepo; edit
        sec_findings "$(fnd fp-$sev python.lang.weak-hash $sev false src/app.rs 3 'weak hash' 'Semgrep OSS')" >"$STUB/security-findings-after.json"
        run "$B" "$REPO"
        expect "$S a new $sev finding that is no secret: silent" "$RC|$OUT" "0|"
    done
    for sev in high critical; do
        mkrepo; edit
        sec_findings "$(fnd fp-$sev python.lang.sqli $sev false src/app.rs 3 'SQL built from a request field' 'Semgrep OSS')" >"$STUB/security-findings-after.json"
        run "$B" "$REPO"
        expect "$S a new $sev finding blocks, with its message, scanner and severity" \
            "$(decision)|$(has "$(reason)" "- python.lang.sqli (src/app.rs:3): SQL built from a request field [Semgrep OSS, $sev]")" "block|yes"
    done

    mkrepo; edit
    sec_findings "$(secret fp-a src/untouched.rs 1),$(secret fp-b src/app.rsx 1)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    expect "$S a new secret in a file the turn did not change (and one a character off a changed path): not this turn's, silent" "$RC|$OUT" "0|"

    mkrepo; edit
    printf 'AWS_ACCESS_KEY_ID=%s\n' "$AWS_KEY" >"$REPO/.env"
    sec_findings "$(secret fp-env .env 1)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    expect "$S a secret in a file the graph does not index (.env: no path, only the report's): scanned, and it blocks" \
        "$(decision)|$(has "$(reason)" '- aws-access-token (.env:1): secret-class finding redacted')|$(has "$(scan_call)" '--path .env')" "block|yes|yes"
    expect "$S ... and the value is nowhere in the output" "$(has "$OUT" "$AWS_KEY")" no

    mkrepo; edit
    printf 'fn other() {}\n' >"$REPO/src/other.rs"
    run "$B" "$REPO"
    expect "$S the stored findings are listed per changed file, before and after the scan: never the whole store" \
        "$(grep '^security findings ' "$CALLS" | sed 's/|.*//')" "security findings --json --suppression active --path src/app.rs
security findings --json --suppression active --path src/other.rs
security findings --json --suppression active --path src/app.rs
security findings --json --suppression active --path src/other.rs"

    # A large store (a full Semgrep, Trivy and OSV scan stored) costs nothing when
    # the findings lie in other files. Read whole, a thousand of them took 31 s
    # under bash 3.2, past the host's 30 s timeout.
    mkrepo; edit
    MANY=""
    for i in $(seq 1 1000); do MANY="$MANY${MANY:+,}$(fnd "fp-other-$i" python.lang.weak medium false "src/f$i.rs" 3 'weak hash' 'Semgrep OSS')"; done
    sec_findings "$MANY" >"$STUB/security-findings.json"
    sec_findings "$MANY,$(secret fp-key src/app.rs 2)" >"$STUB/security-findings-after.json"
    T0=$SECONDS
    run "$B" "$REPO"
    expect "$S a store of 1000 findings in other files: the new secret blocks, within seconds" \
        "$RC|$(decision)|$([ $((SECONDS - T0)) -lt 10 ] && echo fast || echo "slow:$((SECONDS - T0))s")" "0|block|fast"

    mkrepo; edit
    MANY=""
    for i in $(seq 1 200); do MANY="$MANY${MANY:+,}$(fnd "fp-app-$i" python.lang.weak medium false src/app.rs "$i" 'weak hash' 'Semgrep OSS')"; done
    sec_findings "$MANY" >"$STUB/security-findings.json"
    sec_findings "$MANY,$(secret fp-key src/app.rs 2)" >"$STUB/security-findings-after.json"
    T0=$SECONDS
    run "$B" "$REPO"
    expect "$S a changed file holding more stored findings than the hook reads: not measured, no block, and quick" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'security not measured: src/app.rs holds more stored findings than the hook reads')|$([ $((SECONDS - T0)) -lt 10 ] && echo fast || echo slow)|$(scan_call)" \
        "0|ABSENT|yes|fast|"

    mkrepo
    run "$B" "$REPO"
    printf 'AWS key: %s\n' "$AWS_KEY" >>"$REPO/README.md"
    sec_findings "$(secret fp-doc README.md 2)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    expect "$S a turn that only added a key to the README: the security leg alone runs, and it blocks" \
        "$RC|$(decision)|$(ncalls check)|$(ncalls review)|$(has "$(reason)" '- aws-access-token (README.md:2)')|$(has "$OUT" "$AWS_KEY")" "0|block|0|0|yes|no"
    run "$B" "$REPO"
    expect "$S ... stopped again with nothing changed: silent, logos not run" "$RC|$OUT|$(calls)" "0||"
    printf 'more\n' >>"$REPO/NOTES.txt"
    : >"$STUB/consent"
    run "$B" "$REPO"
    expect "$S ... a documentation-only turn without consent says so, and spawns nothing" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'security not measured: no consent')|$(gl_calls | grep -c NOTES)" "0|ABSENT|yes|0"

    mkrepo; edit
    review_json true "$DEEP" "" >"$STUB/review.json"
    sec_findings "$(secret fp-key src/app.rs 2)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    R="$(reason)"
    expect "$S a regression and a secret in one turn: one block naming both, with both pointers" \
        "$(decision)|$(printf '%s\n' "$R" | head -1)|$(has "$R" 'Run /logos:review')|$(has "$R" 'Run /logos:sec-review')|$(has "$R" '- deep (src/app.rs:5)')" \
        "block|logos: this turn left a quality regression against the merge-base and a new secret or high-severity finding in the files it changed. Fix it before stopping:|yes|yes|yes"

    mkrepo; edit
    printf '%s\n' '{"passed":false,"checked_rules":1,"rules_present":true,"violations":[{"rule":"max_cc","rule_type":"constraint","severity":"error","file":"src/app.rs","node_id":3,"message":"too complex"}],"ran_at":1,"freshness":"x","warnings":[]}' >"$STUB/check.json"
    echo 1 >"$STUB/check.rc"
    review_json true "$DEEP,$(off deeper new null 7 4 resolved)" "" >"$STUB/review.json"
    sec_findings "$(secret fp-k1 src/app.rs 1),$(secret fp-k2 src/app.rs 2)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    R="$(reason)"
    expect "$S a failing rule, two regressions and two secrets: each subject is named once" "$(printf '%s\n' "$R" | head -1)" \
        "logos: this turn left a quality regression against the merge-base and a new secret or high-severity finding in the files it changed. Fix it before stopping:"
    expect "$S ... and each pointer is given once" \
        "$(printf '%s' "$R" | grep -o 'Run /logos:review' | wc -l | tr -d ' ')|$(printf '%s' "$R" | grep -o 'Run /logos:sec-review' | wc -l | tr -d ' ')" "1|1"

    for consent in '' 'semgrep' 'gitleaks-extra' 'trivy semgrep'; do
        mkrepo; edit
        echo 'gitleaks semgrep' >"$STUB/installed"
        printf '%s\n' "$consent" >"$STUB/consent"
        sec_findings "$(secret fp-key src/app.rs 1)" >"$STUB/security-findings-after.json"
        run "$B" "$REPO"
        expect "$S no consent for Gitleaks (consented: [$consent]): not blocked, and the leg says 'not measured: no consent'" \
            "$RC|$(decision)|$(has "$(jf systemMessage)" 'security not measured: no consent')" "0|ABSENT|yes"
        expect "$S ... the scan refused it before anything ran: no scanner spawned, no status probe, nothing listed after, no Semgrep scan" \
            "$(gl_calls)|$(ncalls 'security status')|$(ncalls 'security findings')|$(scan_call)" "|0|1|security scan --json --scanner gitleaks --path src/app.rs"
    done

    for state in not_installed disabled; do
        mkrepo; edit
        echo semgrep >"$STUB/installed"
        sec_scan "$state" not_selected >"$STUB/security-scan-gitleaks.json"
        sec_findings "$(secret fp-key src/app.rs 1)" >"$STUB/security-findings-after.json"
        run "$B" "$REPO"
        case "$state" in not_installed) why='Gitleaks is not installed' ;; *) why='Gitleaks is disabled' ;; esac
        expect "$S Gitleaks $state: nothing scanned, not blocked, the leg says why, and Semgrep is not scanned alone" \
            "$RC|$(decision)|$(has "$(jf systemMessage)" "security not measured: $why")|$(gl_calls)|$(scan_call)" \
            "0|ABSENT|yes||security scan --json --scanner gitleaks --path src/app.rs"
    done

    mkrepo; edit
    sec_findings "$(secret fp-key src/app.rs 1)" >"$STUB/security-findings-after.json"
    echo 2 >"$STUB/security-scan-gitleaks.delay"
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=900
    expect "$S over budget (a scan slower than the budget left): not measured, and no block although it would find a secret" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'not measured')|$(has "$(jf systemMessage)" 'the security scan did not finish within the 900 ms budget')" "0|ABSENT|yes|yes"
    expect "$S ... the hook leaves no scan running behind it" "$(ps -ax -o command= | grep -c "[${STUBBIN:0:1}]${STUBBIN:1}/logos security scan")" 0
    # A scan that stored a new secret, and a read-back the budget cut: not measured
    # now, and the secret is still new to the next turn, not absorbed as known.
    mkrepo; edit
    sec_findings "$(secret fp-key src/app.rs 2)" >"$STUB/security-findings-after.json"
    # The read-back is held until the hook has given up on it, whatever the load.
    : >"$STUB/security-findings.after-scan.gate"
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=8000
    expect "$S the scan stored a secret but its findings were not read back in budget: not measured, no block" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'the security scan did not finish within the 8000 ms budget')|$(ncalls 'security scan')" "0|ABSENT|yes|1"
    expect "$S ... the HEAD the next scan starts from does not move, and the baseline from before that scan is kept" \
        "$([ -e "$REPO/.logos/stop-hook/scanned-head" ] && echo moved || echo none)|$([ -s "$REPO/.logos/stop-hook/scan-unjudged" ] && echo kept || echo none)" "none|kept"
    rm -f "$STUB/security-findings.after-scan.gate"
    printf 'fn other() {}\n' >"$REPO/src/other.rs"
    run "$B" "$REPO"
    expect "$S ... the next turn judges it: the secret already in the store is still new, and it blocks" \
        "$RC|$(decision)|$(has "$(reason)" '- aws-access-token (src/app.rs:2)')" "0|block|yes"
    expect "$S ... and, judged, the baseline is dropped and the HEAD recorded" \
        "$([ -e "$REPO/.logos/stop-hook/scan-unjudged" ] && echo kept || echo dropped)|$([ -e "$REPO/.logos/stop-hook/scanned-head" ] && echo recorded || echo none)" "dropped|recorded"
    printf 'fn third() {}\n' >"$REPO/src/third.rs"
    run "$B" "$REPO"
    expect "$S ... the turn after that: known now, silent" "$RC|$OUT" "0|"

    mkrepo; edit
    echo 1 >"$STUB/check.delay"
    run "$B" "$REPO" LOGOS_STOP_HOOK_BUDGET_MS=1
    expect "$S a budget spent before the leg: not measured, nothing asked of security at all" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'the security scan was not run: the 1 ms budget was spent')|$(ncalls security)" "0|ABSENT|yes|0"

    # semgrep_installed CONSENT: Semgrep installed and enabled, CONSENT the consent record.
    semgrep_installed() {
        echo 'gitleaks semgrep' >"$STUB/installed"
        echo "$1" >"$STUB/consent"
        sec_scan ran not_selected >"$STUB/security-scan-gitleaks.json"
        sec_scan not_selected ran >"$STUB/security-scan-semgrep.json"
    }
    mkrepo; edit
    semgrep_installed 'gitleaks semgrep'
    sec_findings "$(fnd fp-sqli python.lang.sqli high false src/app.rs 3 'SQL built from a request field' 'Semgrep OSS')" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    expect "$S Semgrep installed and consented: a second scan names it, and its new high finding blocks" \
        "$(scan_call)|$(decision)" "security scan --json --scanner gitleaks --path src/app.rs
security scan --json --scanner semgrep --path src/app.rs|block"
    mkrepo; edit
    semgrep_installed gitleaks
    run "$B" "$REPO"
    expect "$S Semgrep installed but not consented: its scan is refused before anything runs, and nothing is said" \
        "$(scan_call)|$RC|$OUT" "security scan --json --scanner gitleaks --path src/app.rs
security scan --json --scanner semgrep --path src/app.rs|0|"
    mkrepo; edit
    echo 'gitleaks semgrep' >"$STUB/consent"
    run "$B" "$REPO"
    expect "$S Semgrep consented but not installed (the Gitleaks scan reads it not_installed): Gitleaks alone" "$(scan_call)" "security scan --json --scanner gitleaks --path src/app.rs"
    mkrepo; edit
    semgrep_installed 'gitleaks semgrep'
    sec_scan not_selected failed >"$STUB/security-scan-semgrep.json"
    sec_findings "$(secret fp-key src/app.rs 1)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    expect "$S Semgrep failing does not lose Gitleaks' secret: it blocks, and names Semgrep as not measured" \
        "$(decision)|$(has "$(reason)" 'Semgrep did not run on the changed files (failed)')" "block|yes"

    mkrepo; edit
    sec_scan failed not_installed >"$STUB/security-scan-gitleaks.json"
    sec_findings "$(secret fp-key src/app.rs 1)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    expect "$S Gitleaks failing: not measured, no block, its findings not read, and the HEAD the next scan starts from does not move" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'Gitleaks did not run (failed)')|$(ncalls 'security findings')|$([ -e "$REPO/.logos/stop-hook/scanned-head" ] && echo moved || echo none)" "0|ABSENT|yes|1|none"
    mkrepo; edit
    echo 2 >"$STUB/security-scan-gitleaks.rc"
    run "$B" "$REPO"
    expect "$S a scan that exits 2 for another reason than consent: 'could not run', not 'no consent'; no block" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'the scan could not run (exit 2)')|$(has "$(jf systemMessage)" 'no consent')" "0|ABSENT|yes|no"
    mkrepo; edit
    printf 'error: unrecognized subcommand security\n' >"$STUB/security-findings.json"; echo 2 >"$STUB/security-findings.rc"
    sec_findings "$(secret fp-key src/app.rs 1)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    expect "$S an older binary with no security findings: not measured, nothing scanned, no block" \
        "$RC|$(decision)|$(has "$(jf systemMessage)" 'security findings could not run (exit 2)')|$(scan_call)" "0|ABSENT|yes|"

    # A project one level below its repository's root (a workspace member): paths
    # are the project's, and a change outside it is not the project's.
    N=$((N + 1))
    MONO="$WORK/mono$N" STUB="$WORK/stub$N" REPO="$WORK/mono$N/svc"
    mkdir -p "$REPO/src" "$STUB"
    (
        cd "$MONO" || exit 1
        git init -q -b main . 2>/dev/null || { git init -q . && git checkout -q -b main; }
        printf 'fn app() {}\n' >svc/src/app.rs
        printf 'fn top() {}\n' >top.rs
        "${GIT[@]}" add -A && "${GIT[@]}" commit -qm base
        git checkout -q -b feat
    ) || exit 1
    MB="$(git -C "$MONO" rev-parse HEAD)"
    mkdir -p "$REPO/.logos/stop-hook"
    keyof 'logos 99.0.0' >"$REPO/.logos/stop-hook/ready-$MB"
    printf '%s\n' "$CHECK_OK" >"$STUB/check.json"
    review_json true "" "" >"$STUB/review.json"
    sec_scan ran not_installed >"$STUB/security-scan-gitleaks.json"
    sec_findings "" >"$STUB/security-findings.json"
    printf 'const KEY: &str = "%s";\n' "$AWS_KEY" >"$REPO/src/app.rs"
    printf 'fn top() { /* edited */ }\n' >"$MONO/top.rs"
    sec_findings "$(secret fp-key src/app.rs 1)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    expect "$S a project in a subdirectory of its repository: project-relative paths, the file outside it left out, and the secret blocks" \
        "$(scan_call)|$(decision)" "security scan --json --scanner gitleaks --path src/app.rs|block"

    # Which files: what differs from HEAD and what is untracked, regular files only.
    mkrepo
    printf 'fn gone() {}\n' >"$REPO/src/gone.rs"
    "${GIT[@]}" -C "$REPO" add -A && "${GIT[@]}" -C "$REPO" commit -qm gone
    run "$B" "$REPO"
    rm "$REPO/src/gone.rs"
    edit
    printf '# changed\n' >>"$REPO/README.md"
    printf 'notes\n' >"$REPO/NOTES.md"
    printf 'fn new() {}\n' >"$REPO/src/new.rs"
    printf 'x\n' >"$REPO/-dash.rs"
    printf 'x\n' >"$REPO/.logos/scratch.rs"
    ln -s app.rs "$REPO/src/link.rs"
    mkdir -p "$REPO/src/sub" && printf 'fn s() {}\n' >"$REPO/src/sub/s.rs"
    run "$B" "$REPO"
    expect "$S the changed files: edited, documentation and untracked ones; never a deleted file, a symlink, .logos/ or a flag-shaped name" \
        "$(scan_call)" "security scan --json --scanner gitleaks --path README.md --path src/app.rs --path NOTES.md --path src/new.rs --path src/sub/s.rs"

    mkrepo
    run "$B" "$REPO"
    rm "$REPO/src/app.rs"
    run "$B" "$REPO"
    expect "$S a turn that only deleted files: nothing to scan, nothing asked of security, silent" "$RC|$OUT|$(ncalls security)" "0||0"

    mkrepo; edit
    run "$B" "$REPO"
    printf 'fn b() {}\n' >"$REPO/src/b.rs"
    "${GIT[@]}" -C "$REPO" add -A && "${GIT[@]}" -C "$REPO" commit -qm "the turn"
    sec_findings "$(secret fp-b src/b.rs 1)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    expect "$S a turn that committed its work: the files committed since the last scan are scanned, and the secret blocks" \
        "$(scan_call)|$(decision)" "security scan --json --scanner gitleaks --path src/app.rs --path src/b.rs|block"
    printf 'fn c() {}\n' >"$REPO/src/c.rs"
    run "$B" "$REPO"
    expect "$S ... the next turn starts from the HEAD that scan saw" "$(scan_call)" "security scan --json --scanner gitleaks --path src/c.rs"
    git -C "$REPO" checkout -q main
    printf 'fn d() {}\n' >"$REPO/src/d.rs"
    run "$B" "$REPO"
    expect "$S ... a HEAD the last scan saw that is no ancestor (another branch checked out): only what differs from HEAD" \
        "$(scan_call)" "security scan --json --scanner gitleaks --path src/c.rs --path src/d.rs"

    # No scan of its own to start from yet: the HEAD the previous measured turn saw.
    mkrepo; edit
    : >"$STUB/consent"
    run "$B" "$REPO"
    expect "$S a first turn without consent: not measured, and no scan to start from is recorded" \
        "$(has "$(jf systemMessage)" 'no consent')|$([ -e "$REPO/.logos/stop-hook/scanned-head" ] && echo recorded || echo none)" "yes|none"
    echo gitleaks >"$STUB/consent"
    printf 'const KEY: &str = "%s";\n' "$AWS_KEY" >"$REPO/src/b.rs"
    "${GIT[@]}" -C "$REPO" add -A && "${GIT[@]}" -C "$REPO" commit -qm "the turn"
    sec_findings "$(secret fp-b src/b.rs 1)" >"$STUB/security-findings-after.json"
    run "$B" "$REPO"
    expect "$S ... consent given, a turn that committed everything is scanned from the HEAD the previous turn saw, and its secret blocks" \
        "$(scan_call)|$(decision)" "security scan --json --scanner gitleaks --path src/app.rs --path src/b.rs|block"

    mkrepo; edit
    echo 1 >"$STUB/security-scan-gitleaks.rc"
    run "$B" "$REPO"
    expect "$S a scan that did not complete does not move the HEAD it starts from" \
        "$([ -e "$REPO/.logos/stop-hook/scanned-head" ] && echo moved || echo none)" none
    expect "$S ... the hook's scan record stays out of git" "$(git -C "$REPO" status --porcelain --untracked-files=all | grep -c 'stop-hook')" 0

done

# --- the real binary, opt-in ----------------------------------------------------

if [ -n "${LOGOS_STOP_E2E_BIN:-}" ]; then
    B="${BASHES[0]}"
    E2E="$WORK/e2e"
    mkdir -p "$E2E/bin" "$E2E/r/src"
    ln -sf "$LOGOS_STOP_E2E_BIN" "$E2E/bin/logos"
    (
        cd "$E2E/r" || exit 1
        git init -q -b main . && printf 'pub fn shallow(x: i32) -> i32 { x }\n' >src/lib.rs
        "${GIT[@]}" add -A && "${GIT[@]}" commit -qm base
        "$E2E/bin/logos" init --quiet >/dev/null 2>&1 && "$E2E/bin/logos" index >/dev/null 2>&1
        git checkout -q -b feat
        printf 'pub fn deep(x: i32) -> i32 { if x > 0 { if x > 1 { if x > 2 { if x > 3 { if x > 4 { if x > 5 { return 1; } } } } } } 0 }\n' >>src/lib.rs
    )
    STUBBIN="$E2E/bin"
    REPO="$E2E/r"
    run "$B" "$REPO"
    expect "[e2e] a cold repository: not measured, the build starts in the background" "$RC|$(decision)|$(has "$(jf systemMessage)" 'built in the background')" "0|ABSENT|yes"
    MBE="$(git -C "$REPO" rev-parse main)"
    wait_for "$REPO/.logos/stop-hook/ready-$MBE" 120 || wait_for "$REPO/.logos/review-cache/$MBE.json" 1
    quiesce
    run "$B" "$REPO"
    expect "[e2e] once built, the 6-deep function blocks once, with its value and threshold" \
        "$RC|$(decision)|$(has "$(reason)" '- deep (src/lib.rs')|$(has "$(reason)" 'nesting_depth 6, threshold 4')" "0|block|yes|yes"
    PAYLOAD='{"stop_hook_active":true}'
    printf '\n' >>"$REPO/src/lib.rs"
    run "$B" "$REPO"
    expect "[e2e] with stop_hook_active it does not block again" "$RC|$OUT" "0|"
    PAYLOAD='{"hook_event_name":"Stop","stop_hook_active":false,"session_id":"t"}'

    # The security leg against the real binary, with a stub Gitleaks that writes the
    # SARIF Gitleaks writes (one aws-access-token result per AKIA… literal, the
    # match redacted) and records its argv.
    STUB="$E2E/gl"
    mkdir -p "$STUB" "$E2E/s/src"
    cat >"$E2E/bin/gitleaks" <<'EOF'
#!/bin/sh
printf '%s\n' "$*" >>"$STUB_DIR/gitleaks.calls"
[ "$1" = version ] && { echo 8.18.4; exit 0; }
out="" src=""
while [ $# -gt 0 ]; do
    case "$1" in --report-path) out="$2"; shift ;; --source) src="$2"; shift ;; esac
    shift
done
res=""
scan() {
    n=0
    while IFS= read -r l || [ -n "$l" ]; do
        n=$((n + 1))
        case "$l" in
            *AKIA[0-9A-Z][0-9A-Z][0-9A-Z][0-9A-Z]*)
                pre="${l%%AKIA*}"; c=$((${#pre} + 1))
                res="$res${res:+,}{\"ruleId\":\"aws-access-token\",\"message\":{\"text\":\"AWS key\"},\"locations\":[{\"physicalLocation\":{\"artifactLocation\":{\"uri\":\"$1\"},\"region\":{\"startLine\":$n,\"startColumn\":$c,\"endLine\":$n,\"endColumn\":$((c + 20)),\"snippet\":{\"text\":\"REDACTED\"}}}}],\"partialFingerprints\":{\"commitSha\":\"\",\"email\":\"\"}}"
                ;;
        esac
    done <"$1"
}
if [ "$src" = . ]; then for f in $(git ls-files); do scan "$f"; done; elif [ -f "$src" ]; then scan "$src"; fi
printf '{"version":"2.1.0","runs":[{"tool":{"driver":{"name":"gitleaks","semanticVersion":"v8.18.4","rules":[{"id":"aws-access-token"}]}},"results":[%s]}]}\n' "$res" >"$out"
EOF
    chmod +x "$E2E/bin/gitleaks"
    (
        cd "$E2E/s" || exit 1
        git init -q -b main . && printf 'pub fn a() {}\n' >src/lib.rs
        "${GIT[@]}" add -A && "${GIT[@]}" commit -qm base
        "$E2E/bin/logos" init --quiet >/dev/null 2>&1 && "$E2E/bin/logos" index >/dev/null 2>&1
        git checkout -q -b feat
    )
    REPO="$E2E/s"
    printf 'pub fn b() {}\n' >"$REPO/src/b.rs"
    : >"$STUB/gitleaks.calls"
    run "$B" "$REPO" HOME="$WORK/no-consent"
    expect "[e2e] security without consent: 'not measured: no consent', and Gitleaks was never started, not even for its version" \
        "$RC|$(has "$(jf systemMessage)" 'security not measured: no consent')|$(wc -l <"$STUB/gitleaks.calls" | tr -d ' ')" "0|yes|0"
    (cd "$REPO" && env HOME="$WORK" PATH="$E2E/bin:$GITDIR:/usr/bin:/bin" STUB_DIR="$STUB" "$E2E/bin/logos" security scan --consent --scanner gitleaks >/dev/null 2>&1)
    printf 'pub fn a() {}\npub const KEY: &str = "%s";\n' "$AWS_KEY" >"$REPO/src/lib.rs"
    : >"$STUB/gitleaks.calls"
    run "$B" "$REPO"
    expect "[e2e] with consent, an added AWS key blocks, redacted, pointing to /logos:sec-review" \
        "$RC|$(decision)|$(has "$(reason)" '- aws-access-token (src/lib.rs:2): secret-class finding redacted: rule aws-access-token at src/lib.rs:2')|$(has "$(reason)" '/logos:sec-review')|$(has "$OUT" "$AWS_KEY")" \
        "0|block|yes|yes|no"
    expect "[e2e] ... Gitleaks ran in content mode, once per changed file, on those files only" \
        "$(grep '^detect' "$STUB/gitleaks.calls" | sed 's/.*--report-path [^ ]* //')" "--no-git --source src/lib.rs
--no-git --source src/b.rs"
    printf 'pub fn c() {}\n' >"$REPO/src/c.rs"
    run "$B" "$REPO"
    expect "[e2e] ... the next turn finds it again under its recorded fingerprint: no security block" \
        "$RC|$(has "$OUT" 'sec-review')" "0|no"
fi

# --- 6. the manifest ------------------------------------------------------------

expect "[manifest] hooks.json declares the Stop hook once, at stop.sh, with room for the default budget" \
    "$(python3 -I - "$PLUGIN/hooks/hooks.json" <<'PY'
import json, sys
g = json.load(open(sys.argv[1]))["hooks"]["Stop"]
cmds = [(h["command"], h["timeout"]) for e in g for h in e["hooks"]]
print(len(cmds), cmds[0][0], cmds[0][1] > 15)
PY
)" '1 "${CLAUDE_PLUGIN_ROOT}/hooks/stop.sh" True'
expect "[manifest] the script's default budget is the 15 s the requirement names" \
    "$(grep -c 'LOGOS_STOP_HOOK_BUDGET_MS:-15000' "$HOOK")" 1
expect "[manifest] the legs are one list a new leg is added to (one line, words only)" "$(grep -c -E '^LEGS="[a-z_]+( [a-z_]+)*"$' "$HOOK")" 1
expect "[manifest] the security leg runs last, after check and review" "$(grep -c '^LEGS="check review security"$' "$HOOK")" 1

echo
echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
