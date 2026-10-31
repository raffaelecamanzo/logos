#!/usr/bin/env bash
#
# test-gate-ci-tier.sh — prove `scripts/gate.sh ci` (S-806, NFR-MA-11, CR-251) does
# what its acceptance criteria say, without a real container: `docker` is a shim
# that records its arguments and, for the container run, writes the logs and exit
# statuses scripts/ci-container.sh would have written. Real container behaviour is
# the story's own verification; what is proved here is everything the HOST decides.
#
#   1. it cannot run         Docker missing, its daemon down, a non-arm64 engine, an
#                            image whose Rust is not the local toolchain's: each
#                            exits 2 naming the cause, before any evidence exists
#   2. what it asks docker   the digest-pinned image, a native arm64 platform, the
#                            build volume as the ONLY persistent mount, an empty
#                            cargo home (nothing mounted over it), CI's tolerance,
#                            and the certified tree (untracked in, ignored out)
#   3. what it records       tier "ci" evidence for three gates, binaries expected /
#                            started / finished and tests passed / failed; a failing
#                            test, a container that dies midway and a suite it never
#                            ran are failures, never a pass
#   4. what the verifier     `verify-evidence.sh --tier ci` accepts that evidence;
#      says                  fast/full evidence is refused in its place and ci
#                            evidence is no use to --tier fast or full; no marker
#   5. it matches ci.yml     the tolerance is the workflow's
#
# Runs under /bin/bash 3.2 and bash 5. Usage: bash scripts/tests/test-gate-ci-tier.sh
# Env:   GATE_SH / VERIFY_SH / ENTRY_SH  the scripts under test (default: scripts/)
# Exit:  0 all cases behaved, 1 a case did not

set -uo pipefail

HERE="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
GATE="${GATE_SH:-$HERE/scripts/gate.sh}"
VERIFY="${VERIFY_SH:-$HERE/scripts/verify-evidence.sh}"
ENTRY="${ENTRY_SH:-$HERE/scripts/ci-container.sh}"

WORK="${TMPDIR:-/tmp}/logos-gate-ci-tier-test.$$"
trap 'rm -rf "$WORK"' EXIT INT TERM
mkdir -p "$WORK" || exit 1
# A fixture repository is never inside any enclosing work tree.
export GIT_CEILING_DIRECTORIES="$WORK"

PASS=0
FAIL=0
ok() { echo "  ok    $1"; PASS=$((PASS + 1)); }
bad() { echo "  FAIL  $1"; FAIL=$((FAIL + 1)); }
expect() { # name got want
    if [ "$2" = "$3" ]; then ok "$1"; else bad "$1: got '$2', wanted '$3'"; fi
}
has() { # name haystack needle
    case "$2" in *"$3"*) ok "$1" ;; *) bad "$1: '$3' not in: $2" ;; esac
}
lacks() { # name haystack needle
    case "$2" in *"$3"*) bad "$1: '$3' is in: $2" ;; *) ok "$1" ;; esac
}

for f in "$GATE" "$VERIFY" "$ENTRY"; do
    [ -f "$f" ] || { echo "FAIL  not found: $f"; exit 1; }
done

# --- the machine the gate runs on ---------------------------------------------
#
# MIN holds only the tools gate.sh needs, so a PATH of MIN alone has no `docker`
# whatever this machine installs; SHIM holds the fake one.
MIN="$WORK/min"
SHIM="$WORK/shim"
mkdir -p "$MIN" "$SHIM"
for tool in bash sh git python3 cargo rustc awk sed grep cat cp rm mkdir tr cut head tail sort \
    uniq basename dirname date env tee wc ls tar find chmod mv touch id uname perl expr cmp \
    mktemp readlink pwd hostname ln; do
    src="$(command -v "$tool" 2>/dev/null || true)"
    [ -n "$src" ] && [ -x "$src" ] && ln -sf "$src" "$MIN/$tool"
done
LOCAL_RUST="$(rustc --version 2>/dev/null | awk '{ print $2 }')"
if [ -z "$LOCAL_RUST" ]; then
    echo "FAIL  no rustc on PATH: the image-version check has nothing to compare"
    exit 1
fi

