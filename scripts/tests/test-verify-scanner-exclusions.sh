#!/usr/bin/env bash
#
# test-verify-scanner-exclusions.sh — prove scripts/verify-scanner-exclusions.sh
# records what it saw and nothing it did not (S-771), and that nothing gated
# calls it.
#
# No real scanner runs here (Sprint 106 Decision 40). The harness finds scanners
# only under the user tools directory, so LOGOS_DATA_HOME points it at a stub
# one: each "scanner" is a Python stub ($WORK/stub.py) that walks the targets it
# is given and behaves the way the committed evidence says the real one does —
# Semgrep skips `target/` only inside a git repository and matches `--exclude`
# by name at any depth; OSV-Scanner loses its ignore handling where `.git` is a
# file; gosec matches `-exclude-dir` as an unanchored expression and roots each
# URI at its package argument; CodeQL honours LGTM_INDEX_FILTERS and
# `paths-ignore`. The harness's verdicts are then checked against what each
# stub did.
#
# What is checked (the denominator is the last line, `passed N, failed M`):
#   - no gate.sh tier, no script gate.sh sources, and no workflow names the
#     harness or globs top-level scripts — and that check fires on a tree
#     whose gate.sh does call it (an in-suite negative fixture);
#   - every scanner absent from the tools directory is `not installed`, one row
#     per fixture, with no verdict, even when a same-named binary is on PATH
#     (which never runs);
#   - with the stubs: every row's verdicts match the stub's behaviour, the
#     manifest's pin is the version run and named even beside a newer copy, the
#     version line skips a warning printed before it, a diagnostic reaches the
#     note column, and neither output file names the work directory;
#   - every mechanism's recorded argv and every run's environment (Semgrep's
#     metrics off, gosec's local toolchain, CodeQL's index filters) are pinned;
#   - without the pin installed, the newest copy at or above the manifest's
#     minimum runs and is marked `not the pin`; a copy below the minimum, or a
#     Semgrep without its interpreter, is `not installed`;
#   - a scanner that exits outside its success set is `failed` with its stderr
#     even though it wrote a readable report, and one that reports nothing for
#     the control is `inconclusive` — never a pass;
#   - a clean exit with no readable report, and a run killed at --timeout
#     after writing its report, are `failed` too;
#   - two runs on two work directories write identical verdicts.tsv;
#   - a TERM mid-run exits 143, writes no evidence over what --out held, and
#     removes the default work directory;
#   - usage errors exit 2; a work directory inside a repository exits 1.
#
# Usage: bash scripts/tests/test-verify-scanner-exclusions.sh
# Exit:  0 every check held, 1 one did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HARNESS="$HERE/scripts/verify-scanner-exclusions.sh"
[ -f "$HARNESS" ] || { echo "cannot find scripts/verify-scanner-exclusions.sh (looked in $HERE)" >&2; exit 1; }
# The harness must run under the bash macOS ships (3.2), as a maintainer would.
BASH_BIN=/bin/bash
[ -x "$BASH_BIN" ] || BASH_BIN="$(command -v bash)"

WORK="${TMPDIR:-/tmp}/logos-verify-scanner-exclusions-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
WORK="$(cd "$WORK" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() {
    echo "  FAIL  $1"
    [ -n "${2:-}" ] && printf '%s\n' "$2" | sed 's/^/        /'
    FAIL=$((FAIL + 1))
}

# harness <out> <work> [args…] -> OUT (combined output), RC
harness() {
    local out="$1" work="$2"
    shift 2
    OUT="$("$BASH_BIN" "$HARNESS" --out "$out" --work "$work" --timeout 60 "$@" 2>&1)"
    RC=$?
}

# row <tsv> <scanner> <fixture> <mechanism>: that row, or nothing.
row() {
    awk -F '\t' -v s="$2" -v f="$3" -v m="$4" '$1 == s && $3 == f && $4 == m' "$1"
}

# field <row> <n>: the n-th tab-separated field.
field() { printf '%s\n' "$1" | cut -f"$2"; }

# expect_row <tsv> <scanner> <fixture> <mechanism> <ignored> <near miss>
expect_row() {
    local r
    r="$(row "$1" "$2" "$3" "$4")"
    if [ -z "$r" ]; then bad "$2 / $3 / $4: no row"; return; fi
    if [ "$(field "$r" 6)" = "$5" ] && [ "$(field "$r" 7)" = "$6" ]; then
        ok "$2 / $3 / $4: $5, $6"
    else
        bad "$2 / $3 / $4: wanted '$5' + '$6'" "$r"
    fi
}

echo "test-verify-scanner-exclusions.sh"
echo "  work $WORK"
echo

# --- nothing gated calls it ---------------------------------------------------

