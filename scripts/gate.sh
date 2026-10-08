#!/usr/bin/env bash
#
# gate.sh — run this project's verification gates and write one machine-readable
# evidence file per gate.
#
# WHY THIS EXISTS
#   Three observed ways a gate reported success while actually failing:
#
#     1. `cargo clippy … | tail -5` — the pipe's exit status replaces clippy's
#        101, so the caller reads 0 and reports "clean".
#     2. A test run killed mid-flight (memory pressure) or truncated by cargo's
#        target-level fail-fast still summarises as "0 failures", because the
#        DENOMINATOR is never reported. "0 failed of 0 binaries" reads exactly
#        like "0 failed of 2693". Observed 2026-09-06: logos-core reported 52
#        binaries and 2579 passing tests against the ~76 that existed then — a
#        third of the suite never ran, and it read as green.
#     3. `npm run lint` — web/ui has no `lint` script. The missing-script error
#        reads like a linter ran, and through a pipe it exits 0.
#
#   So, three rules, and every line below serves one of them:
#     - no pipe on any measured command; the real status captured on the next
#       line into `rc=$?`;
#     - for every test unit, the number of binaries that STARTED, the number
#       that FINISHED, and the number cargo says EXIST, all recorded;
#     - every command this script intends to run is checked to exist first, and
#       a missing one is recorded as a failure, never skipped silently.
#
# `set -uo pipefail` WITHOUT -e: every gate's exit code is data to be recorded,
# not a reason to abort. This matches the convention of the other scripts in
# this repo and in .agents/skills/*/scripts/. Do not "fix" this to -e.
#
# Targets bash 3.2 (the macOS system bash): no associative arrays, no ${v^^},
# and empty-array expansion is written ${a[@]+"${a[@]}"} because plain
# "${a[@]}" is an unbound-variable error under `set -u` in 3.2.
#
# Never PIPESTATUS: an agent harness may invoke this under zsh, where the array
# is $pipestatus and 1-indexed. This script never pipes a measured command, so
# it never needs either.
#
# Usage:
#   bash scripts/gate.sh probe    # resolve paths + counts, run no gates
#   bash scripts/gate.sh fast     # iteration loop: clippy, touched packages, arch
#                                 (skips the two timing suites; `full` does not)
#   bash scripts/gate.sh full     # pre-handoff: every package, deny, agents, ui,
#                                 the ui-e2e browser leg (full only, never fast)
#
# `full` RESUMES: a gate that already passed for this exact tree is not re-run,
# so a run killed by memory pressure continues where it stopped when invoked
# again. GATE_FORCE=yes redoes everything.
#
# Evidence is written to `git rev-parse --git-path gate-evidence`, i.e.
# .git/gate-evidence/ in the main tree and .git/worktrees/<n>/gate-evidence/ in
# a linked worktree. That location is deliberate: it is per-worktree by git's
# own construction, it sits OUTSIDE the work tree so `git add -A` can never see
# it (and so it cannot perturb the tree hash it certifies), it is immune to
# CARGO_TARGET_DIR and `cargo clean`, and `git worktree remove` takes it away.
#
# Exit codes:
#   0  every gate in the tier passed
#   1  at least one gate failed or was skipped
#   2  usage error / not a git repository / missing prerequisite

set -uo pipefail

SELF="${BASH_SOURCE[0]}"
TIER="${1:-}"

usage() {
    cat >&2 <<'EOF'
usage: bash scripts/gate.sh {probe|fast|full}

  probe  resolve the evidence path, the tree id and the per-package expected
         test-binary counts, then exit. Runs no gates and writes no evidence.
  fast   clippy + the packages the working tree touches + the architecture
         gate. Emits tier "fast", which is never sufficient for handoff.
  full   all packages, cargo-deny, the agents feature legs, and the web/ui
         gates, including the ui-e2e browser leg (never run by fast). Emits
         tier "full", which is what verify-evidence.sh requires.
EOF
    exit 2
}

case "$TIER" in
    probe | fast | full) ;;
    *) usage ;;
esac

ROOT="$(git rev-parse --show-toplevel 2>/dev/null)"
if [ -z "$ROOT" ]; then
    echo "gate.sh: not inside a git repository" >&2
    exit 2
fi
cd "$ROOT" || exit 2

