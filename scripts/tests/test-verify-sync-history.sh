#!/usr/bin/env bash
#
# test-verify-sync-history.sh — prove scripts/verify-sync-history.sh replays a
# range the way UAT-SY-08 describes, reports what it saw, never touches the
# original checkout's store, stays inside its time budget (S-753), and that
# nothing gated calls it.
#
# No real logos runs here: a real-history replay takes minutes. `--logos`
# points the harness at a Python stub ($WORK/stub/logos) over a tiny model —
# each `*.src` file's `def NAME` lines are nodes, its `call NAME` lines are
# edges from the file's module node to every def of that name — kept in a
# SQLite store with the four tables (and the column names) the harness's edge
# comparison reads. STUB_MODE picks its behaviour:
#   ok          sync re-derives the whole graph: always equal to a fresh index;
#   lose        sync re-extracts only the paths it is given, dropping the
#               inbound edges from files it did not re-extract (CR-236's defect);
#   lie         `lose`, and `verify` reports ok regardless — only the cold
#               comparison can see the drift;
#   phantom     sync is correct, but `verify` reports a lost edge — only
#               `verify` sees a drift;
#   surplus     sync keeps one edge a fresh index does not have, and `verify`
#               reports ok regardless — only the cold comparison's `extra`
#               side can see it;
#   field-KEY   sync is correct and `verify` exits 0 with a clean report but
#               for one field: `ok` false, or KEY (a delta or total) at 1;
#   lose-then-fail  `lose`, but a sync of `d.src` exits 5;
#   fail-sync   sync exits 5;  no-seed  the first open seeds no store;
#   fallback    the first open indexes afresh instead of seeding;
#   verify-crash  verify exits 7;  garbage  verify prints no JSON, exit 0;
#   corrupt-cold  the cold clone's index writes a store that is not SQLite;
#   hook        index installs a git `post-checkout` hook, as `logos init`
#               does, that appends to $HOOK_LOG each time it fires;
#   dirty       sync leaves an untracked `d.src` behind, which stops the
#               checkout of the commit that adds it;
#   slow        index sleeps 60s;  slow-version  `--version` sleeps 60s.
# Every stub call logs its working directory, argv, its own path (argv[0]) and
# whether the clone held the project's config to $STUB_LOG.
#
# What is checked (the denominator is the last line, `passed N, failed M`):
#   - nothing gated (gate.sh, the scripts it sources, any workflow) names the
#     harness or globs `scripts/*.sh` / `scripts/verify-*`, the files that check
#     reads exist, and it fires on a fixture that calls it by name and by glob;
#   - --list prints exactly the three pinned Sprint 106 ranges, and where this
#     repository holds them each is a linear, ancestral range;
#   - ok: EQUIVALENT, exit 0; the step order (index the primary, open the
#     worktree, one sync per non-empty commit with exactly the paths it
#     changed, after `--` — a path starting with `-` stays a path, a rename
#     syncs both, a deletion of a path with a space and a non-ASCII letter
#     syncs it as given, an empty commit syncs nothing —
#     verify, index the cold clone);
#     every call ran the binary copied into the work directory, in a clone
#     under it, with the config copied; the source's store bytes are
#     unchanged; the default work directory is removed and a given one kept;
#   - lose: DRIFT, exit 1, the lost edge named by verify and by the cold
#     comparison; lie: DRIFT on the cold comparison alone; surplus: DRIFT on
#     the cold comparison's extra side, the edge named; phantom: DRIFT on
#     verify alone; each of verify's six verdict fields is a DRIFT alone, even
#     when verify exits 0;
#   - a post-checkout hook installed in the primary never fires;
#   - two ranges are each replayed and counted, and a FAILED range outranks a
#     DRIFT one (exit 3);
#   - a repository that tracks `.logos/rules.toml` replays with its own copy;
#   - fail-sync, no-seed, fallback, verify-crash, garbage, corrupt-cold and
#     dirty are FAILED, exit 3, never a verdict; a store present before the
#     first open is FAILED too;
#   - slow (an index) and slow-version (the version probe) with --timeout 3:
#     exit 4, well within 20s, no stub left running;
#   - usage errors exit 2: unknown argument, bad --timeout, missing binary, a
#     malformed or duplicated range, a base that is not a commit, not an
#     ancestor, an empty range, a range holding a merge, and a work directory
#     inside a repository.
#
# Usage: bash scripts/tests/test-verify-sync-history.sh
# Exit:  0 every check held, 1 one did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
HARNESS="$HERE/scripts/verify-sync-history.sh"
[ -f "$HARNESS" ] || { echo "cannot find scripts/verify-sync-history.sh (looked in $HERE)" >&2; exit 1; }
# The harness must run under the bash macOS ships (3.2), as a maintainer would.
BASH_BIN=/bin/bash
[ -x "$BASH_BIN" ] || BASH_BIN="$(command -v bash)"

