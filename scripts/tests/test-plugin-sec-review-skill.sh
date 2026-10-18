#!/usr/bin/env bash
#
# test-plugin-sec-review-skill.sh — prove the logos-sec-review skill of the logos
# Claude Code plugin (plugins/logos/skills/logos-sec-review/, S-687, FR-SC-19)
# fixes only what an agent may close, escalates the rest, and proves each fix by a
# targeted re-scan.
#
# The skill's judgement is the model's and is not tested here. What is tested is
# every deterministic step it stands on, each script against a fixture, each
# stating its denominator:
#
#   skills/logos-sec-review/scripts/sec-delta.sh        the security section of the delta
#   skills/logos-sec-review/scripts/sec-triage.sh       the verdict proposed per finding
#   skills/logos-sec-review/scripts/sec-rescan.sh       the targeted re-scan of the fix files
#   skills/logos-sec-review/scripts/sec-proof.sh        a fix proven: the fingerprint absent
#   skills/logos-sec-review/scripts/sec-coverage.sh     the scanners and the uncovered OWASP categories
#   skills/logos-sec-review/scripts/suppress-draft.sh   the ignore entry a false positive gets
#   skills/logos-sec-review/scripts/sec-check-report.sh the rules on the report
#
# and the logos-review scripts it reuses in place (edit-scope.sh, merge-base.sh).
#
# `logos` is a stub that answers the documented JSON of `review`, `security scan`
# and `security findings` from fixture files, so no binary, scanner, index or
# network is involved. The shapes are the ones in docs/howto/commands.md and
# logos-core/src/models/quality.rs (SecurityDelta), security/surface.rs.
#
# Layers:
#   1. the skill's files, and the logos-review lib it sources;
#   2. sec-delta.sh: measured, not measured, missing prerequisites;
#   3. sec-triage.sh: one finding of each class gives the five verdicts, and the
#      near misses of every rule;
#   4. the whole loop on that fixture, under each distinct bash: only the fix file
#      is edited, the re-scan proves it with the fingerprint absent, the report
#      says rotate, the suppression stops at ask;
#   5. the proof: every way a fix is not proven;
#   6. the re-scan: every way it did not happen;
#   7. the report check: each rule, each by a report one line from valid, and the
#      claim grep with its near misses;
#   8. the draft, and the guardrail that holds its write.
#
# Usage: bash scripts/tests/test-plugin-sec-review-skill.sh
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
PLUGIN="$HERE/plugins/logos"
SKILL="$PLUGIN/skills/logos-sec-review"
SCR="$SKILL/scripts"
RSCR="$PLUGIN/skills/logos-review/scripts"

WORK="${TMPDIR:-/tmp}/logos-plugin-sec-review-skill-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi; }
contains() { case "$2" in *"$3"*) ok "$1" ;; *) bad "$1: '$3' not in '$2'" ;; esac; }
lacks() { case "$2" in *"$3"*) bad "$1: '$3' found in '$2'" ;; *) ok "$1" ;; esac; }

for f in sec-delta sec-triage sec-rescan sec-proof sec-coverage suppress-draft sec-check-report; do
    [ -f "$SCR/$f.sh" ] || { echo "FAIL  missing $SCR/$f.sh"; echo "0 passed, 1 failed (of 1)"; exit 1; }
done
[ -f "$SCR/lib/secrev.py" ] && [ -f "$SCR/lib/secrev.sh" ] ||
    { echo "FAIL  missing scripts/lib"; echo "0 passed, 1 failed (of 1)"; exit 1; }
for f in review-delta merge-base edit-scope; do
    [ -f "$RSCR/$f.sh" ] || { echo "FAIL  missing the logos-review script $RSCR/$f.sh"; echo "0 passed, 1 failed (of 1)"; exit 1; }
done

BASHES=()
for b in /bin/bash "$(command -v bash)"; do
    [ -x "$b" ] || continue
    r="$(cd "$(dirname "$b")" && pwd -P)/$(basename "$b")"
    dup=no
    for s in "${BASHES[@]+"${BASHES[@]}"}"; do [ "$s" = "$r" ] && dup=yes; done
    [ "$dup" = yes ] || BASHES+=("$r")
done

py() { python3 -I -c "$@"; }

# --- fixture: the stub logos ----------------------------------------------------------

FX="$WORK/fx"
BIN="$WORK/bin"
CALLS="$WORK/calls"
mkdir -p "$FX" "$BIN"

# Builtins only, so the same stub also runs under a PATH that holds nothing else.
cat >"$BIN/logos" <<'STUB'
#!/bin/bash
printf '%s\n' "$*" >>"$STUB_CALLS"
# Each argument in its own <...>, so a flattened argv shows; and the directory the call ran in.
{ printf '<%s>' "$@"; echo; } >>"$STUB_CALLS.argv"
printf '%s|%s\n' "$PWD" "$*" >>"$STUB_CALLS.pwd"
emit() { local l; while IFS= read -r l || [ -n "$l" ]; do printf '%s\n' "$l"; done <"$1"; }
sub="${1:-}"
[ $# -gt 0 ] && shift
case "$sub" in
    --version) echo "logos ${STUB_VERSION:-99.0.0}" ;;
    review)
        echo "merge-base before-state read from the cache" >&2
        emit "$STUB_DIR/${STUB_REVIEW:-review}.json"
        exit "${STUB_REVIEW_RC:-0}"
        ;;
    sync) exit "${STUB_SYNC_RC:-0}" ;;
    security)
        what="${1:-} ${2:-}"
        case "$what" in
            "scan --help")
                if [ -n "${STUB_OLD_SCAN:-}" ]; then
                    echo "Usage: logos security scan [OPTIONS]"
                    echo "      --consent         Record consent"
                else
                    echo "Usage: logos security scan [OPTIONS]"
                    echo "      --scanner <NAME>  Run only the named scanners"
                    echo "      --path <PATH>     Scan only these paths"
                fi
                ;;
            scan*)
                if [ -n "${STUB_SCAN_FAIL:-}" ]; then
                    echo "error: the first scan needs --consent" >&2
                    exit 2
                fi
                # With STUB_SCAN_PER, each scanner has its own answer: <STUB_SCAN>.<scanner letters>.json.
                slug="" prev=""
                for a in "$@"; do
                    [ "$prev" = "--scanner" ] && slug="${a//[^A-Za-z0-9]/}"
                    prev="$a"
                done
                if [ -n "${STUB_SCAN_FAIL_FOR:-}" ] && [ "$slug" = "$STUB_SCAN_FAIL_FOR" ]; then
                    echo "error: scan refused" >&2
                    exit 2
                fi
                if [ -n "${STUB_SCAN_PER:-}" ]; then
                    emit "$STUB_DIR/${STUB_SCAN:-scan}.$slug.json"
                else
                    emit "$STUB_DIR/${STUB_SCAN:-scan}.json"
                fi
                ;;
            findings*) emit "$STUB_DIR/${STUB_FINDINGS:-findings}.json"; exit "${STUB_FINDINGS_RC:-0}" ;;
            *) echo "error: unrecognized: security $what" >&2; exit 2 ;;
        esac
        ;;
    *)
        echo "error: unrecognized subcommand '$sub'" >&2
        exit 2
        ;;
esac
STUB
chmod +x "$BIN/logos"

# The fixture: one finding of each class the skill judges, and three it does not.
# Fingerprints are 64 hex characters, as the store gives them.
python3 -I - "$FX" <<'PY'
import hashlib, json, sys

fx = sys.argv[1]
fp = lambda s: hashlib.sha256(s.encode()).hexdigest()


def finding(name, status, scanner, rule, severity, path, line, **kw):
    f = {
        "status": status, "method": "fingerprint", "scanner": scanner, "fingerprint": fp(name),
        "rule_id": rule, "severity": severity, "owasp": ["A05:2025"], "message": f"{rule} at {path}",
        "redacted": False, "path": path, "start_line": line, "end_line": line, "freshness": "fresh",
        "reachability": "reachable", "confidence": "resolved",
    }
    f.update(kw)
    return f


known = finding("known", "pre-existing", "Semgrep OSS", "python.lang.security.audit.known", "critical", "app/routes.py", 40)
findings = [
    finding("secret", "introduced", "gitleaks", "aws-access-key", "high", "config/loader.py", 4,
            redacted=True, message="secret-class finding redacted: rule aws-access-key at config/loader.py:4"),
    finding("fix", "introduced", "Semgrep OSS", "python.sqlalchemy.security.sql-injection", "high", "app/routes.py", 12),
    finding("fp", "introduced", "Semgrep OSS", "python.lang.security.audit.insecure-hash", "medium", "app/util.py", 7),
    finding("untrusted", "introduced", "Semgrep OSS", "python.lang.security.audit.dynamic-urllib", "medium", "app/dyn.py", 3,
            confidence="unresolved", reachability="unknown"),
    finding("dep", "touched", "osv-scanner", "GHSA-abcd-efgh-ijkl", "high", "requirements.txt", 2),
    finding("pre", "pre-existing", "Semgrep OSS", "python.lang.security.audit.eval", "medium", "app/old.py", 30),
    finding("res", "resolved", "Semgrep OSS", "python.lang.security.audit.exec", "medium", "app/gone.py", 9),
    known,
]
suppressed = [dict(finding("sup", "introduced", "Semgrep OSS", "python.flask.security.debug", "low", "app/dbg.py", 1),
                   reason="dev only", expires="2999-12-31")]
section = {
    "measured": True, "not_measured_because": None,
    "scanners": [{"scanner": "Semgrep OSS", "method": "fingerprint", "latest_head": "abc"},
                 {"scanner": "gitleaks", "method": "hunk", "latest_head": "abc"}],
    "findings": findings,
    "counts": {"introduced": 4, "touched": 1, "resolved": 1, "stale": 0, "pre_existing": 2, "suppressed": 1},
    "suppressed": suppressed,
    "notes": ["Semgrep OSS has not re-scanned since the merge-base"],
}
review = {"base": "main", "merge_base": "abc123", "measured": True, "offenders": [], "violations": [],
          "introduced_clone_groups": [], "stale_files": [], "sections": {"security": section}}
json.dump(review, open(f"{fx}/review.json", "w"))
json.dump({"base": "main", "merge_base": "abc123", "measured": False, "not_measured_because": "no merge-base",
           "sections": {}}, open(f"{fx}/review-unmeasured.json", "w"))
nosec = dict(review, sections={"security": dict(section, measured=False, findings=[], counts={},
                                                  not_measured_because="no security report is stored")})
json.dump(nosec, open(f"{fx}/review-nosec.json", "w"))
clean = dict(review, sections={"security": dict(section, findings=[findings[5]], suppressed=[],
                                                  counts={"introduced": 0, "touched": 0, "resolved": 0, "stale": 0,
                                                          "pre_existing": 1, "suppressed": 0})})
json.dump(clean, open(f"{fx}/review-clean.json", "w"))

scan_run = lambda scanner, driver, outcome="ran", scope=None, version="1.85.0": dict(
    {"scanner": scanner, "driver": driver, "version": version, "outcome": outcome},
    **({"scope": scope} if scope else {}))
paths = lambda *p: {"kind": "paths", "paths": list(p)}
json.dump({"consent_recorded": [], "findings": 0, "scanners": [
    scan_run("semgrep", "Semgrep OSS", scope=paths("app/routes.py")), scan_run("gitleaks", "gitleaks", "not_selected")]},
    open(f"{fx}/scan.json", "w"))
json.dump({"consent_recorded": [], "findings": 0, "scanners": [
    scan_run("semgrep", "Semgrep OSS", "failed")]}, open(f"{fx}/scan-failed.json", "w"))
json.dump({"consent_recorded": [], "findings": 0, "scanners": [
    scan_run("semgrep", "Semgrep OSS", scope=paths("app/other.py"))]}, open(f"{fx}/scan-otherpath.json", "w"))
json.dump({"consent_recorded": [], "findings": 0, "scanners": [
    scan_run("semgrep", "Semgrep OSS", scope={"kind": "unscoped", "reason": "whole-manifest only"})]},
    open(f"{fx}/scan-unscoped.json", "w"))