for prereq in python3 cargo git; do
    if ! command -v "$prereq" >/dev/null 2>&1; then
        echo "gate.sh: required command not found: $prereq" >&2
        exit 2
    fi
done

EVID="$(git rev-parse --path-format=absolute --git-path gate-evidence)"
mkdir -p "$EVID" || exit 2

# ---------------------------------------------------------------- tree identity
#
# The gate runs on a dirty tree and the commit happens afterwards, so HEAD is
# not the identity. This is: apply `git add -A` to a THROWAWAY COPY of the index
# and take the resulting tree object. It matters because
# .agents/skills/*/scripts/git-commit-and-verify.sh does literally
# `git add -A && git commit`, so the commit's tree IS this hash — the identity
# is invariant across the commit boundary, which is what lets the handoff check
# prove "this evidence describes the tree being handed off".
#
# The live index is never touched. Measured at ~110 ms on this repo.
compute_tree_id() {
    local idx tmpidx out rc
    idx="$(git rev-parse --path-format=absolute --git-path index)"
    tmpidx="$EVID/.tmp-index.$$"
    cp "$idx" "$tmpidx" || return 1
    if ! GIT_INDEX_FILE="$tmpidx" git add -A >/dev/null 2>&1; then
        rm -f "$tmpidx"
        return 1
    fi
    out="$(GIT_INDEX_FILE="$tmpidx" git write-tree 2>/dev/null)"
    rc=$?
    rm -f "$tmpidx"
    [ $rc -eq 0 ] || return 1
    printf '%s\n' "$out"
}

TREE_ID="$(compute_tree_id)"
if [ -z "$TREE_ID" ]; then
    echo "gate.sh: could not compute the tree identity" >&2
    exit 2
fi
HEAD_SHA="$(git rev-parse HEAD 2>/dev/null || echo unknown)"
RUNNER="$(git hash-object "$SELF" 2>/dev/null || echo unknown)"
WORKTREE="$ROOT"

# ------------------------------------------------------- expected binary counts
#
# Authority is `cargo metadata`, not a glob. The documented `ls tests/*.rs | wc -l`
# + 1 cross-check is a FLOOR, not an equality: it misses each package's doctest
# target, and misses mcp's [[bin]] unittest target entirely. Both are recorded so
# a disagreement is visible rather than assumed away.
META="$EVID/.metadata.json"
if ! cargo metadata --no-deps --format-version 1 >"$META" 2>"$EVID/.metadata.err"; then
    echo "gate.sh: cargo metadata failed; see $EVID/.metadata.err" >&2
    exit 2
fi

# Emits one line per package: name<TAB>expected<TAB>test_targets<TAB>doctests<TAB>has_agents
PKG_TABLE="$(
    python3 - "$META" <<'PY'
import json, sys
with open(sys.argv[1]) as fh:
    meta = json.load(fh)
rows = []
for pkg in meta.get("packages", []):
    targets = pkg.get("targets", [])
    tests = sum(1 for t in targets if t.get("test"))
    doctests = sum(
        1 for t in targets if t.get("doctest") and "lib" in (t.get("kind") or [])
    )
    has_agents = "agents" in (pkg.get("features") or {})
    rows.append((pkg["name"], tests + doctests, tests, doctests, has_agents))
for name, expected, tests, doctests, has_agents in sorted(rows):
    print("%s\t%d\t%d\t%d\t%s" % (name, expected, tests, doctests,
                                  "yes" if has_agents else "no"))
PY
)"

if [ -z "$PKG_TABLE" ]; then
    echo "gate.sh: could not derive package table from cargo metadata" >&2
    exit 2
fi

pkg_field() { # pkg_field <name> <field-number>
    printf '%s\n' "$PKG_TABLE" | awk -F'\t' -v p="$1" -v n="$2" '$1 == p { print $n }'
}

pkg_names() { printf '%s\n' "$PKG_TABLE" | awk -F'\t' '{ print $1 }'; }

# Source directory for a package, so the floor cross-check and the touched-package
# mapping can both be derived rather than hardcoded. Package `logos` lives in cli/.
pkg_dir() {
    python3 - "$META" "$1" <<'PY'
import json, os, sys
with open(sys.argv[1]) as fh:
    meta = json.load(fh)
for pkg in meta.get("packages", []):
    if pkg["name"] == sys.argv[2]:
        print(os.path.dirname(pkg["manifest_path"]))
        break
PY
}