cat >"$SHIM/docker" <<'SH'
#!/usr/bin/env bash
# The fake docker. FAKE_DOCKER_* select what it reports; every call is recorded.
echo "$*" >>"$FAKE_DOCKER_CALLS"
case "${1:-}" in
    info)
        if [ "${FAKE_DOCKER_DAEMON:-up}" = down ]; then
            echo "Cannot connect to the Docker daemon" >&2
            exit 1
        fi
        case "$*" in
            *Architecture*) echo "${FAKE_DOCKER_ARCH:-aarch64}" ;;
            *) echo "fake docker" ;;
        esac
        exit 0
        ;;
    run) ;;
    *) exit 0 ;;
esac
# `docker run ... rustc --version`: the image's compiler.
case "$*" in
    *"rustc --version")
        [ "${FAKE_DOCKER_RUST_FAIL:-no}" = yes ] && { echo "pull failed" >&2; exit 1; }
        echo "rustc ${FAKE_DOCKER_RUST} (fake 2026-01-01)"
        exit 0
        ;;
esac
# The container run: find the output mount, keep the tree it was fed.
out=""
prev=""
for a in "$@"; do
    case "$a" in *:/out) out="${a%:/out}" ;; esac
    # the container's environment, as `-e K=V` gives it
    [ "$prev" = "-e" ] && export "$a"
    prev="$a"
done
[ -n "$out" ] || exit 0
ls -ld "$out" | cut -c1-10 >"$FAKE_DOCKER_OUTMODE"
cat >"$FAKE_DOCKER_STDIN"
leg() { # name exit [log]
    printf '%s\n' "${3:-}" >"$out/$1.log"
    [ "$2" = none ] || echo "$2" >"$out/$1.rc"
}
cargo_ok='   Compiling fx v0.1.0
     Running unittests src/lib.rs (/ci-target/debug/deps/fx-aa)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/t.rs (/ci-target/debug/deps/t-bb)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests fx
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s'
cargo_red='     Running unittests src/lib.rs (/ci-target/debug/deps/fx-aa)
test lib::bad ... FAILED
failures:
test result: FAILED. 1 passed; 1 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/t.rs (/ci-target/debug/deps/t-bb)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests fx
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s'
case "${FAKE_DOCKER_SCENARIO:-pass}" in
    pass)
        leg script-suites--test-a 0 "test-a: 4 passed, 0 failed"
        leg ui-build 0 "built"
        leg test 0 "$cargo_ok"
        ;;
    red-test)
        leg script-suites--test-a 0 "test-a: 4 passed, 0 failed"
        leg ui-build 0 "built"
        leg test 101 "$cargo_red"
        ;;
    dies-midway)
        leg script-suites--test-a 0 "test-a: 4 passed, 0 failed"
        exit 137
        ;;
    suite-never-ran)
        leg ui-build 0 "built"
        leg test 0 "$cargo_ok"
        ;;
    red-suite)
        leg script-suites--test-a 1 "test-a: 3 passed, 1 failed"
        leg ui-build 0 "built"
        leg test 0 "$cargo_ok"
        ;;
    red-ui)
        leg script-suites--test-a 0 "test-a: 4 passed, 0 failed"
        leg ui-build 1 "tsc: error"
        leg test 0 "$cargo_ok"
        ;;
    real-entry)
        # scripts/ci-container.sh itself, on the tree it was fed, with recorder tools.
        w="$FAKE_DOCKER_WORK"
        rm -rf "$w"
        mkdir -p "$w"
        tar -x -C "$w" <"$FAKE_DOCKER_STDIN"
        (cd "$w" && env PATH="$FAKE_TOOLS:$PATH" CI_PHASE=legs CI_OUT="$out" bash scripts/ci-container.sh) >"$out/../ci-entry.out" 2>&1
        echo "$?" >"$FAKE_DOCKER_WORK.rc"
        ;;
    touch-tree)
        leg script-suites--test-a 0 "test-a: 4 passed, 0 failed"
        leg ui-build 0 "built"
        leg test 0 "$cargo_ok"
        echo changed-while-running >"$FAKE_DOCKER_REPO/added-while-running.txt"
        ;;
    rc-garbage)
        leg script-suites--test-a 0 "test-a: 4 passed, 0 failed"
        leg ui-build none "built"
        : >"$out/ui-build.rc"
        leg test none "$cargo_ok"
        echo x >"$out/test.rc"
        ;;
    prep-fails)
        echo "ci-container.sh: node-v22 does not match its pinned sha256" >&2
        exit 1
        ;;
    symlinks)
        # What a hostile leg could leave: logs and statuses that are links to host files.
        leg script-suites--test-a 0 "test-a: 4 passed, 0 failed"
        echo "SECRET-HOST-CONTENT 0" >"$FAKE_DOCKER_SECRET"
        ln -s "$FAKE_DOCKER_SECRET" "$out/ui-build.log"
        echo 0 >"$FAKE_DOCKER_SECRET.rc"
        ln -s "$FAKE_DOCKER_SECRET.rc" "$out/ui-build.rc"
        leg test 0 "$cargo_ok"
        ;;
    huge-rc)
        leg script-suites--test-a 0 "test-a: 4 passed, 0 failed"
        leg ui-build 0 "built"
        echo 99999999999999999999 >"$out/ui-build.rc"
        leg test 0 "$cargo_ok"
        ;;