cats = [
    {"id": "A01:2025", "name": "Broken Access Control", "scanners": ["semgrep"], "enabled_scanners": ["semgrep"],
     "state": {"state": "covered", "findings": 1}},
    {"id": "A02:2025", "name": "Security Misconfiguration", "scanners": [], "enabled_scanners": [],
     "state": {"state": "not_covered"}},
    {"id": "A03:2025", "name": "Software Supply Chain Failures", "scanners": ["osv-scanner"], "enabled_scanners": ["osv-scanner"],
     "state": "covered"},
    {"id": "A10:2025", "name": "Mishandling of Exceptional Conditions", "scanners": [], "enabled_scanners": [],
     "state": "not_covered"},
]
scanned = [{"driver": "Semgrep OSS", "version": "1.85.0", "head_sha": "abc", "findings": 3},
           {"driver": "gitleaks", "version": "8.18.4", "head_sha": "abc", "findings": 1}]
base_f = {"edition": "2025", "as_of": "2026-10-17", "scanned": scanned, "never_run": [], "notice": None,
          "total": 1, "categories": cats, "suppressions": []}
json.dump(dict(base_f, findings=[]), open(f"{fx}/findings.json", "w"))
# After the fix: the injection's fingerprint is gone. A neighbour in the same file (known
# at the delta), and one in another file, remain.
other = dict(finding("other", "pre-existing", "Semgrep OSS", "python.lang.security.audit.other", "critical",
                      "app/elsewhere.py", 5))
json.dump(dict(base_f, findings=[other]), open(f"{fx}/findings-after.json", "w"))
json.dump(dict(base_f, findings=[finding("fix", "introduced", "Semgrep OSS", "python.sqlalchemy.security.sql-injection",
                                          "high", "app/routes.py", 12)]), open(f"{fx}/findings-still.json", "w"))
json.dump(dict(base_f, findings=[finding("moved", "introduced", "Semgrep OSS", "python.sqlalchemy.security.sql-injection",
                                          "high", "app/routes.py", 14)]), open(f"{fx}/findings-moved.json", "w"))
json.dump(dict(base_f, findings=[finding("worse", "introduced", "Semgrep OSS", "python.x.critical", "critical",
                                          "app/routes.py", 20)]), open(f"{fx}/findings-worse.json", "w"))
json.dump(dict(base_f, findings=[finding("lower", "introduced", "Semgrep OSS", "python.x.low", "medium",
                                          "app/routes.py", 20)]), open(f"{fx}/findings-lower.json", "w"))
json.dump(dict(base_f, findings=[known]), open(f"{fx}/findings-known-fp.json", "w"))
json.dump(dict(base_f, findings=[finding("stale", "pre-existing", "Semgrep OSS", "python.x.stale", "low", "app/routes.py", 3,
                                          freshness="stale")]), open(f"{fx}/findings-stale.json", "w"))
json.dump(dict(base_f, scanned=[scanned[1]], findings=[]), open(f"{fx}/findings-noreport.json", "w"))
json.dump(dict(base_f, categories=[], scanned=scanned), open(f"{fx}/findings-nocats.json", "w"))
open(f"{fx}/secret-fp", "w").write(fp("secret"))
open(f"{fx}/fix-fp", "w").write(fp("fix"))
open(f"{fx}/fp-fp", "w").write(fp("fp"))
open(f"{fx}/dep-fp", "w").write(fp("dep"))
open(f"{fx}/untrusted-fp", "w").write(fp("untrusted"))
PY
[ -f "$FX/review.json" ] || { echo "FAIL  the fixture was not written"; echo "0 passed, 1 failed (of 1)"; exit 1; }

# --- fixture: the enrolled git repository --------------------------------------------

GR="$WORK/repo"
mkdir -p "$GR/app" "$GR/config" "$GR/.logos"
(
    cd "$GR" || exit 1
    git init -q . && git config user.email t@t && git config user.name t
    printf 'def q(db, name):\n    return db.execute("select * from t where n=" + name)\n' >app/routes.py
    printf 'import hashlib\nh = hashlib.md5\n' >app/util.py
    printf 'KEY = "AKIAEXAMPLE"\n' >config/loader.py
    printf 'urllib3==1.0\n' >requirements.txt
    printf 'x = 1\n' >app/dyn.py
    git add -A && git commit -q -m base
)
git -C "$GR" rev-parse HEAD >/dev/null 2>&1 || { echo "FAIL  no fixture repository"; echo "0 passed, 1 failed (of 1)"; exit 1; }

STUB_ENV=(STUB_DIR="$FX" STUB_CALLS="$CALLS")
# run [VAR=value ...] -- BASH SCRIPT ARGS...
run() {
    local envs=()
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    local b="$1"
    shift
    RC=0
    (cd "$GR" && env PATH="$BIN:$PATH" "${STUB_ENV[@]}" "${envs[@]+"${envs[@]}"}" "$b" "$@") >"$WORK/out" 2>"$WORK/err" || RC=$?
    OUT="$(cat "$WORK/out")" ERR="$(cat "$WORK/err")"
}
den() { contains "$1: states its denominator" "$ERR" "denominator:"; }
B0="${BASHES[0]}"

# --- 1. the skill's files ---------------------------------------------------------------

echo "== 1. the skill's files"
SKILL_MD="$SKILL/SKILL.md"
CMD_MD="$PLUGIN/commands/sec-review.md"
[ -f "$SKILL_MD" ] && ok "SKILL.md exists" || bad "SKILL.md is missing"
[ -f "$CMD_MD" ] && ok "commands/sec-review.md exists" || bad "commands/sec-review.md is missing"
if [ -f "$SKILL_MD" ]; then
    expect "SKILL.md names the skill logos-sec-review" "$(sed -n 's/^name: //p' "$SKILL_MD" | head -1)" "logos-sec-review"
    expect "SKILL.md opens with frontmatter" "$(head -1 "$SKILL_MD")" "---"
    contains "SKILL.md has a description" "$(grep -c '^description: .\{40,\}' "$SKILL_MD")" "1"
    n=0
    miss=""
    for s in sec-delta sec-triage sec-rescan sec-proof sec-coverage suppress-draft sec-check-report; do
        n=$((n + 1))
        grep -q "scripts/$s.sh" "$SKILL_MD" || miss="$miss $s"
    done
    expect "SKILL.md names each of the $n scripts it ships" "$miss" ""
    for s in merge-base edit-scope; do
        grep -q "logos-review/scripts/$s.sh" "$SKILL_MD" && ok "SKILL.md reuses logos-review/scripts/$s.sh in place" ||
            bad "SKILL.md does not name logos-review/scripts/$s.sh"
    done
    for w in rotate "git history" ask fingerprint uncovered "never says the code is fine"; do
        grep -qi "$w" "$SKILL_MD" && ok "SKILL.md says '$w'" || bad "SKILL.md never says '$w'"
    done
fi
if [ -f "$CMD_MD" ]; then
    contains "the command names the skill" "$(cat "$CMD_MD")" "logos-sec-review"
    expect "the command has a description" "$(grep -c '^description: ' "$CMD_MD")" "1"
fi
# A script sources the logos-review lib; it does not carry a copy of it.
expect "no script of the skill copies a logos-review file" "$(find "$SKILL" -name common.sh -o -name delta.py | wc -l | tr -d ' ')" "0"
expect "the logos-review scripts are untouched here (not moved)" "$([ -f "$RSCR/lib/common.sh" ] && [ -f "$RSCR/lib/delta.py" ] && echo yes)" "yes"
contains "secrev.sh sources logos-review's common.sh" "$(grep '^\. ' "$SCR/lib/secrev.sh")" "SR_REVIEW_SCRIPTS"

# --- 2. sec-delta.sh --------------------------------------------------------------------

echo "== 2. sec-delta.sh"
run -- "$B0" "$SCR/sec-delta.sh" --out "$WORK/delta.json"
expect "measured: exit 0" "$RC" 0
contains "measured: the counts" "$OUT" "5 findings to judge (introduced 4, touched 1)"
contains "measured: the merge-base is named" "$OUT" "merge-base abc123"
contains "measured: the note the section carries" "$OUT" "note: Semgrep OSS has not re-scanned since the merge-base"
contains "measured: the denominator names what was not judged" "$ERR" "5 to judge of 8 unsuppressed finding(s) listed, 1 suppressed (not judged)"
expect "measured: the file holds the security section only" "$(py 'import json,sys; d=json.load(open(sys.argv[1])); print(sorted(d), len(d["security"]["findings"]))' "$WORK/delta.json")" "['base', 'merge_base', 'security'] 8"
expect "measured: no intermediate file is left" "$(ls "$WORK" | grep -c 'delta.json.review')" 0
run STUB_REVIEW=review-nosec -- "$B0" "$SCR/sec-delta.sh" --out "$WORK/delta-nosec.json"
expect "no security report: exit 1, not an empty list" "$RC" 1
contains "no security report: says why" "$OUT" "not measured: no security report is stored"
contains "no security report: denominator says empty is not clean" "$ERR" "an empty list is not a clean one"
run STUB_REVIEW=review-unmeasured STUB_REVIEW_RC=1 -- "$B0" "$SCR/sec-delta.sh" --out "$WORK/delta-un.json"
expect "an unmeasured review carries no section: exit 1" "$RC" 1
contains "an unmeasured review: the review's own reason" "$OUT" "no merge-base"
run -- "$B0" "$SCR/sec-delta.sh"
expect "no --out: exit 2" "$RC" 2
run -- "$B0" "$SCR/sec-delta.sh" --out "$WORK/x.json" --bogus
expect "an unknown argument: exit 2" "$RC" 2
run -- "$B0" "$SCR/sec-delta.sh" --out "$WORK/no/such/dir/x.json"
expect "an unwritable target: exit 3" "$RC" 3
run STUB_VERSION=1.19.0 -- "$B0" "$SCR/sec-delta.sh" --out "$WORK/x.json"
expect "a logos older than 1.20.0: exit 3" "$RC" 3
(cd "$WORK" && mkdir -p plain) && RC=0 && (cd "$WORK/plain" && env PATH="$BIN:$PATH" "${STUB_ENV[@]}" "$B0" "$SCR/sec-delta.sh" --out "$WORK/x.json") >"$WORK/out" 2>"$WORK/err" || RC=$?
expect "not an enrolled repository: exit 3" "$RC" 3
contains "not enrolled: says to run logos init" "$(cat "$WORK/err")" "logos init"

# --- 3. sec-triage.sh: the five verdicts -----------------------------------------------

echo "== 3. sec-triage.sh"
run -- "$B0" "$SCR/sec-triage.sh" --delta "$WORK/delta.json" --out "$WORK/triage.json"
expect "triage: exit 0" "$RC" 0
den "triage"
contains "triage: the denominator counts the unjudged" "$ERR" "5 triaged of 8 unsuppressed finding(s) listed (1 suppressed, 2 pre-existing, 1 resolved, 0 stale: not judged)"
verdict_of() { py 'import json,sys; d=json.load(open(sys.argv[1])); print([e["proposed"]+(":final" if e["final"] else ":open") for e in d["entries"] if e["rule_id"]==sys.argv[2]][0])' "$WORK/triage.json" "$1"; }
expect "the secret is escalate-secret, final" "$(verdict_of aws-access-key)" "escalate-secret:final"
expect "the dependency finding is escalate-dependency, final" "$(verdict_of GHSA-abcd-efgh-ijkl)" "escalate-dependency:final"
expect "the injection is a proposed fix, open" "$(verdict_of python.sqlalchemy.security.sql-injection)" "fix:open"
expect "the unresolved-reachability finding is untrusted, final" "$(verdict_of python.lang.security.audit.dynamic-urllib)" "untrusted:final"
expect "the safe-in-context pattern is a proposed fix, open (the model moves it to false-positive)" "$(verdict_of python.lang.security.audit.insecure-hash)" "fix:open"
expect "five rows, in the delta's order" "$(py 'import json,sys; print(",".join(e["id"] for e in json.load(open(sys.argv[1]))["entries"]))' "$WORK/triage.json")" "s1,s2,s3,s4,s5"
contains "the table shows the open row as fix?" "$OUT" "| fix? | app/routes.py:12 python.sqlalchemy.security.sql-injection [$(cut -c1-12 "$FX/fix-fp")]"
contains "the table shows the final row bare" "$OUT" "| escalate-secret | config/loader.py:4 aws-access-key ["
lacks "the unjudged pre-existing finding is not a row" "$OUT" "audit.eval"
lacks "the unjudged resolved finding is not a row" "$OUT" "audit.exec"
lacks "the suppressed finding is not a row" "$OUT" "flask.security.debug"
expect "the triage keeps the not-judged counts" "$(py 'import json,sys; print(json.load(open(sys.argv[1]))["not_judged"])' "$WORK/triage.json")" "{'resolved': 1, 'stale': 0, 'pre_existing': 2, 'suppressed': 1}"

