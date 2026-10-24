#!/usr/bin/env bash
#
# verify-scanner-exclusions.sh — record what the Logos-managed Semgrep,
# OSV-Scanner, gosec and CodeQL really do with a project's ignored directories
# (S-771, CR-239, FR-SC-26).
#
# HAND-RUN ONLY. Tests never run a real scanner (Sprint 106 Decision 40), so no
# gate.sh tier and no workflow calls this script;
# scripts/tests/test-verify-scanner-exclusions.sh asserts that. Run it by hand
# when a scanner's pinned version changes, and commit what it writes.
#
# WHAT IT DOES
#   Builds three fixtures under the work directory, each with the same
#   `.gitignore` (`/target/`, so only the top-level `target/` is ignored):
#     git       a git checkout: everything committed but `target/`;
#     worktree  a linked worktree of that checkout (`.git` is a file there);
#     non-git   a plain copy, no repository anywhere above it.
#   Every fixture plants the same finding in four directories:
#     app/          the control — a leg that does not report it proves nothing;
#     target/       the ignored directory, and target/nested/ below it: a
#                   package an exclusion matching `target` alone still reads;
#     src/target/   near miss: the same name one level down (gitignore-anchored
#                   `/target/` does not match it);
#     targets/      near miss: the name as a prefix.
#   Each installed scanner then runs, from the fixture root as the child's
#   working directory, once per candidate mechanism, and the paths it reports
#   are sorted into those four zones.
#
# VERDICTS (one row per scanner per fixture per mechanism, in verdicts.tsv)
#   ignored    `ignored skipped` | `ignored scanned`
#   near miss  `near miss kept` | `near miss dropped (<zones>)`
#   both read  `inconclusive` when the control was not reported, `failed` when
#   the scanner exited outside its success set or wrote nothing readable, and
#   `not installed` — one row per fixture, no run — when the managed copy is
#   absent. A skipped leg is never recorded as a pass. Each row also carries the
#   per-zone hit counts, the first diagnostic line the scanner printed (an
#   ignore file it could not read), and the argv; evidence.md renders the same
#   rows with the versions and each mechanism's meaning.
#
# HOW EACH SCANNER IS READ (the path-selection probe)
#   semgrep      the template's argv, with a local one-rule config in place of
#                `p/default` (the registry needs the network); SARIF result URIs.
#   osv-scanner  the template's path argv with `--format json --offline
#                --all-packages` in place of `--format sarif --output-file`:
#                every lockfile it scanned, no advisory needed, no network. The
#                2.x spellings of the `--experimental-offline` and
#                `--experimental-all-packages` the 1.9.2 run used. Exit 127 (no
#                local database) is success here.
#                A managed 1.9.x copy rejects those flags (every leg reads `failed`),
#                so install the pin first; evidence.md marks a run off the pin.
#   gosec        the template's argv; SARIF result URIs.
#   codeql       the template's `database create`; the source archive
#                (`src.zip`) is every file extraction kept, which bounds what
#                any query can report.
#
# The managed copies are found the way Logos finds them (tools::resolve) —
# under the user tools directory (LOGOS_DATA_HOME, else
# $XDG_DATA_HOME/logos/tools, else ~/.local/share/logos/tools), the pinned
# `<tool>/<version>/` from logos-core/src/tools/manifest.rs if it holds the
# executable, else the newest dotted-numeric version at or above the manifest's
# minimum — and never on PATH: what is recorded is what Logos would run.
# evidence.md names the pin beside the version run and marks a run that was
# not on the pin, which is no evidence for it.
#
# `set -uo pipefail` without -e: a failing scanner is a verdict, not an abort.
# bash 3.2.
#
# Usage:
#   bash scripts/verify-scanner-exclusions.sh [--out DIR] [--work DIR]
#                                             [--scanners a,b,…] [--timeout SECONDS]
#     --out      where verdicts.tsv and evidence.md go
#                (default scripts/tests/fixtures/scanner-exclusions)
#     --work     where the fixtures and raw outputs go (default a fresh
#                directory under $TMPDIR, removed afterwards); must not be
#                inside a git repository
#     --scanners a subset of semgrep,osv-scanner,gosec,codeql
#     --timeout  wall-clock limit per scanner run (default 900)
#
# Exit codes:
#   0  the evidence was written (whatever the verdicts)
#   1  a fixture could not be built, or the work directory is inside a repository
#   2  usage error / missing prerequisite (git, python3, perl, or a scanner pin
#      the tool manifest does not name)
#   130/143  interrupted (INT/TERM): no evidence written. The scanner then
#      running keeps its own process group (run-bounded.pl) and finishes alone

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
ALL_SCANNERS="semgrep osv-scanner gosec codeql"
OUT="$HERE/scripts/tests/fixtures/scanner-exclusions"
WORK=""
SCANNERS="$ALL_SCANNERS"
TIMEOUT=900