floor_for() { # tests/*.rs + 1, the documented cheap cross-check
    local dir n
    dir="$(pkg_dir "$1")"
    n=0
    if [ -d "$dir/tests" ]; then
        n="$(/bin/ls -1 "$dir"/tests/*.rs 2>/dev/null | wc -l | tr -d ' ')"
    fi
    echo $((n + 1))
}

# ------------------------------------------------------------------ probe mode
if [ "$TIER" = "probe" ]; then
    echo "root:        $ROOT"
    echo "evidence:    $EVID"
    echo "tree_id:     $TREE_ID"
    echo "head:        $HEAD_SHA"
    echo "runner:      $RUNNER"
    echo
    printf '%-12s %9s %7s %9s %8s %8s\n' package expected tests doctests floor agents
    total=0
    pkg_names | while read -r p; do
        exp="$(pkg_field "$p" 2)"
        tst="$(pkg_field "$p" 3)"
        doc="$(pkg_field "$p" 4)"
        agt="$(pkg_field "$p" 5)"
        flr="$(floor_for "$p")"
        warn=""
        if [ "$exp" -lt "$flr" ]; then warn="  <- BELOW FLOOR"; fi
        printf '%-12s %9s %7s %9s %8s %8s%s\n' "$p" "$exp" "$tst" "$doc" "$flr" "$agt" "$warn"
    done
    echo
    echo "expected total: $(pkg_names | while read -r p; do pkg_field "$p" 2; done |
        awk '{ s += $1 } END { print s + 0 }')"
    exit 0
fi

# ------------------------------------------------------------- evidence writing
#
# Per-unit rows accumulate in a TSV, then python3 renders the JSON. Building
# JSON by hand in shell is how you get a file that cannot be parsed on the one
# occasion it matters.
UNITS=""

reset_units() { UNITS=""; }

add_unit() { # name expected floor started finished passed failed ignored exit verdict truncation
    UNITS="${UNITS}${1}	${2}	${3}	${4}	${5}	${6}	${7}	${8}	${9}	${10}	${11}
"
}

write_evidence() { # gate argv exit verdict truncation
    local gate="$1" argv="$2" ex="$3" verdict="$4" trunc="$5"
    printf '%s' "$UNITS" >"$EVID/.units.$$"
    python3 - \
        "$EVID/$gate.json" "$gate" "$TIER" "$TREE_ID" "$HEAD_SHA" "$WORKTREE" \
        "$argv" "$ex" "$verdict" "$trunc" "$RUNNER" "$EVID/$gate.log" \
        "$EVID/.units.$$" <<'PY'
import json, os, sys
(out, gate, tier, tree_id, head, worktree, argv, exit_code, verdict, trunc,
 runner, log, units_path) = sys.argv[1:14]
units = []
if os.path.exists(units_path):
    with open(units_path) as fh:
        for line in fh:
            line = line.rstrip("\n")
            if not line:
                continue
            f = line.split("\t")
            units.append({
                "unit": f[0],
                "expected": int(f[1]),
                "floor": int(f[2]),
                "started": int(f[3]),
                "finished": int(f[4]),
                "passed": int(f[5]),
                "failed": int(f[6]),
                "ignored": int(f[7]),
                "exit_code": int(f[8]),
                "verdict": f[9],
                "truncation": f[10],
            })
doc = {
    "schema": 1,
    "gate": gate,
    "tier": tier,
    "tree_id": tree_id,
    "head": head,
    "worktree": worktree,
    "argv": argv,
    "exit_code": int(exit_code),
    "verdict": verdict,
    "truncation": trunc,
    "units": units,
    "log": log,
    "runner": runner,
}
with open(out, "w") as fh:
    json.dump(doc, fh, indent=2, sort_keys=True)
    fh.write("\n")
PY
    local rc=$?
    rm -f "$EVID/.units.$$"
    return $rc
}