# The near miss of every rule, one field from the fixture's own finding.
variant() { # name python-expression-on-finding-dict   (the finding is f; findings = [f])
    python3 -I - "$FX/review.json" "$WORK/v-$1.json" "$2" <<'PY'
import json, sys
src, out, expr = sys.argv[1:4]
d = json.load(open(src))
sec = d["sections"]["security"]
f = dict(sec["findings"][1])  # the injection
exec(expr)
sec["findings"] = [f]
sec["counts"] = {"introduced": 1, "touched": 0, "resolved": 0, "stale": 0, "pre_existing": 0, "suppressed": 0}
sec["suppressed"] = []
json.dump({"base": "main", "merge_base": "abc123", "security": sec}, open(out, "w"))
PY
}
proposed_of() { # name -> "<verdict> <reason>"
    run -- "$B0" "$SCR/sec-triage.sh" --delta "$WORK/v-$1.json" --out "$WORK/vt-$1.json"
    py 'import json,sys; e=json.load(open(sys.argv[1]))["entries"][0]; print(e["proposed"], "|", e["reason"])' "$WORK/vt-$1.json"
}
variant plain 'pass'
contains "near miss: the plain injection stays a proposed fix" "$(proposed_of plain)" "fix |"
variant redacted 'f["redacted"]=True'
contains "a redacted entry is a secret whatever its scanner" "$(proposed_of redacted)" "escalate-secret | secret-class: rotate the credential"
variant secretrule 'f["rule_id"]="generic.secrets.password"'
contains "near miss: a rule id that says 'secret' is not enough without the redaction flag" "$(proposed_of secretrule)" "fix |"
variant trivycve 'f["scanner"]="Trivy"; f["rule_id"]="CVE-2024-12345"'
contains "Trivy with an advisory id is a dependency finding" "$(proposed_of trivycve)" "escalate-dependency"
variant trivyds 'f["scanner"]="Trivy"; f["rule_id"]="DS002"'
contains "near miss: Trivy with a misconfiguration id is code" "$(proposed_of trivyds)" "fix |"
variant semgrepcve 'f["rule_id"]="CVE-2024-12345"'
contains "near miss: an advisory-shaped id from Semgrep is code" "$(proposed_of semgrepcve)" "fix |"
variant cvetail 'f["scanner"]="Trivy"; f["rule_id"]="CVE-2024-12345-extra"'
contains "near miss: an advisory id with a tail is code" "$(proposed_of cvetail)" "fix |"
variant depcheck 'f["scanner"]="dependency-check"; f["rule_id"]="CVE-2024-1"'
contains "dependency-check is a dependency scanner whatever the rule" "$(proposed_of depcheck)" "escalate-dependency"
variant osvx 'f["scanner"]="osv-scanner-x"'
contains "near miss: a scanner named osv-scanner-x is not osv-scanner" "$(proposed_of osvx)" "fix |"
variant unbound 'f["confidence"]="unbound"'
contains "confidence unbound is untrusted" "$(proposed_of unbound)" "untrusted | reachability confidence is unbound"
variant caps 'f["confidence"]="Resolved"'
contains "near miss: 'Resolved' is not the label 'resolved'" "$(proposed_of caps)" "untrusted"
variant nolabel 'del f["confidence"]'
contains "a missing label is untrusted" "$(proposed_of nolabel)" "untrusted | reachability confidence is missing"
variant stale 'f["freshness"]="stale"'
contains "a file edited since the scan is untrusted" "$(proposed_of stale)" "untrusted | stale: the file changed"
variant nopath 'f["path"]=None'
contains "a finding bound to no file is untrusted" "$(proposed_of nopath)" "untrusted | the finding binds to no file"
variant secretunres 'f["redacted"]=True; f["confidence"]="unresolved"'
contains "a secret stays escalate-secret even when its reachability is unresolved" "$(proposed_of secretunres)" "escalate-secret"
run -- "$B0" "$SCR/sec-triage.sh" --delta "$WORK/delta-nosec.json" --out "$WORK/t-nosec.json"
expect "triage of an unmeasured delta: exit 1" "$RC" 1
contains "triage of an unmeasured delta: says so" "$OUT" "not measured"
python3 -I - "$FX/review-clean.json" "$WORK/delta-clean.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
json.dump({"base": "main", "merge_base": "abc123", "security": d["sections"]["security"]}, open(sys.argv[2], "w"))
PY
run -- "$B0" "$SCR/sec-triage.sh" --delta "$WORK/delta-clean.json" --out "$WORK/t-clean.json"
expect "a branch that introduced and touched nothing: exit 0" "$RC" 0
contains "a clean branch prints NO ENTRIES" "$OUT" "NO ENTRIES"
contains "a clean branch states 0 triaged of 1 listed" "$ERR" "0 triaged of 1 unsuppressed finding(s) listed"
run -- "$B0" "$SCR/sec-triage.sh" --delta "$WORK/missing.json" --out "$WORK/t.json"
expect "an unreadable delta: exit 3" "$RC" 3
run -- "$B0" "$SCR/sec-triage.sh" --delta "$WORK/delta.json"
expect "triage without --out: exit 2" "$RC" 2

# --- 4. the whole loop, under each bash ------------------------------------------------

NB="${#BASHES[@]}"
for B in "${BASHES[@]}"; do
    echo "== 4. the loop on the fixture, under $B"
    L="$WORK/loop.$$.$RANDOM"
    mkdir -p "$L"
    (cd "$GR" && git checkout -q -- . && git clean -fdq -e .logos)

    run -- "$B" "$SCR/sec-delta.sh" --out "$L/delta.json"
    expect "[$B] delta: exit 0" "$RC" 0
    run -- "$B" "$SCR/sec-triage.sh" --delta "$L/delta.json" --out "$L/triage.json"
    expect "[$B] triage: exit 0" "$RC" 0
    expect "[$B] the five rows are the five classes" "$(py 'import json,sys; print(",".join(sorted(e["class"]+":"+e["proposed"] for e in json.load(open(sys.argv[1]))["entries"])))' "$L/triage.json")" "code:fix,code:fix,code:untrusted,dependency:escalate-dependency,secret:escalate-secret"

    # The model's judgement, stood in for: s2 stays a fix, s3 becomes a false positive.
    run -- "$B" "$RSCR/edit-scope.sh" begin --state "$L/scope.state"
    expect "[$B] edit-scope begin: exit 0" "$RC" 0
    printf 'def q(db, name):\n    return db.execute("select * from t where n=%%s", (name,))\n' >"$GR/app/routes.py"
    run -- "$B" "$RSCR/edit-scope.sh" check --state "$L/scope.state" --allow app/routes.py
    expect "[$B] only the fix file was edited: edit-scope exit 0" "$RC" 0
    contains "[$B] edit-scope states its denominator" "$ERR" "1 dirty path(s) now, 1 changed since begin, 1 allowed; 0 outside"
    printf 'KEY = ""\n' >"$GR/config/loader.py"
    run -- "$B" "$RSCR/edit-scope.sh" check --state "$L/scope.state" --allow app/routes.py
    expect "[$B] deleting the secret's value is an edit outside the fix set: exit 1" "$RC" 1
    contains "[$B] edit-scope names the secret's file" "$OUT" "outside the fix set: config/loader.py"
    (cd "$GR" && git checkout -q -- config/loader.py)

    : >"$CALLS"
    run -- "$B" "$SCR/sec-rescan.sh" --triage "$L/triage.json" --out "$L/rescan.json" --entry s2
    expect "[$B] re-scan: exit 0" "$RC" 0
    contains "[$B] re-scan: names the scanner, its version and the path" "$OUT" "rescanned: Semgrep OSS 1.85.0 on app/routes.py (paths)"
    contains "[$B] re-scan: states its denominator" "$ERR" "1 scanner run(s) held to 1 asked, 0 problem(s)"
    expect "[$B] re-scan: the edit was synced first" "$(grep -n '^sync' "$CALLS" | head -1 | cut -d: -f2)" "sync -- app/routes.py"
    expect "[$B] re-scan: one targeted scan, one scanner, one path, no consent flag" "$(grep '^security scan --scanner' "$CALLS")" "security scan --scanner Semgrep OSS --path app/routes.py --json"
    lacks "[$B] the re-scan never records consent" "$(cat "$CALLS")" "--consent"
    lacks "[$B] the re-scan never scans the whole project" "$(grep '^security scan --scanner' "$CALLS")" "security scan --json"

    run STUB_FINDINGS=findings-after -- "$B" "$SCR/sec-proof.sh" --triage "$L/triage.json" --delta "$L/delta.json" --rescan "$L/rescan.json" --entry s2
    expect "[$B] proof: exit 0" "$RC" 0
    expect "[$B] proof: cites the re-scan with the fingerprint absent" "$OUT" "PROVEN s2 | app/routes.py:12 python.sqlalchemy.security.sql-injection [$(cut -c1-12 "$FX/fix-fp")] | rescan: Semgrep OSS 1.85.0 paths=app/routes.py fingerprint absent"
    contains "[$B] proof: denominator" "$ERR" "1 fix entry checked, 1 proven, 0 not proven"

    # The report the skill writes from those outputs.
    python3 -I - "$L/triage.json" "$L/report.md" "$FX" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))
fx = sys.argv[3]
sub = {e["rule_id"]: e for e in t["entries"]}
S = lambda r: sub[r]["subject"]
fp = sub["python.lang.security.audit.insecure-hash"]["fingerprint"]
lines = [
    "```",
    f"ENTRY | escalate-secret | {S('aws-access-key')} | secret-class, redacted",
    f"ENTRY | fix | {S('python.sqlalchemy.security.sql-injection')} | a trusted injection in code the branch added",
    f"ENTRY | false-positive | {S('python.lang.security.audit.insecure-hash')} | the digest is not used for a credential",
    f"ENTRY | untrusted | {S('python.lang.security.audit.dynamic-urllib')} | reachability confidence is unresolved",
    f"ENTRY | escalate-dependency | {S('GHSA-abcd-efgh-ijkl')} | an advisory on a pinned package",
    f"ROTATE | {S('aws-access-key')} | rotate the credential; the value remains in git history",
    f"FIXED | {S('python.sqlalchemy.security.sql-injection')} | rescan: Semgrep OSS 1.85.0 paths=app/routes.py fingerprint absent | tests: pytest -q | 12 passed, 0 failed (of 12)",
    f"SUPPRESS | {S('python.lang.security.audit.insecure-hash')} | fingerprint {fp} | state: held at ask | reason: the digest is not used for a credential",
    f"DEPENDENCY | {S('GHSA-abcd-efgh-ijkl')} | proposal: urllib3 1.0 -> 1.26.19 | changelog risk: a minor line, no API removal | blast radius: requests, 3 callers",
    "SCANNED | Semgrep OSS 1.85.0 | paths: app/routes.py",
    "COVERAGE | uncovered: A02:2025, A10:2025",
    "```",
    "",
    "Rotate the AWS credential and revoke the old one: it stays in git history.",
]
open(sys.argv[2], "w").write("\n".join(lines) + "\n")
PY
    run -- "$B" "$SCR/sec-coverage.sh" --out "$L/coverage.json" --paths app/routes.py
    expect "[$B] coverage: exit 0" "$RC" 0
    contains "[$B] coverage: the uncovered categories, by id" "$OUT" "COVERAGE | uncovered: A02:2025, A10:2025"
    contains "[$B] coverage: a SCANNED line per stored report" "$OUT" "SCANNED | gitleaks 8.18.4 | paths: app/routes.py"
    contains "[$B] coverage: denominator" "$ERR" "2 stored report(s), 4 OWASP categories (edition 2025), 2 uncovered"
    expect "[$B] coverage: no finding is kept in the output file" "$(py 'import json,sys; print(sorted(json.load(open(sys.argv[1]))))' "$L/coverage.json")" "['categories', 'edition', 'scanned', 'uncovered']"
    run -- "$B" "$SCR/sec-check-report.sh" --report "$L/report.md" --triage "$L/triage.json" --coverage "$L/coverage.json"
    expect "[$B] the report holds, against the gate and the coverage: exit 0" "$RC" 0
    contains "[$B] check-report: denominator with the held-to parts" "$ERR" "15 report line(s), 5 ENTRY, 1 FIXED, 0 UNPROVEN; 0 problem(s); held to the triage; held to the coverage"
    expect "[$B] the report says rotate" "$(grep -c '^ROTATE .*rotate the credential' "$L/report.md")" 1
    expect "[$B] the report says the suppression is held at ask" "$(grep -c '^SUPPRESS .*state: held at ask' "$L/report.md")" 1
    expect "[$B] the report makes no claim of security (grep, 0 hits)" "$(grep -c -i -E '\bsecure[ds]?\b|\bsafe(ly)?\b|vulnerability[- ]free|all clear' "$L/report.md")" 0
    expect "[$B] the report lists the uncovered OWASP categories" "$(grep -c '^COVERAGE | uncovered: A02:2025, A10:2025$' "$L/report.md")" 1
    expect "[$B] no secret value appears anywhere in the report" "$(grep -c 'AKIAEXAMPLE' "$L/report.md")" 0
    expect "[$B] exactly one file differs from the base: the fix file" "$(git -C "$GR" status --porcelain | sed 's/^ *//')" "M app/routes.py"
    (cd "$GR" && git checkout -q -- . && git clean -fdq -e .logos)
    rm -rf "$L"