esac
exit 0
SH
chmod +x "$SHIM/docker"

# The tools the container script calls, as recorders: scripts/ci-container.sh runs for
# real (the shim docker extracts the tree it is fed and runs the entry on it), so the
# names it writes and the flags it passes are the ones the host then reads.
TOOLS="$WORK/tools"
mkdir -p "$TOOLS"
cat >"$TOOLS/cargo" <<'SH'
#!/usr/bin/env bash
echo "$* | tolerance=${LOGOS_PERF_TOLERANCE:-unset} target=${CARGO_TARGET_DIR:-unset}" >>"$FAKE_TOOLS_LOG/cargo.argv"
cat <<'OUT'
     Running unittests src/lib.rs (/ci-target/debug/deps/fx-aa)
test result: ok. 2 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
     Running tests/t.rs (/ci-target/debug/deps/t-bb)
test result: ok. 1 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
   Doc-tests fx
test result: ok. 0 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out; finished in 0.00s
OUT
exit "${FAKE_CARGO_RC:-0}"
SH
cat >"$TOOLS/npm" <<'SH'
#!/usr/bin/env bash
echo "$*" >>"$FAKE_TOOLS_LOG/npm.argv"
echo "npm $*"
exit "${FAKE_NPM_RC:-0}"
SH
printf '#!/usr/bin/env bash\necho "rustc 0.0.0 (shim)"\n' >"$TOOLS/rustc"
printf '#!/usr/bin/env bash\necho v0.0.0\n' >"$TOOLS/node"
chmod +x "$TOOLS"/*

# --- the fixture repository: a one-package workspace --------------------------
REPO="$WORK/repo"
mkdir -p "$REPO/scripts/tests" "$REPO/src" "$REPO/tests"
cp "$GATE" "$REPO/scripts/gate.sh"
cp "$VERIFY" "$REPO/scripts/verify-evidence.sh"
cp "$ENTRY" "$REPO/scripts/ci-container.sh"
cat >"$REPO/Cargo.toml" <<'TOML'
[package]
name = "fx"
version = "0.1.0"
edition = "2021"
TOML
echo '/// doc
pub fn f() {}' >"$REPO/src/lib.rs"
echo '#[test] fn t() {}' >"$REPO/tests/t.rs"
printf '%s\n' 'echo "test-a: 4 passed, 0 failed"' >"$REPO/scripts/tests/test-a.sh"
printf 'ignored.txt\n' >"$REPO/.gitignore"
mkdir -p "$REPO/web/ui"
echo '{}' >"$REPO/web/ui/package.json"
(
    cd "$REPO" || exit 1
    git init -q . >/dev/null 2>&1
    git config user.email t@example.com
    git config user.name test
    git config maintenance.auto false
    git config gc.auto 0
    git add -A >/dev/null 2>&1
    git commit -qm base >/dev/null 2>&1
) || { echo "FAIL  could not build the fixture repository"; exit 1; }
echo untracked >"$REPO/untracked.txt"
echo ignored >"$REPO/ignored.txt"
EVID="$(cd "$REPO" && git rev-parse --path-format=absolute --git-path gate-evidence)"

CALLS="$WORK/docker-calls"
STDIN_TAR="$WORK/stdin.tar"

# gate <path-dir> [env...] -> GOUT, GRC. `env` entries are VAR=value.
gate() {
    local pathdir="$1"
    shift
    : >"$CALLS"
    rm -f "$STDIN_TAR"
    GOUT="$(cd "$REPO" && env PATH="$pathdir" FAKE_DOCKER_CALLS="$CALLS" FAKE_DOCKER_STDIN="$STDIN_TAR" \
        FAKE_DOCKER_OUTMODE="$WORK/outmode" FAKE_DOCKER_SECRET="$WORK/host-secret" \
        FAKE_DOCKER_WORK="$WORK/entry-tree" FAKE_TOOLS="$TOOLS" FAKE_TOOLS_LOG="$WORK/toolslog" \
        FAKE_DOCKER_REPO="$REPO" \
        FAKE_DOCKER_RUST="$LOCAL_RUST" "$@" /bin/bash scripts/gate.sh ci 2>&1)"
    GRC=$?
}
ev() { # file key -> a top-level JSON value of an evidence file
    python3 - "$EVID/$1.json" "$2" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))[sys.argv[2]])
PY
}
unit() { # file key -> that field of the first unit
    python3 - "$EVID/$1.json" "$2" <<'PY'
import json, sys
print(json.load(open(sys.argv[1]))["units"][0][sys.argv[2]])
PY
}
have_ci_evidence() { ls "$EVID"/ci-*.json >/dev/null 2>&1 && echo yes || echo no; }
verify() { # tier... -> VOUT, VRC
    VOUT="$(cd "$REPO" && /bin/bash scripts/verify-evidence.sh --strict "$@" 2>&1)"
    VRC=$?
}

# --- 1. it cannot run ---------------------------------------------------------

rm -rf "$EVID"
gate "$MIN"
expect "[no docker] exits 2" "$GRC" "2"
has "[no docker] names Docker as missing" "$GOUT" "Docker is not installed"
expect "[no docker] writes no evidence" "$(have_ci_evidence)" "no"

gate "$SHIM:$MIN" FAKE_DOCKER_DAEMON=down
expect "[daemon down] exits 2" "$GRC" "2"
has "[daemon down] names the daemon as not running" "$GOUT" "daemon is not running"
lacks "[daemon down] does not say Docker is missing" "$GOUT" "not installed"
expect "[daemon down] writes no evidence" "$(have_ci_evidence)" "no"

gate "$SHIM:$MIN" FAKE_DOCKER_ARCH=x86_64
expect "[x86_64 engine] exits 2" "$GRC" "2"
has "[x86_64 engine] says it will not emulate" "$GOUT" "not a native arm64 engine"

gate "$SHIM:$MIN" FAKE_DOCKER_RUST=0.0.1
expect "[rust mismatch] exits 2" "$GRC" "2"
has "[rust mismatch] names both versions" "$GOUT" "image's Rust is 0.0.1 but the local toolchain is $LOCAL_RUST"
expect "[rust mismatch] writes no evidence" "$(have_ci_evidence)" "no"

gate "$SHIM:$MIN" FAKE_DOCKER_RUST_FAIL=yes
expect "[image will not run] a pull that fails exits 2" "$GRC" "2"
has "[image will not run] ... saying the image could not be run" "$GOUT" "could not run \`rustc --version\`"
has "[image will not run] the first-run pull is announced before it happens" "$GOUT" "the first run pulls"
gate "$SHIM:$MIN" FAKE_DOCKER_ARCH=arm64
expect "[arm64] the engine that spells its architecture arm64 is native too" "$GRC" "0"
gate "$SHIM:$MIN" "FAKE_DOCKER_RUST=${LOCAL_RUST}0"
has "[rust near miss] ... naming both versions" "$GOUT" "image's Rust is ${LOCAL_RUST}0"

# The Rust version is compared as a word, not a prefix: 1.99.10 is not 1.99.1.
gate "$SHIM:$MIN" "FAKE_DOCKER_RUST=${LOCAL_RUST}0"
expect "[rust near miss] a version that merely starts the same is a mismatch" "$GRC" "2"

# --- 2. what it asks docker ---------------------------------------------------

gate "$SHIM:$MIN"
expect "[pass] a green container run exits 0" "$GRC" "0"
RUNLINE="$(grep -- ':/out' "$CALLS" | head -1)"
has "[args] the image is pinned by digest" "$RUNLINE" "@sha256:"
case "$RUNLINE" in
    *rust:*@sha256:????????????????????????????????????????????????????????????????\ bash*) ok "[args] the digest is a full sha256 and the image is the run's last word before its command" ;;
    *) bad "[args] image reference malformed: $RUNLINE" ;;
esac
has "[args] a native arm64 platform" "$RUNLINE" "--platform linux/arm64"
has "[args] the container is removed with its cargo home" "$RUNLINE" "--rm"
has "[args] the build volume is mounted" "$RUNLINE" "-v logos-ci-target:/ci-target"
has "[args] CI's tolerance is passed" "$RUNLINE" "LOGOS_PERF_TOLERANCE=3"
expect "[args] the volume and the output are the only two mounts" \
    "$(printf '%s\n' "$RUNLINE" | tr ' ' '\n' | grep -c '^-v$')" "2"
lacks "[args] nothing is mounted over a cargo home" "$RUNLINE" ".cargo"
lacks "[args] ... and none is named" "$RUNLINE" "CARGO_HOME"
gate "$SHIM:$MIN" GATE_CI_VOLUME=my-vol
has "[args] the volume's name can be overridden" "$(grep -- ':/out' "$CALLS" | head -1)" "-v my-vol:/ci-target"
gate "$SHIM:$MIN"

has "[args] stdin stays open: the tree is fed to it" "$RUNLINE" "run --rm -i --init"
has "[args] the output directory is passed" "$RUNLINE" "-e CI_OUT=/out"
has "[args] the build directory is the volume" "$RUNLINE" "-e CARGO_TARGET_DIR=/ci-target"
has "[args] the compile bound is passed" "$RUNLINE" "-e CI_CARGO_JOBS=2"
has "[args] the test threads are passed" "$RUNLINE" "-e CI_TEST_THREADS=4"
has "[args] the container runs the tree's own entry script" "$RUNLINE" "mkdir -p /work && tar -x -C /work && cd /work && exec bash scripts/ci-container.sh"
CHECKLINE="$(grep -- 'rustc --version' "$CALLS" | head -1)"
has "[args] the Rust check runs on the same platform and image" "$CHECKLINE" "--platform linux/arm64 rust:"
expect "[args] no container is left behind by the check" "$(printf '%s\n' "$CHECKLINE" | grep -c -- '--rm')" "1"
[ ! -e "$EVID/ci-src.tar" ] && ok "[tree] the archive is removed after the run" || bad "[tree] ci-src.tar was left in the evidence directory"
expect "[args] the output directory is the host user's alone (0700)" "$(cat "$WORK/outmode" 2>/dev/null)" "drwx------"
has "[args] the legs run as the host's uid" "$RUNLINE" "-e CI_UID=$(id -u) -e CI_GID=$(id -g)"

TARLIST="$(tar -tf "$STDIN_TAR" 2>/dev/null)"
has "[tree] the container is fed the tree's entry script" "$TARLIST" "scripts/ci-container.sh"
has "[tree] ... and a file the commit does not hold yet" "$TARLIST" "untracked.txt"
lacks "[tree] ... but not an ignored one" "$TARLIST" "ignored.txt"
lacks "[tree] ... and no .git" "$TARLIST" ".git/"

# A run that stops before the container leaves no earlier pass behind.
gate "$SHIM:$MIN"
[ "$GRC" -eq 0 ] && [ "$(have_ci_evidence)" = yes ] && ok "[stale] a green run leaves its evidence (control)" || bad "[stale] control run did not pass"
GITFAIL="$WORK/gitfail"
mkdir -p "$GITFAIL"
printf '#!/bin/sh\n[ "$1" = archive ] && exit 1\nexec %s "$@"\n' "$(command -v git)" >"$GITFAIL/git"
chmod +x "$GITFAIL/git"
gate "$GITFAIL:$SHIM:$MIN"
expect "[stale] a tree that cannot be archived exits 2" "$GRC" "2"
expect "[stale] ... and the earlier pass for the same tree is gone" "$(have_ci_evidence)" "no"
verify --tier ci
expect "[stale] ... so the verifier refuses" "$VRC" "1"

# --- 3. what it records -------------------------------------------------------

gate "$SHIM:$MIN"
for g in ci-ui-build ci-script-suites ci-test; do
    [ -f "$EVID/$g.json" ] && ok "[evidence] $g.json is written" || bad "[evidence] $g.json is missing"
    expect "[evidence] $g is tier ci" "$(ev "$g" tier 2>/dev/null)" "ci"
    expect "[evidence] $g passed" "$(ev "$g" verdict 2>/dev/null)" "pass"
done
expect "[evidence] the run did not write the full tier's evidence" \
    "$(ls "$EVID"/test.json "$EVID"/script-suites.json 2>/dev/null | wc -l | tr -d ' ')" "0"
expect "[denominator] binaries expected (cargo metadata: lib, tests/t.rs, doctest)" "$(unit ci-test expected)" "3"
expect "[denominator] binaries started" "$(unit ci-test started)" "3"
expect "[denominator] binaries finished" "$(unit ci-test finished)" "3"
expect "[denominator] tests passed" "$(unit ci-test passed)" "3"
expect "[denominator] tests failed" "$(unit ci-test failed)" "0"
has "[evidence] the gate's argv records the run's flags" "$(ev ci-test argv)" "cargo test --workspace --jobs 2 --no-fail-fast"
has "[evidence] ... and the image pin" "$(ev ci-test argv)" "@sha256:"
expect "[evidence] the suite unit is the tree's suite" "$(unit ci-script-suites unit)" "test-a"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=red-test
expect "[red test] a failing test exits 1" "$GRC" "1"
expect "[red test] the gate's verdict is fail" "$(ev ci-test verdict)" "fail"
expect "[red test] one failed test is counted" "$(unit ci-test failed)" "1"
expect "[red test] the other legs are still recorded" "$(ev ci-script-suites verdict)" "pass"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=dies-midway
expect "[dies midway] a container killed mid-run exits 1" "$GRC" "1"
expect "[dies midway] the cargo leg that never ran is a failure" "$(ev ci-test verdict)" "fail"
expect "[dies midway] ... that started nothing" "$(ev ci-test truncation)" "no_binaries"
expect "[dies midway] ... and the UI leg too" "$(ev ci-ui-build verdict)" "fail"
expect "[dies midway] ... recorded as never having run" "$(ev ci-ui-build truncation)" "no_binaries"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=suite-never-ran
expect "[suite never ran] a suite of the tree the container skipped exits 1" "$GRC" "1"
expect "[suite never ran] it is recorded as killed" "$(unit ci-script-suites truncation)" "killed"
expect "[suite never ran] its unit did not finish" "$(unit ci-script-suites finished)" "0"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=red-suite
expect "[red suite] a failing suite exits 1" "$GRC" "1"
expect "[red suite] its failed checks are counted" "$(unit ci-script-suites failed)" "1"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=red-suite
has "[red suite] the console names the failing suite" "$GOUT" "test-a"
has "[red suite] ... and its exit" "$GOUT" "exit 1"
has "[red suite] the suites' output is kept in the evidence log" "$(cat "$EVID/ci-script-suites.log")" "test-a: 3 passed, 1 failed"
has "[red suite] the summary counts the checks" "$GOUT" "1 suite(s); 3 passed, 1 failed"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=red-test
has "[red test] the failing test is listed on the console" "$GOUT" "test lib::bad ... FAILED"
has "[red test] the cargo output is kept in the evidence log" "$(cat "$EVID/ci-test.log")" "test result: FAILED. 1 passed; 1 failed"
has "[red test] the evidence argv records the thread count" "$(ev ci-test argv)" "--test-threads=4 (LOGOS_PERF_TOLERANCE=3)"
expect "[red test] the floor is the sum of the packages' floors (lib+tests/t.rs+1)" "$(unit ci-test floor)" "2"
has "[red test] the container's own log exists" "$(ls "$EVID")" "ci-container.log"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=red-ui
has "[red ui] the build's output is kept in the evidence log" "$(cat "$EVID/ci-ui-build.log")" "tsc: error"
has "[red ui] the evidence argv names the build" "$(ev ci-ui-build argv)" "npm ci && npm run build"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=dies-midway
has "[dies midway] the console says the container exited and why legs failed" "$GOUT" "container exited 137"
has "[dies midway] ... and which leg never ran" "$GOUT" "never ran: no exit status"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=rc-garbage
expect "[rc garbage] an empty exit-status file is a leg that never ran" "$(ev ci-ui-build truncation)" "no_binaries"
expect "[rc garbage] a non-numeric one fails the leg too" "$(ev ci-test verdict)" "fail"
lacks "[rc garbage] ... without a shell arithmetic error" "$GOUT" "integer expression"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=touch-tree
expect "[tree moves] a tree that changes while the container runs fails the run" "$GRC" "1"
has "[tree moves] ... saying the evidence describes a tree that no longer exists" "$GOUT" "the tree changed while gating"
has "[tree moves] ... and naming the gate" "$GOUT" "tree-stability"
rm -f "$REPO/added-while-running.txt"

# The real entry script, on the tree the container is fed, with recorder tools.
printf '%s\n' 'echo "test-0fail: 0 passed, 3 failed"; exit 1' >"$REPO/scripts/tests/test-0fail.sh"
rm -rf "$WORK/toolslog"
mkdir -p "$WORK/toolslog"
gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=real-entry GATE_CI_JOBS=5 GATE_CI_TEST_THREADS=7
OUTD="$EVID/ci-out"
expect "[entry] the container script ran to its end" "$(cat "$WORK/entry-tree.rc")" "0"
for f in script-suites--test-a script-suites--test-0fail ui-build test; do
    [ -f "$OUTD/$f.rc" ] && [ -f "$OUTD/$f.log" ] && ok "[entry] it left $f.log and $f.rc" || bad "[entry] $f.log/.rc missing: $(ls "$OUTD")"
done
expect "[entry] a suite's real exit status is recorded" "$(cat "$OUTD/script-suites--test-0fail.rc")" "1"
expect "[entry] a failing suite does not stop the next" "$(cat "$OUTD/script-suites--test-a.rc")" "0"
expect "[entry] every leg's status is recorded after its log" "$(cat "$OUTD/test.rc")" "0"
has "[entry] cargo runs the whole workspace" "$(cat "$WORK/toolslog/cargo.argv")" "test --workspace "
has "[entry] ... without fail-fast" "$(cat "$WORK/toolslog/cargo.argv")" "--no-fail-fast"
has "[entry] ... with the compile bound GATE_CI_JOBS gave" "$(cat "$WORK/toolslog/cargo.argv")" "--jobs 5"
has "[entry] ... and the test threads GATE_CI_TEST_THREADS gave" "$(cat "$WORK/toolslog/cargo.argv")" "--test-threads=7"
has "[entry] ... at CI's tolerance" "$(cat "$WORK/toolslog/cargo.argv")" "tolerance=3"
has "[entry] ... into the build volume" "$(cat "$WORK/toolslog/cargo.argv")" "target=/ci-target"
expect "[entry] the web UI is installed from the lockfile, then built" "$(tr '\n' ',' <"$WORK/toolslog/npm.argv")" "ci,run build,"
expect "[entry] the host reads what the container left: the red suite fails the gate" "$GRC" "1"
expect "[entry] ... the build and tests, which passed, pass" "$(ev ci-ui-build verdict)/$(ev ci-test verdict)" "pass/pass"
expect "[entry] ... and the suites unit counts the failing one" "$(unit ci-script-suites failed)" "3"
rm -f "$REPO/scripts/tests/test-0fail.sh"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=real-entry FAKE_CARGO_RC=101
expect "[entry] a cargo failure's status is what the leg records" "$(cat "$OUTD/test.rc")" "101"
gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=real-entry FAKE_NPM_RC=7
expect "[entry] a failed web UI build stops neither the other legs" "$(cat "$OUTD/test.rc")" "0"
expect "[entry] ... and records its own status" "$(cat "$OUTD/ui-build.rc")" "7"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=prep-fails
expect "[prep fails] a container that cannot be prepared exits 1" "$GRC" "1"
has "[prep fails] and its reason is shown, not buried in a log" "$GOUT" "does not match its pinned sha256"
expect "[prep fails] every leg is a failure that started nothing" "$(ev ci-test truncation)" "no_binaries"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=symlinks
expect "[symlinks] the scenario really left links (control)" "$([ -L "$EVID/ci-out/ui-build.log" ] && [ -L "$EVID/ci-out/ui-build.rc" ] && echo links)" "links"
expect "[symlinks] links in the output directory are not followed: the build never ran" "$(ev ci-ui-build truncation)" "no_binaries"
expect "[symlinks] ... so the gate fails" "$GRC" "1"
expect "[symlinks] ... and no host file's content reaches the evidence log" \
    "$(grep -c SECRET-HOST-CONTENT "$EVID/ci-ui-build.log")" "0"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=huge-rc
expect "[huge rc] an exit status no process leaves is not a pass" "$(ev ci-ui-build verdict)" "fail"
expect "[huge rc] ... and the gate fails" "$GRC" "1"

printf 'untracked.txt export-ignore\n' >"$REPO/.gitattributes"
gate "$SHIM:$MIN"
expect "[export-ignore] a tree git archive would shrink is refused" "$GRC" "2"
has "[export-ignore] ... saying why" "$GOUT" "export-ignore"
rm -f "$REPO/.gitattributes"

gate "$SHIM:$MIN" FAKE_DOCKER_SCENARIO=red-ui
expect "[red ui] a failing web UI build exits 1" "$GRC" "1"
expect "[red ui] the build gate fails on its exit status" "$(ev ci-ui-build exit_code)" "1"

# --- 4. what the verifier says ------------------------------------------------

gate "$SHIM:$MIN"
verify --tier ci
expect "[verify ci] accepts tier ci evidence for the tree" "$VRC" "0"
has "[verify ci] ... and says which tier" "$VOUT" "tier ci"
verify --tier fast
expect "[verify fast] ci evidence does not stand in for tier fast" "$VRC" "1"
verify --tier full
expect "[verify full] ci evidence does not stand in for tier full" "$VRC" "1"
lacks "[verify full] ... and a ci gate is not read as a full-tier row" "$VOUT" "ci-test"
verify --tier ci --write-marker "$WORK/marker"
expect "[verify ci] writes no marker (a publish precondition, not a handoff)" "$VRC" "2"
expect "[verify ci] ... and none exists" "$([ -e "$WORK/marker" ] && echo yes || echo no)" "no"

# Fast and full evidence, however good, is refused in its place.
rm -rf "$EVID"
mkdir -p "$EVID"
TREE_NOW="$(cd "$REPO" && idx="$(git rev-parse --path-format=absolute --git-path index)" &&
    cp "$idx" "$WORK/idx" && GIT_INDEX_FILE="$WORK/idx" git add -A >/dev/null 2>&1 &&
    GIT_INDEX_FILE="$WORK/idx" git write-tree)"
for g in clippy test arch public-terms timing-assertions offline-cargo; do
    python3 - "$EVID/$g.json" "$g" "$TREE_NOW" <<'PY'
import json, sys
json.dump({"schema": 1, "gate": sys.argv[2], "tier": "full", "tree_id": sys.argv[3],
           "verdict": "pass", "exit_code": 0, "truncation": "none", "argv": "x",
           "units": [{"unit": "u", "expected": 1, "floor": 1, "started": 1, "finished": 1,
                      "passed": 1, "failed": 0, "ignored": 0, "exit_code": 0,
                      "verdict": "pass", "truncation": "none", "argv": "x"}]},
          open(sys.argv[1], "w"))
PY
done
verify --tier ci
expect "[fast/full evidence] --tier ci refuses it" "$VRC" "1"
has "[fast/full evidence] ... naming the missing ci gates" "$VOUT" "ci-test: no evidence file"
has "[fast/full evidence] ... and saying the other tiers do not stand in" "$VOUT" "never stands in for tier ci"
verify --tier fast
expect "[fast/full evidence] the same files satisfy --tier fast (control: the verifier reads them)" "$VRC" "0"

# A ci gate's file carrying another tier is refused, whichever tier.
gate "$SHIM:$MIN"
for t in fast full; do
    python3 - "$EVID/ci-test.json" "$t" <<'PY'
import json, sys
p = sys.argv[1]
d = json.load(open(p))
d["tier"] = sys.argv[2]
json.dump(d, open(p, "w"))
PY
    verify --tier ci
    expect "[tier forged: $t] --tier ci refuses a ci gate that says tier $t" "$VRC" "1"
    has "[tier forged: $t] ... naming the gate and the tier" "$VOUT" "ci-test: tier is '$t', not \"ci\""
done

# A counter that contradicts the verdict is refused, as in the other tiers.
gate "$SHIM:$MIN"
python3 - "$EVID/ci-test.json" <<'PY'
import json, sys
d = json.load(open(sys.argv[1]))
d["units"][0]["started"] = d["units"][0]["finished"] = 2
json.dump(d, open(sys.argv[1], "w"))
PY
verify --tier ci
expect "[truncated] a workspace run that ran 2 of 3 binaries is refused" "$VRC" "1"
has "[truncated] ... as truncated" "$VOUT" "the run was truncated"

# Evidence for another tree is stale.
gate "$SHIM:$MIN"
echo changed >>"$REPO/untracked.txt"
verify --tier ci
expect "[stale] ci evidence for an earlier tree is refused" "$VRC" "1"
has "[stale] ... as stale" "$VOUT" "stale evidence"

# --- 5. it matches ci.yml -----------------------------------------------------

WF_TOL="$(sed -n 's/^ *LOGOS_PERF_TOLERANCE: *"\{0,1\}\([0-9.]*\)"\{0,1\} *$/\1/p' "$HERE/.github/workflows/ci.yml" | head -1)"
GATE_TOL="$(sed -n 's/^CI_PERF_TOLERANCE=\([0-9.]*\)$/\1/p' "$GATE" | head -1)"
expect "[ci.yml] the gate's tolerance is the workflow's" "$GATE_TOL" "$WF_TOL"
[ -n "$GATE_TOL" ] && ok "[ci.yml] ... and both were read" || bad "[ci.yml] no tolerance read from the gate"

echo
echo "$PASS passed, $FAIL failed (of $((PASS + FAIL)))"
[ "$FAIL" -eq 0 ] || exit 1
exit 0