# Timing-sensitive suites (NFR-PE-05 cold start, NFR-PE-03 single-file sync).
#
# The FAST tier skips them, so the iteration loop is not hostage to machine load.
# The FULL tier does NOT: `.github/workflows/ci.yml` runs `cargo test --workspace`
# and skips nothing, so a gate that skipped them would be strictly more permissive
# than CI while claiming to match it — which is the same class of false assurance
# this script exists to remove.
#
# A timing breach on a loaded host should be re-run in isolation before it is read
# as a regression; NFR-PE-05 exposes LOGOS_PERF_TOLERANCE for exactly that, and
# widening it is a decision to record, not a default to apply here. There is
# deliberately no auto-retry: a retry loop that turns a red into a green is
# indistinguishable from one that hides a real regression.
declare -a SKIPS
SKIPS=()
if [ "$TIER" = "fast" ]; then
    SKIPS=(--skip pe05_budget --skip pe03_budget)
fi

GATES_RUN=""
GATES_FAILED=""

FORCE="${GATE_FORCE:-no}"

# 0 when this gate already passed for the CURRENT tree at tier full.
#
# Resuming is sound because evidence is per-gate and keyed on tree_id, and
# verify-evidence.sh already requires every gate's tree_id to agree with its
# siblings AND with the tree as it stands. Completing the tier across several
# invocations is therefore indistinguishable from completing it in one; and if
# the tree changes in between, every cached gate is stale and gets redone —
# which is exactly what the validator would demand anyway.
gate_done() {
    local f="$EVID/$1.json"
    [ "$TIER" = "full" ] || return 1
    [ "$FORCE" = "no" ] || return 1
    [ -f "$f" ] || return 1
    python3 -c 'import json,sys
try:
    d = json.load(open(sys.argv[1]))
except Exception:
    sys.exit(1)
sys.exit(0 if (d.get("tree_id") == sys.argv[2] and d.get("tier") == "full"
               and d.get("verdict") == "pass") else 1)' "$f" "$TREE_ID"
}

record() { # gate verdict [display-note]
    # The verdict is compared EXACTLY, and any human-readable qualifier goes in
    # the third argument. Passing "pass (cached, same tree)" as the verdict made
    # every cached gate count as a failure — gate.sh reported FAILED and exited 1
    # while the evidence and the validator both correctly said pass. A false RED
    # is the safe direction, but it would teach a reader to distrust the exit code,
    # which is the one thing here that must stay trustworthy.
    GATES_RUN="$GATES_RUN $1"
    if [ "$2" != "pass" ]; then GATES_FAILED="$GATES_FAILED $1"; fi
    printf '  %-14s %s%s\n' "$1" "$2" "${3:+ $3}"
}