done
expect "importing the sibling skill left no __pycache__ in the plugin" "$(find "$PLUGIN" -name __pycache__ | wc -l | tr -d ' ')" "0"
expect "the loop ran under every distinct bash ($NB)" "$NB" "${#BASHES[@]}"

# State for the layers that follow: the triage and the delta of the fixture.
python3 -I - "$FX/review.json" "$WORK/delta.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
json.dump({"base": "main", "merge_base": "abc123", "security": d["sections"]["security"]}, open(sys.argv[2], "w"))
PY
run -- "$B0" "$SCR/sec-triage.sh" --delta "$WORK/delta.json" --out "$WORK/triage.json"
printf 'def q(db, name):\n    return db.execute("select 1")\n' >"$GR/app/routes.py"
run -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage.json" --out "$WORK/rescan.json" --entry s2

# --- 5. the proof: every way a fix is not proven -------------------------------------------

echo "== 5. sec-proof.sh"
proof() { # findings-name [triage-entry] [rescan-file]
    run STUB_FINDINGS="$1" -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "${3:-$WORK/rescan.json}" --entry "${2:-s2}"
}
proof findings-after
expect "baseline: proven" "$RC" 0
proof findings-still
expect "the fingerprint is still listed: not proven, exit 1" "$RC" 1
contains "still listed: the reason" "$OUT" "NOT PROVEN s2 | app/routes.py:12 python.sqlalchemy.security.sql-injection [$(cut -c1-12 "$FX/fix-fp")] | the fingerprint is still listed after the re-scan"
proof findings-moved
expect "the offence moved to a new fingerprint at the same severity: not proven" "$RC" 1
contains "moved: the reason names the new finding" "$OUT" "a new finding at or above high appeared in app/routes.py"
proof findings-worse
expect "a new critical finding where a high one was: not proven" "$RC" 1
proof findings-lower
expect "near miss: a new finding below the original severity does not fail the proof" "$RC" 0
proof findings-known-fp
expect "near miss: a finding the delta already knew (suppressed or not) does not count as new" "$RC" 0
proof findings-after
contains "near miss: a critical finding in another file does not count (proven)" "$OUT" "PROVEN s2"
# A suppressed finding of the same scanner and file is known to the delta too: it is not new.
python3 -I - "$WORK/delta.json" "$WORK/delta-sup.json" "$FX/findings-after.json" "$FX/findings-sup.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
sup = d["security"]["suppressed"][0]
sup.update(path="app/routes.py", severity="critical", start_line=50)
json.dump(d, open(sys.argv[2], "w"))
a = json.load(open(sys.argv[3]))
a["findings"] = [dict(sup, status="pre-existing")]
json.dump(a, open(sys.argv[4], "w"))
PY
run STUB_FINDINGS=findings-sup -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta-sup.json" --rescan "$WORK/rescan.json" --entry s2
expect "near miss: a suppressed critical finding the delta knew does not count as new: proven" "$RC" 0
run STUB_FINDINGS=findings-sup -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
expect "the same critical finding, unknown to the delta (it sat in another file): not proven" "$RC" 1
proof findings-stale
expect "a stale finding of the file: not proven" "$RC" 1
proof findings-noreport
expect "no stored report of the scanner after the re-scan: not proven" "$RC" 1
contains "no stored report: the reason" "$OUT" "no stored report of this scanner after the re-scan"
run STUB_FINDINGS=missing-file -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
expect "an answer that is not JSON: not proven, exit 1" "$RC" 1
contains "not JSON: the reason" "$OUT" "did not answer"
python3 -I -c 'import os,sys; r=os.stat(sys.argv[1]).st_mtime; os.utime(sys.argv[2], (r+30, r+30))' "$WORK/rescan.json" "$GR/app/routes.py"
proof findings-after
expect "the file changed after the re-scan: not proven" "$RC" 1
contains "changed after: the reason" "$OUT" "app/routes.py changed after the re-scan; scan it again"
python3 -I -c 'import os,sys; r=os.stat(sys.argv[1]).st_mtime; os.utime(sys.argv[2], (r-30, r-30))' "$WORK/rescan.json" "$GR/app/routes.py"
proof findings-after
expect "near miss: the file edited before the re-scan: proven" "$RC" 0
proof findings-after s1
expect "a secret is never proven: not a fix entry" "$RC" 1
contains "secret: the reason" "$OUT" "not a fix entry"
python3 -I - "$WORK/rescan.json" "$WORK/rescan-none.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d["rescan"] = []
json.dump(d, open(sys.argv[2], "w"))
PY
proof findings-after s2 "$WORK/rescan-none.json"
expect "no re-scan recorded for the scanner: not proven" "$RC" 1
contains "no re-scan: the reason" "$OUT" "no targeted re-scan of this scanner is recorded"
run STUB_FINDINGS=findings-after -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s9
expect "an unknown entry id: exit 2" "$RC" 2
run -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json"
expect "proof without the other inputs: exit 2" "$RC" 2
proof findings-after
den "proof"

# --- 6. the re-scan: every way it did not happen ------------------------------------------

echo "== 6. sec-rescan.sh"
rescan() { # scan-name [entry]
    run STUB_SCAN="$1" -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage.json" --out "$WORK/rescan2.json" --entry "${2:-s2}"
}
rescan scan-failed
expect "the scanner failed: exit 1" "$RC" 1
contains "failed: NOT RESCANNED, with the scanner named" "$OUT" "NOT RESCANNED: Semgrep OSS: no \`ran\` outcome"
rescan scan-otherpath
expect "the re-scan did not cover the path asked: exit 1" "$RC" 1
contains "other path: the reason" "$OUT" "the re-scan did not cover app/routes.py"
rescan scan-unscoped
expect "a scanner that cannot be narrowed runs on the whole project: a superset, exit 0" "$RC" 0
contains "unscoped: the record says whole project" "$OUT" "(whole project)"
run STUB_SCAN_FAIL=1 -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage.json" --out "$WORK/rescan2.json" --entry s2
expect "the scan refuses (no consent): exit 1" "$RC" 1
contains "refusal: NOT RESCANNED, and no consent was recorded" "$OUT" "NOT RESCANNED: \`logos security scan --scanner Semgrep OSS --path app/routes.py\` exited 2"
den "re-scan on a refusal"
rescan scan s1
expect "a secret is never re-scanned: exit 2" "$RC" 2
contains "secret: says only a fix is re-scanned" "$ERR" "only a fix is re-scanned"
rescan scan s9
expect "an unknown entry: exit 2" "$RC" 2
run STUB_OLD_SCAN=1 -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage.json" --out "$WORK/rescan2.json" --entry s2
expect "a logos with no scoped scan: exit 3" "$RC" 3
contains "no scoped scan: says a fix cannot be proven" "$ERR" "a fix cannot be proven by a targeted re-scan"
run STUB_SYNC_RC=1 -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage.json" --out "$WORK/rescan2.json" --entry s2
expect "a sync that fails: exit 3, no scan started" "$RC" 3
run -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage.json" --out "$WORK/rescan2.json"
expect "no --entry: exit 2" "$RC" 2
# A repository file name holding a tab or a line break never becomes scan arguments
# of its own (`--consent`): the run stops before any sync or scan.
for sep in tab newline; do
    python3 -I - "$WORK/triage.json" "$WORK/triage-$sep.json" "$sep" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))
s = "\t" if sys.argv[3] == "tab" else "\n"
e = next(e for e in t["entries"] if e["id"] == "s2")
e["path"] = f"x{s}--consent{s}--scanner{s}bandit{s}--path{s}app/routes.py"
json.dump(t, open(sys.argv[2], "w"))
PY
    : >"$CALLS"
    run STUB_SCAN=scan -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage-$sep.json" --out "$WORK/rescan4.json" --entry s2
    expect "a $sep in the file name: exit 2" "$RC" 2
    contains "a $sep in the file name: escalated" "$ERR" "it is not re-scanned by this script: escalate it"
    expect "a $sep in the file name: no sync and no scan" "$(grep -c -e '^security scan --scanner' -e '^sync' "$CALLS")" 0
done
# Two fix entries of one scanner go in one scan, one --path each.
: >"$CALLS"
python3 -I - "$WORK/triage.json" "$WORK/triage2.json" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))
t["entries"][2]["proposed"] = "fix"  # the model confirmed the second open row too
json.dump(t, open(sys.argv[2], "w"))
PY
: >"$CALLS"
run STUB_SCAN=scan -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage2.json" --out "$WORK/rescan3.json" --entry s2 --entry s3
expect "two entries of one scanner: one scan with both paths" "$(grep -c '^security scan --scanner' "$CALLS")" 1
contains "two entries: both paths in that scan" "$(grep '^security scan --scanner' "$CALLS")" "--path app/routes.py --path app/util.py"
expect "two entries: the second path was not in the stub's scope, so exit 1" "$RC" 1

# --- 7. the report check -------------------------------------------------------------------

echo "== 7. sec-check-report.sh"
python3 -I - "$WORK/triage.json" "$WORK" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))
w = sys.argv[2]
sub = {e["rule_id"]: e for e in t["entries"]}
S = lambda r: sub[r]["subject"]
sec, fix, fp, unt, dep = (S("aws-access-key"), S("python.sqlalchemy.security.sql-injection"),
                          S("python.lang.security.audit.insecure-hash"), S("python.lang.security.audit.dynamic-urllib"),
                          S("GHSA-abcd-efgh-ijkl"))
fpfp = sub["python.lang.security.audit.insecure-hash"]["fingerprint"]
good = {
    "entry_sec": f"ENTRY | escalate-secret | {sec} | secret-class, redacted",
    "entry_fix": f"ENTRY | fix | {fix} | a trusted injection",
    "entry_fp": f"ENTRY | false-positive | {fp} | the digest is not used for a credential",
    "entry_unt": f"ENTRY | untrusted | {unt} | reachability confidence is unresolved",
    "entry_dep": f"ENTRY | escalate-dependency | {dep} | an advisory on a pinned package",
    "rotate": f"ROTATE | {sec} | rotate the credential; the value remains in git history",
    "fixed": f"FIXED | {fix} | rescan: Semgrep OSS 1.85.0 paths=app/routes.py fingerprint absent | tests: pytest -q | 12 passed, 0 failed (of 12)",
    "suppress": f"SUPPRESS | {fp} | fingerprint {fpfp} | state: held at ask | reason: the digest is not used for a credential",
    "dependency": f"DEPENDENCY | {dep} | proposal: urllib3 1.0 -> 1.26.19 | changelog risk: a minor line | blast radius: 3 callers",
    "scanned": "SCANNED | Semgrep OSS 1.85.0 | paths: app/routes.py",
    "coverage": "COVERAGE | uncovered: A02:2025, A10:2025",
}
json.dump(good, open(f"{w}/good.json", "w"))
PY
mk_report() { # name  [key=replacement-line | key= (drop) ...]
    local name="$1"
    shift
    python3 -I - "$WORK/good.json" "$WORK/r-$name.md" "$@" <<'PY'
import json, sys
good = json.load(open(sys.argv[1]))
for a in sys.argv[3:]:
    k, _, v = a.partition("=")
    if v == "":
        good.pop(k, None)
    else:
        good[k] = v
order = ["entry_sec", "entry_fix", "entry_fp", "entry_unt", "entry_dep", "rotate", "fixed", "suppress", "dependency", "scanned", "coverage"]
lines = [good[k] for k in order if k in good] + [good[k] for k in good if k not in order]
open(sys.argv[2], "w").write("\n".join(lines) + "\n")
PY
}
chk() { # name
    run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-$1.md" --triage "$WORK/triage.json" --coverage "$WORK/coverage-fx.json"
}
python3 -I - "$FX/findings.json" "$WORK/coverage-fx.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
json.dump({"edition": "2025", "scanned": [{"driver": s["driver"], "version": s["version"]} for s in d["scanned"]],
           "uncovered": [{"id": c["id"], "name": c["name"]} for c in d["categories"]
                         if (c["state"]["state"] if isinstance(c["state"], dict) else c["state"]) == "not_covered"]},
          open(sys.argv[2], "w"))