WORK="${TMPDIR:-/tmp}/logos-verify-sync-history-test.$$"
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
# expect <name> <wanted> <got> [<context>]
expect() { if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$3', wanted '$2'" "${4:-}"; fi; }
# has <name> <needle> <haystack>: a fixed-string line match.
has() { if printf '%s\n' "$3" | grep -qF -- "$2"; then ok "$1"; else bad "$1: no line with '$2'" "$3"; fi; }
lacks() { if printf '%s\n' "$3" | grep -qF -- "$2"; then bad "$1: a line with '$2'" "$3"; else ok "$1"; fi; }

G=(git -c maintenance.auto=false -c gc.auto=0 -c init.defaultBranch=main -c core.hooksPath=/dev/null
    -c user.email=harness@example.invalid -c user.name=harness -c commit.gpgsign=false)

# ------------------------------------------------------------------ stub logos

mkdir -p "$WORK/stub"
STUB="$WORK/stub/logos"
cat >"$STUB" <<'PY'
#!/usr/bin/env python3
"""A stand-in for `logos` over a tiny model; see the suite's header."""
import json, os, shutil, sqlite3, subprocess, sys, time

MODE = os.environ.get("STUB_MODE", "ok")
ROOT = os.getcwd()
DB = os.path.join(ROOT, ".logos", "logos.db")
args = sys.argv[1:]

if os.environ.get("STUB_LOG"):
    with open(os.environ["STUB_LOG"], "a") as fh:
        cfg = "yes" if os.path.isfile(os.path.join(ROOT, ".logos", "config.toml")) else "no"
        fh.write("%s\t%s\targv0=%s\tcfg=%s\n" % (ROOT, "\x1f".join(args), sys.argv[0], cfg))

def mod(p): return "stub . %s/" % p
def fn(p, n): return "stub . %s/%s()." % (p, n)

def tree():
    out = []
    for d, dirs, files in os.walk(ROOT):
        dirs[:] = [x for x in dirs if x not in (".git", ".logos")]
        out += [os.path.relpath(os.path.join(d, f), ROOT) for f in files if f.endswith(".src")]
    return sorted(out)

def parse(p):
    defs, calls = [], []
    with open(os.path.join(ROOT, p)) as fh:
        for line in fh:
            w = line.split()
            if len(w) == 2 and w[0] == "def": defs.append(w[1])
            if len(w) == 2 and w[0] == "call": calls.append(w[1])
    return defs, calls

def add_file(nodes, p):
    defs, _ = parse(p)
    nodes[mod(p)] = p
    for n in defs: nodes[fn(p, n)] = p

def add_edges(nodes, edges, p):
    _, calls = parse(p)
    for n in calls:
        for s, q in nodes.items():
            if s == fn(q, n): edges.add((2, mod(p), s))

def fresh():
    nodes, edges = {}, set()
    for p in tree(): add_file(nodes, p)
    for p in tree(): add_edges(nodes, edges, p)
    return nodes, edges

def write(nodes, edges):
    os.makedirs(os.path.dirname(DB), exist_ok=True)
    if os.path.exists(DB): os.remove(DB)
    c = sqlite3.connect(DB)
    c.executescript("CREATE TABLE files (id INTEGER PRIMARY KEY, path TEXT);"
                    "CREATE TABLE symbols (id INTEGER PRIMARY KEY, symbol TEXT);"
                    "CREATE TABLE nodes (id INTEGER PRIMARY KEY, symbol_id INTEGER, file_id INTEGER);"
                    "CREATE TABLE edges (id INTEGER PRIMARY KEY, source INTEGER, target INTEGER, kind INTEGER);")
    fid, nid = {}, {}
    for p in sorted(set(nodes.values())):
        fid[p] = c.execute("INSERT INTO files (path) VALUES (?)", (p,)).lastrowid
    for s in sorted(nodes):
        sid = c.execute("INSERT INTO symbols (symbol) VALUES (?)", (s,)).lastrowid
        nid[s] = c.execute("INSERT INTO nodes (symbol_id, file_id) VALUES (?, ?)", (sid, fid[nodes[s]])).lastrowid
    for k, s, t in sorted(edges):
        c.execute("INSERT INTO edges (source, target, kind) VALUES (?, ?, ?)", (nid[s], nid[t], k))
    c.commit()
    c.close()

def read():
    c = sqlite3.connect(DB)
    nodes = dict(c.execute("SELECT s.symbol, f.path FROM nodes n JOIN symbols s ON s.id = n.symbol_id "
                           "JOIN files f ON f.id = n.file_id"))
    edges = set(c.execute("SELECT e.kind, ss.symbol, ts.symbol FROM edges e "
                          "JOIN nodes sn ON sn.id = e.source JOIN symbols ss ON ss.id = sn.symbol_id "
                          "JOIN nodes tn ON tn.id = e.target JOIN symbols ts ON ts.id = tn.symbol_id"))
    c.close()
    return nodes, edges

cmd = args[0] if args else ""
if cmd == "--version":
    if MODE == "slow-version": time.sleep(60)
    print("logos 0.0.0-stub")
elif cmd == "index":
    if MODE == "slow": time.sleep(60)
    write(*fresh())
    if MODE == "hook":
        common = subprocess.check_output(["git", "rev-parse", "--path-format=absolute",
                                          "--git-common-dir"], text=True).strip()
        hook = os.path.join(common, "hooks", "post-checkout")
        os.makedirs(os.path.dirname(hook), exist_ok=True)
        with open(hook, "w") as fh:
            fh.write("#!/bin/sh\necho fired >>'%s'\n" % os.environ["HOOK_LOG"])
        os.chmod(hook, 0o755)
    if MODE == "corrupt-cold" and os.path.basename(ROOT) == "cold":
        with open(DB, "w") as fh:
            fh.write("not a database\n")
elif cmd == "status":
    if not os.path.exists(DB) and MODE == "fallback":
        write(*fresh())
    elif not os.path.exists(DB) and MODE != "no-seed":
        common = subprocess.check_output(["git", "rev-parse", "--path-format=absolute",
                                          "--git-common-dir"], text=True).strip()
        primary = os.path.join(os.path.dirname(common), ".logos", "logos.db")
        os.makedirs(os.path.dirname(DB), exist_ok=True)
        shutil.copy(primary, DB)
        # Like the engine: an `info` line, hidden under the default filter.
        if "logos_core::engine=info" in os.environ.get("RUST_LOG", ""):
            sys.stderr.write("INFO logos_core::engine: seeded the worktree store "
                             "from the primary checkout (ADR-15)\n")
    print(json.dumps({"indexed": os.path.exists(DB)}))
elif cmd == "sync":
    # Like clap: an argument that starts with `-` before `--` is a flag.
    rest = args[1:]
    if "--" in rest:
        flags, paths = rest[:rest.index("--")], rest[rest.index("--") + 1:]
    else:
        flags, paths = rest, rest
    if any(a.startswith("-") for a in flags):
        sys.stderr.write("stub: unexpected argument %r\n" % [a for a in flags if a.startswith("-")][0])
        sys.exit(2)
    if MODE == "dirty":
        with open(os.path.join(ROOT, "d.src"), "w") as fh:
            fh.write("def stray\n")
    if MODE == "fail-sync" or (MODE == "lose-then-fail" and "d.src" in paths):
        sys.stderr.write("stub: sync failed\n")
        sys.exit(5)
    if MODE in ("lose", "lie", "lose-then-fail"):
        nodes, edges = read()
        gone = set(s for s, p in nodes.items() if p in paths)
        nodes = dict((s, p) for s, p in nodes.items() if s not in gone)
        edges = set(e for e in edges if e[1] not in gone and e[2] not in gone)
        for p in paths:
            if os.path.exists(os.path.join(ROOT, p)): add_file(nodes, p)
        for p in paths:
            if os.path.exists(os.path.join(ROOT, p)): add_edges(nodes, edges, p)
        write(nodes, edges)
    elif MODE == "surplus":
        nodes, edges = fresh()
        if mod("a.src") in nodes and fn("b.src", "beta") in nodes:
            edges.add((2, mod("a.src"), fn("b.src", "beta")))
        write(nodes, edges)
    else:
        write(*fresh())
elif cmd == "verify":
    if MODE == "verify-crash": sys.exit(7)
    if MODE == "garbage":
        print("this is not json")
        sys.exit(0)
    (ln, le), (fn_, fe) = read(), fresh()
    def rows(keys, nodes):
        return [{"kind": "calls", "source_symbol": s, "target_symbol": t,
                 "source_file": nodes.get(s), "target_file": nodes.get(t)} for _, s, t in sorted(keys)]
    lost, extra = fe - le, le - fe
    r = {"node_delta": len(ln) - len(fn_), "edge_delta": len(le) - len(fe),
         "file_delta": len(set(ln.values())) - len(set(fn_.values())),
         "lost_edges_total": len(lost), "lost_edges": rows(lost, fn_),
         "extra_edges_total": len(extra), "extra_edges": rows(extra, ln)}
    r["ok"] = not (lost or extra or r["node_delta"] or r["edge_delta"] or r["file_delta"])
    if MODE == "phantom":
        lost = {(2, mod("ghost.src"), fn("a.src", "alpha"))}
        r.update(ok=False, lost_edges_total=1, lost_edges=rows(lost, fn_))
    if MODE.startswith("field-"):
        key = MODE[len("field-"):]
        r[key] = False if key == "ok" else 1
        print(json.dumps(r))
        sys.exit(0)
    if MODE in ("lie", "surplus"):
        r.update(ok=True, node_delta=0, edge_delta=0, file_delta=0,
                 lost_edges_total=0, lost_edges=[], extra_edges_total=0, extra_edges=[])
    print(json.dumps(r))
    sys.exit(0 if r["ok"] else 1)
else:
    sys.stderr.write("stub: unknown command %r\n" % args)
    sys.exit(2)
PY
chmod +x "$STUB"

# ------------------------------------------------------------------ fixture

# The source repository: a base and four commits — an edit to the file others
# call into (with a new file whose name starts with `-`), a rename, a deletion
# of a path with a space and a non-ASCII letter (which `git diff-tree` quotes
# unless it runs with -z), an empty commit.
SRC="$WORK/src"
mkdir -p "$SRC/.logos"
(
    cd "$SRC" || exit 1
    "${G[@]}" init -q .
    printf '.logos/\n' >.gitignore
    printf 'def alpha\n' >a.src
    printf 'call alpha\ndef beta\n' >b.src
    printf 'call beta\n' >c.src
    printf 'call alpha\n' >'sp ace ü.src'
    "${G[@]}" add -A && "${G[@]}" commit -q -m base
    "${G[@]}" tag base
    printf 'def alpha\ndef gamma\n' >a.src
    printf 'def delta\n' >-dash.src
    "${G[@]}" add -A && "${G[@]}" commit -q -m 'edit a'
    "${G[@]}" tag c1
    "${G[@]}" mv c.src d.src && "${G[@]}" commit -q -m 'rename c'
    "${G[@]}" rm -q 'sp ace ü.src' && "${G[@]}" commit -q -m 'delete sp ace'
    "${G[@]}" commit -q --allow-empty -m empty
    "${G[@]}" tag tip
) || { echo "cannot build the fixture repository" >&2; exit 1; }
printf '[index]\n' >"$SRC/.logos/config.toml"
printf '# rules\n' >"$SRC/.logos/rules.toml"
printf 'SENTINEL store: the harness must never open this\n' >"$SRC/.logos/logos.db"
SENTINEL_SUM="$(cksum <"$SRC/.logos/logos.db")"
RANGE="r=base..tip"

# harness [args…] -> OUT (combined output), RC. STUB_MODE comes from the caller.
harness() {
    OUT="$("$BASH_BIN" "$HARNESS" --logos "$STUB" --repo "$SRC" "$@" 2>&1)"
    RC=$?
}

# ------------------------------------------------------------------ not gated

echo "nothing gated calls the harness"
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
    } | sort -u | grep -v -E '/scripts/(verify-sync-history|tests/test-verify-sync-history)\.sh$'
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
            scripts/gate.sh|.github/workflows/*) pattern='verify-sync-history|scripts/\*\.sh|scripts/verify-\*' ;;
            *) pattern='verify-sync-history' ;;
        esac
        grep -n -E "$pattern" "$f" | grep -v -E '^[0-9]+:[[:space:]]*#' | sed "s|^|${f#"$root"/}:|"
    done
}
expect "no gate tier, gate library or workflow names or globs it" "" "$(callers "$HERE")"
if [ -f "$HERE/scripts/gate.sh" ] && ls "$HERE"/.github/workflows/*.yml >/dev/null 2>&1; then
    ok "the files that check reads exist (scripts/gate.sh, .github/workflows/*.yml)"
else
    bad "scripts/gate.sh or .github/workflows/*.yml is missing: the check above read nothing"
fi
# The check must fire on a tree that does call it, by name or by glob.
NEG="$WORK/negative"
mkdir -p "$NEG/scripts" "$NEG/.github/workflows"
printf '#!/usr/bin/env bash\nbash scripts/verify-sync-history.sh\n' >"$NEG/scripts/gate.sh"
printf 'jobs:\n  x:\n    steps:\n      - run: for s in scripts/verify-*; do bash "$s"; done\n' >"$NEG/.github/workflows/ci.yml"
NEG2="$WORK/negative-indirect"
mkdir -p "$NEG2/scripts/tests" "$NEG2/.github/workflows"
printf '#!/usr/bin/env bash\nbash scripts/check-public-terms.sh\n' >"$NEG2/scripts/gate.sh"
printf '#!/usr/bin/env bash\n# verify-sync-history is named here only in a comment\nbash scripts/verify-sync-history.sh\n' >"$NEG2/scripts/check-public-terms.sh"
printf '#!/usr/bin/env bash\nbash scripts/verify-sync-history.sh --list\n' >"$NEG2/scripts/tests/test-other.sh"
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

# ------------------------------------------------------------------ pinned ranges

echo "the pinned ranges"
LIST="$("$BASH_BIN" "$HARNESS" --list)"
expect "--list exits 0" 0 $?
expect "three ranges are pinned" 3 "$(printf '%s\n' "$LIST" | grep -c .)"
expect "they are the Sprint 106 replays" "S-681 S-685 S-747" "$(printf '%s\n' "$LIST" | sed 's/=.*//' | tr '\n' ' ' | sed 's/ $//')"
expect "each is two full commit ids" 3 "$(printf '%s\n' "$LIST" | grep -cE '^[A-Z0-9-]+=[0-9a-f]{40}\.\.[0-9a-f]{40}$')"
held=0
while IFS= read -r spec; do
    span="${spec#*=}"
    b="${span%%..*}"
    t="${span#*..}"
    git -C "$HERE" cat-file -e "$t^{commit}" 2>/dev/null || continue
    held=$((held + 1))
    if git -C "$HERE" merge-base --is-ancestor "$b" "$t" \
        && [ "$(git -C "$HERE" rev-list --merges --count "$b..$t")" = 0 ]; then
        ok "${spec%%=*} is a linear, ancestral range here"
    else
        bad "${spec%%=*} is not a linear, ancestral range here"
    fi