# ------------------------------------------------------------------ test units
#
# Four outcomes must be distinguishable, and only the counters distinguish them:
#   started == 0            nothing ran (build error, or --features on a package
#                           that has none: exit 101 having executed nothing)
#   started >  finished     a binary announced itself and never summarised —
#                           the OOM-kill signature
#   finished <  expected     truncation (only reachable if --no-fail-fast is lost)
#   failed   >  0            genuine failures
run_test_unit() { # gate pkg expected floor use_agents -> sets UNIT_VERDICT
    local gate="$1" pkg="$2" expected="$3" floor="$4" use_agents="$5"
    local log="$EVID/$gate--$pkg.log" rc started finished p f g trunc verdict
    local -a feat
    feat=()
    if [ "$use_agents" = "yes" ]; then feat=(--features agents); fi

    # `--jobs 2` bounds the COMPILE, which is what exhausts memory here — not the
    # test run. Measured on a 16 GiB machine with ~8 GB of swap already in use: an
    # unbounded `cargo test` linking 82 logos-core test binaries, then relinking
    # all of them for the `agents` feature set, was OOM-killed twice at exactly
    # that transition. RAYON_NUM_THREADS/--test-threads bound the execution;
    # neither touches the link step.
    CARGO_TERM_COLOR=never RAYON_NUM_THREADS=2 \
        cargo test -p "$pkg" --jobs 2 --no-fail-fast ${feat[@]+"${feat[@]}"} -- \
        --test-threads=2 ${SKIPS[@]+"${SKIPS[@]}"} \
        >"$log" 2>&1
    rc=$?

    # `Running` / `Doc-tests` are cargo's stderr; `test result:` is libtest's
    # stdout. Both are in this one file, in order, which is what makes the
    # started-vs-finished comparison meaningful.
    started="$(grep -c -E '^[[:space:]]*(Running|Doc-tests)[[:space:]]' "$log")"
    finished="$(grep -c -E '^test result: (ok|FAILED)\.' "$log")"

    # Sum across BOTH `ok.` and `FAILED.` lines — a failed suite still passed
    # most of its tests, so counting only `ok.` undercounts the moment anything
    # fails. Key on the FOLLOWING field name, never a fixed column: the two line
    # shapes have different token counts before the numbers.
    read -r p f g <<EOF
$(awk '/^test result: (ok|FAILED)\./ {
         for (i = 1; i <= NF; i++) {
           if ($(i+1) == "passed;")  p += $i
           if ($(i+1) == "failed;")  f += $i
           if ($(i+1) == "ignored;") g += $i
         }
       }
       END { printf "%d %d %d\n", p, f, g }' "$log")
EOF

    trunc=none
    verdict=pass
    if grep -q -E '^error: (could not compile|none of the selected packages)' "$log"; then
        trunc=build_error
        verdict=fail
    elif [ "$started" -eq 0 ]; then
        trunc=no_binaries
        verdict=fail
    elif [ "$started" -gt "$finished" ]; then
        trunc=killed
        verdict=fail
    elif [ "$finished" -lt "$expected" ]; then
        trunc=fail_fast
        verdict=fail
    elif [ "$f" -gt 0 ] || [ "$rc" -ne 0 ]; then
        verdict=fail
    fi

    add_unit "$pkg" "$expected" "$floor" "$started" "$finished" \
        "$p" "$f" "$g" "$rc" "$verdict" "$trunc"
    UNIT_VERDICT="$verdict"
    printf '    %-12s %s  %s/%s binaries, %s passed, %s failed%s\n' \
        "$pkg" "$verdict" "$finished" "$expected" "$p" "$f" \
        "$([ "$trunc" = none ] || echo "  [$trunc]")"
}

gate_tests() { # gate_name use_agents pkg...
    local gate="$1" use_agents="$2"
    shift 2
    if gate_done "$gate"; then record "$gate" pass "(cached, same tree)"; return; fi
    local pkg expected floor worst=pass ex=0
    reset_units
    echo "$gate:"
    for pkg in "$@"; do
        expected="$(pkg_field "$pkg" 2)"
        floor="$(floor_for "$pkg")"
        if [ "$use_agents" = "yes" ] && [ "$(pkg_field "$pkg" 5)" != "yes" ]; then
            # Passing --features to a package without it exits 101 having run
            # nothing. Refusing to ask is the fix, not recording the 101.
            continue
        fi
        run_test_unit "$gate" "$pkg" "$expected" "$floor" "$use_agents"
        if [ "$UNIT_VERDICT" != "pass" ]; then
            worst=fail
            ex=1
        fi
    done
    cat "$EVID/$gate"--*.log >"$EVID/$gate.log" 2>/dev/null
    write_evidence "$gate" \
        "cargo test -p <pkg> --jobs 2 --no-fail-fast$([ "$use_agents" = yes ] && echo ' --features agents') -- --test-threads=2 ${SKIPS[*]:-}" \
        "$ex" "$worst" none
    record "$gate" "$worst"
}

# ----------------------------------------------------------------------- clippy
gate_clippy() {
    if gate_done clippy; then record clippy pass "(cached, same tree)"; return; fi
    local log="$EVID/clippy.log" rc verdict
    reset_units
    # --jobs 4 is compile-only work: it does not hit the rayon-pool-per-test
    # oversubscription that bounds the test gate, but it does bound memory on a
    # 16 GiB machine. -D warnings mirrors CI exactly.
    CARGO_TERM_COLOR=never cargo clippy --workspace --all-targets --jobs 4 -- -D warnings \
        >"$log" 2>&1
    rc=$?
    if [ $rc -eq 0 ]; then verdict=pass; else verdict=fail; fi
    write_evidence clippy \
        "cargo clippy --workspace --all-targets --jobs 4 -- -D warnings" \
        "$rc" "$verdict" none
    record clippy "$verdict"
}