PY
mk_report ok
chk ok
expect "the valid report holds" "$RC" 0
den "check-report"
contains "the denominator says what it was held to" "$ERR" "held to the triage; held to the coverage"
no_cov() { run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-$1.md" --triage "$WORK/triage.json"; }
no_cov ok
contains "without --coverage the denominator says the comparison was not made" "$ERR" "no coverage given, so the scanners and categories are not compared"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-ok.md"
contains "without --triage the denominator says so" "$ERR" "no triage given, so the verdicts are not compared with the gate"
expect "without --triage the valid report still holds" "$RC" 0

bad_report() { # label name expected-problem-fragment  key=line ...
    local label="$1" name="$2" frag="$3"
    shift 3
    mk_report "$name" "$@"
    chk "$name"
    if [ "$RC" -eq 1 ] && case "$OUT" in *"$frag"*) true ;; *) false ;; esac; then ok "$label"; else bad "$label: rc=$RC, '$frag' not in '$OUT'"; fi
}
SECS="$(py 'import json,sys; print([e["subject"] for e in json.load(open(sys.argv[1]))["entries"] if e["rule_id"]=="aws-access-key"][0])' "$WORK/triage.json")"
FIXS="$(py 'import json,sys; print([e["subject"] for e in json.load(open(sys.argv[1]))["entries"] if e["class"]=="code" and e["proposed"]=="fix"][0])' "$WORK/triage.json")"
bad_report "a verdict outside the five" v1 "is not a verdict" "entry_unt=ENTRY | no-action-warranted | $(py 'import json,sys; print([e["subject"] for e in json.load(open(sys.argv[1]))["entries"] if e["id"]=="s4"][0])' "$WORK/triage.json") | x"
bad_report "an ENTRY without a reason" v2 "needs a verdict, a subject and a reason" "entry_unt=ENTRY | untrusted | x |"
bad_report "a fix with neither FIXED nor UNPROVEN" v3 "needs exactly one of FIXED or UNPROVEN" "fixed="
bad_report "a fix with both FIXED and UNPROVEN" v4 "needs exactly one of FIXED or UNPROVEN" "extra=UNPROVEN | $FIXS | the re-scan listed a new finding"
bad_report "a FIXED line that cites no re-scan" v5 "must cite the targeted re-scan" "fixed=FIXED | $FIXS | edited | tests: pytest -q | 12 passed, 0 failed (of 12)"
bad_report "a FIXED line whose re-scan does not say the fingerprint is absent" v6 "must cite the targeted re-scan" "fixed=FIXED | $FIXS | rescan: Semgrep OSS 1.85.0 paths=app/routes.py | tests: pytest -q | 12 passed, 0 failed (of 12)"
bad_report "a FIXED line whose re-scan misses the entry's file" v7 "does not list app/routes.py" "fixed=FIXED | $FIXS | rescan: Semgrep OSS 1.85.0 paths=app/routes2.py fingerprint absent | tests: pytest -q | 12 passed, 0 failed (of 12)"
bad_report "a FIXED line with a failing test" v8 "a fix is claimed only for a passing run" "fixed=FIXED | $FIXS | rescan: Semgrep OSS 1.85.0 paths=app/routes.py fingerprint absent | tests: pytest -q | 11 passed, 1 failed (of 12)"
bad_report "a FIXED line with no test run" v9 "a fix is claimed only for a passing run" "fixed=FIXED | $FIXS | rescan: Semgrep OSS 1.85.0 paths=app/routes.py fingerprint absent | tests: pytest -q | 0 passed, 0 failed (of 0)"
bad_report "a FIXED line with no count" v10 "needs the test count" "fixed=FIXED | $FIXS | rescan: Semgrep OSS 1.85.0 paths=app/routes.py fingerprint absent | tests: pytest -q | all passed"
bad_report "a FIXED line with no test command" v11 "must name the test command" "fixed=FIXED | $FIXS | rescan: Semgrep OSS 1.85.0 paths=app/routes.py fingerprint absent | tests: | 12 passed, 0 failed (of 12)"
bad_report "a secret with no ROTATE line" v12 "needs a ROTATE line" "rotate="
bad_report "a ROTATE line that does not say the value stays in history" v13 "needs a ROTATE line" "rotate=ROTATE | $SECS | rotate the credential"
bad_report "a ROTATE line that does not say rotate" v14 "needs a ROTATE line" "rotate=ROTATE | $SECS | remove the line; the value remains in git history"
bad_report "a secret reported FIXED" v15 "it is never edited" "extra=FIXED | $SECS | rescan: gitleaks 8.18.4 paths=config/loader.py fingerprint absent | tests: pytest -q | 1 passed, 0 failed (of 1)"
bad_report "a dependency finding with no proposal" v16 "needs DEPENDENCY" "dependency="
bad_report "a dependency finding with no blast radius" v17 "needs DEPENDENCY" "dependency=DEPENDENCY | $(py 'import json,sys; print([e["subject"] for e in json.load(open(sys.argv[1]))["entries"] if e["class"]=="dependency"][0])' "$WORK/triage.json") | proposal: urllib3 1.0 -> 1.26.19 | changelog risk: minor | blast radius:"
bad_report "a false positive with no SUPPRESS draft" v18 "needs a SUPPRESS line" "suppress="
bad_report "a SUPPRESS line whose state is not an ask outcome" v19 "SUPPRESS state must be one of" "suppress=SUPPRESS | $(py 'import json,sys; print([e["subject"] for e in json.load(open(sys.argv[1]))["entries"] if e["rule_id"].endswith("insecure-hash")][0])' "$WORK/triage.json") | fingerprint $(cat "$FX/fp-fp") | state: written | reason: x"
bad_report "a SUPPRESS line for another finding's fingerprint" v20 "names a fingerprint other than the finding's" "suppress=SUPPRESS | $(py 'import json,sys; print([e["subject"] for e in json.load(open(sys.argv[1]))["entries"] if e["rule_id"].endswith("insecure-hash")][0])' "$WORK/triage.json") | fingerprint $(cat "$FX/fix-fp") | state: held at ask | reason: x"
bad_report "a SUPPRESS line with a fingerprint that is not 64 hex" v21 "not a 64-character" "suppress=SUPPRESS | $(py 'import json,sys; print([e["subject"] for e in json.load(open(sys.argv[1]))["entries"] if e["rule_id"].endswith("insecure-hash")][0])' "$WORK/triage.json") | fingerprint abc123 | state: held at ask | reason: x"
bad_report "no COVERAGE line" v22 "needs exactly one COVERAGE line" "coverage="
bad_report "a COVERAGE line that omits an uncovered category" v23 "COVERAGE lists A02:2025; the stored coverage leaves A02:2025, A10:2025 uncovered" "coverage=COVERAGE | uncovered: A02:2025"
bad_report "a COVERAGE line that says none while categories are uncovered" v24 "COVERAGE lists none" "coverage=COVERAGE | uncovered: none"
bad_report "a COVERAGE line with an extra category" v25 "COVERAGE lists" "coverage=COVERAGE | uncovered: A02:2025, A05:2025, A10:2025"
bad_report "no SCANNED line" v26 "needs a SCANNED line" "scanned="
bad_report "a SCANNED line for a version no stored report carries" v27 "which no stored report carries" "scanned=SCANNED | Semgrep OSS 9.9.9 | paths: app/routes.py"
bad_report "a SCANNED line with no paths" v28 "SCANNED needs" "scanned=SCANNED | Semgrep OSS 1.85.0 | paths:"
bad_report "a row of the triage that is not judged" v29 "is a row of the triage and is not judged" "entry_unt="
bad_report "a verdict the gate fixed, changed by the model" v30 "the gate set untrusted and nothing may change it" "entry_unt=ENTRY | fix | $(py 'import json,sys; print([e["subject"] for e in json.load(open(sys.argv[1]))["entries"] if e["id"]=="s4"][0])' "$WORK/triage.json") | looks wrong"
bad_report "a proposed fix upgraded to a secret" v31 "a proposed fix may become false-positive and nothing else" "entry_fix=ENTRY | escalate-secret | $FIXS | x"
bad_report "an ENTRY for a finding the triage never had" v32 "is not a row of the triage" "extra=ENTRY | untrusted | nowhere.py:1 made.up [000000000000] | x"
bad_report "two ENTRY lines for one subject" v33 "two ENTRY lines name" "extra=ENTRY | untrusted | $FIXS | twice"
# The claim grep, with its near misses.
claim() { # label name line  expected(0|1)
    mk_report "$2" "extra=$3"
    chk "$2"
    if [ "$4" = 1 ]; then
        if [ "$RC" -eq 1 ] && case "$OUT" in *"a claim of safety or security"*) true ;; *) false ;; esac; then ok "$1"; else bad "$1: rc=$RC '$OUT'"; fi
    else
        expect "$1" "$RC" 0
    fi
}
claim "prose: 'the code is secure' is refused" c1 "The code is secure." 1
claim "prose: 'secured' is refused" c2 "The path is now secured." 1
claim "prose: 'safe' is refused" c3 "This change is safe to merge." 1
claim "prose: 'vulnerability-free' is refused" c4 "The branch is vulnerability-free." 1
claim "prose: 'no vulnerabilities' is refused" c5 "No vulnerabilities remain." 1
claim "prose: 'no findings' is refused" c6 "There are no findings." 1
claim "prose: 'all clear' is refused" c7 "All clear." 1
claim "a claim inside an evidence line (a reason) is refused" c8 "ENTRY | untrusted | extra.py:1 r [aaaaaaaaaaaa] | this file is safe" 1
claim "near miss: 'insecure' is not a claim" c9 "The query was insecure before the fix." 0
claim "near miss: 'security' is not a claim" c10 "Security findings were read from the stored reports." 0
claim "near miss: 'not exploitable' is not a claim" c11 "The pattern is not exploitable here: the input is a constant." 0
claim "near miss: 'unsafe' is not a claim" c12 "The call is unsafe." 0
mk_report c13 "extra=NO ENTRIES"
chk c13
expect "NO ENTRIES beside an ENTRY: refused" "$RC" 1
printf 'NOT MEASURED | no security report is stored\nCOVERAGE | uncovered: none\n' >"$WORK/r-nm.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-nm.md"
expect "NOT MEASURED alone, with its COVERAGE line, holds" "$RC" 0
printf 'NOT MEASURED | no security report is stored\nENTRY | untrusted | x.py:1 r [aaaaaaaaaaaa] | y\nCOVERAGE | uncovered: none\n' >"$WORK/r-nm2.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-nm2.md"
expect "NOT MEASURED beside a judgement: refused" "$RC" 1
printf 'NO ENTRIES\nSCANNED | Semgrep OSS 1.85.0 | paths: whole project\nCOVERAGE | uncovered: A02:2025\n' >"$WORK/r-no.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-no.md"
expect "a clean branch's report holds" "$RC" 0
printf '' >"$WORK/r-empty.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-empty.md"
expect "an empty report is refused" "$RC" 1
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/no-such.md"
expect "an unreadable report: exit 3" "$RC" 3
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-ok.md" --coverage "$WORK/coverage-fx.json"
expect "--coverage without --triage: exit 2" "$RC" 2
run -- "$B0" "$SCR/sec-check-report.sh"
expect "no --report: exit 2" "$RC" 2

# --- 8. the draft, and the guardrail that holds its write -----------------------------------

echo "== 8. suppress-draft.sh and the guardrail"
FPR="$(cat "$FX/fp-fp")"
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR" --reason "the digest is not used for a credential" --expires 2026-12-31
expect "a draft: exit 0" "$RC" 0
expect "the draft is one [[ignore]] entry in the ignore file's own shape" "$OUT" "[[ignore]]
fingerprint = \"$FPR\"
reason = \"the digest is not used for a credential\"
expires = 2026-12-31"
contains "the draft states its denominator and that it wrote nothing" "$ERR" "1 draft entry (printed, not written"
expect "the draft wrote no file" "$(ls "$GR/.logos")" ""
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR" --reason "no expiry"
expect "an entry without an expiry has none" "$(printf '%s\n' "$OUT" | grep -c '^expires')" 0
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "${FPR%?}" --reason x
expect "a 63-character fingerprint: exit 2" "$RC" 2
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$(printf '%s' "$FPR" | tr a-f A-F)" --reason x
expect "an upper-case fingerprint: exit 2" "$RC" 2
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR" --reason "   "
expect "a blank reason: exit 2" "$RC" 2
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR" --reason 'say "x"'
expect "a reason with a quote: exit 2" "$RC" 2
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR" --reason x --expires 2026-02-30
expect "a date that is not a calendar date: exit 2" "$RC" 2
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR"
expect "no --reason: exit 2" "$RC" 2