usage() {
    sed -n '/^# Usage:/,/^# Exit codes:/p' "$0" | sed 's/^# \{0,1\}//' | sed '$d' >&2
    exit 2
}

while [ $# -gt 0 ]; do
    case "$1" in
        --out) [ $# -ge 2 ] || usage; OUT="$2"; shift 2 ;;
        --work) [ $# -ge 2 ] || usage; WORK="$2"; shift 2 ;;
        --scanners) [ $# -ge 2 ] || usage; SCANNERS="$(printf '%s' "$2" | tr ',' ' ')"; shift 2 ;;
        --timeout) [ $# -ge 2 ] || usage; TIMEOUT="$2"; shift 2 ;;
        -h | --help) usage ;;
        *) echo "verify-scanner-exclusions: unknown argument '$1'" >&2; usage ;;
    esac
done

for s in $SCANNERS; do
    case " $ALL_SCANNERS " in
        *" $s "*) ;;
        *) echo "verify-scanner-exclusions: unknown scanner '$s' (known: $ALL_SCANNERS)" >&2; exit 2 ;;
    esac
done
case "$TIMEOUT" in
    '' | *[!0-9]* | 0*) echo "verify-scanner-exclusions: --timeout wants a positive number of seconds" >&2; exit 2 ;;
esac
for tool in git python3 perl; do
    command -v "$tool" >/dev/null 2>&1 || { echo "verify-scanner-exclusions: $tool is required" >&2; exit 2; }
done

G=(git -c maintenance.auto=false -c gc.auto=0 -c init.defaultBranch=main
    -c user.email=harness@example.invalid -c user.name=harness -c commit.gpgsign=false)
BOUNDED="$HERE/scripts/run-bounded.pl"

# ------------------------------------------------------------------ work dir

# An interrupt aborts the run. It must never fall through to writing evidence
# (the default --out is the committed evidence) over legs it cut short.
trap 'exit 130' INT
trap 'exit 143' TERM
if [ -z "$WORK" ]; then
    WORK="${TMPDIR:-/tmp}/logos-scanner-exclusions.$$"
    trap 'rm -rf "$WORK"' EXIT
fi
mkdir -p "$WORK" || { echo "verify-scanner-exclusions: cannot create $WORK" >&2; exit 1; }
WORK="$(cd "$WORK" && pwd -P)"
mkdir -p "$OUT" || { echo "verify-scanner-exclusions: cannot create $OUT" >&2; exit 1; }
OUT="$(cd "$OUT" && pwd -P)"
export GIT_CEILING_DIRECTORIES="$WORK"

# A scanner that opens repositories itself (OSV-Scanner's go-git) ignores
# GIT_CEILING_DIRECTORIES and would find an enclosing repository: refuse one.
d="$WORK"
while [ "$d" != "/" ]; do
    if [ -e "$d/.git" ]; then
        echo "verify-scanner-exclusions: the work directory $WORK is inside the git repository at $d;" >&2
        echo "  the non-git fixture would not be one. Pass --work outside any repository." >&2
        exit 1
    fi
    d="$(dirname "$d")"
done

# ------------------------------------------------------------------ tools dir