# ------------------------------------------------------------------- cargo-deny
gate_deny() {
    if gate_done deny; then record deny pass "(cached, same tree)"; return; fi
    local log="$EVID/deny.log" rc verdict trunc=none
    reset_units
    if ! command -v cargo-deny >/dev/null 2>&1; then
        # Not installed is not clean. CI has never actually executed this gate
        # (the test step fails first), so an absent local one is worth shouting
        # about rather than skipping.
        echo "cargo-deny is not installed" >"$log"
        write_evidence deny "cargo deny check" 127 skip no_binaries
        record deny skip
        return
    fi
    cargo deny check >"$log" 2>&1
    rc=$?
    if [ $rc -eq 0 ]; then verdict=pass; else verdict=fail; fi
    write_evidence deny "cargo deny check" "$rc" "$verdict" "$trunc"
    record deny "$verdict"
}

# -------------------------------------------------------------- architecture
#
# `logos check` is tri-state since CR-112 / FR-GV-22: 0 pass, 1 error
# violations, 4 no rules contract loaded. Exit 4 is NOT success — it is the
# vacuous pass this whole script exists to make impossible, so it is recorded
# as a failure with truncation "no_binaries": nothing was evaluated.
gate_arch() {
    if gate_done arch; then record arch pass "(cached, same tree)"; return; fi
    local log="$EVID/arch.log" rc verdict trunc=none
    reset_units
    if ! command -v logos >/dev/null 2>&1; then
        echo "logos is not on PATH" >"$log"
        write_evidence arch "logos check --json" 127 skip no_binaries
        record arch skip
        return
    fi
    logos check --json >"$log" 2>&1
    rc=$?
    case $rc in
        0) verdict=pass ;;
        4)
            verdict=fail
            trunc=no_binaries
            ;;
        *) verdict=fail ;;
    esac
    write_evidence arch "logos check --json" "$rc" "$verdict" "$trunc"
    record arch "$verdict"
}

# ------------------------------------------------------------------- web/ui
#
# Every npm script this gate intends to run is checked against package.json
# first. `npm run lint` here reports text that reads like ESLint ran and exits 0
# through a pipe — there is no `lint` script. A missing script is a failure.
ui_has_script() {
    python3 - "$ROOT/web/ui/package.json" "$1" <<'PY'
import json, sys
try:
    with open(sys.argv[1]) as fh:
        scripts = json.load(fh).get("scripts", {})
except OSError:
    sys.exit(1)
sys.exit(0 if sys.argv[2] in scripts else 1)
PY
}

gate_ui() { # gate_name npm_script
    # Two `local` statements, not one: bash expands every argument to `local`
    # BEFORE the builtin assigns any of them, so a single
    # `local gate="$1" log="$EVID/$gate.log"` reads $gate while it is still
    # unbound — fatal under `set -u`. run_test_unit already split for this
    # reason; gate_ui did not, and only the ui path exercised it.
    local gate="$1" script="$2"
    local log="$EVID/$gate.log" rc verdict
    if gate_done "$gate"; then record "$gate" pass "(cached, same tree)"; return; fi
    reset_units
    if ! command -v npm >/dev/null 2>&1; then
        echo "npm is not installed" >"$log"
        write_evidence "$gate" "npm run $script" 127 skip no_binaries
        record "$gate" skip
        return
    fi
    if ! ui_has_script "$script"; then
        echo "web/ui/package.json declares no \"$script\" script" >"$log"
        write_evidence "$gate" "npm run $script" 127 fail no_binaries
        record "$gate" fail
        return
    fi
    (cd "$ROOT/web/ui" && npm run "$script") >"$log" 2>&1
    rc=$?
    if [ $rc -eq 0 ]; then verdict=pass; else verdict=fail; fi
    write_evidence "$gate" "npm run $script" "$rc" "$verdict" none
    record "$gate" "$verdict"
}

# ----------------------------------------------------------------- web/ui e2e
#
# The browser layout specs (S-611, CR-203 §11): Playwright drives a TREE-BUILT
# `logos serve --ui` over the checked-in fixtures and asserts computed style.
# FULL tier only — it builds the SPA and the binary, and needs a browser.
#
# Its denominator is Playwright's own JSON report, recorded as one unit: tests
# listed (expected), executed (started/finished), passed, failed, skipped. The
# verdict comes from scripts/ui-e2e-classify.py, and each way it can read green
# while testing nothing is a failure there:
#   - no browser installed      -> `missing_browser`
#   - no / unreadable report,
#     or nothing executed       -> `no_binaries`
#   - a test skipped            -> `skipped` (a layout check that did not run)
#   - the run hangs             -> the watchdog kills it at UI_E2E_TIMEOUT
#                                  seconds (macOS has no `timeout`): `killed`
UI_E2E_TIMEOUT="${UI_E2E_TIMEOUT:-900}"