# The write of that draft is the guardrail's: the host asks the human, so the draft stops at `ask`.
GUARD="$PLUGIN/hooks/guard.sh"
PAYLOAD="$(py 'import json,sys; print(json.dumps({"session_id":"t","hook_event_name":"PreToolUse","tool_name":sys.argv[1],"tool_input":{"file_path":sys.argv[2],"content":sys.argv[3]}}))' Write "$GR/.logos/security-ignore.toml" "$OUT")"
for B in "${BASHES[@]}"; do
    RC=0
    printf '%s' "$PAYLOAD" | (cd "$GR" && env -i HOME="$WORK" PATH="/usr/bin:/bin" CLAUDE_PLUGIN_ROOT="$PLUGIN" CLAUDE_PROJECT_DIR="$GR" "$B" "$GUARD") >"$WORK/out" 2>"$WORK/err" || RC=$?
    expect "[$B] guard.sh on the draft's Write to .logos/security-ignore.toml: exit 0" "$RC" 0
    contains "[$B] the guardrail's decision is ask" "$(cat "$WORK/out")" '"permissionDecision":"ask"'
    contains "[$B] the guardrail names the file" "$(cat "$WORK/out")" "security-ignore.toml"
done
EDITP="$(py 'import json,sys; print(json.dumps({"session_id":"t","hook_event_name":"PreToolUse","tool_name":"Edit","tool_input":{"file_path":sys.argv[1],"old_string":"a","new_string":"b"}}))' "$GR/.logos/security-ignore.toml")"
RC=0
printf '%s' "$EDITP" | (cd "$GR" && env -i HOME="$WORK" PATH="/usr/bin:/bin" CLAUDE_PLUGIN_ROOT="$PLUGIN" CLAUDE_PROJECT_DIR="$GR" "$B0" "$GUARD") >"$WORK/out" 2>"$WORK/err" || RC=$?
contains "an Edit of the same file asks too" "$(cat "$WORK/out")" '"permissionDecision":"ask"'
OTHER="$(py 'import json,sys; print(json.dumps({"session_id":"t","hook_event_name":"PreToolUse","tool_name":"Write","tool_input":{"file_path":sys.argv[1],"content":"x"}}))' "$GR/app/routes.py")"
printf '%s' "$OTHER" | (cd "$GR" && env -i HOME="$WORK" PATH="/usr/bin:/bin" CLAUDE_PLUGIN_ROOT="$PLUGIN" CLAUDE_PROJECT_DIR="$GR" "$B0" "$GUARD") >"$WORK/out" 2>"$WORK/err" || true
expect "near miss: a Write of a fix file passes the guardrail untouched" "$(cat "$WORK/out")" ""

# --- 9. defects the review reproduced ----------------------------------------------------

echo "== 9. review findings"
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR"$'\n' --reason ok
expect "a fingerprint with a trailing newline would print invalid TOML: exit 2" "$RC" 2
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR" --reason ok --expires 20261231
expect "an expiry TOML would read as an integer: exit 2" "$RC" 2
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR" --reason ok --expires 2026-W52-1
expect "an ISO week date is not YYYY-MM-DD: exit 2" "$RC" 2
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR" --reason "a"$'\033'"b"
expect "a control character in the reason would reject the whole ignore file: exit 2" "$RC" 2
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR" --reason ok --expires 2026-12-31
expect "near miss: the plain date still passes" "$RC" 0

python3 -I - "$WORK/triage.json" "$WORK/triage-nosev.json" "$FX" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))
t["entries"][1]["severity"] = None  # the injection, with no severity recorded
json.dump(t, open(sys.argv[2], "w"))
fx = sys.argv[3]
base = json.load(open(f"{fx}/findings-after.json"))
fix = json.load(open(f"{fx}/findings-still.json"))["findings"][0]
base["findings"] = [dict(fix, path=None, report_path="./app/routes.py")]
json.dump(base, open(f"{fx}/findings-unbound.json", "w"))
nov = json.load(open(f"{fx}/findings.json"))
nov["scanned"] = [dict(nov["scanned"][1], version=None)]
json.dump(nov, open(f"{fx}/findings-nullver.json", "w"))
PY
run STUB_FINDINGS=findings-worse -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage-nosev.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
expect "an entry with no severity: a new critical finding still fails the proof" "$RC" 1
run STUB_FINDINGS=findings-after -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage-nosev.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
expect "near miss: an entry with no severity is proven when nothing new appears" "$RC" 0
run STUB_FINDINGS=findings-unbound -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
expect "the fingerprint listed under a path that did not bind: not proven" "$RC" 1
contains "unbound: the reason" "$OUT" "the fingerprint is still listed after the re-scan"
run STUB_FINDINGS=findings-nullver -- "$B0" "$SCR/sec-coverage.sh" --out "$WORK/cov-nullver.json"
contains "a stored report with no version reads 'unknown', as the proof prints it" "$OUT" "SCANNED | gitleaks unknown | paths: whole project"

claim "prose: 'zero findings' is refused" c20 "Zero findings remain on this branch." 1
claim "prose: 'no security problems' is refused" c21 "There are no security problems." 1
claim "prose: 'free of vulnerabilities' is refused" c22 "The change is free of vulnerabilities." 1
claim "near miss: 'findings were fixed' is not a claim" c23 "Two findings were fixed and proven." 0
claim "a scanner's own wording with 'Secure' is refused, and says why" c24 "ENTRY | untrusted | extra.py:1 r [aaaaaaaaaaaa] | the cookie is missing the Secure flag" 1
claim "near miss: the reworded scanner message passes the claim scan" c25 "The cookie lacks its transport-only flag." 0

cat >"$WORK/r-space.md" <<'RSPACE'
ENTRY | fix | my dir/a.py:3 r [aaaaaaaaaaaa] | a trusted finding
FIXED | my dir/a.py:3 r [aaaaaaaaaaaa] | rescan: Semgrep OSS 1.85.0 paths=my dir/a.py,app/b.py fingerprint absent | tests: pytest -q | 3 passed, 0 failed (of 3)
SCANNED | Semgrep OSS 1.85.0 | paths: my dir/a.py
COVERAGE | uncovered: none
RSPACE
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-space.md"
expect "a FIXED line for a path with a space can be written and holds" "$RC" 0

# The sibling skill is a contract: a helper it no longer offers is a legible exit 3, not a traceback.
SIB="$WORK/sib"
mkdir -p "$SIB/skills/logos-review/scripts/lib" "$SIB/skills/logos-sec-review"
cp -R "$SKILL/scripts" "$SIB/skills/logos-sec-review/"
printf 'def eprint(*a):\n    pass\n' >"$SIB/skills/logos-review/scripts/lib/delta.py"
RC=0
python3 -I "$SIB/skills/logos-sec-review/scripts/lib/secrev.py" draft --fingerprint "$FPR" --reason ok >"$WORK/out" 2>"$WORK/err" || RC=$?
expect "a sibling helper that lost a name: exit 3" "$RC" 3
contains "that exit names what went missing" "$(cat "$WORK/err")" "no longer offers what this skill reads"
rm -rf "$SIB/skills/logos-review"
RC=0
python3 -I "$SIB/skills/logos-sec-review/scripts/lib/secrev.py" draft --fingerprint "$FPR" --reason ok >"$WORK/out" 2>"$WORK/err" || RC=$?
expect "no sibling skill at all: exit 3" "$RC" 3
contains "that exit names the missing helper" "$(cat "$WORK/err")" "helper is missing"

# --- 10. what a mutation sweep of the suite found unasserted ------------------------------------

echo "== 10. argv boundaries, --project, several scanners, report rules, figures"
ARGV="$CALLS.argv"
PWDLOG="$CALLS.pwd"
mkdir -p "$WORK/elsewhere"
# runat DIR [VAR=value ...] -- BASH SCRIPT ARGS...: like run, from another directory.
runat() {
    local dir="$1" envs=()
    shift
    while [ "$1" != "--" ]; do envs+=("$1"); shift; done
    shift
    local b="$1"
    shift
    RC=0
    (cd "$dir" && env PATH="$BIN:$PATH" "${STUB_ENV[@]}" "${envs[@]+"${envs[@]}"}" "$b" "$@") >"$WORK/out" 2>"$WORK/err" || RC=$?
    OUT="$(cat "$WORK/out")" ERR="$(cat "$WORK/err")"
}

# The argv reaches logos intact: "Semgrep OSS" is one argument.
: >"$ARGV"
run -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage.json" --out "$WORK/rescan-argv.json" --entry s2
expect "the scan's argv, one argument each: the scanner name keeps its space" "$(grep '^<security><scan><--scanner>' "$ARGV")" "<security><scan><--scanner><Semgrep OSS><--path><app/routes.py><--json>"
expect "sync's argv, one argument each" "$(grep '^<sync>' "$ARGV")" "<sync><--><app/routes.py>"
run STUB_FINDINGS=findings-after -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
expect "the proof asks for this scanner's findings of this file, each argument intact" "$(grep '^<security><findings>' "$ARGV" | tail -1)" "<security><findings><--json><--scanner><Semgrep OSS><--path><app/routes.py>"

# Several scanners: one scan each, every answer recorded, one failure fails the run.
python3 -I - "$WORK/triage.json" "$WORK/triage-two.json" "$FX" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))
fx = sys.argv[3]
b = dict(t["entries"][1], id="s6", scanner="Bandit", path="app/b.py", fingerprint="b" * 64, rule_id="B608")
b["subject"] = "app/b.py:3 B608 [bbbbbbbbbbbb]"
d = dict(t["entries"][1], id="s7", path="app/routes.py", fingerprint="c" * 64, rule_id="other", subject="app/routes.py:30 other [cccccccccccc]")
t["entries"] += [b, d]
json.dump(t, open(sys.argv[2], "w"))
run = lambda scanner, driver, outcome, paths, version="1.7.10": {
    "consent_recorded": [], "findings": 0,
    "scanners": [{"scanner": scanner, "driver": driver, "version": version, "outcome": outcome,
                  "scope": {"kind": "paths", "paths": paths}}]}