done <<EOF
$LIST
EOF
[ "$held" -gt 0 ] || echo "  note  this clone holds none of the pinned commits (a squashed public history)"

# ------------------------------------------------------------------ ok

echo "a correct sync replays to EQUIVALENT"
export STUB_LOG="$WORK/ok.log"
mkdir -p "$WORK/tmp-ok"
STUB_MODE=ok TMPDIR="$WORK/tmp-ok" harness --range "$RANGE"
expect "exit 0" 0 "$RC" "$OUT"
has "the verdict is EQUIVALENT" "verdict: EQUIVALENT" "$OUT"
has "four commits, five paths" "replay:  4 commits, 5 paths synced" "$OUT"
has "verify reads clean" "verify:  exit 0, ok true; node Δ 0, edge Δ 0, file Δ 0; lost 0, extra 0" "$OUT"
has "the cold comparison reads clean" "cold:    2 edges at the tip, 2 incremental; lost 0, extra 0" "$OUT"
has "the summary counts it" "ranges 1: equivalent 1, drift 0, failed 0" "$OUT"
has "the binary's version is named" "(logos 0.0.0-stub)" "$OUT"
expect "the steps run in UAT-SY-08's order" "--version index status sync sync sync verify index" \
    "$(cut -f2 "$STUB_LOG" | awk -F"$(printf '\037')" '{print $1}' | tr '\n' ' ' | sed 's/ $//')"