# gated_files <root>: what the gate and CI run — scripts/gate.sh, every workflow,
# each `scripts/….sh` those name, and every scripts/tests/test-*.sh suite, which
# gate.sh runs by glob — less the harness and this suite, which runs it against
# stubs. Derived from what runs rather than listed, so a script a later story
# adds to a tier is read too.
gated_files() {
    local root="$1" f s
    {
        for f in "$root/scripts/gate.sh" "$root"/.github/workflows/*.yml "$root"/.github/workflows/*.yaml; do
            [ -f "$f" ] || continue
            echo "$f"
            grep -o -E 'scripts/[A-Za-z0-9_./-]+\.sh' "$f" | while IFS= read -r s; do
                [ -f "$root/$s" ] && echo "$root/$s"
            done
        done
        for f in "$root"/scripts/tests/test-*.sh; do
            [ -f "$f" ] && echo "$f"
        done
    } | sort -u | grep -v -E '/scripts/(verify-scanner-exclusions|tests/test-verify-scanner-exclusions)\.sh$'
}

# callers <root>: every non-comment line of a gated file that names the harness,
# and of gate.sh or a workflow — the runners — that runs a glob that would pick
# it up without naming it (`scripts/*.sh`, `scripts/verify-*`); in a suite such a
# glob is fixture text. gate.sh's own `scripts/*)` case arm only classifies a
# changed path, so it is not matched.
callers() {
    local root="$1" f pattern
    for f in $(gated_files "$root"); do
        case "${f#"$root"/}" in
            scripts/gate.sh|.github/workflows/*) pattern='verify-scanner-exclusions|scripts/\*\.sh|scripts/verify-\*' ;;
            *) pattern='verify-scanner-exclusions' ;;
        esac
        grep -n -E "$pattern" "$f" | grep -v -E '^[0-9]+:[[:space:]]*#' | sed "s|^|${f#"$root"/}:|"
    done
}

echo "nothing gated calls it"
found="$(callers "$HERE")"
if [ -z "$found" ]; then
    ok "no gate tier, gate library or workflow names or globs the harness"
else
    bad "a gated file calls the harness" "$found"
fi
if [ -f "$HERE/scripts/gate.sh" ] && ls "$HERE"/.github/workflows/*.yml >/dev/null 2>&1; then
    ok "the files that check reads exist (scripts/gate.sh, .github/workflows/*.yml)"
else
    bad "scripts/gate.sh or .github/workflows/*.yml is missing: the check above read nothing"
fi
# The check must fire on a tree that does call it.
NEG="$WORK/negative"
mkdir -p "$NEG/scripts" "$NEG/.github/workflows"
printf '#!/usr/bin/env bash\nbash scripts/verify-scanner-exclusions.sh\n' >"$NEG/scripts/gate.sh"
printf 'jobs:\n  x:\n    steps:\n      - run: for s in scripts/*.sh; do bash "$s"; done\n' >"$NEG/.github/workflows/ci.yml"
NEG2="$WORK/negative-indirect"
mkdir -p "$NEG2/scripts/tests" "$NEG2/.github/workflows"
printf '#!/usr/bin/env bash\nbash scripts/check-public-terms.sh\n' >"$NEG2/scripts/gate.sh"
printf '#!/usr/bin/env bash\n# verify-scanner-exclusions is named here only in a comment\nbash scripts/verify-scanner-exclusions.sh\n' >"$NEG2/scripts/check-public-terms.sh"
printf '#!/usr/bin/env bash\nbash scripts/verify-scanner-exclusions.sh --list\n' >"$NEG2/scripts/tests/test-other.sh"
printf 'jobs: {}\n' >"$NEG2/.github/workflows/ci.yml"
found2="$(callers "$NEG2")"
if [ "$(printf '%s\n' "$found2" | grep -c .)" -eq 2 ] && printf '%s\n' "$found2" | grep -q '^scripts/check-public-terms.sh:3:' && printf '%s\n' "$found2" | grep -q '^scripts/tests/test-other.sh:2:'; then
    ok "the check names a script gate.sh runs and a suite gate.sh globs, each calling the harness, and no comment"
else
    bad "the check missed an indirect caller" "$found2"
fi
found="$(callers "$NEG")"
if printf '%s\n' "$found" | grep -q '^scripts/gate.sh:2:' && printf '%s\n' "$found" | grep -q '^.github/workflows/ci.yml:4:'; then
    ok "the check names a gate.sh that calls the harness and a workflow that globs scripts/"
else
    bad "the check missed a planted caller" "$found"
fi

# --- stub scanners ------------------------------------------------------------

cat >"$WORK/stub.py" <<'PY'
import json, os, re, sys, zipfile

name, args = sys.argv[1], sys.argv[2:]
cwd = os.getcwd()

def listed(var):
    return name in os.environ.get(var, "").split(",")

VERSIONS = {
    "semgrep": ["/stub/semgrep/lib/dependencies.py:4: UserWarning: pkg_resources is deprecated 1.2.3", "1.85.0"],
    "osv-scanner": ["osv-scanner version: 1.9.2", "commit: 0000000"],
    "gosec": ["Version: 2.21.4", "Git tag: v2.21.4"],
    "codeql": ["2.27.2"],
}
if args and args[0] in ("--version", "-version", "version"):
    print("\n".join(VERSIONS[name]))
    sys.exit(0)
with open(os.environ["STUB_LOG"], "a") as fh:
    fh.write(name + " " + " ".join(args) + "\n")
# The environment the registry fixes for each scanner, one line per run.
ENV_KEYS = ("SEMGREP_ENABLE_VERSION_CHECK", "SEMGREP_SEND_METRICS", "GOTOOLCHAIN", "LGTM_INDEX_FILTERS")
with open(os.environ["STUB_ENV_LOG"], "a") as fh:
    fh.write(" ".join([name] + [k + "=" + os.environ[k] for k in ENV_KEYS if k in os.environ]) + "\n")
if listed("STUB_READ_STDIN"):
    sys.stdin.read()
if listed("STUB_TERM"):
    # Interrupt the harness mid-leg, as a maintainer's kill would.
    import signal, time
    pid_file = os.environ["STUB_TERM_PID_FILE"]
    for _ in range(50):
        if os.path.exists(pid_file):
            break
        time.sleep(0.1)
    with open(pid_file) as fh:
        os.kill(int(fh.read().strip()), signal.SIGTERM)
if listed("STUB_FAIL"):
    # Fail after writing a full, readable report: only the exit status says
    # the run went wrong, as for a scanner that crashes after its report.
    import atexit
    def boom():
        sys.stdout.flush()
        sys.stderr.write("stub: boom in " + cwd + "\n")
        sys.stderr.flush()
        os._exit(2)
    atexit.register(boom)

def parse(value_flags):
    values, positional, i = {}, [], 0
    while i < len(args):
        a = args[i]
        if a in value_flags:
            values.setdefault(a, []).append(args[i + 1])
            i += 2
        elif a.startswith("-"):
            values.setdefault(a, []).append(True)
            i += 1
        else:
            positional.append(a)
            i += 1
    return values, positional

def walk(target, keep):
    out = []
    if os.path.isfile(target):
        return [os.path.normpath(target)] if keep(target) else []
    for d, dirs, files in os.walk(target):
        dirs[:] = sorted(x for x in dirs if x != ".git")
        for f in sorted(files):
            p = os.path.normpath(os.path.join(d, f))
            if keep(p):
                out.append(p)
    return out

def sarif(path, uris):
    results = [{"ruleId": "stub", "message": {"text": "planted"},
                "locations": [{"physicalLocation": {"artifactLocation": {"uri": u}}}]} for u in uris]
    with open(path, "w") as fh:
        json.dump({"runs": [{"results": results}]}, fh)

if listed("STUB_NOREPORT"):
    # A clean exit that wrote no report at all.
    sys.exit(0)
if listed("STUB_HANG") and not os.path.exists(os.environ["STUB_HANG_MARK"]):
    # Hang once, after the report is written: only the harness's bound ends it.
    open(os.environ["STUB_HANG_MARK"], "w").close()
    import atexit, time
    atexit.register(lambda: (sys.stdout.flush(), time.sleep(30)))

if listed("STUB_SILENT"):
    found = []
else:
    found = None

if name == "semgrep":
    v, targets = parse({"--config", "--output", "--metrics", "--exclude"})
    excl = [e.strip("/") for e in v.get("--exclude", [])]
    in_git = os.path.exists(".git")
    paths = []
    for t in targets:
        paths += walk(t, lambda p: p.endswith(".py"))
    paths = [p for p in paths if not (in_git and p.startswith("target/"))]
    paths = [p for p in paths if not any(e in p.split("/") for e in excl)]
    sarif(v["--output"][0], found if found is not None else paths)
elif name == "osv-scanner":
    v, targets = parse({"--format", "--output"})
    ignores = os.path.isdir(".git")
    if os.path.isfile(".git"):
        sys.stderr.write("Unable to parse git ignores: open " + cwd + "/.git/info/exclude: not a directory\n")
    paths = []
    for t in targets:
        paths += walk(t, lambda p: os.path.basename(p) in ("requirements.txt", "go.mod"))
    paths = [p for p in paths if not (ignores and p.startswith("target/"))]
    res = [{"source": {"path": os.path.join(cwd, p), "type": "lockfile"}} for p in (found if found is not None else paths)]
    print(json.dumps({"results": res}))
    sys.stderr.write("could not find local databases for ecosystems: PyPI\n")
    sys.exit(127)
elif name == "gosec":
    v, patterns = parse({"-fmt", "-out", "-exclude-dir"})
    excl = v.get("-exclude-dir", [])
    uris = []
    for pat in patterns:
        base = pat[:-4] if pat.endswith("/...") else pat
        for p in walk(base, lambda p: p.endswith(".go")):
            if any(re.search(e, os.path.dirname(p)) for e in excl):
                continue
            uris.append(os.path.relpath(p, base))
    uris = found if found is not None else uris
    if uris:
        sarif(v["-out"][0], uris)
elif name == "codeql":
    v, pos = parse({"--codescanning-config", "--language", "--source-root"})
    db = pos[2]
    skip = os.environ.get("LGTM_INDEX_FILTERS") == "exclude:target/**"
    if "--codescanning-config" in v:
        # Only a config that ignores target/ excludes it; one CodeQL cannot read fails.
        try:
            with open(v["--codescanning-config"][0]) as fh:
                cfg = fh.read().splitlines()
        except OSError:
            sys.stderr.write("stub: cannot read the codescanning config\n")
            sys.exit(3)
        skip = skip or ("paths-ignore:" in cfg and "  - target" in cfg)
    paths = [p for p in walk(".", lambda p: p.endswith(".py")) if not (skip and p.startswith("target/"))]
    os.makedirs(db, exist_ok=True)
    with zipfile.ZipFile(os.path.join(db, "src.zip"), "w") as z:
        for p in (found if found is not None else paths):
            z.writestr(os.path.join(cwd, p).lstrip("/"), "")
PY

# install_stubs <dir> <tool:version[:nopython]>…: one stub per spec, at the
# path Logos expects; a Semgrep also gets the `bin/python` its copy needs to
# count, unless the spec says `nopython`.
install_stubs() {
    local t="$1" spec tool ver exe
    shift
    for spec in "$@"; do
        tool="${spec%%:*}"
        ver="${spec#*:}"; ver="${ver%%:*}"
        exe="$tool"
        [ "$tool" = semgrep ] && exe=bin/semgrep
        mkdir -p "$(dirname "$t/$tool/$ver/$exe")"
        printf '#!/bin/sh\nexec python3 "%s" %s "$@"\n' "$WORK/stub.py" "$tool" >"$t/$tool/$ver/$exe"
        chmod +x "$t/$tool/$ver/$exe"
        if [ "$tool" = semgrep ] && [ "${spec##*:}" != nopython ]; then
            printf '#!/bin/sh\n' >"$t/$tool/$ver/bin/python"
            chmod +x "$t/$tool/$ver/bin/python"
        fi
    done
}

# stub_tools <dir>: a tools directory holding the four stubs at their manifest
# pins, plus copies Logos would not run: a newer Semgrep (the pin wins), one
# below the minimum, a gosec directory without an executable, and one whose
# name is not a version.
stub_tools() {
    install_stubs "$1" semgrep:1.85.0 semgrep:1.90.0 semgrep:1.80.0 osv-scanner:1.9.2 \
        gosec:2.21.4 gosec:current codeql:2.27.2
    mkdir -p "$1/gosec/9.9.9"
}

export STUB_LOG="$WORK/stub.log" STUB_ENV_LOG="$WORK/stub-env.log"

# --- not installed ------------------------------------------------------------

echo
echo "a scanner that is not installed"
mkdir -p "$WORK/empty-tools" "$WORK/pathbin"
stub_tools "$WORK/path-tools"
for s in semgrep osv-scanner gosec codeql; do
    ln -s "$(find "$WORK/path-tools/$s" -type f -name "$s" | head -1)" "$WORK/pathbin/$s"
done
: >"$STUB_LOG"
OUT="$(PATH="$WORK/pathbin:$PATH" LOGOS_DATA_HOME="$WORK/empty-tools" "$BASH_BIN" "$HARNESS" \
    --out "$WORK/ni/out" --work "$WORK/ni/work" --timeout 60 2>&1)"
RC=$?
TSV="$WORK/ni/out/verdicts.tsv"
if [ "$RC" -eq 0 ] && [ -f "$TSV" ]; then ok "an empty tools directory still writes the evidence (exit 0)"; else bad "exit $RC" "$OUT"; fi
n_ni="$(awk -F '\t' 'NR > 1 && $5 == "not installed" && $6 == "not installed" && $7 == "not installed"' "$TSV" 2>/dev/null | wc -l | tr -d ' ')"
n_rows="$(awk 'NR > 1' "$TSV" 2>/dev/null | wc -l | tr -d ' ')"
if [ "$n_ni" = 12 ] && [ "$n_rows" = 12 ]; then
    ok "each of the four scanners is 'not installed' once per fixture: 12 of 12 rows"
else
    bad "wanted 12 not-installed rows of 12, got $n_ni of $n_rows" "$(cat "$TSV" 2>/dev/null)"
fi
if grep -q -E 'ignored skipped|near miss kept' "$TSV" 2>/dev/null; then
    bad "a not-installed scanner carries a verdict" "$(cat "$TSV")"
else
    ok "no not-installed row carries a verdict"
fi
if [ -s "$STUB_LOG" ]; then
    bad "a scanner on PATH ran although the tools directory is empty" "$(cat "$STUB_LOG")"
else
    ok "a same-named binary on PATH is never run"
fi
if grep -q '^## codeql' "$WORK/ni/out/evidence.md" 2>/dev/null &&
    grep -q '^| codeql | not installed |' "$WORK/ni/out/evidence.md"; then
    ok "evidence.md lists codeql as not installed"
else
    bad "evidence.md does not report codeql as not installed" "$(cat "$WORK/ni/out/evidence.md" 2>/dev/null)"
fi

# --- stubbed scanners -----------------------------------------------------------

echo
echo "with the stubbed scanners"
stub_tools "$WORK/tools"
export LOGOS_DATA_HOME="$WORK/tools"
: >"$STUB_LOG"; : >"$STUB_ENV_LOG"
harness "$WORK/a/out" "$WORK/a/work"
TSV="$WORK/a/out/verdicts.tsv"
MD="$WORK/a/out/evidence.md"
if [ "$RC" -eq 0 ]; then ok "the harness exits 0"; else bad "exit $RC" "$OUT"; fi
n_rows="$(awk 'NR > 1' "$TSV" 2>/dev/null | wc -l | tr -d ' ')"
if [ "$n_rows" = 45 ]; then
    ok "45 rows: (4 Semgrep + 2 OSV-Scanner + 6 gosec + 3 CodeQL mechanisms) x 3 fixtures"
else
    bad "wanted 45 rows, got $n_rows" "$(cat "$TSV" 2>/dev/null)"
fi

expect_row "$TSV" semgrep git template "ignored skipped" "near miss kept"
expect_row "$TSV" semgrep worktree template "ignored skipped" "near miss kept"
expect_row "$TSV" semgrep non-git template "ignored scanned" "near miss kept"
expect_row "$TSV" semgrep non-git exclude-name "ignored skipped" "near miss dropped (src/target)"
expect_row "$TSV" semgrep non-git explicit-targets "ignored skipped" "near miss kept"
expect_row "$TSV" osv-scanner git template "ignored skipped" "near miss kept"
expect_row "$TSV" osv-scanner worktree template "ignored scanned" "near miss kept"
expect_row "$TSV" osv-scanner non-git explicit-targets "ignored skipped" "near miss kept"
expect_row "$TSV" gosec git template "ignored scanned" "near miss kept"
expect_row "$TSV" gosec git exclude-dir-name "ignored skipped" "near miss dropped (src/target, targets)"
expect_row "$TSV" gosec worktree exclude-dir-caret "ignored skipped" "near miss dropped (targets)"
expect_row "$TSV" gosec git exclude-dir-anchored "ignored scanned" "near miss kept"
expect_row "$TSV" gosec worktree exclude-dir-anchored "ignored scanned" "near miss kept"
expect_row "$TSV" gosec non-git exclude-dir-anchored "ignored scanned" "near miss kept"
expect_row "$TSV" gosec git exclude-dir-tree "ignored skipped" "near miss kept"
expect_row "$TSV" gosec worktree exclude-dir-tree "ignored skipped" "near miss kept"
expect_row "$TSV" gosec non-git exclude-dir-tree "ignored skipped" "near miss kept"
expect_row "$TSV" gosec git explicit-packages "ignored skipped" "near miss kept"
expect_row "$TSV" codeql git template "ignored scanned" "near miss kept"
expect_row "$TSV" codeql worktree config-paths-ignore "ignored skipped" "near miss kept"
expect_row "$TSV" codeql non-git index-filters "ignored skipped" "near miss kept"

r="$(row "$TSV" gosec git explicit-packages)"
if [ "$(field "$r" 8)" = "app=1 target=0 src/target=1 targets=1 other=0 unrooted=3" ]; then
    ok "URIs rooted at a package argument are placed by name and counted unrooted"
else
    bad "gosec explicit-packages hits" "$r"
fi
for f in git worktree non-git; do
    r="$(row "$TSV" gosec "$f" exclude-dir-anchored)"
    if [ "$(field "$r" 8)" = "app=1 target=1 src/target=1 targets=1 other=0 unrooted=0" ]; then
        ok "$f: an expression matching target/ alone still reads the package in target/nested/"
    else
        bad "gosec $f exclude-dir-anchored hits" "$r"
    fi
done
r="$(row "$TSV" osv-scanner git template)"
if [ "$(field "$r" 8)" = "app=1 target=0 src/target=1 targets=1 other=1 unrooted=0" ]; then
    ok "the root go.mod OSV-Scanner reads counts as other"
else
    bad "osv-scanner template hits" "$r"
fi
r="$(row "$TSV" osv-scanner worktree template)"
if [ "$(field "$r" 9)" = "Unable to parse git ignores: open <root>/.git/info/exclude: not a directory" ]; then
    ok "the scanner's diagnostic reaches the note column, its fixture path spelled <root>"
else
    bad "osv-scanner worktree note" "$r"
fi
r="$(row "$TSV" gosec git exclude-dir-anchored)"
if [ "$(field "$r" 2)" = 2.21.4 ]; then ok "a row names the version run"; else bad "gosec row version" "$r"; fi
r="$(row "$TSV" osv-scanner git explicit-targets)"
if [ "$(field "$r" 8)" = "app=1 target=0 src/target=1 targets=1 other=1 unrooted=0" ]; then
    ok "OSV-Scanner's explicit targets still read the root go.mod"
else
    bad "osv-scanner explicit-targets hits" "$r"
fi

# Every mechanism's recorded argv, pinned: the evidence is only as good as the
# command it names (the network and metrics switches among them).
SG='scan --config <run>/rule.yml --sarif --output <run>/out.sarif --metrics off --disable-version-check --quiet --exclude .logos'
OSV='--format json --experimental-offline --experimental-all-packages --recursive'
GS='-fmt sarif -out <run>/out.sarif -no-fail -quiet'
CQ='database create <run>/db --language=python --build-mode=none --source-root=. --overwrite'
n_argv=0
while IFS='|' read -r s m want; do
    r="$(row "$TSV" "$s" git "$m")"
    if [ -n "$r" ] && [ "$(field "$r" 10)" = "$want" ]; then
        n_argv=$((n_argv + 1))
    else
        bad "$s / $m argv: wanted '$want'" "$(field "$r" 10)"
    fi
done <<EOF2
semgrep|template|$SG .
semgrep|exclude-name|$SG --exclude target .
semgrep|exclude-anchored|$SG --exclude /target .
semgrep|explicit-targets|$SG .gitignore app go.mod src targets
osv-scanner|template|$OSV .
osv-scanner|explicit-targets|$OSV .gitignore app go.mod src targets
gosec|template|$GS ./...
gosec|exclude-dir-name|$GS -exclude-dir target ./...
gosec|exclude-dir-caret|$GS -exclude-dir ^target ./...
gosec|exclude-dir-anchored|$GS -exclude-dir ^target\$ ./...
gosec|exclude-dir-tree|$GS -exclude-dir ^target(/|\$) ./...
gosec|explicit-packages|$GS ./app/... ./src/... ./targets/...
codeql|template|$CQ
codeql|config-paths-ignore|$CQ --codescanning-config <run>/codeql-config.yml
codeql|index-filters|$CQ
EOF2
if [ "$n_argv" = 15 ]; then ok "every mechanism records its argv: 15 of 15"; fi

# Every run's environment, pinned: Semgrep never phones home, gosec never
# fetches a toolchain, and only the index-filters leg sets LGTM_INDEX_FILTERS.
env_lines() { grep -c -x -F -- "$1" "$STUB_ENV_LOG"; }
n_sg="$(env_lines 'semgrep SEMGREP_ENABLE_VERSION_CHECK=0 SEMGREP_SEND_METRICS=off')"
n_gs="$(env_lines 'gosec GOTOOLCHAIN=local')"
n_cqf="$(env_lines 'codeql LGTM_INDEX_FILTERS=exclude:target/**')"
n_cq="$(env_lines 'codeql')"
if [ "$n_sg" = 12 ] && [ "$n_gs" = 18 ] && [ "$n_cqf" = 3 ] && [ "$n_cq" = 6 ]; then
    ok "each scanner runs with the registry's environment: 12 Semgrep, 18 gosec, 3+6 CodeQL runs"
else
    bad "run environments: semgrep $n_sg/12, gosec $n_gs/18, codeql filtered $n_cqf/3, plain $n_cq/6" "$(sort "$STUB_ENV_LOG" | uniq -c)"
fi
if grep -q '^| semgrep | 1.85.0 | 1.85.0 | `1.85.0` |$' "$MD" && grep -q '^| gosec | 2.21.4 | 2.21.4 | `Version: 2.21.4` |$' "$MD"; then
    ok "evidence.md names the pinned version run, the pin, and its version line, past a warning"
else
    bad "evidence.md versions" "$(grep '^| ' "$MD" | head -8)"
fi
if grep -q '1\.90\.0\|1\.80\.0\|9\.9\.9\|current' "$TSV" "$MD"; then
    bad "a copy other than the pin was used" "$(grep -n '1\.90\.0\|1\.80\.0\|9\.9\.9\|current' "$TSV" "$MD")"
else
    ok "the pin runs: not the newer or older Semgrep, nor a gosec directory without an executable or a version name"
fi
if grep -q "$WORK" "$TSV" "$MD"; then
    bad "the evidence names the work directory" "$(grep -n "$WORK" "$TSV" "$MD" | head -3)"
else
    ok "neither evidence file names the work directory"
fi
if grep -q -- '- `index-filters` — LGTM_INDEX_FILTERS=exclude:target/\*\*: `codeql database create' "$MD"; then
    ok "evidence.md gives each mechanism its meaning and argv"
else
    bad "evidence.md mechanism list" "$(grep -n 'index-filters' "$MD")"
fi
if grep -q 'gosec .*-exclude-dir ^target\$ ./\.\.\.' "$STUB_LOG"; then
    ok "the stubs really ran (their argv log has the anchored gosec leg)"
else
    bad "the stub log lacks the gosec anchored leg" "$(head -5 "$STUB_LOG")"
fi

# --- off the pin ---------------------------------------------------------------

echo
echo "without the pinned copy"
install_stubs "$WORK/offpin-tools" gosec:2.22.0 gosec:2.20.9 osv-scanner:1.8.0 semgrep:1.85.0:nopython
LOGOS_DATA_HOME="$WORK/offpin-tools" harness "$WORK/e/out" "$WORK/e/work" --scanners gosec,osv-scanner,semgrep
TSV_E="$WORK/e/out/verdicts.tsv"
MD_E="$WORK/e/out/evidence.md"
r="$(row "$TSV_E" gosec git template)"
if [ "$RC" -eq 0 ] && [ "$(field "$r" 2)" = 2.22.0 ] && [ "$(field "$r" 6)" = "ignored scanned" ]; then
    ok "the newest copy at or above the minimum runs when the pin is not installed"
else
    bad "off-pin gosec row (exit $RC)" "$r"
fi
if grep -q '^| gosec | 2.22.0 — \*\*not the pin\*\* | 2.21.4 | ' "$MD_E"; then
    ok "evidence.md marks a run off the pin and names the pin"
else
    bad "evidence.md does not mark the off-pin gosec run" "$(grep '^| ' "$MD_E" | head -8)"
fi
n_ni="$(awk -F '\t' 'NR > 1 && ($1 == "osv-scanner" || $1 == "semgrep") && $5 == "not installed"' "$TSV_E" | wc -l | tr -d ' ')"
if [ "$n_ni" = 6 ]; then
    ok "a copy below the minimum, and a Semgrep without its interpreter, are not installed: 6 of 6 rows"
else
    bad "wanted 6 not-installed rows for osv-scanner and semgrep, got $n_ni" "$(cat "$TSV_E" 2>/dev/null)"
fi

# --- reproducible -------------------------------------------------------------

harness "$WORK/b/out" "$WORK/b/work"
if [ "$RC" -eq 0 ] && cmp -s "$TSV" "$WORK/b/out/verdicts.tsv"; then
    ok "a second run on another work directory writes an identical verdicts.tsv"
else
    bad "the second run differs (exit $RC)" "$(diff "$TSV" "$WORK/b/out/verdicts.tsv" 2>&1 | head -10)"
fi

# --- failed and inconclusive are not passes -----------------------------------

echo
echo "failed and inconclusive legs"
# The failing gosec also reads its stdin: it must not swallow the legs after it.
STUB_FAIL=gosec STUB_READ_STDIN=gosec STUB_SILENT=semgrep harness "$WORK/c/out" "$WORK/c/work" --scanners gosec,semgrep
TSV="$WORK/c/out/verdicts.tsv"
n_failed="$(awk -F '\t' 'NR > 1 && $1 == "gosec" && $6 == "failed" && $7 == "failed" && $5 == "failed (exit 2): stub: boom in <root>"' "$TSV" | wc -l | tr -d ' ')"
if [ "$RC" -eq 0 ] && [ "$n_failed" = 18 ]; then
    ok "a scanner that exits 2 after a readable report is 'failed' with its stderr, paths as <root>, on all 18 of its legs"
else
    bad "wanted 18 failed gosec legs, got $n_failed (exit $RC)" "$(cat "$TSV" 2>/dev/null)"
fi
n_inc="$(awk -F '\t' 'NR > 1 && $1 == "semgrep" && $6 == "inconclusive" && $7 == "inconclusive"' "$TSV" | wc -l | tr -d ' ')"
if [ "$n_inc" = 12 ]; then
    ok "a scanner that reports nothing for the control is 'inconclusive' on all 12 of its legs"
else
    bad "wanted 12 inconclusive semgrep legs, got $n_inc" "$(cat "$TSV" 2>/dev/null)"
fi
if awk -F '\t' 'NR > 1 { print $1 }' "$TSV" | grep -q -E '^(osv-scanner|codeql)$'; then
    bad "--scanners gosec,semgrep ran another scanner" "$(cat "$TSV")"
else
    ok "--scanners limits the run to the scanners named"
fi
if grep -q -F "$WORK" "$TSV" "$WORK/c/out/evidence.md"; then
    bad "a failed leg's stderr leaks the work directory into the evidence" "$(grep -n -F "$WORK" "$TSV" | head -3)"
else
    ok "a failed leg's stderr does not name the work directory"
fi

# A clean exit with no report, and a run the --timeout bound kills after its
# report was written, are both `failed`.
STUB_NOREPORT=semgrep STUB_HANG=osv-scanner STUB_HANG_MARK="$WORK/g-hung" \
    harness "$WORK/g/out" "$WORK/g/work" --scanners semgrep,osv-scanner --timeout 3
TSV="$WORK/g/out/verdicts.tsv"
n_norep="$(awk -F '\t' 'NR > 1 && $1 == "semgrep" && $5 == "failed (exit 0)" && $6 == "failed" && $7 == "failed"' "$TSV" | wc -l | tr -d ' ')"
if [ "$RC" -eq 0 ] && [ "$n_norep" = 12 ]; then
    ok "a scanner that exits 0 without a readable report is 'failed (exit 0)' on all 12 of its legs"
else
    bad "wanted 12 'failed (exit 0)' semgrep legs, got $n_norep (exit $RC)" "$(cat "$TSV" 2>/dev/null)"
fi
r="$(row "$TSV" osv-scanner git template)"
n_ran="$(awk -F '\t' 'NR > 1 && $1 == "osv-scanner" && $5 == "ran (exit 127)"' "$TSV" | wc -l | tr -d ' ')"
case "$(field "$r" 5)" in
    "failed (exit 142)"*)
        if [ "$(field "$r" 6)" = failed ] && [ "$n_ran" = 5 ]; then
            ok "a run killed at --timeout is 'failed (exit 142)' despite its report; the other 5 legs ran"
        else
            bad "the timed-out leg's verdicts, or the legs after it" "$(cat "$TSV")"
        fi
        ;;
    *) bad "the hanging leg was not cut at --timeout 3" "$r" ;;
esac

# --- interrupted ----------------------------------------------------------------

echo
echo "an interrupted run"
mkdir -p "$WORK/f/out" "$WORK/f/tmp"
printf 'sentinel\n' >"$WORK/f/out/verdicts.tsv"
STUB_TERM=gosec STUB_TERM_PID_FILE="$WORK/f/pid" TMPDIR="$WORK/f/tmp" \
    "$BASH_BIN" "$HARNESS" --out "$WORK/f/out" --scanners gosec --timeout 60 >"$WORK/f/log" 2>&1 &
pid=$!
echo "$pid" >"$WORK/f/pid.tmp" && mv "$WORK/f/pid.tmp" "$WORK/f/pid"
wait "$pid"
RC=$?
if [ "$RC" -eq 143 ]; then ok "a TERM mid-run exits 143"; else bad "TERM mid-run: exit $RC" "$(cat "$WORK/f/log")"; fi
if [ "$(cat "$WORK/f/out/verdicts.tsv")" = sentinel ] && [ ! -e "$WORK/f/out/evidence.md" ]; then
    ok "an interrupted run writes no evidence over what --out held"
else
    bad "an interrupted run wrote evidence" "$(head -3 "$WORK/f/out/verdicts.tsv")"
fi
if [ -z "$(ls -A "$WORK/f/tmp")" ]; then
    ok "an interrupted run removes its default work directory"
else
    bad "the default work directory survived the interrupt" "$(ls -A "$WORK/f/tmp")"
fi

# --- usage and refusals -------------------------------------------------------

echo
echo "usage and refusals"
harness "$WORK/d/out" "$WORK/d/work" --scanners semgrep,trivy
if [ "$RC" -eq 2 ] && printf '%s\n' "$OUT" | grep -q "unknown scanner 'trivy'"; then ok "an unknown scanner exits 2"; else bad "exit $RC" "$OUT"; fi
harness "$WORK/d/out" "$WORK/d/work" --timeout 0
if [ "$RC" -eq 2 ]; then ok "--timeout 0 exits 2"; else bad "--timeout 0: exit $RC" "$OUT"; fi
mkdir -p "$WORK/repo"
git -c init.defaultBranch=main -C "$WORK/repo" init -q .
harness "$WORK/d/out" "$WORK/repo/work"
if [ "$RC" -eq 1 ] && printf '%s\n' "$OUT" | grep -q "is inside the git repository at $WORK/repo"; then
    ok "a work directory inside a repository exits 1 and names it"
else
    bad "work inside a repository: exit $RC" "$OUT"
fi

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ] && [ "$PASS" -gt 0 ]