gate_ui_e2e() {
    local gate=ui-e2e
    local log="$EVID/$gate.log" ui="$ROOT/web/ui" rc verdict trunc=none
    local report="$ROOT/web/ui/e2e/.results/results.json" listed ran passed failed skipped
    if gate_done "$gate"; then record "$gate" pass "(cached, same tree)"; return; fi
    reset_units
    for tool in npm perl; do
        if ! command -v "$tool" >/dev/null 2>&1; then
            echo "$tool is not installed" >"$log"
            write_evidence "$gate" "npm run test:e2e" 127 fail no_binaries
            record "$gate" fail "($tool missing)"
            return
        fi
    done
    if ! ui_has_script test:e2e; then
        echo "web/ui/package.json declares no \"test:e2e\" script" >"$log"
        write_evidence "$gate" "npm run test:e2e" 127 fail no_binaries
        record "$gate" fail
        return
    fi

    # The binary must embed a real SPA build: build the bundle, then the binary,
    # then restore the tracked dist placeholder (rust-embed's debug-embed has
    # already copied the bundle into the binary, so the restore cannot undo it).
    {
        echo "== npm run build"
        (cd "$ui" && npm run build)
    } >"$log" 2>&1
    rc=$?
    if [ $rc -eq 0 ]; then
        echo "== cargo build -p logos --bin logos" >>"$log"
        CARGO_TERM_COLOR=never cargo build -p logos --bin logos --jobs 4 >>"$log" 2>&1
        rc=$?
    fi
    git checkout -- web/ui/dist 2>/dev/null
    if [ $rc -ne 0 ]; then
        write_evidence "$gate" "npm run build && cargo build -p logos" "$rc" fail build_error
        record "$gate" fail "(build failed)"
        return
    fi

    rm -f "$report"
    echo "== npm run test:e2e (bounded at ${UI_E2E_TIMEOUT}s)" >>"$log"
    (cd "$ui" && LOGOS_E2E_BIN="${CARGO_TARGET_DIR:-$ROOT/target}/debug/logos" \
        perl -e 'alarm shift; exec @ARGV' "$UI_E2E_TIMEOUT" npm run test:e2e) >>"$log" 2>&1
    rc=$?
    # A killed run cannot stop its fixture servers; nothing else serves from .run/.
    pkill -f "$ROOT/web/ui/e2e/.run/" >/dev/null 2>&1

    # The verdict is scripts/ui-e2e-classify.py's (tested by
    # scripts/tests/test-ui-e2e-classify.sh): listed = every test reported,
    # ran = those that executed, so a skipped test leaves finished < expected
    # and verify-evidence.sh rejects the unit as well.
    read -r verdict trunc listed ran passed failed skipped <<EOF
$(python3 "$ROOT/scripts/ui-e2e-classify.py" "$report" "$rc" "$log")
EOF
    if [ -z "${skipped:-}" ]; then
        verdict=fail trunc=classifier_error listed=0 ran=0 passed=0 failed=0 skipped=0
    fi
    add_unit playwright "$listed" 1 "$ran" "$ran" "$passed" "$failed" "$skipped" \
        "$rc" "$verdict" "$trunc"
    write_evidence "$gate" "npm run test:e2e" "$rc" "$verdict" "$trunc"
    record "$gate" "$verdict" "($passed passed, $failed failed, $skipped skipped of $listed$([ "$trunc" = none ] || echo "; $trunc"))"
}