SYNCS="$(grep -F "$(printf '\tsync\037')" "$STUB_LOG" | cut -f2 | sed 's/^sync.//' | tr '\037' '|')"
expect "each sync carries exactly its commit's paths, after --" "--|-dash.src|a.src
--|c.src|d.src
--|sp ace ü.src" "$SYNCS"
DIRS="$(cut -f1 "$STUB_LOG" | sed -n '2,$p' | sed 's|.*/r/||' | tr '\n' ' ' | sed 's/ $//')"
expect "each call runs in its clone" "primary worktree worktree worktree worktree worktree cold" "$DIRS"
expect "no call runs outside the work directory" "" \
    "$(cut -f1 "$STUB_LOG" | grep -v "^$WORK/tmp-ok/logos-sync-history\.[0-9]*\(/ranges/r/\|$\)")"
expect "every call runs the binary copied into the work directory" "" \
    "$(cut -f3 "$STUB_LOG" | grep -v "^argv0=$WORK/tmp-ok/logos-sync-history\.[0-9]*/bin/logos$")"
expect "every clone call holds the project's config" "" "$(sed -n '2,$p' "$STUB_LOG" | grep -v 'cfg=yes$')"
expect "the source store's bytes are unchanged" "$SENTINEL_SUM" "$(cksum <"$SRC/.logos/logos.db")"
expect "the source's .logos holds only what it held" "config.toml logos.db rules.toml" \
    "$(ls "$SRC/.logos" | tr '\n' ' ' | sed 's/ $//')"