json.dump(run("semgrep", "Semgrep OSS", "ran", ["app/routes.py"], "1.85.0"), open(f"{fx}/scanp.SemgrepOSS.json", "w"))
json.dump(run("bandit", "Bandit", "ran", ["app/b.py"]), open(f"{fx}/scanp.Bandit.json", "w"))
json.dump(run("semgrep", "Semgrep OSS", "ran", ["app/routes.py"], "1.85.0"), open(f"{fx}/scanq.SemgrepOSS.json", "w"))
json.dump(run("bandit", "Bandit", "failed", ["app/b.py"]), open(f"{fx}/scanq.Bandit.json", "w"))
json.dump(run("semgrep", "Semgrep OSS", "ran", ["app/routes.py"], None), open(f"{fx}/scanr.SemgrepOSS.json", "w"))
PY
: >"$ARGV"
run STUB_SCAN=scanp STUB_SCAN_PER=1 -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage-two.json" --out "$WORK/rescan-two.json" --entry s2 --entry s6
expect "two scanners, both ran: exit 0" "$RC" 0
expect "two scanners: one scan each" "$(grep -c '^<security><scan><--scanner>' "$ARGV")" 2
expect "two scanners: every answer is in the record" "$(py 'import json,sys; print(sorted(r["driver"] for r in json.load(open(sys.argv[1]))["rescan"]))' "$WORK/rescan-two.json")" "['Bandit', 'Semgrep OSS']"
contains "two scanners: the denominator counts both" "$ERR" "2 scanner run(s) held to 2 asked, 0 problem(s)"
run STUB_SCAN=scanq STUB_SCAN_PER=1 -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage-two.json" --out "$WORK/rescan-two.json" --entry s2 --entry s6
expect "one of two scanners failed: exit 1" "$RC" 1
contains "one failed: it is named, the other is recorded" "$OUT" "NOT RESCANNED: Bandit"
contains "one failed: the other still re-scanned" "$OUT" "rescanned: Semgrep OSS 1.85.0 on app/routes.py"
run STUB_SCAN=scanp STUB_SCAN_PER=1 STUB_SCAN_FAIL_FOR=Bandit -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage-two.json" --out "$WORK/rescan-two.json" --entry s2 --entry s6
expect "one scan exits non-zero while the other answers: exit 1, never 0" "$RC" 1
contains "that failure is reported and the other is recorded" "$OUT" "NOT RESCANNED: \`logos security scan --scanner Bandit"
contains "that run still records the scanner that answered" "$OUT" "rescanned: Semgrep OSS"
run STUB_SCAN=scanr STUB_SCAN_PER=1 -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage.json" --out "$WORK/rescan-nover.json" --entry s2
contains "a scanner that reports no version reads unknown" "$OUT" "rescanned: Semgrep OSS unknown on app/routes.py"
run STUB_FINDINGS=findings-after -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan-nover.json" --entry s2
contains "the proof prints that unknown version too" "$OUT" "rescan: Semgrep OSS unknown paths=app/routes.py fingerprint absent"
python3 -I "$SCR/lib/secrev.py" files --triage "$WORK/triage-two.json" --entry s2 --entry s7 >"$WORK/out" 2>"$WORK/err"
expect "two entries of one file list it once" "$(cat "$WORK/out")" "app/routes.py"
contains "the files denominator counts entries and files separately" "$(cat "$WORK/err")" "2 entries, 1 file"
python3 -I "$SCR/lib/secrev.py" plan --triage "$WORK/triage-two.json" --entry s2 --entry s7 >"$WORK/out" 2>"$WORK/err"
expect "two entries of one scanner and one file: one path in the plan" "$(cat "$WORK/out")" "--scanner	Semgrep OSS	--path	app/routes.py"
contains "the plan denominator counts entries and scans separately" "$(cat "$WORK/err")" "2 entries, 1 scan"
python3 -I "$SCR/lib/secrev.py" plan --triage "$WORK/triage-two.json" --entry s2 --entry s6 >"$WORK/out" 2>"$WORK/err"
contains "two scanners: two scans in the plan" "$(cat "$WORK/err")" "2 entries, 2 scans"
RC=0
python3 -I "$SCR/lib/secrev.py" files --triage "$WORK/triage.json" --entry s9 >"$WORK/out" 2>"$WORK/err" || RC=$?
expect "files with an unknown entry: exit 2" "$RC" 2
RC=0
python3 -I "$SCR/lib/secrev.py" plan --triage "$WORK/triage.json" --entry s9 >"$WORK/out" 2>"$WORK/err" || RC=$?
expect "plan with an unknown entry: exit 2" "$RC" 2
run STUB_FINDINGS=findings-still -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
contains "a not-proven run states its denominator" "$ERR" "1 fix entry checked, 0 proven, 1 not proven"

# --project: every logos call runs in the project, whatever the working directory.
: >"$PWDLOG"
runat "$WORK/elsewhere" -- "$B0" "$SCR/sec-rescan.sh" --project "$GR" --triage "$WORK/triage.json" --out "$WORK/rescan-p.json" --entry s2
expect "--project: re-scan from another directory: exit 0" "$RC" 0
expect "--project: sync ran in the project" "$(grep -c "^$GR|sync " "$PWDLOG")" 1
expect "--project: the scan ran in the project" "$(grep -c "^$GR|security scan --scanner" "$PWDLOG")" 1
: >"$PWDLOG"
runat "$WORK/elsewhere" STUB_FINDINGS=findings-after -- "$B0" "$SCR/sec-proof.sh" --project "$GR" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
expect "--project: proof from another directory: exit 0 (the file is read in the project)" "$RC" 0
expect "--project: the findings call ran in the project" "$(grep -c "^$GR|security findings" "$PWDLOG")" 1
: >"$PWDLOG"
runat "$WORK/elsewhere" -- "$B0" "$SCR/sec-coverage.sh" --project "$GR" --out "$WORK/cov-p.json"
expect "--project: coverage from another directory: exit 0" "$RC" 0
expect "--project: the coverage read ran in the project" "$(grep -c "^$GR|security findings" "$PWDLOG")" 1
: >"$PWDLOG"
: >"$ARGV"
runat "$WORK/elsewhere" -- "$B0" "$SCR/sec-delta.sh" --project "$GR" --base release --out "$WORK/delta-p.json"
expect "--project: delta from another directory: exit 0" "$RC" 0
expect "--project and --base reach the review: in the project, with the base" "$(grep '^<review>' "$ARGV")" "<review><--json><--base><release>"
expect "--project: the review ran in the project" "$(grep -c "^$GR|review " "$PWDLOG")" 1
# A relative --out is made absolute before the review runs in the project.
run -- "$B0" "$SCR/sec-delta.sh" --out rel-delta.json
expect "a relative --out: exit 0" "$RC" 0
expect "a relative --out lands in the working directory, not the scratch" "$([ -f "$GR/rel-delta.json" ] && echo yes)" "yes"
rm -f "$GR/rel-delta.json"
run -- "$B0" "$SCR/sec-coverage.sh" --out "$WORK/no/such/dir/c.json"
expect "coverage into a missing directory: exit 3" "$RC" 3
run -- "$B0" "$SCR/sec-rescan.sh" --triage "$WORK/triage.json" --out "$WORK/no/such/dir/r.json" --entry s2
expect "re-scan into a missing directory: exit 3" "$RC" 3
run STUB_FINDINGS_RC=1 STUB_FINDINGS=missing-file -- "$B0" "$SCR/sec-coverage.sh" --out "$WORK/cov-bad.json"
expect "a findings read that fails with no answer: exit 3" "$RC" 3
run STUB_REVIEW_RC=2 -- "$B0" "$SCR/sec-delta.sh" --out "$WORK/delta-rc2.json"
expect "a review that exits 2 with a document: exit 3, not read as an answer" "$RC" 3
expect "no helper leaves a file beside its target" "$(ls "$WORK" | grep -c -E '\.(findings|scan|review|err)\.[0-9.]*$|\.tmp\.')" 0
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-ok.md" --bogus
expect "check-report with an unknown argument: exit 2" "$RC" 2
run -- "$B0" "$SCR/suppress-draft.sh" --fingerprint "$FPR" --reason 'a\b'
expect "a backslash in the reason: exit 2" "$RC" 2

# Python missing: the scripts that need it say so (exit 3); nothing else is on this PATH but what they use.
NOPY="$WORK/nopy"
mkdir -p "$NOPY"
for t in dirname cat sed head tr grep wc rm mv env git basename ls date; do
    ln -sf "$(command -v "$t")" "$NOPY/$t" 2>/dev/null
done
for sc in "sec-check-report.sh --report x" "sec-triage.sh --delta x --out x" "suppress-draft.sh --fingerprint $FPR --reason r"; do
    RC=0
    # shellcheck disable=SC2086
    (cd "$GR" && env PATH="$NOPY" "$B0" "$SCR/${sc%% *}" ${sc#* }) >"$WORK/out" 2>"$WORK/err" || RC=$?
    expect "no python3: ${sc%% *} exits 3" "$RC" 3
    contains "no python3: ${sc%% *} says python3" "$(cat "$WORK/err")" "python3"
done

# --- the report check, rule by rule ---------------------------------------------------------

OKR="$WORK/r-ok.md"
twice() { # label name sed-expression-over-r-ok [fragment the refusal must carry]
    sed -e "$3" "$OKR" >"$WORK/r-$2.md"
    chk "$2"
    if [ "$RC" -eq 1 ] && [ -n "$OUT" ] && case "$ERR" in *Traceback*) false ;; *) true ;; esac &&
        case "$OUT" in *"${4:-line }"*) true ;; *) false ;; esac; then ok "$1"; else bad "$1: rc=$RC '$OUT' '$ERR'"; fi
    if [ -n "${4:-}" ]; then
        run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-$2.md"
        if [ "$RC" -eq 1 ]; then ok "$1 (without the triage)"; else bad "$1 (without the triage): rc=$RC '$OUT'"; fi
    fi
}
twice "two COVERAGE lines" t1 '/^COVERAGE/p'
twice "COVERAGE without 'uncovered:'" t2 's/^COVERAGE | uncovered: /COVERAGE | missing: /' "COVERAGE needs"
twice "a SCANNED line with an extra field" t3 's/^\(SCANNED | Semgrep OSS 1.85.0 | paths: app\/routes.py\)$/\1 | extra/'
twice "a SCANNED line without the 'paths: ' prefix" t4 's/| paths: app/| in app/'
twice "a SUPPRESS line with a field missing" t5 's/ | reason: the digest is not used for a credential//' "SUPPRESS needs"
twice "a SUPPRESS line without 'state: '" t6 's/| state: held at ask/| held at ask/'
twice "a SUPPRESS line without 'reason: '" t7 's/| reason: the/| the/'
twice "a FIXED line whose counts do not add up" t8 's/12 passed, 0 failed (of 12)/3 passed, 0 failed (of 12)/'
twice "a FIXED line with a sixth field" t9 's/(of 12)$/(of 12) | extra/'
twice "a re-scan citation that does not start the field" t10 's/| rescan: Semgrep/| later rescan: Semgrep/'
twice "two FIXED lines naming one subject" t11 '/^FIXED/p'
twice "a verdict line with an empty field" t12 's/| tests: pytest -q |/| tests: pytest -q | |/'
for state in "approved at ask" "declined at ask"; do
    sed -e "s/state: held at ask/state: $state/" "$OKR" >"$WORK/r-state.md"
    chk state
    expect "a SUPPRESS state of '$state' holds" "$RC" 0
done
# An outcome line on a subject judged differently, or never judged.
SUBJ_FIX="$(grep '^FIXED' "$OKR" | cut -d'|' -f2 | sed 's/^ //; s/ $//')"
SUBJ_SEC="$(grep '^ROTATE' "$OKR" | cut -d'|' -f2 | sed 's/^ //; s/ $//')"
SUBJ_FP="$(grep '^SUPPRESS' "$OKR" | cut -d'|' -f2 | sed 's/^ //; s/ $//')"
SUBJ_DEP="$(grep '^DEPENDENCY' "$OKR" | cut -d'|' -f2 | sed 's/^ //; s/ $//')"
{ cat "$OKR"; echo "ROTATE | $SUBJ_FIX | rotate the credential; the value remains in git history"; } >"$WORK/r-p1.md"
chk p1
expect "a ROTATE line on a subject judged fix: refused" "$RC" 1
contains "that refusal says which verdict it is" "$OUT" "ROTATE names $SUBJ_FIX, which is fix, not escalate-secret"
{ cat "$OKR"; echo "DEPENDENCY | $SUBJ_SEC | proposal: a | changelog risk: b | blast radius: c"; } >"$WORK/r-p2.md"
chk p2
expect "a DEPENDENCY line on a secret: refused" "$RC" 1
{ cat "$OKR"; echo "SUPPRESS | $SUBJ_SEC | fingerprint $(cat "$FX/secret-fp") | state: held at ask | reason: x"; } >"$WORK/r-p3.md"
chk p3
expect "a SUPPRESS line on a secret: refused" "$RC" 1
{ cat "$OKR"; echo "UNPROVEN | $SUBJ_FP | x"; } >"$WORK/r-p4.md"
chk p4
expect "an UNPROVEN line on a false positive: refused" "$RC" 1
{ cat "$OKR"; echo "ROTATE | nowhere.py:1 made.up [000000000000] | rotate it; git history keeps it"; } >"$WORK/r-p5.md"
chk p5
expect "a ROTATE line on a subject never judged: refused" "$RC" 1
contains "that refusal says it was not judged" "$OUT" "which is not judged"
sed -e 's/^ENTRY | escalate-secret | \(.*\)$/ENTRY | escalate-dependency | \1/' "$OKR" >"$WORK/r-p6.md"
chk p6
expect "a secret judged escalate-dependency: refused (the gate set it)" "$RC" 1
{ cat "$OKR"; echo "ENTRY | untrusted | extra.py:1 r [aaaaaaaaaaaa] | y"; echo "NO ENTRIES"; } >"$WORK/r-p7.md"
chk p7
expect "NO ENTRIES beside ENTRY lines: refused" "$RC" 1
printf 'SCANNED | Semgrep OSS 1.85.0 | paths: x\nCOVERAGE | uncovered: none\n' >"$WORK/r-p8.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-p8.md"
expect "neither NO ENTRIES nor an ENTRY nor NOT MEASURED: refused" "$RC" 1
printf 'NOT MEASURED | x\nNO ENTRIES\nCOVERAGE | uncovered: none\n' >"$WORK/r-p9.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-p9.md"
expect "NOT MEASURED beside NO ENTRIES: refused" "$RC" 1
{ cat "$OKR"; echo "ENTRY points were reviewed first."; echo "FIXED is not a word we use here."; } >"$WORK/r-p10.md"
chk p10
expect "near miss: prose that starts with a keyword but not 'KEYWORD |' is prose" "$RC" 0
sed -e 's/^COVERAGE | uncovered: A02:2025, A10:2025$/COVERAGE | uncovered: A10:2025, A02:2025/' "$OKR" >"$WORK/r-p11.md"
chk p11
expect "near miss: the uncovered categories in another order hold" "$RC" 0
sed -e 's/^COVERAGE | uncovered: A02:2025, A10:2025$/  COVERAGE | uncovered: A02:2025, A10:2025  /' "$OKR" >"$WORK/r-p12.md"
chk p12
expect "near miss: a line padded with spaces is read as the line it is" "$RC" 0
for phrase in "The result is safely contained." "The branch is vulnerability-free." "There are no remaining vulnerabilities." \
    "There are no further findings." "No known vulnerability was found." "There is no security issue." "There is no finding."; do
    claim "prose: '$phrase' is refused" "cw-$(printf '%s' "$phrase" | cksum | cut -d' ' -f1)" "$phrase" 1