# ------------------------------------------------- what this branch changed, and from where
#
# The fast tier scopes its test and UI legs to the packages that changed. The
# question is "changed since WHEN", and the answer used to be `HEAD` — i.e. the
# UNCOMMITTED set.
#
# Under the handoff contract every session commits and THEN gates, so that set is
# always empty: the test leg ran over zero packages, recorded `"verdict": "pass"`
# over `"units": []` with a 0-byte log, and the UI legs never ran at all. Four of
# eight sessions in Sprint 72 reported this upward independently, and the
# evidence marker's `tests=0/0` is what made it visible from the outside.
#
# The set that actually matters is what this BRANCH changed: everything since it
# diverged from the integration branch. On the integration branch itself there is
# no such divergence, so the honest answer there is the uncommitted set — and the
# `full` tier, which tests every package via `pkg_names`, is what runs there.
#
# GATE_BASE overrides the detection for a caller that knows better.
gate_base() {
    if [ -n "${GATE_BASE:-}" ]; then printf '%s\n' "$GATE_BASE"; return 0; fi
    local integ base head
    head="$(git rev-parse HEAD 2>/dev/null)" || { printf '%s\n' HEAD; return 0; }
    for integ in main master; do
        git rev-parse --verify --quiet "$integ" >/dev/null 2>&1 || continue
        base="$(git merge-base HEAD "$integ" 2>/dev/null || true)"
        # Empty on unrelated histories; equal to HEAD when we ARE the integration
        # branch (or strictly behind it) — in both cases there is no branch delta
        # to scope to, so fall back to the uncommitted set rather than to nothing.
        if [ -n "$base" ] && [ "$base" != "$head" ]; then printf '%s\n' "$base"; return 0; fi
        break
    done
    printf '%s\n' HEAD
}

# ------------------------------------------------------- which packages changed
touched_packages() {
    local changed p dir
    changed="$(
        {
            git diff --name-only "$(gate_base)" 2>/dev/null
            git ls-files --others --exclude-standard 2>/dev/null
        } | sort -u
    )"
    [ -n "$changed" ] || return 0
    pkg_names | while read -r p; do
        dir="$(pkg_dir "$p")"
        dir="${dir#$ROOT/}"
        if printf '%s\n' "$changed" | grep -q "^$dir/"; then printf '%s\n' "$p"; fi
    done
}

# --------------------------------------------------------------------- the tiers
echo "gate.sh $TIER"
echo "  tree_id  $TREE_ID"
echo "  evidence $EVID"
# The fast tier's legs are change-scoped, so the base they scope against is part
# of reading the result: "0 packages changed" means something different against a
# merge-base than against HEAD.
[ "$TIER" = fast ] && echo "  base     $(gate_base)"
echo

case "$TIER" in
    fast)
        gate_clippy
        TOUCHED="$(touched_packages)"
        if [ -n "$TOUCHED" ]; then
            # shellcheck disable=SC2086
            gate_tests test no $TOUCHED
        else
            echo "test: no package sources changed; nothing to run"
            reset_units
            : >"$EVID/test.log"
            write_evidence test "cargo test (no package sources changed)" 0 pass none
            record test pass
        fi
        gate_arch
        if git diff --name-only "$(gate_base)" 2>/dev/null | grep -q '^web/ui/'; then
            gate_ui ui-typecheck typecheck
            gate_ui ui-test test
        fi
        ;;
    full)
        gate_clippy
        # shellcheck disable=SC2086
        gate_tests test no $(pkg_names)
        # shellcheck disable=SC2086
        gate_tests test-agents yes $(pkg_names)
        gate_deny
        gate_arch
        gate_ui ui-typecheck typecheck
        gate_ui ui-test test
        gate_ui ui-build build
        # Building the SPA rewrites the TRACKED placeholder web/ui/dist/index.html,
        # which would change the tree identity mid-gate. Restore it and re-verify.
        if ! git diff --quiet -- web/ui/dist 2>/dev/null; then
            git checkout -- web/ui/dist 2>/dev/null
        fi
        # Browser layout specs: full tier only, never fast (S-611).
        gate_ui_e2e
        AFTER="$(compute_tree_id)"
        if [ "$AFTER" != "$TREE_ID" ]; then
            echo
            echo "gate.sh: the tree changed while gating ($TREE_ID -> $AFTER)." >&2
            echo "         The evidence describes a tree that no longer exists." >&2
            GATES_FAILED="$GATES_FAILED tree-stability"
        fi
        ;;
esac

echo
if [ -n "$GATES_FAILED" ]; then
    echo "FAILED:$GATES_FAILED"
    exit 1
fi
echo "all gates passed (tier $TIER)"
exit 0