expect "the default work directory is removed" "" "$(ls "$WORK/tmp-ok")"

KEEP="$WORK/kept"
: >"$STUB_LOG"
STUB_MODE=ok harness --range "$RANGE" --work "$KEEP"
expect "a given work directory is kept" "yes" "$([ -s "$KEEP/ranges/r/out/verify.json" ] && echo yes)"
STUB_MODE=ok harness --range "$RANGE" --work "$KEEP"
expect "a kept work directory can be reused" 0 "$RC" "$OUT"

# ------------------------------------------------------------------ drift

echo "a sync that loses inbound edges replays to DRIFT"
export STUB_LOG="$WORK/lose.log"
STUB_MODE=lose harness --range "$RANGE"
expect "exit 1" 1 "$RC" "$OUT"
has "the verdict is DRIFT" "verdict: DRIFT" "$OUT"
has "verify counts the lost edge" "verify:  exit 1, ok false; node Δ 0, edge Δ -1, file Δ 0; lost 1, extra 0" "$OUT"
has "verify names it" "lost calls b.src -> a.src | stub . b.src/ -> stub . a.src/alpha()." "$OUT"
has "the cold comparison counts it" "cold:    2 edges at the tip, 1 incremental; lost 1, extra 0" "$OUT"
has "the cold comparison names it" "lost kind=2 b.src -> a.src | stub . b.src/ -> stub . a.src/alpha()." "$OUT"
has "the summary counts it" "ranges 1: equivalent 0, drift 1, failed 0" "$OUT"