tools_dir() {
    case "${LOGOS_DATA_HOME:-}" in /*) printf '%s\n' "$LOGOS_DATA_HOME"; return ;; esac
    case "${XDG_DATA_HOME:-}" in /*) printf '%s\n' "$XDG_DATA_HOME/logos/tools"; return ;; esac
    case "${HOME:-}" in /*) printf '%s\n' "$HOME/.local/share/logos/tools"; return ;; esac
}
TOOLS="$(tools_dir)"

# executable <scanner>: its path inside a `<tool>/<version>/` directory.
executable() {
    case "$1" in
        semgrep) echo bin/semgrep ;;
        *) echo "$1" ;;
    esac
}

MANIFEST="$HERE/logos-core/src/tools/manifest.rs"

# resolve <scanner> -> "<version>\t<binary>\t<pinned>" for the copy Logos
# would run, or nothing when none qualifies. Exits 2 when the manifest does not
# name the scanner's pin: a run that cannot say which version is pinned is not
# evidence for it.
resolve() {
    python3 - "$TOOLS" "$1" "$(executable "$1")" "$MANIFEST" <<'PY'
import os, re, sys
tools, tool, exe, manifest = sys.argv[1:5]
try:
    with open(manifest) as fh:
        text = fh.read()
except OSError:
    text = ""
m = re.search(r'tool: "' + re.escape(tool) + r'"\.to_string\(\),\s*binary: [^\n]*\n\s*version: "([^"]+)"[^\n]*\n\s*min_version: "([^"]+)"', text)
if not m:
    m = re.search(r'python_tool\("' + re.escape(tool) + r'", "([^"]+)", "([^"]+)"', text)
if not m:
    sys.stderr.write("verify-scanner-exclusions: cannot read the pinned version of " + tool + " from " + manifest + "\n")
    sys.exit(2)
pin, minimum = m.group(1), m.group(2)

def key(v):
    parts = v.split(".")
    return [int(p) for p in parts] if all(p.isdigit() for p in parts) else None

def at_least(v, lo):
    a, b = key(v), key(lo)
    if a is None or b is None:
        return False
    n = max(len(a), len(b))
    return a + [0] * (n - len(a)) >= b + [0] * (n - len(b))

def usable(v):
    d = os.path.join(tools, tool, v)
    ok = lambda p: os.path.isfile(p) and os.access(p, os.X_OK)
    # A Python scanner's copy counts only while its interpreter does.
    return ok(os.path.join(d, exe)) and (tool != "semgrep" or ok(os.path.join(d, "bin", "python")))

chosen = None
if tools and usable(pin):
    chosen = pin
elif tools and os.path.isdir(os.path.join(tools, tool)):
    cands = [v for v in os.listdir(os.path.join(tools, tool)) if key(v) is not None and at_least(v, minimum)]
    cands.sort(key=key)
    for v in reversed(cands):
        if usable(v):
            chosen = v
            break
if chosen:
    print(chosen + "\t" + os.path.join(tools, tool, chosen, exe) + "\t" + pin)
PY
}

# reported_version <scanner> <binary>: the first line the binary prints that
# carries a version and is not a warning (Semgrep's Python prints some first).
reported_version() {
    local args envs=() line
    case "$1" in
        gosec) args=(-version) ;;
        codeql) args=(version --format=terse) ;;
        *) args=(--version) ;;
    esac
    while IFS= read -r line; do [ -n "$line" ] && envs+=("$line"); done <<EOF
$(probe_env "$1" -)
EOF
    env ${envs[@]+"${envs[@]}"} perl "$BOUNDED" 60 "$2" "${args[@]}" </dev/null 2>&1 |
        grep -v -i -E 'warning|^[[:space:]]|new version' | grep -m1 -E '[0-9]+\.[0-9]+'
}

# ------------------------------------------------------------------ fixtures

ZONES="app target src/target targets"

# plant <root> <zone> [<subdir>]: the same three findings in one directory —
# the zone's, or <subdir> below it, still named for the zone. They are
# deliberately vulnerable (an eval, a hardcoded credential, an old jinja2) and
# never executed: each only has to be reported. The source files carry the
# zone in their name (`planted_src_target.go`), so a reported path is placed in
# its zone even when a scanner roots it somewhere other than the fixture (gosec
# reports a URI relative to the package argument it came from).
# Each Go file imports the standard library without adding a finding, so a gosec
# that cannot type it under the local Go fails every leg instead of reading a
# project with no import (S-782).
plant() {
    local root="$1" zone="$2" dir key
    dir="$2${3:+/$3}"
    key="$(printf '%s' "$2" | tr '/' '_')"
    mkdir -p "$root/$dir"
    printf 'def run(cmd):\n    return eval(cmd)\n' >"$root/$dir/planted_$key.py"
    printf 'package zone\n\nimport "strings"\n\n// Password is a planted G101 finding.\nvar Password = "Xk9mP2vL7qR4wZ8nB3jF6tY1hD5sG0aE"\n\n// Trim imports the standard library (S-782): gosec must type it, or the run fails\n// with "without types was imported" and writes no report, whatever the zone.\nvar Trim = strings.TrimSpace\n' >"$root/$dir/planted_$key.go"
    printf 'jinja2==2.4.1\n' >"$root/$dir/requirements.txt"
}

skeleton() {
    mkdir -p "$1"
    printf '/target/\n' >"$1/.gitignore"
    printf 'module example.com/fixture\n\ngo 1.21\n' >"$1/go.mod"
}

die_fixture() {
    echo "verify-scanner-exclusions: fixture: $1" >&2
    exit 1
}

build_fixtures() {
    local co="$WORK/fixtures/git" wt="$WORK/fixtures/worktree" ng="$WORK/fixtures/non-git" z
    rm -rf "$WORK/fixtures"
    skeleton "$co"
    for z in app src/target targets; do plant "$co" "$z"; done
    "${G[@]}" -C "$co" init -q . || die_fixture "git init failed"
    "${G[@]}" -C "$co" add -A && "${G[@]}" -C "$co" commit -qm fixture || die_fixture "commit failed"
    plant "$co" target && plant "$co" target nested
    "${G[@]}" -C "$co" worktree add -q --detach "$wt" HEAD || die_fixture "git worktree add failed"
    plant "$wt" target && plant "$wt" target nested
    skeleton "$ng"
    for z in $ZONES; do plant "$ng" "$z"; done
    plant "$ng" target nested

    # The fixtures must be what their names say.
    for root in "$co" "$wt"; do
        [ "$(cd "$root" && git rev-parse --show-toplevel)" = "$root" ] || die_fixture "$root is not its own work tree"
        for f in target/requirements.txt target/nested/requirements.txt; do
            (cd "$root" && git check-ignore -q "$f") || die_fixture "${f%/*}/ is not ignored in $root"
        done
        for z in src/target targets app; do
            if (cd "$root" && git check-ignore -q "$z/requirements.txt"); then die_fixture "$z/ is ignored in $root"; fi
        done
    done
    [ -f "$wt/.git" ] || die_fixture "$wt is not a linked worktree (.git is not a file)"
    if (cd "$ng" && git rev-parse --git-dir >/dev/null 2>&1); then die_fixture "$ng is inside a repository"; fi
    # check-ignore matches a path that does not exist: each ignored zone,
    # nested copy included, must also be planted.
    for root in "$co" "$wt" "$ng"; do
        for f in target/requirements.txt target/nested/requirements.txt; do
            [ -f "$root/$f" ] || die_fixture "$f was not planted in $root"
        done
    done
}

# ------------------------------------------------------------------ mechanisms
#
# mechanisms <scanner>: one line per candidate, "<id>|<what it is>". `template`
# is what Logos passes today; the others are what an exclusion could be.
mechanisms() {
    case "$1" in
        semgrep)
            echo "template|no exclusion: Semgrep's own target selection over ."
            echo "exclude-name|--exclude target (a name, as Semgrep matches it)"
            echo "exclude-anchored|--exclude /target (a root-anchored pattern)"
            echo "explicit-targets|every unignored top-level entry as a target: .gitignore app go.mod src targets"
            ;;
        osv-scanner)
            echo "template|no exclusion: --recursive . with OSV-Scanner's own .gitignore handling"
            echo "explicit-targets|every unignored top-level entry: --recursive .gitignore app go.mod src targets"
            ;;
        gosec)
            echo "template|no exclusion: ./..."
            echo "exclude-dir-name|-exclude-dir target"
            echo "exclude-dir-caret|-exclude-dir ^target"
            echo "exclude-dir-anchored|-exclude-dir ^target\$"
            echo "exclude-dir-tree|-exclude-dir ^target(/|\$) (the directory and every directory below it)"
            echo "explicit-packages|the unignored top-level directories as packages (gosec takes no file): ./app/... ./src/... ./targets/..."
            ;;
        codeql)
            echo "template|no exclusion: database create --source-root=."
            echo "config-paths-ignore|--codescanning-config <cfg> with paths-ignore: [target] (cfg outside the project)"
            echo "index-filters|LGTM_INDEX_FILTERS=exclude:target/**"
            ;;
    esac
}

# argv <scanner> <mechanism> <run dir>: the argv words, one per line, after the
# binary. <run dir> holds the outputs and any config the harness writes.
argv() {
    local s="$1" m="$2" r="$3"
    case "$s" in
        semgrep)
            printf '%s\n' scan --config "$r/rule.yml" --sarif --output "$r/out.sarif" \
                --metrics off --disable-version-check --quiet --exclude .logos
            case "$m" in
                exclude-name) printf '%s\n' --exclude target . ;;
                exclude-anchored) printf '%s\n' --exclude /target . ;;
                explicit-targets) printf '%s\n' .gitignore app go.mod src targets ;;
                *) echo . ;;
            esac
            ;;
        osv-scanner)
            printf '%s\n' --format json --offline --all-packages --recursive
            case "$m" in
                explicit-targets) printf '%s\n' .gitignore app go.mod src targets ;;
                *) echo . ;;
            esac
            ;;
        gosec)
            printf '%s\n' -fmt sarif -out "$r/out.sarif" -no-fail -quiet
            case "$m" in
                exclude-dir-name) printf '%s\n' -exclude-dir target ./... ;;
                exclude-dir-caret) printf '%s\n' -exclude-dir '^target' ./... ;;
                exclude-dir-anchored) printf '%s\n' -exclude-dir '^target$' ./... ;;
                exclude-dir-tree) printf '%s\n' -exclude-dir '^target(/|$)' ./... ;;
                explicit-packages) printf '%s\n' ./app/... ./src/... ./targets/... ;;
                *) echo ./... ;;
            esac
            ;;
        codeql)
            printf '%s\n' database create "$r/db" --language=python --build-mode=none --source-root=. --overwrite
            case "$m" in
                config-paths-ignore) printf '%s\n' --codescanning-config "$r/codeql-config.yml" ;;
            esac
            ;;
    esac
}

# The success exit statuses of each probe.
ok_exit() {
    case "$1" in
        osv-scanner) echo "0 1 127" ;; # 1 vulnerabilities, 127 no local database
        *) echo 0 ;;
    esac
}

# The environment each probe runs with (the registry's, plus the mechanism's).
probe_env() {
    case "$1" in
        semgrep) printf '%s\n' SEMGREP_ENABLE_VERSION_CHECK=0 SEMGREP_SEND_METRICS=off ;;
        gosec) echo GOTOOLCHAIN=local ;;
        codeql) [ "$2" = index-filters ] && echo 'LGTM_INDEX_FILTERS=exclude:target/**' ;;
    esac
    return 0
}

# Files a run needs in its run directory before it starts.
prepare_run() {
    local s="$1" m="$2" r="$3"
    case "$s" in
        semgrep)
            cat >"$r/rule.yml" <<'YML'
rules:
  - id: planted-eval
    pattern: eval(...)
    message: planted finding
    languages: [python]
    severity: WARNING
YML
            ;;
        codeql)
            [ "$m" = config-paths-ignore ] && printf 'paths-ignore:\n  - target\n' >"$r/codeql-config.yml"
            ;;
    esac
    return 0
}

# tally <scanner> <run dir> <fixture root>: "app target src/target targets other
# unrooted" — how many reported paths fell in each zone (`other`: none of the
# four), and how many do not name a file relative to the fixture root
# (`unrooted`). Prints nothing when the output could not be read.
tally() {
    python3 - "$1" "$2" "$3" <<'PY'
import json, os, re, sys, zipfile
scanner, run, root = sys.argv[1], sys.argv[2], sys.argv[3]
paths = []
try:
    if scanner in ("semgrep", "gosec"):
        with open(os.path.join(run, "out.sarif")) as fh:
            doc = json.load(fh)
        for r in doc.get("runs", []):
            for res in r.get("results", []):
                for loc in res.get("locations", []):
                    uri = loc.get("physicalLocation", {}).get("artifactLocation", {}).get("uri")
                    if uri:
                        paths.append(uri)
    elif scanner == "osv-scanner":
        with open(os.path.join(run, "stdout")) as fh:
            doc = json.load(fh)
        for r in doc.get("results") or []:
            p = (r.get("source") or {}).get("path")
            if p:
                paths.append(p)
    elif scanner == "codeql":
        with zipfile.ZipFile(os.path.join(run, "db", "src.zip")) as z:
            paths = [n for n in z.namelist() if not n.endswith("/")]
except (OSError, ValueError, zipfile.BadZipFile):
    sys.exit(0)

prefixes = []
for form in (root, os.path.realpath(root)):
    prefixes.append(form.rstrip("/") + "/")
    prefixes.append(form.strip("/") + "/")
order = ("app", "target", "src/target", "targets", "other", "unrooted")
counts = dict((z, 0) for z in order)
for p in paths:
    if p.startswith("file://"):
        p = p[len("file://"):]
    for pre in prefixes:
        if p.startswith(pre):
            p = p[len(pre):]
            break
    while p.startswith("./"):
        p = p[2:]
    # The planted file's own name says its zone; a lockfile's path does.
    m = re.match(r"planted_(app|target|src_target|targets)\.", os.path.basename(p))
    if m:
        zone = m.group(1).replace("_", "/")
    else:
        zone = "other"
        for z in ("src/target", "targets", "target", "app"):
            if p.startswith(z + "/"):
                zone = z
                break
    counts[zone] += 1
    # A path that does not name a file from the fixture root is one Logos
    # could not bind to the project.
    if not os.path.isfile(os.path.join(root, p)):
        counts["unrooted"] += 1
print(" ".join(str(counts[z]) for z in order))
PY
}

# placeholders <text> <run dir> <fixture root>: the text with the harness's own
# paths spelled `<run>`, `<root>` and `<work>`, so no evidence names $WORK.
placeholders() {
    local t="$1"
    t="${t//"$2"/<run>}"
    t="${t//"$3"/<root>}"
    t="${t//"$WORK"/<work>}"
    printf '%s' "$t"
}

# note <stderr file> <run dir> <fixture root>: the first line of a run's
# stderr that explains a verdict — a scanner giving up on the ignore files, or
# failing to load what it was given — with its paths as placeholders.
note() {
    local line
    line="$(grep -m1 -E 'Unable to parse git ignores|internal error|not a git repository|without types' "$1" 2>/dev/null)"
    placeholders "$line" "$2" "$3" | cut -c1-200 | tr '\t' ' '
}

# ------------------------------------------------------------------ runs

TSV_ROWS=""
row() {
    # scanner version fixture mechanism status ignored near_miss hits note argv
    local IFS=$'\t'
    TSV_ROWS="$TSV_ROWS$*"$'\n'
}

# render_argv <run dir> <fixture root> <words…>: the argv with the run's own
# paths replaced by placeholders, so the evidence does not depend on $WORK.
render_argv() {
    local r="$1" root="$2" out="" w
    shift 2
    for w in "$@"; do
        out="$out${out:+ }$(placeholders "$w" "$r" "$root")"
    done
    printf '%s' "$out"
}

run_leg() {
    local s="$1" version="$2" bin="$3" fixture="$4" m="$5"
    local root="$WORK/fixtures/$fixture" r="$WORK/runs/$s/$fixture/$m"
    local words=() envs=() rc=0 line counts status ignored near hits
    rm -rf "$r" && mkdir -p "$r"
    prepare_run "$s" "$m" "$r"
    while IFS= read -r line; do words+=("$line"); done <<EOF
$(argv "$s" "$m" "$r")
EOF
    while IFS= read -r line; do [ -n "$line" ] && envs+=("$line"); done <<EOF
$(probe_env "$s" "$m")
EOF
    # stdin is /dev/null: the caller's loop feeds the mechanism list on stdin,
    # and a scanner that read it would end the loop without a row.
    (cd "$root" && env ${envs[@]+"${envs[@]}"} perl "$BOUNDED" "$TIMEOUT" "$bin" "${words[@]}" \
        </dev/null >"$r/stdout" 2>"$r/stderr") || rc=$?

    hits="-"
    case " $(ok_exit "$s") " in
        *" $rc "*) counts="$(tally "$s" "$r" "$root")" ;;
        *) counts="" ;;
    esac
    if [ -z "$counts" ]; then
        status="failed (exit $rc)"
        line="$(awk 'NF { l = $0 } END { print l }' "$r/stderr")"
        line="$(placeholders "$line" "$r" "$root" | cut -c1-200 | tr '\t' ' ')"
        [ -n "$line" ] && status="$status: $line"
        ignored=failed
        near=failed
    else
        set -- $counts
        hits="app=$1 target=$2 src/target=$3 targets=$4 other=$5 unrooted=$6"
        if [ "$1" -eq 0 ]; then
            status="ran (exit $rc)"
            ignored=inconclusive
            near=inconclusive
        else
            status="ran (exit $rc)"
            if [ "$2" -gt 0 ]; then ignored="ignored scanned"; else ignored="ignored skipped"; fi
            if [ "$3" -gt 0 ] && [ "$4" -gt 0 ]; then
                near="near miss kept"
            else
                near="near miss dropped ("
                [ "$3" -eq 0 ] && near="${near}src/target"
                [ "$3" -eq 0 ] && [ "$4" -eq 0 ] && near="$near, "
                [ "$4" -eq 0 ] && near="${near}targets"
                near="$near)"
            fi
        fi
    fi
    row "$s" "$version" "$fixture" "$m" "$status" "$ignored" "$near" "$hits" \
        "$(note "$r/stderr" "$r" "$root")" "$(render_argv "$r" "$root" "${words[@]}")"
    printf '  %-12s %-9s %-21s %-16s %s\n' "$s" "$fixture" "$m" "$ignored" "$near"
}

FIXTURES="git worktree non-git"
VERSIONS=""

echo "verify-scanner-exclusions.sh"
echo "  tools $TOOLS"
echo "  work  $WORK"
echo "  out   $OUT"
echo

build_fixtures

for s in $SCANNERS; do
    found="$(resolve "$s")" || exit 2
    if [ -z "$found" ]; then
        VERSIONS="$VERSIONS$s"$'\t'"not installed"$'\t'"-"$'\n'
        for f in $FIXTURES; do
            row "$s" "-" "$f" "-" "not installed" "not installed" "not installed" "-" "" "-"
            printf '  %-12s %-9s %s\n' "$s" "$f" "not installed"
        done
        continue
    fi
    version="${found%%$'\t'*}"
    pinned="${found##*$'\t'}"
    bin="${found#*$'\t'}"
    bin="${bin%$'\t'*}"
    reports="$(reported_version "$s" "$bin" | tr '\t' ' ')"
    VERSIONS="$VERSIONS$s"$'\t'"$version"$'\t'"${reports:--}"$'\t'"$pinned"$'\n'
    for f in $FIXTURES; do
        while IFS='|' read -r m _; do
            run_leg "$s" "$version" "$bin" "$f" "$m"
        done <<EOF
$(mechanisms "$s")
EOF
    done
done

# ------------------------------------------------------------------ evidence

{
    printf 'scanner\tversion\tfixture\tmechanism\tstatus\tignored\tnear_miss\thits\tnote\targv\n'
    printf '%s' "$TSV_ROWS"
} >"$OUT/verdicts.tsv"

GO_VERSION="$(go version 2>/dev/null || echo 'not installed')"
GIT_VERSION="$(git --version)"
HOST="$(uname -sm)"
DATE="$(date -u +%Y-%m-%d)"

printf '%s' "$VERSIONS" >"$WORK/versions.tsv"
for s in $SCANNERS; do mechanisms "$s" | sed "s/^/$s|/"; done >"$WORK/mechanisms.txt"
python3 - "$OUT/verdicts.tsv" "$OUT/evidence.md" "$WORK/versions.tsv" "$WORK/mechanisms.txt" "$DATE" \
    "$HOST" "$GIT_VERSION" "$GO_VERSION" "$SCANNERS" <<'PY'
import os, sys
tsv, md, vers, mechs, date, host, gitv, gov, scanners = sys.argv[1:10]
with open(vers) as fh:
    versions = [l.split("\t") for l in fh.read().splitlines() if l.strip()]
what = {}
with open(mechs) as fh:
    for l in fh.read().splitlines():
        parts = l.split("|", 2)
        if len(parts) == 3:
            what[(parts[0], parts[1])] = parts[2]
with open(tsv) as fh:
    lines = fh.read().splitlines()
head = lines[0].split("\t")
rows = [dict(zip(head, l.split("\t"))) for l in lines[1:] if l]

probe = {
    "semgrep": "SARIF result URIs from the registry argv, with a local one-rule config "
               "(`eval(...)` in Python) in place of `p/default`, which needs the network.",
    "osv-scanner": "the lockfiles it scanned (`results[].source.path` of `--format json "
                   "--offline --all-packages`, in place of "
                   "`--format sarif`): path selection without an advisory and without the network.",
    "gosec": "SARIF result URIs (a planted G101 hardcoded credential) from the registry argv.",
    "codeql": "the files `database create` extracted (its `src.zip`), which bounds what any "
              "query can report.",
}

out = []
w = out.append
w("# Scanner exclusion evidence")
w("")
w("Written by `bash scripts/verify-scanner-exclusions.sh` (S-771). Hand-run only: no "
  "`gate.sh` tier and no workflow runs it. `verdicts.tsv` beside this file holds every row; "
  "re-running the harness on the same versions reproduces it.")
w("")
w("- Run: " + date + " on " + host)
w("- " + gitv + "; " + gov)
w("- Scanners asked for: " + scanners.replace(" ", ", "))
w("- Network: none needed — every probe reads path selection offline.")
w("")
w("## Fixtures")
w("")
w("Each fixture has the `.gitignore` line `/target/` and plants one finding of each "
  "scanner's kind in `app/` (the control), `target/` and `target/nested/` (ignored; "
  "an exclusion that matches `target` alone still reads the second), and the near "
  "misses `src/target/` and `targets/`.")
w("")
w("| Fixture | What it is |")
w("|---|---|")
w("| git | a git checkout, everything committed except the ignored `target/` |")
w("| worktree | a linked worktree of that checkout (`.git` is a file) |")
w("| non-git | a plain copy with the same `.gitignore`, no repository above it |")
w("")
w("A leg whose control is not reported is `inconclusive`; one that exits outside its "
  "success set is `failed`; a scanner that is not installed is `not installed` and "
  "runs nothing. None of the three is a pass.")
w("")
w("`Hits` counts the reported paths per zone (`other`: in none of the four). "
  "`unrooted` counts those that do not name a file relative to the fixture root — "
  "a path Logos could not bind to the project as reported; the planted file's "
  "zone-bearing name still places it.")
w("")
w("## Versions")
w("")
w("A run on a version other than the manifest's pin is no evidence for the pin; it "
  "is marked **not the pin** (re-run after `logos security install --update`).")
w("")
w("| Scanner | Managed version | Pinned | Reports |")
w("|---|---|---|---|")
for v in versions:
    v = (v + ["-", "-", "-"])[:4]
    ran = v[1]
    if v[3] != "-" and v[1] != v[3]:
        ran = v[1] + " — **not the pin**"
    w("| " + v[0] + " | " + ran + " | " + v[3] + " | `" + v[2].replace("|", "\\|") + "` |")
w("")
for v in versions:
    name = v[0]
    w("## " + name)
    w("")
    mine = [r for r in rows if r["scanner"] == name]
    if v[1] == "not installed":
        w("Not installed under the tools directory: every fixture's leg was skipped. "
          "No verdict is recorded for it.")
        w("")
        continue
    w("Probe: " + probe.get(name, "-"))
    w("")
    w("| Fixture | Mechanism | Ignored | Near miss | Hits | Status |")
    w("|---|---|---|---|---|---|")
    for r in mine:
        w("| " + " | ".join([r["fixture"], "`" + r["mechanism"] + "`", r["ignored"],
                               r["near_miss"], r["hits"], r["status"].replace("|", "\\|")]) + " |")
    notes = []
    for r in mine:
        if r.get("note") and r["note"] not in [n for _, n in notes]:
            notes.append((r["fixture"] + " / " + r["mechanism"], r["note"]))
    if notes:
        w("")
        w("Diagnostics the scanner printed (first occurrence of each):")
        w("")
        for where, n in notes:
            w("- " + where + ": `" + n.replace("`", "'") + "`")
    w("")
    w("Argv per mechanism (`<root>` is the fixture, the child's working directory; "
      "`<run>` the harness's output directory):")
    w("")
    seen = set()
    for r in mine:
        if r["mechanism"] in seen:
            continue
        seen.add(r["mechanism"])
        w("- `" + r["mechanism"] + "` — " + what.get((name, r["mechanism"]), "-") + ": `"
          + name + " " + r["argv"] + "`")
    w("")
with open(md, "w") as fh:
    fh.write("\n".join(out))
PY

echo
echo "wrote $OUT/verdicts.tsv ($(($(wc -l <"$OUT/verdicts.tsv") - 1)) rows) and $OUT/evidence.md"
exit 0