done
# A subject may hold the words; any other field may not.
sed -e 's#app/routes.py:12#app/safe/secure.py:12#g' "$OKR" >"$WORK/r-s1.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-s1.md"
expect "near miss: a subject path holding 'safe' and 'secure' passes the claim scan (no triage)" "$RC" 0
sed -e "s#config/loader.py:4#config/secure.py:4#g; s#app/util.py:7#app/safe.py:7#g" "$OKR" >"$WORK/r-s2.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-s2.md"
expect "near miss: ROTATE, SUPPRESS and ENTRY subjects holding the words pass (no triage)" "$RC" 0
sed -e 's#^ROTATE \(.*\)| rotate the credential;#ROTATE \1| rotate the credential, it is safe once rotated;#' "$OKR" >"$WORK/r-s3.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-s3.md"
expect "a claim in a ROTATE evidence field: refused" "$RC" 1
sed -e 's#^DEPENDENCY \(.*\)| blast radius: 3 callers#DEPENDENCY \1| blast radius: safe#' "$OKR" >"$WORK/r-s4.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-s4.md"
expect "a claim in a DEPENDENCY evidence field: refused" "$RC" 1
sed -e 's#reason: the digest is not used for a credential#reason: the digest is safe#' "$OKR" >"$WORK/r-s5.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-s5.md"
expect "a claim in a SUPPRESS evidence field: refused" "$RC" 1
sed -e 's#| tests: pytest -q |#| tests: pytest is secure |#' "$OKR" >"$WORK/r-s6.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-s6.md"
expect "a claim in a FIXED evidence field: refused" "$RC" 1
{ cat "$OKR"; echo "UNPROVEN | $SUBJ_DEP | the code is safe"; } >"$WORK/r-s7.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-s7.md"
expect "a claim in an UNPROVEN evidence field: refused" "$RC" 1
sed -e 's#^SCANNED | Semgrep OSS 1.85.0 | paths: app/routes.py#SCANNED | Semgrep OSS 1.85.0 | paths: the secure ones#' "$OKR" >"$WORK/r-s8.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-s8.md"
expect "a claim in a SCANNED field: refused" "$RC" 1

printf 'SCANNED | the safe scanner 1.0 | paths: x\nCOVERAGE | uncovered: none\nNO ENTRIES\n' >"$WORK/r-q1.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-q1.md"
expect "a claim in the scanner field of a SCANNED line: refused" "$RC" 1
contains "that refusal is the claim scan" "$OUT" "a claim of safety or security: 'safe'"
printf 'SCANNED | Semgrep OSS 1.85.0 | paths: x\nCOVERAGE | uncovered: none, all clear\nNO ENTRIES\n' >"$WORK/r-q2.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-q2.md"
expect "a claim in the first field of a COVERAGE line: refused" "$RC" 1
contains "that refusal names the claim" "$OUT" "'all clear'"
printf 'NOT MEASURED | the branch is secure\nCOVERAGE | uncovered: none\n' >"$WORK/r-q3.md"
run -- "$B0" "$SCR/sec-check-report.sh" --report "$WORK/r-q3.md"
expect "a claim in a NOT MEASURED reason: refused" "$RC" 1

# --- the triage's classes and figures ---------------------------------------------------------

for id in GHSA-abcd-efgh-ijkl GO-2024-1234 PYSEC-2024-12 RUSTSEC-2024-0001 OSV-2024-1 cve-2024-12345; do
    variant "adv-$id" "f['scanner']='Trivy'; f['rule_id']='$id'"
    contains "Trivy with $id is a dependency finding" "$(proposed_of "adv-$id")" "escalate-dependency"
done
variant advprefix "f['scanner']='Trivy'; f['rule_id']='xCVE-2024-12345'"
contains "near miss: an advisory id with a prefix is code" "$(proposed_of advprefix)" "fix |"
variant secretdep "f['redacted']=True; f['scanner']='osv-scanner'"
contains "a redacted finding from a dependency scanner is still a secret" "$(proposed_of secretdep)" "escalate-secret"
variant nopath2 "f['path']=None"
proposed_of nopath2 >/dev/null
expect "a path-less subject starts with '-', not an empty string" "$(py 'import json,sys; print(json.load(open(sys.argv[1]))["entries"][0]["subject"][:2])' "$WORK/vt-nopath2.json")" "- "
expect "the triage keeps each field the model reads" "$(py '
import json, sys
e = json.load(open(sys.argv[1]))["entries"][1]
print(e["status"], e["line"], e["confidence"], e["freshness"], e["owasp"], e["severity"], e["message"] != "")' "$WORK/triage.json")" "introduced 12 resolved fresh ['A05:2025'] high True"
expect "the triage keeps the base" "$(py 'import json,sys; d=json.load(open(sys.argv[1])); print(d["base"], d["merge_base"])' "$WORK/triage.json")" "main abc123"
expect "the delta file keeps the base" "$(py 'import json,sys; d=json.load(open(sys.argv[1])); print(d["base"], d["merge_base"])' "$WORK/delta.json")" "main abc123"
run -- "$B0" "$SCR/sec-delta.sh" --out "$WORK/delta-fig.json"
contains "the delta summary states each count" "$OUT" "resolved 1, stale 0, pre-existing 2, suppressed 1"
contains "the delta summary states how each scanner was compared" "$OUT" "compared by Semgrep OSS=fingerprint, gitleaks=hunk"
contains "the delta denominator counts the scanners compared" "$ERR" "2 scanner(s) compared"

# --- the coverage read -------------------------------------------------------------------------

python3 -I - "$FX" <<'PY'
import json, sys
fx = sys.argv[1]
d = json.load(open(f"{fx}/findings.json"))
all_covered = dict(d, categories=[dict(c, state={"state": "covered", "findings": 0}) for c in d["categories"]])
json.dump(all_covered, open(f"{fx}/findings-allcovered.json", "w"))
json.dump(dict(d, scanned=[]), open(f"{fx}/findings-nonescanned.json", "w"))
PY
run STUB_FINDINGS=findings-allcovered -- "$B0" "$SCR/sec-coverage.sh" --out "$WORK/cov-all.json"
contains "no category uncovered: the line says none" "$OUT" "COVERAGE | uncovered: none"
contains "no category uncovered: 0 in the denominator" "$ERR" "0 uncovered"
run STUB_FINDINGS=findings-nonescanned -- "$B0" "$SCR/sec-coverage.sh" --out "$WORK/cov-none.json"
contains "no stored report: the SCANNED line says so" "$OUT" "SCANNED | no scanner has a stored report | paths: none"
run -- "$B0" "$SCR/sec-coverage.sh" --out "$WORK/cov-det.json"
contains "each uncovered category is listed with its name" "$OUT" "  A02:2025 Security Misconfiguration"
expect "the coverage file keeps the edition and the counts" "$(py 'import json,sys; d=json.load(open(sys.argv[1])); print(d["edition"], d["categories"], len(d["scanned"]), len(d["uncovered"]))' "$WORK/cov-det.json")" "2025 4 2 2"
run STUB_FINDINGS=findings-nocats -- "$B0" "$SCR/sec-coverage.sh" --out "$WORK/cov-nocat.json"
expect "an answer with no categories: exit 1, not an empty 'none'" "$RC" 1

# --- the proof, branch by branch -----------------------------------------------------------------

python3 -I - "$WORK/triage.json" "$WORK/triage-upper.json" "$FX" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))
t["entries"][1]["severity"] = "HIGH"
json.dump(t, open(sys.argv[2], "w"))
fx = sys.argv[3]
d = json.load(open(f"{fx}/findings-moved.json"))
d["findings"][0] = dict(d["findings"][0], path=None, report_path="app/routes.py")
json.dump(d, open(f"{fx}/findings-reportpath.json", "w"))
PY
run STUB_FINDINGS=findings-moved -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage-upper.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
expect "a severity written in capitals is read as high: a new high finding fails the proof" "$RC" 1
run STUB_FINDINGS=findings-reportpath -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
expect "a new finding listed only by its report path still counts against the file" "$RC" 1
python3 -I - "$WORK/triage.json" "$WORK/triage-gone.json" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))
t["entries"][1]["path"] = "app/no-such-file.py"
json.dump(t, open(sys.argv[2], "w"))
PY
python3 -I - "$WORK/rescan.json" "$WORK/rescan-gone.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d["rescan"][0]["paths"] = ["app/no-such-file.py"]
json.dump(d, open(sys.argv[2], "w"))
PY
run STUB_FINDINGS=findings-after -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage-gone.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan-gone.json" --entry s2
expect "a file that cannot be read cannot be shown scanned as it stands: not proven" "$RC" 1
contains "that reason is the change after the re-scan" "$OUT" "changed after the re-scan"
python3 -I - "$WORK/rescan.json" "$WORK/rescan-other.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d["rescan"][0]["paths"] = ["app/other.py"]
json.dump(d, open(sys.argv[2], "w"))
PY
run STUB_FINDINGS=findings-after -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan-other.json" --entry s2
expect "a re-scan record that does not list the file: not proven" "$RC" 1
contains "that reason names the file" "$OUT" "the re-scan did not cover app/routes.py"
python3 -I - "$FX" <<'PY'
import json, sys
fx = sys.argv[1]
d = json.load(open(f"{fx}/findings-lower.json"))
d["findings"][0]["severity"] = "MEDIUM"
json.dump(d, open(f"{fx}/findings-lowcaps.json", "w"))
PY
run STUB_FINDINGS=findings-lowcaps -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
expect "near miss: a new finding whose severity is written in capitals and sits below the entry's does not fail the proof" "$RC" 0
run STUB_FINDINGS=findings-lower -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage-upper.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan.json" --entry s2
expect "near miss: an entry whose severity is written in capitals is read as high, so a new medium one is below it" "$RC" 0
# Two entries, same scanner, two files: each file's findings are read for itself.
python3 -I - "$WORK/triage.json" "$WORK/triage-twofile.json" "$WORK/rescan.json" "$WORK/rescan-twofile.json" <<'PY'
import json, sys
t = json.load(open(sys.argv[1]))
e = dict(t["entries"][1], id="s8", path="app/util.py", fingerprint="d" * 64, rule_id="x", subject="app/util.py:1 x [dddddddddddd]")
t["entries"].append(e)
json.dump(t, open(sys.argv[2], "w"))
r = json.load(open(sys.argv[3]))
r["rescan"][0]["paths"] = ["app/routes.py", "app/util.py"]
json.dump(r, open(sys.argv[4], "w"))
PY
: >"$ARGV"
run STUB_FINDINGS=findings-after -- "$B0" "$SCR/sec-proof.sh" --triage "$WORK/triage-twofile.json" --delta "$WORK/delta.json" --rescan "$WORK/rescan-twofile.json" --entry s2 --entry s8
expect "two files of one scanner: both proven" "$RC" 0
expect "two files of one scanner: one findings read per file" "$(grep -c '^<security><findings>' "$ARGV")" 2
contains "two files: the denominator counts both" "$ERR" "2 fix entries checked, 2 proven, 0 not proven"

T=$((PASS + FAIL))
echo
echo "$PASS passed, $FAIL failed (of $T)"
[ "$FAIL" -eq 0 ]