echo "a drift verify does not report is still DRIFT"
STUB_MODE=lie harness --range "$RANGE"
expect "exit 1" 1 "$RC" "$OUT"
has "verify reads clean" "verify:  exit 0, ok true;" "$OUT"
has "the cold comparison catches it" "cold:    2 edges at the tip, 1 incremental; lost 1, extra 0" "$OUT"
has "the verdict is DRIFT" "verdict: DRIFT" "$OUT"

echo "a drift only verify reports is still DRIFT"
STUB_MODE=phantom harness --range "$RANGE"
expect "exit 1" 1 "$RC" "$OUT"
has "verify reports it" "verify:  exit 1, ok false; node Δ 0, edge Δ 0, file Δ 0; lost 1, extra 0" "$OUT"
has "the cold comparison reads clean" "cold:    2 edges at the tip, 2 incremental; lost 0, extra 0" "$OUT"
has "the verdict is DRIFT" "verdict: DRIFT" "$OUT"

echo "a work directory whose path holds '#' or '?' still compares two stores"
mkdir -p "$WORK/uri"
STUB_MODE=lie harness --range "$RANGE" --work "$WORK/uri/w#x?y"
expect "exit 1" 1 "$RC" "$OUT"
has "the cold comparison catches the drift" "cold:    2 edges at the tip, 1 incremental; lost 1, extra 0" "$OUT"
expect "nothing is created beside the work directory" "w#x?y" "$(ls "$WORK/uri")"

echo "no git hook fires in the clones"
export HOOK_LOG="$WORK/hook.log"
STUB_MODE=hook harness --range "$RANGE"
expect "the replay runs: exit 0" 0 "$RC" "$OUT"
expect "an installed post-checkout hook never fires" 0 "$(cat "$HOOK_LOG" 2>/dev/null | wc -l | tr -d ' ')"

echo "an edge only the incremental store has is DRIFT"
STUB_MODE=surplus harness --range "$RANGE"
expect "exit 1" 1 "$RC" "$OUT"
has "the cold comparison counts it as extra" "cold:    2 edges at the tip, 3 incremental; lost 0, extra 1" "$OUT"
has "and names it" "extra kind=2 a.src -> b.src | stub . a.src/ -> stub . b.src/beta()." "$OUT"
has "the verdict is DRIFT" "verdict: DRIFT" "$OUT"

echo "each verify field decides a drift alone, whatever verify's exit"
for key in ok node_delta edge_delta file_delta lost_edges_total extra_edges_total; do
    STUB_MODE=field-$key harness --range "$RANGE"
    expect "$key off: exit 1" 1 "$RC" "$OUT"
    has "$key off: verify exited 0" "verify:  exit 0," "$OUT"
    has "$key off: DRIFT" "verdict: DRIFT" "$OUT"
done

echo "two ranges are each replayed"
STUB_MODE=ok harness --range one=base..c1 --range two=c1..tip
expect "exit 0" 0 "$RC" "$OUT"
has "the first is replayed" "replay:  1 commits, 2 paths synced" "$OUT"
has "the second is replayed" "replay:  3 commits, 3 paths synced" "$OUT"
has "both are counted" "ranges 2: equivalent 2, drift 0, failed 0" "$OUT"
STUB_MODE=lose harness --range one=base..c1 --range two=c1..tip
has "a drift in one range leaves the other's verdict" "ranges 2: equivalent 1, drift 1, failed 0" "$OUT"
STUB_MODE=lose-then-fail harness --range one=base..c1 --range two=c1..tip
expect "a failed range outranks a drifted one: exit 3" 3 "$RC" "$OUT"
has "both are counted" "ranges 2: equivalent 0, drift 1, failed 1" "$OUT"

# ------------------------------------------------------------------ failed

echo "a step that cannot run is FAILED, never a verdict"
for mode in fail-sync no-seed fallback verify-crash garbage corrupt-cold dirty; do
    STUB_MODE=$mode harness --range "$RANGE"
    expect "$mode exits 3" 3 "$RC" "$OUT"
    has "$mode is counted as failed" "ranges 1: equivalent 0, drift 0, failed 1" "$OUT"
    lacks "$mode reaches no verdict but FAILED" "verdict: EQUIVALENT" "$OUT"
done
STUB_MODE=fail-sync harness --range "$RANGE"
has "a failed sync names its commit" "FAILED: logos sync of commit 1" "$OUT"
has "and shows what logos said" "stub: sync failed" "$OUT"
STUB_MODE=no-seed harness --range "$RANGE"
has "an unseeded worktree is named" "FAILED: the first open did not seed a worktree store" "$OUT"
STUB_MODE=fallback harness --range "$RANGE"
has "a store built without the seed is named" "FAILED: the first open built a store without seeding it from the primary" "$OUT"
STUB_MODE=garbage harness --range "$RANGE"
has "an unreadable verify report is named" "FAILED: logos verify wrote no readable report" "$OUT"
STUB_MODE=corrupt-cold harness --range "$RANGE"
has "an unreadable store is named" "FAILED: cannot read the stores (unreadable" "$OUT"
STUB_MODE=dirty harness --range "$RANGE"
has "a checkout that cannot run is named" "FAILED: cannot check out" "$OUT"
has "with git's reason" "untracked working tree files would be overwritten" "$OUT"
# A repository that commits its store hands the worktree one at checkout, so
# the first open seeds nothing and the replay would prove nothing.
SRC2="$WORK/src-with-store"
(
    mkdir -p "$SRC2/.logos" && cd "$SRC2" || exit 1
    "${G[@]}" init -q .
    printf 'def alpha\n' >a.src
    printf 'committed\n' >.logos/logos.db
    "${G[@]}" add -A && "${G[@]}" commit -q -m base && "${G[@]}" tag base
    printf 'def beta\n' >a.src && "${G[@]}" commit -q -am edit && "${G[@]}" tag tip
) || bad "cannot build the committed-store fixture"
STUB_MODE=ok harness --repo "$SRC2" --range "$RANGE"
expect "a worktree with a store before its first open exits 3" 3 "$RC" "$OUT"
has "and is named" "FAILED: the worktree had a store before its first open" "$OUT"

# A repository that tracks its rules: the checkout's own (edited) copy must not
# be copied over the tracked one, or the next checkout stops.
SRC3="$WORK/src-tracked-rules"
(
    mkdir -p "$SRC3/.logos" && cd "$SRC3" || exit 1
    "${G[@]}" init -q .
    printf '.logos/logos.db\n' >.gitignore
    printf 'def alpha\n' >a.src
    printf '# rules v1\n' >.logos/rules.toml
    "${G[@]}" add -A && "${G[@]}" commit -q -m base && "${G[@]}" tag base
    printf '# rules v2\n' >.logos/rules.toml
    printf 'def beta\n' >a.src && "${G[@]}" commit -q -am edit && "${G[@]}" tag tip
    printf '# rules, edited and uncommitted\n' >.logos/rules.toml
) || bad "cannot build the tracked-rules fixture"
STUB_MODE=ok harness --repo "$SRC3" --range "$RANGE"
expect "a tracked rules.toml is left to the checkout: exit 0" 0 "$RC" "$OUT"
has "and the replay reaches its verdict" "verdict: EQUIVALENT" "$OUT"

# ------------------------------------------------------------------ time bound

echo "the run is bounded in time"
export STUB_LOG="$WORK/slow.log"
t0=$SECONDS
STUB_MODE=slow harness --range "$RANGE" --timeout 3 --work "$WORK/slow"
took=$((SECONDS - t0))
expect "exit 4" 4 "$RC" "$OUT"
has "the timeout is named" "TIMEOUT: the 3s budget ran out during range r; no verdict" "$OUT"
lacks "no verdict is printed" "verdict:" "$OUT"
expect "it stops within the budget" yes "$([ "$took" -lt 20 ] && echo yes)" "took ${took}s"
sleep 1
if pgrep -f "$WORK/slow/bin/logos" >/dev/null 2>&1; then
    pkill -f "$WORK/slow/bin/logos"
    bad "no stub survives the timeout"
else
    ok "no stub survives the timeout"
fi

t0=$SECONDS
STUB_MODE=slow-version harness --range "$RANGE" --timeout 3
took=$((SECONDS - t0))
expect "a hanging version probe is bounded too: exit 4" 4 "$RC" "$OUT"
has "and named, before any range" "TIMEOUT: the 3s budget ran out; no verdict" "$OUT"
expect "and stops within 20s of a 3s budget" yes "$([ "$took" -lt 20 ] && echo yes)" "took ${took}s"

# ------------------------------------------------------------------ usage

echo "usage errors exit 2"
export STUB_LOG="$WORK/usage.log"
: >"$STUB_LOG"
usage_case() { # usage_case <name> <stderr needle> <args…>
    local name="$1" needle="$2"
    shift 2
    STUB_MODE=ok harness "$@"
    expect "$name exits 2" 2 "$RC" "$OUT"
    has "$name says why" "$needle" "$OUT"
}
usage_case "an unknown argument" "unknown argument '--nope'" --nope
usage_case "a bad --timeout" "--timeout wants a positive number" --timeout 0 --range "$RANGE"
# A later --logos / --repo overrides the one harness() passes first.
usage_case "a missing binary" "no executable logos binary at '$WORK/no-such-logos'" --logos "$WORK/no-such-logos" --range "$RANGE"
usage_case "a malformed range" "--range wants LABEL=BASE..TIP" --range base..tip
mkdir -p "$WORK/victim" "$WORK/trav/w"
printf 'keep\n' >"$WORK/victim/keep"
for bad in ../victim .. . -r; do
    usage_case "the label '$bad'" "range label '$bad' wants a letter or digit" --range "$bad=base..tip" --work "$WORK/trav/w"
done
expect "no label reaches outside the work directory" keep "$(cat "$WORK/victim/keep" 2>/dev/null)"
usage_case "a duplicated label" "range label 'r' is given twice" --range "$RANGE" --range "$RANGE"
usage_case "a base that is not a commit" "r: base 'nope' is not a commit" --range r=nope..tip
usage_case "a base that is not an ancestor" "is not an ancestor of tip" --range r=tip..base
usage_case "an empty range" "r: the range is empty" --range r=tip..tip
(
    cd "$SRC" || exit 1
    "${G[@]}" checkout -q -b side base
    printf 'def side\n' >e.src && "${G[@]}" add e.src && "${G[@]}" commit -q -m side
    "${G[@]}" checkout -q main
    "${G[@]}" merge -q --no-edit side >/dev/null
    "${G[@]}" tag merged
) || bad "cannot build the merge fixture"
usage_case "a range holding a merge" "holds 1 merge commit(s)" --range r=base..merged
mkdir -p "$SRC/inside"
usage_case "a work directory inside a repository" "is inside a git repository" --range "$RANGE" --work "$SRC/inside"
expect "the stub never ran on a usage error" 0 "$(wc -c <"$STUB_LOG" | tr -d ' ')"

echo
echo "passed $PASS, failed $FAIL"
[ "$FAIL" -eq 0 ]
